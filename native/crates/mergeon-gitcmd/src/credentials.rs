//! Usuario y contraseña (o token) para remotos HTTPS que no son de GitHub.
//!
//! Git corre con `GIT_TERMINAL_PROMPT=0`: si ningún credential helper del
//! usuario responde, fetch/pull/push/clone fallan con "could not read
//! Username…" o "Authentication failed…". La app detecta ese fallo
//! ([`auth_failure_origin`]), pide usuario y contraseña y los deja aquí, en
//! memoria y por origen (`https://host[:puerto]`), con
//! [`set_session_credentials`]. A partir de ahí cada git **de red** (ver
//! [`is_network_command`]) lleva, solo para ese origen:
//!
//! ```text
//! -c credential.https://host.helper=          (vacía la lista de helpers)
//! -c credential.https://host.helper=!f() { test "$1" = get || exit 0; cat '/tmp/…'; }; f
//! ```
//!
//! El secreto se escribe en un archivo temporal efímero por comando (permisos
//! `0600` en Unix) que se sobrescribe con ceros y se borra al terminar el
//! proceso git ([`CredentialGuard`]). Así ni el usuario ni la contraseña pasan
//! por argv (`ps`), por el `Debug` de [`crate::Git`] ni por variables de
//! entorno heredadas por hooks (`pre-push`, `post-merge`,
//! `reference-transaction`, …). Vaciar antes la lista de helpers de ese origen
//! hace que git use lo tecleado y no una credencial vieja del llavero, y que al
//! terminar bien no la guarde en el helper del usuario sin permiso: eso solo lo
//! hace [`approve_http_credentials`] (`git credential approve` por stdin)
//! cuando el usuario marca "Remember". Los demás orígenes siguen con los
//! helpers del usuario.
//!
//! Idea tomada de gitcraft (`src/git.rs`, MIT OR Apache-2.0): helper en línea y
//! lista de mensajes de fallo de autenticación.

use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use regex::Regex;

use crate::runner::{Git, GitError, GitResult};

/// Usuario y contraseña de un origen HTTP(S). El `Debug` no los muestra y al
/// soltarse se sobrescriben con ceros.
pub struct HttpCredentials {
    origin: String,
    username: String,
    password: String,
}

impl HttpCredentials {
    /// `origin` es la forma `esquema://host[:puerto]` de [`credential_origin`].
    pub fn new(
        origin: impl Into<String>,
        username: impl Into<String>,
        password: impl Into<String>,
    ) -> Self {
        Self {
            origin: origin.into(),
            username: username.into(),
            password: password.into(),
        }
    }

    pub fn origin(&self) -> &str {
        &self.origin
    }

    pub fn username(&self) -> &str {
        &self.username
    }

    pub fn password(&self) -> &str {
        &self.password
    }
}

impl std::fmt::Debug for HttpCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpCredentials")
            .field("origin", &self.origin)
            .field("username", &"<redacted>")
            .field("password", &"<redacted>")
            .finish()
    }
}

impl Drop for HttpCredentials {
    fn drop(&mut self) {
        wipe(&mut self.username);
        wipe(&mut self.password);
    }
}

/// Sobrescribe con ceros el contenido de `text` (sin que el compilador lo
/// elimine por inútil) y lo vacía.
pub fn wipe(text: &mut String) {
    // SAFETY: ceros es UTF-8 válido, y el `String` se vacía justo después.
    let bytes = unsafe { text.as_bytes_mut() };
    for byte in bytes.iter_mut() {
        // SAFETY: `byte` es una referencia válida y alineada.
        unsafe { std::ptr::write_volatile(byte, 0) };
    }
    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
    text.clear();
}

static SESSION: LazyLock<Mutex<Vec<Arc<HttpCredentials>>>> = LazyLock::new(Mutex::default);

fn session() -> std::sync::MutexGuard<'static, Vec<Arc<HttpCredentials>>> {
    SESSION.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Guarda (o reemplaza) las credenciales de su origen para los git de red de
/// esta sesión.
pub fn set_session_credentials(credentials: HttpCredentials) {
    let mut session = session();
    session.retain(|known| known.origin != credentials.origin);
    session.push(Arc::new(credentials));
}

