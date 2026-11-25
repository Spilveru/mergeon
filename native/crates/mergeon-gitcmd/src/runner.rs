//! Ejecutor del binario `git`.
//!
//! Sustituye a `simple-git` y conserva la semántica de la que depende el resto
//! del port:
//!
//! - El entorno es el del usuario más `LC_ALL=C`, `LANG=C`,
//!   `GIT_TERMINAL_PROMPT=0` y `GIT_PAGER=cat` (salida estable y sin prompts
//!   interactivos).
//! - La configuración del usuario no cambia el formato de la salida que leen
//!   los parsers: todo git va con `OUTPUT_CONFIG` y `--no-optional-locks`,
//!   y los que pueden sacar parches (`diff`, `show`, `log`, `stash show`) con
//!   `--no-ext-diff`. Ver `git_arguments`.
//! - [`Git::raw`] falla **solo** si git sale con código distinto de cero **y**
//!   escribió algo en stderr; el mensaje del error es stdout seguido de stderr.
//!   Un código distinto de cero con stderr vacío (p. ej. `git diff --quiet`) se
//!   resuelve con stdout, igual que en simple-git.
//! - Con un [`OperationContext`] activo, el proceso se mata si la operación se
//!   cancela o si pasa `timeout` sin producir salida.
//! - Con [`Git::with_progress`] (y `--progress` en los argumentos) stderr se
//!   lee en streaming: las líneas de progreso se publican en el receptor del
//!   contexto y se quitan del stderr que se devuelve.

use std::ffi::{OsStr, OsString};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::context::{ControlCode, OperationContext, current_operation_context};
use crate::credentials;
use crate::progress::{ProgressFilter, ProgressSink};

/// Timeouts por tipo de operación (`GIT_OPERATION_TIMEOUTS` en la app Electron).
pub const LOCAL_TIMEOUT: Duration = Duration::from_millis(30_000);
pub const NETWORK_TIMEOUT: Duration = Duration::from_millis(120_000);
/// Timeout por inactividad aplicado a lecturas sin [`OperationContext`]
/// explícito: evita que un `git` colgado bloquee un hilo del pool para siempre.
pub const READ_TIMEOUT: Duration = Duration::from_millis(60_000);

/*
 * Configuración que se impone a todo git, antes de la de quien llama (que
 * puede pisarla con `with_config`). Lo que hay aquí cambia el formato de la
 * salida según la configuración del usuario y rompía los parsers:
 *
 * - `color.*`: con `color.ui=always` (o `color.diff=always`...) git pinta con
 *   ANSI aunque la salida vaya a una tubería. `color.ui` solo es el valor por
 *   defecto: cada `color.<comando>` lo pisa, así que van todos.
 * - `core.quotePath=false`: rutas no ASCII literales, no `"a\303\261o.txt"`.
 *   Ningún parser desentrecomilla; los que usan `-z` ya las recibían literales.
 *   Las rutas con comillas, tabuladores o saltos de línea siguen entrecomilladas.
 * - `diff.noprefix`, `diff.mnemonicPrefix`, `diff.srcPrefix`/`dstPrefix`: las
 *   cabeceras de los parches siempre como `a/<ruta>` y `b/<ruta>`.
 * - `log.showSignature`: verificar firmas mete la salida de gpg en el log.
 * - `core.pager=cat`: git no pagina hacia una tubería, pero así no depende de ello.
 */
const OUTPUT_CONFIG: &[&str] = &[
    "color.ui=false",
    "color.advice=false",
    "color.branch=false",
    "color.diff=false",
    "color.grep=false",
    "color.interactive=false",
    "color.push=false",
    "color.remote=false",
    "color.showBranch=false",
    "color.status=false",
    "color.transport=false",
    "core.fsmonitor=false",
    "core.quotePath=false",
    "core.pager=cat",
    "diff.noprefix=false",
    "diff.mnemonicPrefix=false",
    "diff.srcPrefix=a/",
    "diff.dstPrefix=b/",
    "log.showSignature=false",
];

/*
 * Argumentos completos de un git: opciones globales, `-c` y el comando.
 *
 * `--no-optional-locks` (= `GIT_OPTIONAL_LOCKS=0`): git se salta solo las
 * operaciones *opcionales* que toman un lock, que hoy son el refresco del
 * índice que `git status` escribe de paso. Las mutaciones toman sus locks
 * igual (no son opcionales), así que vale para todo; y las lecturas de fondo
 * dejan de chocar con el `index.lock` de un `git` que el usuario lance a la vez.
 *
 * `--no-ext-diff`: un `diff.external`, `GIT_EXTERNAL_DIFF` o el `command` de un
 * driver de `.gitattributes` sustituirían el parche por la salida de otro
 * programa. Va justo detrás del comando (`git stash show` incluido), antes de
 * cualquier `--` de rutas. `--no-textconv` no se pone: un textconv da un
 * parche válido (del texto convertido) y es lo que el usuario configuró ver.
 */
