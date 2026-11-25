//! "Clone from URL": clonar pegando la URL de cualquier remoto (HTTPS, SSH,
//! `git@host:org/repo.git`, `file://` o una ruta local). Aquí está lo que no
//! es de la vista: validar la URL, proponer el nombre de la carpeta,
//! comprobar el destino en disco y abrir el diálogo.
//!
//! El clon en sí es [`AppModel::clone_repository`] (progreso, cola,
//! reintento con credenciales HTTPS y avisos), el mismo que usa el selector de
//! GitHub.

use std::path::Path;

use gpui_kit::{Context, Task};

use crate::local_paths::clean_typed_path;
use crate::{AppEvent, AppModel, DialogRequest};

/// Esquemas que entiende `git clone` sin ayudantes externos.
const SCHEMES: &[&str] = &["https", "http", "ssh", "git", "file", "git+ssh", "ssh+git"];

/// Por qué la URL escrita no sirve para clonar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloneUrlError {
    Empty,
    /// Lleva espacios en medio.
    Whitespace,
    /// `github.com/org/repo`: falta `https://`.
    MissingScheme,
    UnsupportedScheme(String),
    /// `https://` sin servidor, `git@host:` sin ruta…
    Incomplete,
    NotAUrl,
}

impl CloneUrlError {
    /// El texto que se enseña bajo el campo.
    pub fn message(&self) -> String {
        match self {
            CloneUrlError::Empty => "Enter the URL of the repository to clone.".to_string(),
            CloneUrlError::Whitespace => "A repository URL can't contain spaces.".to_string(),
            CloneUrlError::MissingScheme => "Add https:// at the start of the URL.".to_string(),
            CloneUrlError::UnsupportedScheme(scheme) => {
                format!("{scheme}:// isn't supported. Use https://, ssh:// or git@host:path.")
            }
            CloneUrlError::Incomplete => "The URL is incomplete.".to_string(),
            CloneUrlError::NotAUrl => {
                "That doesn't look like a Git URL or a local repository path.".to_string()
            }
        }
    }
}

