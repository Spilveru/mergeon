//! Acceso al sistema de archivos con los mensajes de error de Node.

use std::io;
use std::path::{Path, PathBuf};

/// Mensaje como el de los errores de `fs` en Node
/// (`ENOENT: no such file or directory, open '/ruta'`), que es lo que el
/// original mostraba al usuario detrás del prefijo del canal.
pub(crate) fn node_error_message(error: &io::Error, syscall: &str, path: &Path) -> String {
    let known = match error.kind() {
        io::ErrorKind::NotFound => Some(("ENOENT", "no such file or directory")),
        io::ErrorKind::PermissionDenied if is_operation_not_permitted(error) => {
            Some(("EPERM", "operation not permitted"))
        }
        io::ErrorKind::PermissionDenied => Some(("EACCES", "permission denied")),
        io::ErrorKind::NotADirectory => Some(("ENOTDIR", "not a directory")),
        // Node abre el directorio sin problema y falla al leer, sin ruta.
        io::ErrorKind::IsADirectory => {
            return "EISDIR: illegal operation on a directory, read".to_string();
        }
        _ => None,
    };

    match known {
        Some((code, description)) => {
            format!("{code}: {description}, {syscall} '{}'", path.display())
        }
        None => error.to_string(),
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

/// `fs.stat(path)`.
pub(crate) fn stat(path: &Path) -> Result<std::fs::Metadata, String> {
    std::fs::metadata(path).map_err(|error| node_error_message(&error, "stat", path))
}

/// `fs.readFile(path, 'utf8')`: los bytes que no son UTF-8 se sustituyen, no fallan.
pub(crate) fn read_utf8(path: &Path) -> Result<String, String> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(String::from_utf8_lossy(&bytes).into_owned()),
        Err(error) => Err(node_error_message(&error, "open", path)),
    }
}

/// `fs.realpath(path)`, sin el prefijo `\\?\` que añade Windows (Node y el
/// resto de la app usan la forma normal).
pub(crate) fn realpath(path: &Path) -> io::Result<PathBuf> {
    mergeon_gitcmd::paths::canonicalize(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_files_read_like_node_errors() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope.txt");
        assert_eq!(
            read_utf8(&missing).unwrap_err(),
            format!(
                "ENOENT: no such file or directory, open '{}'",
                missing.display()
            )
        );
        assert_eq!(
            stat(&missing).unwrap_err(),
            format!(
                "ENOENT: no such file or directory, stat '{}'",
                missing.display()
            )
        );
    }

    #[test]
    fn reading_a_directory_reports_eisdir() {
        let dir = tempfile::tempdir().unwrap();
        // En Windows abrir un directorio falla antes, con otro error.
        if cfg!(unix) {
            assert_eq!(
                read_utf8(dir.path()).unwrap_err(),
                "EISDIR: illegal operation on a directory, read"
            );
        }
    }

    #[test]
    fn invalid_utf8_is_replaced_instead_of_failing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("latin1.txt");
        std::fs::write(&path, [b'a', 0xf1, b'o']).unwrap();
        assert_eq!(read_utf8(&path).unwrap(), "a\u{fffd}o");
    }
}