/// Olvida las credenciales de `origin`. Devuelve si había.
pub fn forget_session_credentials(origin: &str) -> bool {
    let mut session = session();
    let before = session.len();
    session.retain(|known| known.origin != origin);
    session.len() != before
}

/// Hay credenciales de sesión para `origin`.
pub fn has_session_credentials(origin: &str) -> bool {
    session().iter().any(|known| known.origin == origin)
}

pub(crate) fn session_credentials() -> Vec<Arc<HttpCredentials>> {
    session().clone()
}

/// Archivos efímeros de credenciales ligados a la vida de un único proceso git:
/// al soltarse se sobrescriben con ceros y se borran del disco.
#[derive(Default)]
pub(crate) struct CredentialGuard {
    files: Vec<PathBuf>,
}

impl Drop for CredentialGuard {
    fn drop(&mut self) {
        for path in self.files.drain(..) {
            if let Ok(meta) = std::fs::metadata(&path) {
                let len = meta.len() as usize;
                if len > 0
                    && let Ok(mut file) = OpenOptions::new().write(true).open(&path)
                {
                    let _ = file.write_all(&vec![0u8; len]);
                    let _ = file.flush();
                }
            }
            let _ = std::fs::remove_file(&path);
        }
    }
}

fn sanitize_credential_value(value: &str) -> String {
    value.replace(['\r', '\n'], "")
}

fn write_ephemeral_credentials(credentials: &HttpCredentials) -> std::io::Result<PathBuf> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let base = std::env::temp_dir();
    let pid = std::process::id();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());

    let mut payload = format!(
        "username={}\npassword={}\n\n",
        sanitize_credential_value(&credentials.username),
        sanitize_credential_value(&credentials.password),
    );

    let mut last_error = None;
    for _ in 0..32 {
        let seq = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = base.join(format!("mergeon-cred-{pid}-{nanos}-{seq}"));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        match options.open(&path) {
            Ok(mut file) => {
                let result = file
                    .write_all(payload.as_bytes())
                    .and_then(|_| file.flush());
                wipe(&mut payload);
                return match result {
                    Ok(()) => Ok(path),
                    Err(error) => {
                        let _ = std::fs::remove_file(&path);
                        Err(error)
                    }
                };
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                last_error = Some(error);
            }
            Err(error) => {
                wipe(&mut payload);
                return Err(error);
            }
        }
    }
    wipe(&mut payload);
    Err(last_error.unwrap_or_else(|| std::io::Error::other("could not create credential file")))
}

fn shell_quote_path(path: &Path) -> String {
    let raw = path.to_string_lossy();
    #[cfg(windows)]
    let raw = raw.replace('\\', "/");
    raw.replace('\'', "'\\''")
}

/// El helper en línea de un origen: contesta a `get` volcando el archivo
/// efímero e ignora `store` y `erase`.
pub(crate) fn inline_helper(path: &Path) -> String {
    let quoted = shell_quote_path(path);
    format!("!f() {{ test \"$1\" = get || exit 0; cat '{quoted}'; }}; f")
}

/// Entradas `-c` y guardia de archivos efímeros que hacen que git use
/// `credentials`. Ningún secreto entra en argv ni en el entorno del hijo.
pub(crate) fn credential_arguments(
    credentials: &[Arc<HttpCredentials>],
) -> (Vec<String>, CredentialGuard) {
    let mut config = Vec::with_capacity(credentials.len() * 2);
    let mut guard = CredentialGuard::default();
    for credentials in credentials {
        let Ok(path) = write_ephemeral_credentials(credentials) else {
            log::warn!(
                "no se pudo crear archivo temporal de credenciales para {}",
                credentials.origin
            );
            continue;
        };
        let origin = &credentials.origin;
        config.push(format!("credential.{origin}.helper="));
        config.push(format!(
            "credential.{origin}.helper={}",
            inline_helper(&path)
        ));
        guard.files.push(path);
    }
    (config, guard)
}