/// `C:` / `C:\…`: una unidad de Windows, no un host `C` de SSH.
fn has_drive_prefix(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

/// Una ruta local: absoluta, con `~` o relativa explícita.
fn is_local_path(text: &str, windows: bool) -> bool {
    text.starts_with('/')
        || text == "~"
        || text.starts_with("~/")
        || text.starts_with("./")
        || text.starts_with("../")
        || (windows
            && (has_drive_prefix(text)
                || text.starts_with("\\\\")
                || text.starts_with(".\\")
                || text.starts_with("..\\")
                || text.starts_with("~\\")))
}

/// Valida la URL escrita o pegada (sin espacios alrededor ni comillas) y la
/// devuelve limpia. Acepta lo que entiende `git clone`: `esquema://…`
/// ([`SCHEMES`]), la forma corta de SSH (`[usuario@]host:ruta`) y rutas
/// locales. `windows` decide si `C:\…` es una ruta.
pub fn parse_clone_url(text: &str, windows: bool) -> Result<String, CloneUrlError> {
    let url = clean_typed_path(text);
    if url.is_empty() {
        return Err(CloneUrlError::Empty);
    }
    if url.chars().any(char::is_whitespace) {
        return Err(CloneUrlError::Whitespace);
    }
    // Nunca una opción de git, aunque el backend ya la separa con `--`.
    if url.starts_with('-') {
        return Err(CloneUrlError::NotAUrl);
    }
    if let Some((scheme, rest)) = url.split_once("://") {
        let lower = scheme.to_ascii_lowercase();
        if !SCHEMES.contains(&lower.as_str()) {
            return Err(if scheme.is_empty() {
                CloneUrlError::NotAUrl
            } else {
                CloneUrlError::UnsupportedScheme(lower)
            });
        }
        let host = rest.split('/').next().unwrap_or_default();
        let host = host.rsplit_once('@').map_or(host, |(_, host)| host);
        let incomplete = if lower == "file" {
            rest.trim_matches('/').is_empty()
        } else {
            host.is_empty() || host.starts_with(':')
        };
        if incomplete {
            return Err(CloneUrlError::Incomplete);
        }
        return Ok(url.to_string());
    }
    if is_local_path(url, windows) {
        return Ok(url.to_string());
    }
    // `[usuario@]host:ruta`: los dos puntos antes de la primera barra.
    if let Some((host, path)) = url.split_once(':')
        && !host.contains(['/', '\\'])
    {
        let server = host.rsplit_once('@').map_or(host, |(_, server)| server);
        if server.is_empty() || path.trim_matches('/').is_empty() {
            return Err(CloneUrlError::Incomplete);
        }
        return Ok(url.to_string());
    }
    // `github.com/org/repo`: un servidor sin esquema.
    let first = url.split(['/', '\\']).next().unwrap_or_default();
    if first.contains('.') && url.contains('/') && !first.starts_with('.') {
        return Err(CloneUrlError::MissingScheme);
    }
    Err(CloneUrlError::NotAUrl)
}

/// El nombre de carpeta que `git clone` daría al clon:
/// `https://host/org/repo.git` → `repo`, `git@host:org/repo` → `repo`,
/// `/srv/git/repo/.git` → `repo`. `None` si no se puede deducir.
pub fn repository_name_from_url(url: &str) -> Option<String> {
    let url = clean_typed_path(url);
    // Sin el usuario de la URL: `https://u:token@host` no es un nombre.
    let without_userinfo = match url.split_once("://") {
        Some((_, rest)) => {
            let (authority, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
            let host = authority
                .rsplit_once('@')
                .map_or(authority, |(_, host)| host);
            format!("{host}{path}")
        }
        None => url.to_string(),
    };
    let mut trimmed = without_userinfo.trim_end_matches(['/', '\\']);
    for suffix in ["/.git", "\\.git"] {
        if let Some(rest) = trimmed.strip_suffix(suffix) {
            trimmed = rest.trim_end_matches(['/', '\\']);
        }
    }
    let last = trimmed.rsplit(['/', '\\', ':']).next().unwrap_or(trimmed);
    let name = last.strip_suffix(".git").unwrap_or(last);
    (!name.is_empty() && name != "." && name != ".." && name != "~").then(|| name.to_string())
}

/// La URL es de github.com (HTTPS, SSH o `git@github.com:`).
pub fn is_github_url(url: &str) -> bool {
    let url = clean_typed_path(url);
    let host = match url.split_once("://") {
        Some((_, rest)) => {
            let authority = rest.split('/').next().unwrap_or_default();
            authority
                .rsplit_once('@')
                .map_or(authority, |(_, host)| host)
        }
        None => url
            .split_once(':')
            .map(|(host, _)| host.rsplit_once('@').map_or(host, |(_, host)| host))
            .unwrap_or_default(),
    };
    let host = host.split(':').next().unwrap_or_default();
    host.eq_ignore_ascii_case("github.com") || host.eq_ignore_ascii_case("www.github.com")
}

/// Por qué no se puede clonar en ese destino.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloneDestinationError {
    /// No se eligió la carpeta donde clonar.
    NoFolder,
    FolderMissing,
    NotAFolder,
    EmptyName,
    InvalidName,
    /// Ya hay algo con ese nombre en la carpeta (y no es una carpeta vacía).
    Exists(String),
    Unreadable(String),
}

impl CloneDestinationError {
    pub fn message(&self) -> String {
        match self {
            CloneDestinationError::NoFolder => "Choose the folder to clone into.".to_string(),
            CloneDestinationError::FolderMissing => "That folder doesn't exist.".to_string(),
            CloneDestinationError::NotAFolder => "That's a file, not a folder.".to_string(),
            CloneDestinationError::EmptyName => "Enter a name for the new folder.".to_string(),
            CloneDestinationError::InvalidName => {
                "The folder name can't contain slashes or be . or ..".to_string()
            }
            CloneDestinationError::Exists(name) => {
                format!("{name} already exists in that folder.")
            }
            CloneDestinationError::Unreadable(message) => {
                format!("Can't read that folder: {message}")
            }
        }
    }

    /// El error es del nombre (se enseña bajo ese campo) y no de la carpeta.
    pub fn is_about_the_name(&self) -> bool {
        matches!(
            self,
            CloneDestinationError::EmptyName
                | CloneDestinationError::InvalidName
                | CloneDestinationError::Exists(_)
        )
    }
}

/// Valida el nombre de la carpeta nueva y lo devuelve sin espacios
/// alrededor. Una sola carpeta: sin separadores, ni `.` ni `..` (ni, en
/// Windows, los caracteres que no admite).
pub fn validate_folder_name(name: &str, windows: bool) -> Result<&str, CloneDestinationError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(CloneDestinationError::EmptyName);
    }
    let forbidden: &[char] = if windows {
        &['/', '\\', '<', '>', ':', '"', '|', '?', '*']
    } else {
        &['/', '\\']
    };
    if name == "." || name == ".." || name.contains(forbidden) {
        return Err(CloneDestinationError::InvalidName);
    }
    Ok(name)
}