fn git_arguments(config: &[String], args: Vec<OsString>) -> Vec<OsString> {
    let mut full: Vec<OsString> = Vec::with_capacity(args.len() + 2 * OUTPUT_CONFIG.len() + 8);
    full.push("--no-optional-locks".into());
    for entry in OUTPUT_CONFIG
        .iter()
        .copied()
        .chain(config.iter().map(String::as_str))
    {
        full.push("-c".into());
        full.push(entry.into());
    }

    let patch_command = match (args.first(), args.get(1)) {
        (Some(first), _) if ["diff", "show", "log"].iter().any(|name| first == *name) => Some(1),
        (Some(first), Some(second)) if first == "stash" && second == "show" => Some(2),
        _ => None,
    };
    match patch_command {
        Some(at) => {
            let mut args = args.into_iter();
            full.extend(args.by_ref().take(at));
            full.push("--no-ext-diff".into());
            full.extend(args);
        }
        None => full.extend(args),
    }
    full
}

/// Resultado de un proceso git que llegó a terminar por sí mismo.
#[derive(Debug, Clone, Default)]
pub struct GitOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    /// `None` si el proceso terminó por una señal.
    pub exit_code: Option<i32>,
}

impl GitOutput {
    pub fn stdout_text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    pub fn stderr_text(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }

    pub fn success(&self) -> bool {
        self.exit_code == Some(0)
    }

    /// Criterio de error de simple-git: código de salida y stderr no vacío.
    pub fn is_task_error(&self) -> bool {
        self.exit_code != Some(0) && !self.stderr.is_empty()
    }
}

/// Resultado de [`Git::exec_capped`].
#[derive(Debug, Clone, Default)]
pub struct CappedOutput {
    /// Con `exceeded`, `stdout` es lo recibido hasta pasar el tope (algo más
    /// que el tope) y `exit_code` el de un proceso matado.
    pub output: GitOutput,
    /// La salida pasó del tope y git se mató sin esperar al resto.
    pub exceeded: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitErrorKind {
    /// No se pudo lanzar el binario (git no instalado, cwd inexistente...).
    Spawn,
    /// Git terminó con error.
    Exit,
    /// La operación se canceló o venció su timeout.
    Control,
}

#[derive(Debug, Clone)]
pub struct GitError {
    pub kind: GitErrorKind,
    /// stdout + stderr, sin recortar (lo que simple-git expone como `message`).
    pub message: String,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub control: Option<ControlCode>,
}

impl GitError {
    pub fn control(code: ControlCode) -> Self {
        Self {
            kind: GitErrorKind::Control,
            message: code.message().to_string(),
            stdout: String::new(),
            stderr: String::new(),
            exit_code: None,
            control: Some(code),
        }
    }

    pub fn from_output(output: &GitOutput) -> Self {
        let stdout = output.stdout_text();
        let stderr = output.stderr_text();
        Self {
            kind: GitErrorKind::Exit,
            message: format!("{stdout}{stderr}"),
            stdout,
            stderr,
            exit_code: output.exit_code,
            control: None,
        }
    }

    /// Error sin proceso detrás (validaciones propias que deben viajar como error de git).
    pub fn message(message: impl Into<String>) -> Self {
        Self {
            kind: GitErrorKind::Exit,
            message: message.into(),
            stdout: String::new(),
            stderr: String::new(),
            exit_code: None,
            control: None,
        }
    }
}

impl std::fmt::Display for GitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for GitError {}

pub type GitResult<T> = Result<T, GitError>;

/// Un `git` configurado para un directorio. Barato de crear: hay que crear uno
/// por operación, en el hilo que la ejecuta, para que herede su contexto.
#[derive(Clone)]
pub struct Git {
    cwd: Option<PathBuf>,
    config: Vec<String>,
    env: Vec<(OsString, OsString)>,
    context: Option<OperationContext>,
    /// Leer stderr como progreso (`with_progress`).
    progress: bool,
    /// Receptor propio; sin él, el del contexto.
    progress_sink: Option<ProgressSink>,
    /// Pasar a los git de red las credenciales HTTP de la sesión
    /// (ver [`crate::credentials`]).
    session_credentials: bool,
    /// Credenciales HTTP específicas de este comando (p. ej. el token de
    /// GitHub). Viajan en archivo efímero `0600`, nunca en `argv`.
    extra_credentials: Vec<Arc<credentials::HttpCredentials>>,
}

fn redact_config_entry(entry: &str) -> &str {
    let lower = entry.to_ascii_lowercase();
    if lower.contains("extraheader=") || lower.contains("authorization:") {
        "<redacted>"
    } else {
        entry
    }
}

impl std::fmt::Debug for Git {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let redacted_config: Vec<&str> = self
            .config
            .iter()
            .map(|entry| redact_config_entry(entry))
            .collect();
        f.debug_struct("Git")
            .field("cwd", &self.cwd)
            .field("config", &redacted_config)
            .field("env", &self.env)
            .field("context", &self.context)
            .field("progress", &self.progress)
            .field("session_credentials", &self.session_credentials)
            .field("extra_credentials", &self.extra_credentials)
            .finish()
    }
}