/// Subcomandos que pueden hablar con un remoto y, por tanto, pedir
/// credenciales. Solo a estos se les adjuntan las de la sesión; además, el
/// secreto viaja en un archivo efímero `0600` (no en variables de entorno),
/// de modo que un hook que vuelque `env` nunca lo expone.
pub fn is_network_command<S: AsRef<std::ffi::OsStr>>(args: &[S]) -> bool {
    let Some(first) = args.first().and_then(|arg| arg.as_ref().to_str()) else {
        return false;
    };
    matches!(
        first,
        "fetch" | "pull" | "push" | "clone" | "ls-remote" | "remote" | "submodule"
    )
}

/// Trozos de los mensajes con que git dice que necesitaba usuario y
/// contraseña y no los tuvo, o que se los rechazaron.
const AUTH_FAILURE_MESSAGES: [&str; 8] = [
    "terminal prompts disabled",
    "could not read username",
    "could not read password",
    "authentication failed",
    "invalid username or password",
    "http basic: access denied",
    "the requested url returned error: 401",
    "the requested url returned error: 403",
];

/// El mensaje de git dice que faltaban credenciales o que se rechazaron.
pub fn is_http_auth_failure(message: &str) -> bool {
    let message = message.to_lowercase();
    AUTH_FAILURE_MESSAGES
        .iter()
        .any(|fragment| message.contains(fragment))
}

static QUOTED_URL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"'(https?://[^'\s]+)'").expect("regex"));

/// Origen (`esquema://host[:puerto]`) del remoto HTTP(S) que pidió
/// credenciales, si el mensaje de git es un fallo de autenticación y nombra
/// la URL (`could not read Username for 'https://host'`, `Authentication
/// failed for 'https://host/repo.git/'`, `unable to access '…': The requested
/// URL returned error: 401`). `None` para SSH y para cualquier otro error.
pub fn auth_failure_origin(message: &str) -> Option<String> {
    if !is_http_auth_failure(message) {
        return None;
    }
    QUOTED_URL
        .captures_iter(message)
        .find_map(|captures| credential_origin(&captures[1]))
}

/// La primera URL HTTP(S) entre comillas del mensaje, tal cual.
pub fn quoted_url(message: &str) -> Option<String> {
    QUOTED_URL
        .captures(message)
        .map(|captures| captures[1].to_string())
}

/// `https://user:pass@Host.example:8443/a/b.git` → `https://host.example:8443`.
/// `None` si no es una URL HTTP(S) con host.
pub fn credential_origin(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    if host.is_empty() || host.starts_with(':') {
        return None;
    }
    Some(format!("{scheme}://{}", host.to_ascii_lowercase()))
}

/// El usuario que trae la propia URL (`https://alice@host/…`), para
/// proponerlo en el diálogo.
pub fn url_username(url: &str) -> Option<String> {
    let (_, rest) = url.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let (userinfo, _) = authority.rsplit_once('@')?;
    let user = userinfo.split(':').next().unwrap_or_default();
    (!user.is_empty()).then(|| user.to_string())
}

static URL_USERINFO: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b([a-z][a-z0-9+.\-]*)://([^/@\s'\x22<>]+)@").expect("regex")
});

/// Enmascara las credenciales que una URL lleve dentro, en cualquier parte
/// de `text` (un mensaje de git, un aviso, una línea de log):
/// `https://user:token@host/r.git` → `https://***@host/r.git`. En HTTP(S)
/// se oculta todo el usuario (un token suele ir solo, sin `:`); en otros
/// esquemas solo la contraseña (`ssh://git@host` queda igual,
/// `ssh://u:p@host` → `ssh://u:***@host`).
pub fn redact_url_credentials(text: &str) -> String {
    URL_USERINFO
        .replace_all(text, |captures: &regex::Captures<'_>| {
            let scheme = &captures[1];
            let userinfo = &captures[2];
            let masked =
                if scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https") {
                    "***".to_string()
                } else if let Some((user, _)) = userinfo.split_once(':') {
                    format!("{user}:***")
                } else {
                    userinfo.to_string()
                };
            format!("{scheme}://{masked}@")
        })
        .into_owned()
}

/// El host (con puerto) de un origen: `https://host:8443` → `host:8443`.
pub fn origin_host(origin: &str) -> &str {
    origin.split_once("://").map_or(origin, |(_, host)| host)
}