/// Comprueba en disco el destino del clon (`target`, ya con el nombre de la
/// carpeta nueva): su carpeta debe existir y él no, o ser una carpeta vacía
/// (lo mismo que acepta `git clone`). Bloquea: llamarla fuera del hilo de UI.
pub fn check_clone_destination(target: &Path) -> Result<(), CloneDestinationError> {
    let parent = target
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or(CloneDestinationError::NoFolder)?;
    match std::fs::metadata(parent) {
        Ok(metadata) if !metadata.is_dir() => return Err(CloneDestinationError::NotAFolder),
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(CloneDestinationError::FolderMissing);
        }
        Err(error) => return Err(CloneDestinationError::Unreadable(error.to_string())),
    }
    let name = target
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    match std::fs::symlink_metadata(target) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(CloneDestinationError::Unreadable(error.to_string())),
        Ok(metadata) if metadata.is_dir() => {
            let empty = std::fs::read_dir(target)
                .map_err(|error| CloneDestinationError::Unreadable(error.to_string()))?
                .next()
                .is_none();
            if empty {
                Ok(())
            } else {
                Err(CloneDestinationError::Exists(name))
            }
        }
        Ok(_) => Err(CloneDestinationError::Exists(name)),
    }
}

impl AppModel {
    /// "Clone from URL…": pide a la cáscara el diálogo.
    pub fn show_clone_from_url(&mut self, cx: &mut Context<Self>) {
        cx.emit(AppEvent::ShowDialog(DialogRequest::CloneFromUrl));
    }

    /// [`check_clone_destination`] en un hilo de fondo.
    pub fn check_clone_destination(
        &mut self,
        target: &str,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), CloneDestinationError>> {
        let target = target.to_string();
        self.blocking(cx, move |_| check_clone_destination(Path::new(&target)))
    }