impl Git {
    /// Git sobre `repo_path`, con el contexto de operación del hilo actual.
    pub fn new(repo_path: impl AsRef<Path>) -> Self {
        Self {
            cwd: Some(repo_path.as_ref().to_path_buf()),
            config: Vec::new(),
            env: Vec::new(),
            context: current_operation_context(),
            progress: false,
            progress_sink: None,
            session_credentials: true,
            extra_credentials: Vec::new(),
        }
    }

    /// Git sin directorio de trabajo (clone, ls-remote sobre una URL...).
    pub fn without_cwd() -> Self {
        Self {
            cwd: None,
            config: Vec::new(),
            env: Vec::new(),
            context: current_operation_context(),
            progress: false,
            progress_sink: None,
            session_credentials: true,
            extra_credentials: Vec::new(),
        }
    }

    pub fn repo_path(&self) -> Option<&Path> {
        self.cwd.as_deref()
    }

    /// Añade entradas `clave=valor` que se pasan como `-c` antes del subcomando.
    pub fn with_config<I, S>(mut self, entries: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.config.extend(entries.into_iter().map(Into::into));
        self
    }

    /// Adjunta credenciales HTTP efímeras solo para comandos de red de este
    /// `Git` (reemplaza las de sesión del mismo origen).
    pub fn with_http_credentials(mut self, creds: credentials::HttpCredentials) -> Self {
        self.extra_credentials
            .retain(|known| known.origin() != creds.origin());
        self.extra_credentials.push(Arc::new(creds));
        self
    }

    /// Autentica contra `https://github.com` usando un helper efímero `0600`
    /// (sin pasar el token por `argv` ni `GIT_CONFIG_PARAMETERS`).
    pub fn with_github_token(self, token: Option<&str>) -> Self {
        match crate::auth::github_credentials(token) {
            Some(creds) => self.with_http_credentials(creds),
            None => self,
        }
    }

    /// Variable de entorno extra para este git (p. ej. `GIT_SEQUENCE_EDITOR`).
    pub fn with_env(mut self, key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> Self {
        self.env
            .push((key.as_ref().to_os_string(), value.as_ref().to_os_string()));
        self
    }

    /// Sin las credenciales HTTP de la sesión (para hablar con los helpers
    /// del propio usuario: `git credential approve`, `config --get-urlmatch`).
    pub fn without_session_credentials(mut self) -> Self {
        self.session_credentials = false;
        self
    }

    pub fn with_context(mut self, context: Option<OperationContext>) -> Self {
        self.context = context;
        self
    }

    pub fn context(&self) -> Option<&OperationContext> {
        self.context.as_ref()
    }

    /*
     * Lee stderr como progreso: las líneas `Receiving objects:  45% ...` van
     * al receptor del contexto de la operación (si lo hay) y no al stderr del
     * resultado, que queda como sin `--progress`. El `--progress` lo pone
     * quien arma los argumentos (va en sitios distintos según el comando).
     */
    pub fn with_progress(mut self) -> Self {
        self.progress = true;
        self
    }

    /// Como [`Git::with_progress`], publicando en `sink` en vez de en el contexto.
    pub fn with_progress_sink(mut self, sink: ProgressSink) -> Self {
        self.progress = true;
        self.progress_sink = Some(sink);
        self
    }

    fn progress_target(&self) -> Option<Option<ProgressSink>> {
        self.progress.then(|| {
            self.progress_sink.clone().or_else(|| {
                self.context
                    .as_ref()
                    .and_then(OperationContext::progress_sink)
            })
        })
    }

    /// Ejecuta git y devuelve su salida completa sea cual sea el código de
    /// salida. Solo falla si no se pudo lanzar o si la operación fue controlada
    /// (cancelación / timeout).
    pub fn exec<I, S>(&self, args: I) -> GitResult<GitOutput>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.exec_inner(args, None, None)
            .map(|capped| capped.output)
    }