/// `true` si `origin` usa `http://` (sin TLS) contra un host que no es local
/// (`localhost`, `127.0.0.1`, `::1`): la contraseña viajará en claro.
pub fn is_insecure_http_origin(origin: &str) -> bool {
    let Some(rest) = origin.strip_prefix("http://") else {
        return false;
    };
    let host = if let Some(bracketed) = rest.strip_prefix('[') {
        bracketed.split(']').next().unwrap_or("")
    } else {
        rest.split(':').next().unwrap_or("")
    };
    !matches!(host, "localhost" | "127.0.0.1" | "::1") && !host.ends_with(".localhost")
}

fn git_for(repo_path: Option<&Path>) -> Git {
    match repo_path {
        Some(path) => Git::new(path),
        None => Git::without_cwd(),
    }
    .without_session_credentials()
}

/// Hay un credential helper configurado para `origin` (en la configuración que
/// ve `repo_path`, o la global sin repositorio): solo entonces tiene sentido
/// ofrecer "Remember".
pub fn has_credential_helper(repo_path: Option<&Path>, origin: &str) -> bool {
    git_for(repo_path)
        .exec(["config", "--get-urlmatch", "credential.helper", origin])
        .is_ok_and(|output| output.success() && !output.stdout_text().trim().is_empty())
}