    /// Dónde proponer el clon: la carpeta del repositorio abierto más
    /// recientemente (suele ser la de los proyectos). `None` sin recientes.
    pub fn default_clone_folder(&self) -> Option<String> {
        self.recent_repositories()
            .iter()
            .map(|recent| recent.path.trim_end_matches(['/', '\\']))
            .find(|path| !path.is_empty())
            .and_then(|path| {
                let cut = path.rfind(['/', '\\'])?;
                let parent = &path[..cut];
                Some(if parent.is_empty() || parent.ends_with(':') {
                    // `/repo` → `/`, `C:\repo` → `C:\`.
                    path[..=cut].to_string()
                } else {
                    parent.to_string()
                })
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_the_urls_git_clone_understands() {
        for url in [
            "https://github.com/org/repo.git",
            "http://localhost:8080/repo",
            "ssh://git@host.example:2222/org/repo.git",
            "git://host/repo",
            "git@github.com:org/repo.git",
            "host:repo",
            "file:///srv/git/repo.git",
            "/srv/git/repo",
            "~/code/repo",
            "../sibling",
            "HTTPS://Host/Repo",
        ] {
            assert_eq!(parse_clone_url(url, false).as_deref(), Ok(url), "{url}");
        }
        // Pegada con comillas y espacios alrededor.
        assert_eq!(
            parse_clone_url("  'https://host/r.git' ", false).as_deref(),
            Ok("https://host/r.git")
        );
        // Rutas de Windows solo en Windows.
        for url in ["C:\\code\\repo", "\\\\server\\share\\repo", "C:/code/repo"] {
            assert_eq!(parse_clone_url(url, true).as_deref(), Ok(url), "{url}");
        }
    }

    #[test]
    fn explains_what_is_wrong_with_a_url() {
        let cases = [
            ("", CloneUrlError::Empty),
            ("   ", CloneUrlError::Empty),
            ("https://host/my repo", CloneUrlError::Whitespace),
            ("github.com/org/repo", CloneUrlError::MissingScheme),
            (
                "ftp://host/repo",
                CloneUrlError::UnsupportedScheme("ftp".into()),
            ),
            ("https://", CloneUrlError::Incomplete),
            ("https://user@/repo", CloneUrlError::Incomplete),
            ("file://", CloneUrlError::Incomplete),
            ("git@github.com:", CloneUrlError::Incomplete),
            ("--upload-pack=evil", CloneUrlError::NotAUrl),
            ("repo", CloneUrlError::NotAUrl),
            ("://host", CloneUrlError::NotAUrl),
        ];
        for (url, expected) in cases {
            assert_eq!(parse_clone_url(url, false), Err(expected), "{url:?}");
        }
        assert_eq!(
            CloneUrlError::MissingScheme.message(),
            "Add https:// at the start of the URL."
        );
    }

    #[test]
    fn derives_the_folder_name_like_git_clone() {
        for (url, name) in [
            ("https://github.com/org/repo.git", Some("repo")),
            ("https://github.com/org/repo/", Some("repo")),
            ("git@github.com:org/repo.git", Some("repo")),
            ("host:repo.git", Some("repo")),
            ("ssh://git@host:22/org/my-project", Some("my-project")),
            ("file:///srv/git/repo/.git", Some("repo")),
            ("/srv/git/repo.git/", Some("repo")),
            ("C:\\code\\repo", Some("repo")),
            ("https://user:token@host.example", Some("host.example")),
            ("https://", None),
            ("~", None),
            ("", None),
        ] {
            assert_eq!(repository_name_from_url(url).as_deref(), name, "{url}");
        }
    }

    #[test]
    fn recognises_github_urls() {
        for url in [
            "https://github.com/org/repo.git",
            "https://token@GitHub.com/org/repo",
            "git@github.com:org/repo.git",
            "ssh://git@github.com:22/org/repo",
        ] {
            assert!(is_github_url(url), "{url}");
        }
        for url in [
            "https://gitlab.com/org/repo",
            "https://github.com.evil.example/org/repo",
            "/srv/git/github.com",
            "git@host:github.com",
        ] {
            assert!(!is_github_url(url), "{url}");
        }
    }

    #[test]
    fn validates_the_folder_name() {
        assert_eq!(validate_folder_name("  repo ", false), Ok("repo"));
        assert_eq!(validate_folder_name("my:repo", false), Ok("my:repo"));
        for name in ["", "  "] {
            assert_eq!(
                validate_folder_name(name, false),
                Err(CloneDestinationError::EmptyName)
            );
        }
        for name in [".", "..", "a/b", "a\\b"] {
            assert_eq!(
                validate_folder_name(name, false),
                Err(CloneDestinationError::InvalidName),
                "{name}"
            );
        }
        assert_eq!(
            validate_folder_name("my:repo", true),
            Err(CloneDestinationError::InvalidName)
        );
    }

    #[test]
    fn checks_the_destination_on_disk() {
        let root = tempfile::tempdir().unwrap();
        let base = root.path();
        std::fs::create_dir(base.join("empty")).unwrap();
        std::fs::create_dir(base.join("full")).unwrap();
        std::fs::write(base.join("full").join("file.txt"), "x").unwrap();
        std::fs::write(base.join("file.txt"), "x").unwrap();

        assert_eq!(check_clone_destination(&base.join("new")), Ok(()));
        assert_eq!(check_clone_destination(&base.join("empty")), Ok(()));
        assert_eq!(
            check_clone_destination(&base.join("full")),
            Err(CloneDestinationError::Exists("full".into()))
        );
        assert_eq!(
            check_clone_destination(&base.join("file.txt")),
            Err(CloneDestinationError::Exists("file.txt".into()))
        );
        assert_eq!(
            check_clone_destination(&base.join("missing").join("new")),
            Err(CloneDestinationError::FolderMissing)
        );
        assert_eq!(
            check_clone_destination(&base.join("file.txt").join("new")),
            Err(CloneDestinationError::NotAFolder)
        );
        assert_eq!(
            check_clone_destination(Path::new("new")),
            Err(CloneDestinationError::NoFolder)
        );
    }
}