    /*
     * Igual que [`Git::exec`], pero sin guardar más de `limit` bytes de
     * stdout: al pasarlo deja de leer, mata a git (y a lo que haya lanzado) y
     * marca `exceeded`. Para salidas que no se van a usar si son enormes (un
     * parche de 500 MB): ni se cargan en memoria ni se espera a que git acabe.
     */
    pub fn exec_capped<I, S>(&self, args: I, limit: usize) -> GitResult<CappedOutput>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.exec_inner(args, None, Some(limit))
    }

    /// Igual que [`Git::exec`], escribiendo `stdin` en la entrada del proceso.
    pub fn exec_with_stdin<I, S>(&self, args: I, stdin: &[u8]) -> GitResult<GitOutput>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.exec_inner(args, Some(stdin), None)
            .map(|capped| capped.output)
    }

    /// Salida de git como texto, sin recortar, con el criterio de error de simple-git.
    pub fn raw<I, S>(&self, args: I) -> GitResult<String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let output = self.exec(args)?;
        if output.is_task_error() {
            return Err(GitError::from_output(&output));
        }
        Ok(output.stdout_text())
    }

    /// [`Git::raw`] con stdin.
    pub fn raw_with_stdin<I, S>(&self, args: I, stdin: &[u8]) -> GitResult<String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let output = self.exec_with_stdin(args, stdin)?;
        if output.is_task_error() {
            return Err(GitError::from_output(&output));
        }
        Ok(output.stdout_text())
    }

    /// Salida de git en bytes (contenido de archivos que puede no ser UTF-8).
    pub fn raw_bytes<I, S>(&self, args: I) -> GitResult<Vec<u8>>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let output = self.exec(args)?;
        if output.is_task_error() {
            return Err(GitError::from_output(&output));
        }
        Ok(output.stdout)
    }

    /// `git rev-parse <args>`, recortado (como `git.revparse()` de simple-git).
    pub fn revparse<I, S>(&self, args: I) -> GitResult<String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut full: Vec<OsString> = vec![OsString::from("rev-parse")];
        full.extend(args.into_iter().map(|arg| arg.as_ref().to_os_string()));
        Ok(self.raw(full)?.trim().to_string())
    }

    fn command<I, S>(&self, args: I) -> (Command, credentials::CredentialGuard)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut command = Command::new("git");
        let args: Vec<OsString> = args
            .into_iter()
            .map(|arg| arg.as_ref().to_os_string())
            .collect();
        // Credenciales HTTP (sesión y/o token de GitHub del comando): el helper
        // va en `-c` y lee un archivo efímero `0600` (nada en argv ni en el
        // entorno de los hooks). Las credenciales explícitas de este `Git`
        // prevalecen sobre las de sesión para el mismo origen.
        let mut attached = Vec::new();
        if credentials::is_network_command(&args) {
            if self.session_credentials {
                attached = credentials::session_credentials();
            }
            for creds in &self.extra_credentials {
                attached.retain(|known| known.origin() != creds.origin());
                attached.push(Arc::clone(creds));
            }
        }
        let (credential_config, credential_guard) = credentials::credential_arguments(&attached);
        if credential_config.is_empty() {
            command.args(git_arguments(&self.config, args));
        } else {
            let config: Vec<String> = self
                .config
                .iter()
                .cloned()
                .chain(credential_config)
                .collect();
            command.args(git_arguments(&config, args));
        }
        if let Some(cwd) = &self.cwd {
            command.current_dir(cwd);
        }
        command
            .env("LC_ALL", "C")
            .env("LANG", "C")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_PAGER", "cat");
        for (key, value) in &self.env {
            command.env(key, value);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // CREATE_NO_WINDOW: sin esto cada git abre una consola un instante.
            command.creation_flags(0x0800_0000);
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // Grupo de procesos propio: al cancelar se mata el grupo entero
            // (git y lo que haya lanzado: hooks, ssh, helpers de credenciales).
            command.process_group(0);
        }
        (command, credential_guard)
    }

    fn exec_inner<I, S>(
        &self,
        args: I,
        stdin: Option<&[u8]>,
        stdout_limit: Option<usize>,
    ) -> GitResult<CappedOutput>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        if let Some(context) = &self.context
            && context.is_cancelled()
        {
            return Err(GitError::control(
                context.control_code().unwrap_or(ControlCode::Cancelled),
            ));
        }

        let (mut command, _credential_guard) = self.command(args);
        command
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let child = command.spawn().map_err(|error| GitError {
            kind: GitErrorKind::Spawn,
            message: format!("spawn git failed: {error}"),
            stdout: String::new(),
            stderr: String::new(),
            exit_code: None,
            control: None,
        })?;
        // Guard RAII: si este hilo entra en pánico antes de terminar, mata el
        // árbol del proceso git y lo recoge para no dejar procesos huérfanos.
        struct ChildGuard {
            child: Option<Child>,
        }
        impl Drop for ChildGuard {
            fn drop(&mut self) {
                if let Some(mut child) = self.child.take() {
                    kill_tree(&mut child);
                }
            }
        }
        let mut child_guard = ChildGuard { child: Some(child) };
        let child = child_guard.child.as_mut().expect("child present");

        let started = Instant::now();
        // Milisegundos desde `started` de la última salida recibida.
        let last_activity = Arc::new(AtomicU64::new(0));

        let stdin_writer = stdin.map(|bytes| {
            let mut pipe = child.stdin.take().expect("stdin piped");
            let bytes = bytes.to_vec();
            std::thread::spawn(move || {
                // Si git cierra antes de leer todo, el error de escritura no importa.
                let _ = pipe.write_all(&bytes);
            })
        });
        let exceeded = Arc::new(AtomicBool::new(false));
        let stdout_reader = spawn_reader(
            child.stdout.take(),
            started,
            last_activity.clone(),
            stdout_limit.map(|limit| (limit, exceeded.clone())),
        );
        let stderr_reader = match self.progress_target() {
            Some(sink) => {
                spawn_progress_reader(child.stderr.take(), started, last_activity.clone(), sink)
            }
            None => spawn_reader(child.stderr.take(), started, last_activity.clone(), None),
        };

        let fallback_context;
        let effective_context = match &self.context {
            Some(context) => Some(context),
            None => {
                fallback_context = OperationContext::new(None, READ_TIMEOUT);
                Some(&fallback_context)
            }
        };
        let waited =
            wait_with_control(child, effective_context, started, &last_activity, &exceeded);
        // El proceso ya terminó o fue matado y recogido por `wait_with_control`.
        child_guard.child = None;

        let stdout = stdout_reader.join().unwrap_or_default();
        let stderr = stderr_reader.join().unwrap_or_default();
        if let Some(writer) = stdin_writer {
            let _ = writer.join();
        }

        match waited {
            Ok(Some(status)) => Ok(CappedOutput {
                output: GitOutput {
                    stdout,
                    stderr,
                    exit_code: status.code(),
                },
                exceeded: exceeded.load(Ordering::Relaxed),
            }),
            Ok(None) | Err(None) => Err(GitError {
                kind: GitErrorKind::Spawn,
                message: "waiting for git failed".to_string(),
                stdout: String::from_utf8_lossy(&stdout).into_owned(),
                stderr: String::from_utf8_lossy(&stderr).into_owned(),
                exit_code: None,
                control: None,
            }),
            Err(Some(code)) => Err(GitError::control(code)),
        }
    }
}

