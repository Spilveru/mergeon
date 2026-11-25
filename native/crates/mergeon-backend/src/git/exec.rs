//! Acceso a git y al disco para los handlers con lógica propia.

use std::io;
use std::path::{Component, Path, PathBuf};

use mergeon_gitcmd::{Git, GitError, GitResult};

use crate::mutation::Thrown;

/// Lo que lanza simple-git al crear una instancia sobre un directorio que no existe.
pub(crate) const MISSING_DIRECTORY: &str =
    "Cannot use simple-git on a directory that does not exist";

const UNSAFE_PACK: &str =
    "Use of --upload-pack or --receive-pack is not permitted without enabling allowUnsafePack";
const UNSAFE_TEMPLATE: &str =
    "Use of --template is not permitted without enabling allowUnsafeTemplateDir";

/// `getGit(repoPath)`: simple-git comprueba que el directorio exista antes de nada.
pub(crate) fn open(repo_path: &Path) -> GitResult<Git> {
    if repo_path.is_dir() {
        Ok(Git::new(repo_path))
    } else {
        Err(GitError::message(MISSING_DIRECTORY))
    }
}

/// Mensaje con el que simple-git rechazaría el comando
/// (`blockUnsafeOperationsPlugin`). `args[0]` es el subcomando; lo que va
/// después de `--` son rutas. Los handlers de este crate solo pasan a git un
/// nombre de rama como texto libre: basta con la revisión de opciones de
/// `mergeon-git-query`, que es la que aquí se repite.
fn unsafe_argument(args: &[&str]) -> Option<&'static str> {
    for argument in args.iter().skip(1) {
        if *argument == "--" {
            break;
        }
        if !argument.starts_with('-') || argument.len() < 2 {
            continue;
        }

        // `--nombre=valor`: solo cuenta el nombre de la opción.
        let name = match argument.find('=') {
            Some(at) if argument.starts_with("--") && at > 2 => &argument[..at],
            _ => argument,
        };
        if name.contains("--upload-pack") || name.contains("--receive-pack") {
            return Some(UNSAFE_PACK);
        }
        if name.contains("--template") {
            return Some(UNSAFE_TEMPLATE);
        }
    }
    None
}

/// `git.raw(args)` con el filtro de simple-git por delante.
pub(crate) fn raw(git: &Git, args: &[&str]) -> GitResult<String> {
    if let Some(message) = unsafe_argument(args) {
        return Err(GitError::message(message));
    }
    git.raw(args)
}

/// `path.resolve(path)`: absoluta y sin `.` ni `..`, sin tocar el disco.
pub(crate) fn resolve_path(path: &Path) -> PathBuf {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut resolved = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            other => resolved.push(other.as_os_str()),
        }
    }
    resolved
}

/// `fs.realpath(path)`, sin el prefijo `\\?\` que añade Windows.
pub(crate) fn realpath(path: &Path) -> io::Result<PathBuf> {
    mergeon_gitcmd::paths::canonicalize(path)
}

/// La ruta como el texto que viajaba por IPC.
pub(crate) fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Un error de `fs` como lo vería el handler original: `error.code` y el
/// mensaje de Node (`EACCES: permission denied, open '/ruta'`).
pub(crate) fn node_fs_error(error: &io::Error, syscall: &str, path: &Path) -> Thrown {
    let known = match error.kind() {
        io::ErrorKind::NotFound => Some(("ENOENT", "no such file or directory")),
        io::ErrorKind::PermissionDenied => Some(if is_operation_not_permitted(error) {
            ("EPERM", "operation not permitted")
        } else {
            ("EACCES", "permission denied")
        }),
        io::ErrorKind::NotADirectory => Some(("ENOTDIR", "not a directory")),
        io::ErrorKind::IsADirectory => Some(("EISDIR", "illegal operation on a directory")),
        _ => None,
    };

    match known {
        // Node abre el directorio sin problema y falla al leer, sin ruta.
        Some((code @ "EISDIR", description)) => {
            Thrown::new(format!("{code}: {description}, read")).with_code(code)
        }
        Some((code, description)) => Thrown::new(format!(
            "{code}: {description}, {syscall} '{}'",
            path.display()
        ))
        .with_code(code),
        None => Thrown::new(error.to_string()),
    }
}

#[cfg(unix)]
fn is_operation_not_permitted(error: &io::Error) -> bool {
    // EPERM; EACCES es 13.
    error.raw_os_error() == Some(1)
}

#[cfg(not(unix))]
fn is_operation_not_permitted(_error: &io::Error) -> bool {
    // libuv traduce ERROR_ACCESS_DENIED a EPERM.
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_pack_and_template_options_but_not_paths() {
        assert_eq!(
            unsafe_argument(&["branch", "--set-upstream-to", "origin/x", "--upload-pack=x"]),
            Some(UNSAFE_PACK)
        );
        assert_eq!(
            unsafe_argument(&["branch", "--template", "x"]),
            Some(UNSAFE_TEMPLATE)
        );
        assert_eq!(
            unsafe_argument(&["branch", "--set-upstream-to", "origin/main", "main"]),
            None
        );
        // Después de `--` solo hay rutas.
        assert_eq!(
            unsafe_argument(&["clone", "--", "--upload-pack=x", "dir"]),
            None
        );
    }

    #[test]
    fn a_rejected_command_never_reaches_git() {
        let directory = tempfile::tempdir().unwrap();
        let error = raw(&Git::new(directory.path()), &["branch", "--upload-pack=x"]).unwrap_err();
        assert_eq!(error.message, UNSAFE_PACK);
    }

    #[test]
    fn refuses_a_directory_that_does_not_exist() {
        let directory = tempfile::tempdir().unwrap();
        assert!(open(directory.path()).is_ok());
        assert_eq!(
            open(&directory.path().join("gone")).unwrap_err().message,
            MISSING_DIRECTORY
        );
    }

    #[test]
    fn resolves_paths_lexically() {
        let base = resolve_path(Path::new("repo"));
        assert!(base.is_absolute());
        assert_eq!(resolve_path(Path::new("repo/./sub/..")), base);
        #[cfg(unix)]
        assert_eq!(
            resolve_path(Path::new("/repo/a/../b/")),
            Path::new("/repo/b")
        );
    }

    #[test]
    fn fs_errors_read_like_node_errors() {
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("nope").join("file");
        let error = std::fs::read(&missing).unwrap_err();
        let thrown = node_fs_error(&error, "open", &missing);
        assert_eq!(thrown.code.as_deref(), Some("ENOENT"));
        assert_eq!(
            thrown.message,
            format!(
                "ENOENT: no such file or directory, open '{}'",
                missing.display()
            )
        );
    }
}