/// `git credential approve`: guarda las credenciales en el helper del usuario.
/// Van por stdin, nunca en los argumentos.
pub fn approve_http_credentials(
    repo_path: Option<&Path>,
    credentials: &HttpCredentials,
) -> GitResult<()> {
    let (protocol, host) = credentials
        .origin
        .split_once("://")
        .ok_or_else(|| GitError::message("Invalid credential origin."))?;
    let mut input = format!(
        "protocol={protocol}\nhost={host}\nusername={}\npassword={}\n\n",
        credentials.username, credentials.password
    );
    let result = git_for(repo_path).raw_with_stdin(["credential", "approve"], input.as_bytes());
    wipe(&mut input);
    result.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_the_credentials_embedded_in_urls() {
        for (text, expected) in [
            (
                "https://alice:s3cr3t@git.example.com/a/b.git",
                "https://***@git.example.com/a/b.git",
            ),
            (
                "fatal: unable to access 'https://ghp_TOKEN@github.com/o/r.git/': 403",
                "fatal: unable to access 'https://***@github.com/o/r.git/': 403",
            ),
            ("HTTP://u:p@host", "HTTP://***@host"),
            ("ssh://git@host/r.git", "ssh://git@host/r.git"),
            ("ssh://git:pw@host:22/r.git", "ssh://git:***@host:22/r.git"),
            ("git@github.com:o/r.git", "git@github.com:o/r.git"),
            ("https://host/a@b", "https://host/a@b"),
            (
                "from https://a:b@one and https://c@two",
                "from https://***@one and https://***@two",
            ),
            ("nothing here", "nothing here"),
        ] {
            assert_eq!(redact_url_credentials(text), expected, "{text}");
        }
    }

    #[test]
    fn recognises_the_messages_of_missing_or_refused_credentials() {
        for message in [
            "fatal: could not read Username for 'https://gitlab.com': terminal prompts disabled",
            "remote: HTTP Basic: Access denied\nfatal: Authentication failed for 'https://gitlab.com/a/b.git/'",
            "remote: Invalid username or password.\nfatal: Authentication failed for 'https://bitbucket.org/a/b.git/'",
            "fatal: unable to access 'https://git.example.com/r.git/': The requested URL returned error: 401",
            "fatal: unable to access 'https://dev.azure.com/o/p/_git/r/': The requested URL returned error: 403",
            "fatal: could not read Password for 'https://alice@host.example': terminal prompts disabled",
        ] {
            assert!(is_http_auth_failure(message), "{message}");
        }
        for message in [
            "git@github.com: Permission denied (publickey).\nfatal: Could not read from remote repository.",
            "fatal: unable to access 'https://host/': Could not resolve host: host",
            "! [rejected] main -> main (fetch first)",
            "",
        ] {
            assert!(!is_http_auth_failure(message), "{message}");
            assert_eq!(auth_failure_origin(message), None, "{message}");
        }
    }

    #[test]
    fn finds_the_origin_that_asked_for_credentials() {
        assert_eq!(
            auth_failure_origin(
                "fatal: could not read Username for 'https://GitLab.com': terminal prompts disabled"
            )
            .as_deref(),
            Some("https://gitlab.com")
        );
        assert_eq!(
            auth_failure_origin(
                "fatal: Authentication failed for 'http://127.0.0.1:8080/repo.git/'"
            )
            .as_deref(),
            Some("http://127.0.0.1:8080")
        );
        // Sin URL no hay a quién pedirle nada.
        assert_eq!(auth_failure_origin("fatal: Authentication failed"), None);
    }

    #[test]
    fn normalises_urls_to_their_credential_origin() {
        assert_eq!(
            credential_origin("https://user:pass@Host.Example:8443/a/b.git").as_deref(),
            Some("https://host.example:8443")
        );
        assert_eq!(
            credential_origin("HTTP://host/x").as_deref(),
            Some("http://host")
        );
        assert_eq!(credential_origin("git@github.com:a/b.git"), None);
        assert_eq!(credential_origin("ssh://git@host/a.git"), None);
        assert_eq!(credential_origin("https:///nothing"), None);
        assert_eq!(
            url_username("https://alice@host/a.git").as_deref(),
            Some("alice")
        );
        assert_eq!(
            url_username("https://alice:pw@host").as_deref(),
            Some("alice")
        );
        assert_eq!(url_username("https://host/a.git"), None);
        assert_eq!(
            origin_host("https://host.example:8443"),
            "host.example:8443"
        );
    }

    #[test]
    fn detects_non_loopback_plain_http_origins() {
        assert!(is_insecure_http_origin("http://git.example.com"));
        assert!(is_insecure_http_origin("http://192.168.1.5:8080"));
        assert!(!is_insecure_http_origin("https://git.example.com"));
        assert!(!is_insecure_http_origin("http://127.0.0.1:18173"));
        assert!(!is_insecure_http_origin("http://localhost:8080"));
        assert!(!is_insecure_http_origin("http://[::1]:8080"));
        assert!(!is_insecure_http_origin("http://git.localhost:8080"));
    }

    #[test]
    fn the_inline_helper_uses_an_ephemeral_file_and_wipes_it_on_drop() {
        let credentials = [Arc::new(HttpCredentials::new(
            "https://git.example.com",
            "alice",
            "s3cret-token",
        ))];
        let (config, guard) = credential_arguments(&credentials);
        assert_eq!(guard.files.len(), 1);
        let path = guard.files[0].clone();
        assert!(path.exists());
        assert_eq!(
            config,
            [
                "credential.https://git.example.com.helper=".to_string(),
                format!(
                    "credential.https://git.example.com.helper={}",
                    inline_helper(&path)
                ),
            ]
        );
        assert!(config.iter().all(|entry| !entry.contains("s3cret-token")));
        assert!(config.iter().all(|entry| !entry.contains("alice")));
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, "username=alice\npassword=s3cret-token\n\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        drop(guard);
        assert!(!path.exists());
    }

    #[test]
    fn debug_never_shows_the_secret_and_drop_wipes_it() {
        let credentials = HttpCredentials::new("https://h", "alice", "s3cret-token");
        let shown = format!("{credentials:?}");
        assert!(
            !shown.contains("s3cret-token") && !shown.contains("alice"),
            "{shown}"
        );

        let mut text = "s3cret-token".to_string();
        wipe(&mut text);
        assert!(text.is_empty());
    }

    #[test]
    fn only_network_commands_get_the_session_credentials() {
        assert!(is_network_command(&["push", "origin"]));
        assert!(is_network_command(&["fetch"]));
        assert!(is_network_command(&["clone", "--", "u", "p"]));
        assert!(!is_network_command(&["commit", "-m", "x"]));
        assert!(!is_network_command(&["status"]));
        assert!(!is_network_command::<&str>(&[]));
    }

    /// Clone contra un servidor HTTP con autenticación básica: sin
    /// credenciales falla con un mensaje que nombra el origen; con las de la
    /// sesión funciona; con una contraseña mala vuelve a fallar.
    #[cfg(unix)]
    #[test]
    fn clones_from_an_http_remote_with_the_session_credentials() {
        use crate::test_support::AuthHttpServer;

        let root = tempfile::tempdir().unwrap();
        let hermetic = |git: Git| {
            git.with_env("GIT_CONFIG_GLOBAL", "/dev/null")
                .with_env("GIT_CONFIG_NOSYSTEM", "1")
        };
        let bare = root.path().join("served").join("demo.git");
        std::fs::create_dir_all(&bare).unwrap();
        hermetic(Git::new(&bare))
            .raw(["init", "--bare", "-b", "main"])
            .unwrap();
        let server =
            AuthHttpServer::start(&root.path().join("served"), "alice", "s3cret-token").unwrap();
        let url = server.repo_url("demo.git");
        let origin = credential_origin(&url).unwrap();

        let clone = |target: &str| {
            hermetic(Git::without_cwd()).raw([
                "clone",
                "--",
                url.as_str(),
                root.path().join(target).to_str().unwrap(),
            ])
        };

        let error = clone("first").unwrap_err();
        assert_eq!(
            auth_failure_origin(&error.message).as_deref(),
            Some(origin.as_str())
        );
        assert!(server.rejected_requests() > 0);
        assert_eq!(server.authorized_requests(), 0);

        set_session_credentials(HttpCredentials::new(&origin, "alice", "s3cret-token"));
        assert!(has_session_credentials(&origin));
        clone("second").unwrap();
        assert!(server.authorized_requests() > 0);
        assert!(root.path().join("second/.git").is_dir());
        // El clon no guarda la contraseña en ningún sitio.
        let config = std::fs::read_to_string(root.path().join("second/.git/config")).unwrap();
        assert!(!config.contains("s3cret-token"), "{config}");

        set_session_credentials(HttpCredentials::new(&origin, "alice", "wrong"));
        let error = clone("third").unwrap_err();
        assert!(is_http_auth_failure(&error.message), "{}", error.message);
        assert!(!error.message.contains("wrong"), "{}", error.message);

        assert!(forget_session_credentials(&origin));
        assert!(!has_session_credentials(&origin));
    }

    /// Los hooks lanzados por un `push` (`pre-push`, `reference-transaction`)
    /// no reciben la contraseña en sus variables de entorno.
    #[cfg(unix)]
    #[test]
    fn hooks_never_receive_http_credentials_in_their_environment() {
        use std::os::unix::fs::PermissionsExt as _;

        use crate::test_support::AuthHttpServer;

        let root = tempfile::tempdir().unwrap();
        let hermetic = |git: Git| {
            git.with_env("GIT_CONFIG_GLOBAL", "/dev/null")
                .with_env("GIT_CONFIG_NOSYSTEM", "1")
        };
        let bare = root.path().join("served").join("hooks.git");
        std::fs::create_dir_all(&bare).unwrap();
        hermetic(Git::new(&bare))
            .raw(["init", "--bare", "-b", "main"])
            .unwrap();
        let server =
            AuthHttpServer::start(&root.path().join("served"), "alice", "hook-s3cret").unwrap();
        let url = server.repo_url("hooks.git");
        let origin = credential_origin(&url).unwrap();

        let repo = root.path().join("work");
        std::fs::create_dir_all(&repo).unwrap();
        hermetic(Git::new(&repo))
            .raw(["init", "-b", "main"])
            .unwrap();
        for (k, v) in [("user.name", "Test"), ("user.email", "t@example.com")] {
            hermetic(Git::new(&repo)).raw(["config", k, v]).unwrap();
        }
        std::fs::write(repo.join("a.txt"), "hello\n").unwrap();
        hermetic(Git::new(&repo)).raw(["add", "a.txt"]).unwrap();
        hermetic(Git::new(&repo))
            .raw(["commit", "-m", "init"])
            .unwrap();
        hermetic(Git::new(&repo))
            .raw(["remote", "add", "origin", &url])
            .unwrap();

        let env_dump = root.path().join("hook-env.txt");
        let hook = repo.join(".git/hooks/pre-push");
        std::fs::write(
            &hook,
            format!("#!/bin/sh\nenv >> '{}'\n", env_dump.display()),
        )
        .unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();

        set_session_credentials(HttpCredentials::new(&origin, "alice", "hook-s3cret"));
        let pushed = hermetic(Git::new(&repo)).raw(["push", "origin", "main"]);
        forget_session_credentials(&origin);
        pushed.unwrap();

        let dumped = std::fs::read_to_string(&env_dump).unwrap();
        assert!(!dumped.is_empty());
        assert!(!dumped.contains("hook-s3cret"), "{dumped}");
        assert!(!dumped.contains("MERGEON_HTTP_"), "{dumped}");
    }

    /// "Remember": `git credential approve` guarda en el helper del usuario
    /// (aquí `store` sobre un archivo) y la contraseña no pasa por argv.
    #[cfg(unix)]
    #[test]
    fn approve_stores_the_credentials_in_the_user_helper() {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        Git::new(&repo).raw(["init", "-b", "main"]).unwrap();
        let store = root.path().join("credentials");
        let helper = format!("store --file={}", store.display());

        Git::new(&repo)
            .raw(["config", "credential.helper", helper.as_str()])
            .unwrap();
        assert!(has_credential_helper(
            Some(&repo),
            "https://git.example.com"
        ));

        let credentials = HttpCredentials::new("https://git.example.com:8443", "alice", "pw-1");
        approve_http_credentials(Some(&repo), &credentials).unwrap();
        let stored = std::fs::read_to_string(&store).unwrap();
        assert!(
            stored.starts_with("https://alice:pw-1@git.example.com"),
            "{stored}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn per_command_http_credentials_authenticate_without_leaking_to_debug_or_hooks() {
        use std::os::unix::fs::PermissionsExt as _;

        use crate::test_support::AuthHttpServer;

        let root = tempfile::tempdir().unwrap();
        let hermetic = |git: Git| {
            git.with_env("GIT_CONFIG_GLOBAL", "/dev/null")
                .with_env("GIT_CONFIG_NOSYSTEM", "1")
        };
        let bare = root.path().join("served").join("cmd-creds.git");
        std::fs::create_dir_all(&bare).unwrap();
        hermetic(Git::new(&bare))
            .raw(["init", "--bare", "-b", "main"])
            .unwrap();
        let server = AuthHttpServer::start(
            &root.path().join("served"),
            "x-access-token",
            "gho_ephemeral",
        )
        .unwrap();
        let url = server.repo_url("cmd-creds.git");
        let origin = credential_origin(&url).unwrap();

        let repo = root.path().join("work");
        std::fs::create_dir_all(&repo).unwrap();
        hermetic(Git::new(&repo))
            .raw(["init", "-b", "main"])
            .unwrap();
        for (k, v) in [("user.name", "Test"), ("user.email", "t@example.com")] {
            hermetic(Git::new(&repo)).raw(["config", k, v]).unwrap();
        }
        std::fs::write(repo.join("a.txt"), "hello\n").unwrap();
        hermetic(Git::new(&repo)).raw(["add", "a.txt"]).unwrap();
        hermetic(Git::new(&repo))
            .raw(["commit", "-m", "init"])
            .unwrap();
        hermetic(Git::new(&repo))
            .raw(["remote", "add", "origin", &url])
            .unwrap();

        let env_dump = root.path().join("hook-env.txt");
        let hook = repo.join(".git/hooks/pre-push");
        std::fs::write(
            &hook,
            format!("#!/bin/sh\nenv >> '{}'\n", env_dump.display()),
        )
        .unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();

        let authed = hermetic(Git::new(&repo))
            .with_http_credentials(HttpCredentials::new(
                &origin,
                "x-access-token",
                "gho_ephemeral",
            ))
            .with_config(["http.https://github.com/.extraHeader=Authorization: Basic c2VjcmV0"]);

        // Debug redacta tanto las credenciales efímeras como cualquier extraHeader manual.
        let debug = format!("{authed:?}");
        assert!(!debug.contains("gho_ephemeral"), "{debug}");
        assert!(!debug.contains("c2VjcmV0"), "{debug}");
        assert!(debug.contains("<redacted>"), "{debug}");

        let authed = hermetic(Git::new(&repo)).with_http_credentials(HttpCredentials::new(
            &origin,
            "x-access-token",
            "gho_ephemeral",
        ));
        authed.raw(["push", "origin", "main"]).unwrap();

        let dumped = std::fs::read_to_string(&env_dump).unwrap();
        assert!(!dumped.is_empty());
        assert!(!dumped.contains("gho_ephemeral"), "{dumped}");
    }
}