/// Lee una tubería entera; con `limit`, deja de leer (y cierra la tubería)
/// en cuanto ha recibido más de `limit` bytes, y lo marca en `exceeded`.
fn spawn_reader<R: Read + Send + 'static>(
    pipe: Option<R>,
    started: Instant,
    last_activity: Arc<AtomicU64>,
    limit: Option<(usize, Arc<AtomicBool>)>,
) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut collected = Vec::new();
        let Some(mut pipe) = pipe else {
            return collected;
        };
        let mut buffer = [0u8; 64 * 1024];
        loop {
            match pipe.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    collected.extend_from_slice(&buffer[..read]);
                    last_activity.store(started.elapsed().as_millis() as u64, Ordering::Relaxed);
                    if let Some((limit, exceeded)) = &limit
                        && collected.len() > *limit
                    {
                        exceeded.store(true, Ordering::Relaxed);
                        break;
                    }
                }
            }
        }
        collected
    })
}

/// Lee stderr en streaming separando el progreso (ver [`ProgressFilter`]):
/// publica cada actualización en `sink` y devuelve el resto.
fn spawn_progress_reader<R: Read + Send + 'static>(
    pipe: Option<R>,
    started: Instant,
    last_activity: Arc<AtomicU64>,
    sink: Option<ProgressSink>,
) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut filter = ProgressFilter::new();
        let Some(mut pipe) = pipe else {
            return Vec::new();
        };
        let publish = |updates: Vec<crate::progress::GitProgress>| {
            if let Some(sink) = &sink {
                for update in updates {
                    sink(update);
                }
            }
        };
        let mut buffer = [0u8; 16 * 1024];
        loop {
            match pipe.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    last_activity.store(started.elapsed().as_millis() as u64, Ordering::Relaxed);
                    publish(filter.push(&buffer[..read]));
                }
            }
        }
        publish(filter.finish());
        filter.into_kept()
    })
}

/// Mata a git y a sus descendientes. Matar solo a git deja vivos a los nietos
/// (un hook, ssh) con las tuberías de salida heredadas, y la lectura de esas
/// tuberías no terminaría hasta que ellos salieran por su cuenta.
fn kill_tree(child: &mut Child) {
    #[cfg(unix)]
    {
        // SAFETY: `kill` con un pid negativo señala al grupo que creó
        // `process_group(0)`; no toca memoria.
        unsafe {
            libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL);
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = Command::new("taskkill")
            .args(["/T", "/F", "/PID", &child.id().to_string()])
            .creation_flags(0x0800_0000)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
}

/// Espera al proceso vigilando cancelación y timeout por inactividad (si hay
/// contexto) y el tope de salida (`exceeded`: se mata y se da por terminado).
/// `Err(Some(code))`: se mató por control. `Err(None)`: falló la espera.
fn wait_with_control(
    child: &mut Child,
    context: Option<&OperationContext>,
    started: Instant,
    last_activity: &AtomicU64,
    exceeded: &AtomicBool,
) -> Result<Option<std::process::ExitStatus>, Option<ControlCode>> {
    let mut pause = Duration::from_micros(500);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(Some(status)),
            Ok(None) => {}
            Err(_) => return Err(None),
        }

        if exceeded.load(Ordering::Relaxed) {
            kill_tree(child);
            return child.wait().map(Some).map_err(|_| None);
        }

        let Some(context) = context else {
            std::thread::sleep(pause);
            pause = (pause * 2).min(Duration::from_millis(10));
            continue;
        };

        if context.is_cancelled() {
            kill_tree(child);
            let _ = child.wait();
            return Err(Some(
                context.control_code().unwrap_or(ControlCode::Cancelled),
            ));
        }

        let idle = started
            .elapsed()
            .saturating_sub(Duration::from_millis(last_activity.load(Ordering::Relaxed)));
        if idle >= context.timeout() {
            context.set_control_code(ControlCode::Timeout);
            kill_tree(child);
            let _ = child.wait();
            return Err(Some(ControlCode::Timeout));
        }

        std::thread::sleep(pause);
        pause = (pause * 2).min(Duration::from_millis(10));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::run_with_operation_context;

    fn init_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        Git::new(dir.path()).raw(["init", "-b", "main"]).unwrap();
        dir
    }

    /// Las credenciales de la sesión van a los git de red como helper en
    /// `-c` que lee de un archivo efímero (0600), nunca en argv, ni en el
    /// entorno (para no filtrarse a hooks), ni en el `Debug`; los demás
    /// comandos (y quien las desactiva) no las reciben.
    #[test]
    fn session_credentials_reach_network_commands_through_ephemeral_files_only() {
        use crate::credentials::{
            HttpCredentials, forget_session_credentials, set_session_credentials,
        };

        let origin = "https://runner-test.example:7443";
        set_session_credentials(HttpCredentials::new(origin, "runner-user", "runner-s3cret"));
        let dir = tempfile::tempdir().unwrap();
        let git = Git::new(dir.path());

        let (command, _guard) = git.command(["push", "origin", "main"]);
        let argv: Vec<String> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert!(
            argv.iter().all(|arg| !arg.contains("runner-s3cret")),
            "{argv:?}"
        );
        assert!(
            argv.iter().all(|arg| !arg.contains("runner-user")),
            "{argv:?}"
        );
        assert!(
            argv.contains(&format!("credential.{origin}.helper=")),
            "{argv:?}"
        );
        assert!(
            argv.iter()
                .any(|arg| arg.starts_with(&format!("credential.{origin}.helper=!f()"))),
            "{argv:?}"
        );
        let secret_in_env = command.get_envs().any(|(key, value)| {
            key.to_string_lossy().starts_with("MERGEON_HTTP_")
                || value.is_some_and(|value| {
                    let text = value.to_string_lossy();
                    text.contains("runner-s3cret") || text.contains("runner-user")
                })
        });
        assert!(
            !secret_in_env,
            "session credentials must never enter git's environment"
        );
        assert!(!format!("{git:?}").contains("runner-s3cret"));

        for (command, _guard) in [
            git.command(["commit", "-m", "x"]),
            git.clone().without_session_credentials().command(["push"]),
        ] {
            assert!(
                command
                    .get_args()
                    .all(|arg| !arg.to_string_lossy().contains(origin))
            );
            assert!(
                command
                    .get_envs()
                    .all(|(key, _)| !key.to_string_lossy().starts_with("MERGEON_HTTP_"))
            );
        }

        forget_session_credentials(origin);
    }

    #[test]
    fn raw_returns_stdout_untrimmed_and_revparse_trims() {
        let dir = init_repo();
        let git = Git::new(dir.path());
        assert_eq!(git.raw(["rev-parse", "--git-dir"]).unwrap(), ".git\n");
        assert_eq!(git.revparse(["--git-dir"]).unwrap(), ".git");
    }

    #[test]
    fn raw_fails_only_when_git_wrote_to_stderr() {
        let dir = init_repo();
        let git = Git::new(dir.path());
        let error = git
            .raw(["rev-parse", "--verify", "missing-ref"])
            .unwrap_err();
        assert_eq!(error.kind, GitErrorKind::Exit);
        assert!(error.message.contains("fatal"), "{}", error.message);

        // Código 1 sin stderr: no es un error (criterio de simple-git).
        std::fs::write(dir.path().join("a.txt"), "x").unwrap();
        git.raw(["add", "a.txt"]).unwrap();
        let output = git.exec(["diff", "--cached", "--quiet"]).unwrap();
        assert_eq!(output.exit_code, Some(1));
        assert_eq!(git.raw(["diff", "--cached", "--quiet"]).unwrap(), "");
    }

    #[test]
    fn config_entries_are_passed_before_the_command() {
        let dir = init_repo();
        let git = Git::new(dir.path()).with_config(["user.name=Config Test"]);
        assert_eq!(
            git.raw(["config", "user.name"]).unwrap().trim(),
            "Config Test"
        );
    }

    fn arguments(config: &[&str], args: &[&str]) -> Vec<String> {
        let config: Vec<String> = config.iter().map(|entry| entry.to_string()).collect();
        let args = args.iter().map(OsString::from).collect();
        git_arguments(&config, args)
            .into_iter()
            .map(|arg| arg.into_string().unwrap())
            .collect()
    }

    /// Lo que va detrás de las opciones globales y la config impuesta.
    fn after_globals(full: &[String]) -> &[String] {
        &full[1 + 2 * OUTPUT_CONFIG.len()..]
    }

    #[test]
    fn every_command_gets_the_output_config_before_the_callers() {
        let full = arguments(&["user.name=X"], &["status", "--porcelain"]);
        assert_eq!(full[0], "--no-optional-locks");
        assert_eq!(full[1], "-c");
        assert_eq!(full[2], "color.ui=false");
        assert!(full.iter().any(|arg| arg == "core.quotePath=false"));
        // La config de quien llama va después: puede pisar la impuesta.
        assert_eq!(
            after_globals(&full),
            ["-c", "user.name=X", "status", "--porcelain"]
        );
    }

    #[test]
    fn patch_commands_get_no_ext_diff_right_after_the_command() {
        for (args, expected) in [
            (
                &["diff", "--cached", "--", "a"][..],
                &["diff", "--no-ext-diff", "--cached", "--", "a"][..],
            ),
            (&["show", "HEAD"], &["show", "--no-ext-diff", "HEAD"]),
            (&["log", "-1"], &["log", "--no-ext-diff", "-1"]),
            (
                &["stash", "show", "-p", "stash@{0}"],
                &["stash", "show", "--no-ext-diff", "-p", "stash@{0}"],
            ),
            (&["stash", "list"], &["stash", "list"]),
            (&["status"], &["status"]),
            (&["--version"], &["--version"]),
        ] {
            assert_eq!(after_globals(&arguments(&[], args)), expected);
        }
    }

    #[test]
    fn user_color_config_does_not_reach_the_output() {
        let dir = init_repo();
        let git = Git::new(dir.path());
        git.raw(["config", "color.ui", "always"]).unwrap();
        git.raw(["config", "color.diff", "always"]).unwrap();
        std::fs::write(dir.path().join("a.txt"), "x\n").unwrap();
        git.raw(["add", "a.txt"]).unwrap();
        let diff = git.raw(["diff", "--cached"]).unwrap();
        assert!(diff.starts_with("diff --git a/a.txt b/a.txt\n"), "{diff:?}");
        // Las opciones globales no rompen un comando sin repo.
        assert!(
            Git::without_cwd()
                .raw(["--version"])
                .unwrap()
                .starts_with("git version")
        );
    }

    #[test]
    fn stdin_is_forwarded() {
        let dir = init_repo();
        let git = Git::new(dir.path());
        let hash = git
            .raw_with_stdin(["hash-object", "--stdin"], b"hello\n")
            .unwrap();
        assert_eq!(hash.trim(), "ce013625030ba8dba906f756967f9e9ca394464a");
    }

    #[test]
    fn cancelled_context_stops_the_process() {
        let dir = init_repo();
        let context = OperationContext::new(Some("op".into()), Duration::from_secs(30));
        let handle = context.clone();
        let started = Instant::now();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            handle.cancel();
        });
        let error = run_with_operation_context(context, || {
            // `cat-file --batch` espera stdin indefinidamente si nadie lo cierra.
            let (mut command, _guard) = Git::new(dir.path()).command(["cat-file", "--batch"]);
            command
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            let mut child = command.spawn().unwrap();
            let context = current_operation_context().unwrap();
            let last = AtomicU64::new(0);
            let exceeded = AtomicBool::new(false);
            wait_with_control(&mut child, Some(&context), Instant::now(), &last, &exceeded)
                .unwrap_err()
        });
        assert_eq!(error, Some(ControlCode::Cancelled));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[cfg(unix)]
    #[test]
    fn cancelling_also_stops_grandchildren_holding_the_pipes() {
        let dir = init_repo();
        let context = OperationContext::new(None, Duration::from_secs(30));
        let handle = context.clone();
        let started = Instant::now();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            handle.cancel();
        });
        // El alias lanza un shell cuyo `sleep` hereda stdout/stderr de git.
        let error = run_with_operation_context(context, || {
            Git::new(dir.path())
                .with_config(["alias.hang=!sleep 30"])
                .raw(["hang"])
                .unwrap_err()
        });
        assert_eq!(error.control, Some(ControlCode::Cancelled));
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn an_already_cancelled_context_does_not_spawn() {
        let dir = init_repo();
        let context = OperationContext::new(None, Duration::from_secs(30));
        context.cancel();
        let error = run_with_operation_context(context, || {
            Git::new(dir.path()).raw(["status"]).unwrap_err()
        });
        assert_eq!(error.kind, GitErrorKind::Control);
        assert_eq!(error.control, Some(ControlCode::Cancelled));
    }

    #[test]
    fn idle_timeout_kills_the_process() {
        let dir = init_repo();
        let context = OperationContext::new(None, Duration::from_millis(200));
        let outcome = run_with_operation_context(context.clone(), || {
            let (mut command, _guard) = Git::new(dir.path()).command(["cat-file", "--batch"]);
            command
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            let mut child = command.spawn().unwrap();
            let last = AtomicU64::new(0);
            let exceeded = AtomicBool::new(false);
            wait_with_control(&mut child, Some(&context), Instant::now(), &last, &exceeded)
                .unwrap_err()
        });
        assert_eq!(outcome, Some(ControlCode::Timeout));
        assert_eq!(context.control_code(), Some(ControlCode::Timeout));
    }

    /// Repo con un archivo de `size` bytes en HEAD, para sacar salidas grandes.
    fn repo_with_file(size: usize) -> tempfile::TempDir {
        let dir = init_repo();
        let git = Git::new(dir.path()).with_config(["user.name=T", "user.email=t@e"]);
        let line = "0123456789abcdefghijklmnopqrstuvwxyz0123456789abcdefghijklmnopqrstuvwxyz\n";
        std::fs::write(dir.path().join("big.txt"), line.repeat(size / line.len())).unwrap();
        git.raw(["add", "big.txt"]).unwrap();
        git.raw(["commit", "-q", "-m", "big"]).unwrap();
        dir
    }

    #[test]
    fn capped_output_stops_reading_and_kills_git_past_the_limit() {
        let dir = repo_with_file(10 * 1024 * 1024);
        let git = Git::new(dir.path());
        let limit = 256 * 1024;

        let capped = git.exec_capped(["show", "HEAD"], limit).unwrap();
        assert!(capped.exceeded);
        // Se para en la primera lectura que pasa el tope, no al final.
        assert!(capped.output.stdout.len() > limit);
        assert!(capped.output.stdout.len() <= limit + 64 * 1024);
        assert!(!capped.output.success());

        // Por debajo del tope es una ejecución normal, también con contexto.
        let context = OperationContext::new(None, Duration::from_secs(30));
        let small = run_with_operation_context(context, || {
            git.exec_capped(["log", "-1", "--format=%s"], limit)
                .unwrap()
        });
        assert!(!small.exceeded);
        assert_eq!(small.output.stdout_text(), "big\n");
        assert!(small.output.success());
    }

    /// Repo con `files` archivos de contenido poco comprimible.
    fn repo_with_files(files: usize) -> tempfile::TempDir {
        let dir = init_repo();
        let git = Git::new(dir.path()).with_config(["user.name=T", "user.email=t@e"]);
        let mut seed = 0x2545_f491_4f6c_dd1d_u64;
        for index in 0..files {
            let mut text = String::with_capacity(8 * 1024);
            while text.len() < 8 * 1024 {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                text.push_str(&format!("{seed:016x}\n"));
            }
            std::fs::write(dir.path().join(format!("f{index}.txt")), text).unwrap();
        }
        git.raw(["add", "."]).unwrap();
        git.raw(["commit", "-q", "-m", "files"]).unwrap();
        dir
    }

    #[test]
    fn progress_is_streamed_and_removed_from_stderr() {
        let source = repo_with_files(300);
        let target = tempfile::tempdir().unwrap();
        let url = format!("file://{}", source.path().display());
        let destination = target.path().join("clone");
        let updates = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink_updates = updates.clone();
        let output = Git::without_cwd()
            .with_progress_sink(Arc::new(move |progress| {
                sink_updates.lock().unwrap().push(progress)
            }))
            .exec([
                OsStr::new("clone"),
                OsStr::new("--progress"),
                OsStr::new(&url),
                destination.as_os_str(),
            ])
            .unwrap();
        assert!(output.success(), "{}", output.stderr_text());
        // stderr queda como sin `--progress`.
        let stderr = output.stderr_text();
        assert!(!stderr.contains('%'), "{stderr:?}");
        assert!(stderr.contains("Cloning into"), "{stderr:?}");

        let updates = updates.lock().unwrap();
        let phases: Vec<&str> = updates.iter().map(|update| update.phase.as_str()).collect();
        assert!(phases.contains(&"Receiving objects"), "{phases:?}");
        assert!(
            phases.contains(&"Resolving deltas") || phases.len() > 2,
            "{phases:?}"
        );
        assert!(updates.iter().any(|update| update.remote), "{phases:?}");
        let receiving: Vec<_> = updates
            .iter()
            .filter(|update| update.phase == "Receiving objects")
            .collect();
        assert!(receiving.last().unwrap().done);
        assert_eq!(receiving.last().unwrap().fraction(), Some(1.0));
    }

    #[test]
    fn progress_goes_to_the_operation_context() {
        let source = repo_with_files(20);
        let target = tempfile::tempdir().unwrap();
        let url = format!("file://{}", source.path().display());
        let destination = target.path().join("clone");
        let context = OperationContext::new(None, Duration::from_secs(30));
        let updates = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink_updates = updates.clone();
        context.set_progress_sink(Arc::new(move |progress| {
            sink_updates.lock().unwrap().push(progress)
        }));
        run_with_operation_context(context, || {
            Git::without_cwd()
                .with_progress()
                .raw([
                    OsStr::new("clone"),
                    OsStr::new("--progress"),
                    OsStr::new(&url),
                    destination.as_os_str(),
                ])
                .unwrap();
            // Sin `with_progress` el stderr no se toca.
            let fetch = Git::new(&destination)
                .exec(["fetch", "--progress", "origin"])
                .unwrap();
            assert!(fetch.success());
        });
        assert!(
            updates
                .lock()
                .unwrap()
                .iter()
                .any(|update| update.phase == "Receiving objects")
        );
    }

    #[test]
    fn missing_directory_is_a_spawn_error() {
        let error = Git::new("/definitely/not/a/real/path")
            .raw(["status"])
            .unwrap_err();
        assert_eq!(error.kind, GitErrorKind::Spawn);
    }
}
