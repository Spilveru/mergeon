//! Rutas reales sin el prefijo verbatim de Windows.
//!
//! En Windows `std::fs::canonicalize` devuelve `\\?\C:\...` (o
//! `\\?\UNC\servidor\recurso\...`). Git no entiende esa forma: la toma por un
//! host de red ("hostname contains invalid characters") o no puede crear
//! directorios en ella, y el resto de la app compara rutas en la forma normal
//! (`C:\...`, `C:/...`). Toda ruta canónica que salga hacia git o hacia la app
//! debe pasar por aquí.

use std::io;
use std::path::{Path, PathBuf};

/// `std::fs::canonicalize` sin el prefijo `\\?\` de Windows. En el resto de
/// sistemas es exactamente `canonicalize`.
pub fn canonicalize(path: impl AsRef<Path>) -> io::Result<PathBuf> {
    std::fs::canonicalize(path).map(strip_verbatim_prefix)
}

/// Quita el prefijo verbatim de Windows (`\\?\C:\x` → `C:\x`,
/// `\\?\UNC\srv\share` → `\\srv\share`). No toca otras rutas.
#[cfg(windows)]
pub fn strip_verbatim_prefix(path: PathBuf) -> PathBuf {
    let Some(text) = path.to_str() else {
        return path;
    };
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    // Solo rutas de unidad (`\\?\C:\`): otras formas verbatim (volúmenes por
    // GUID) no tienen equivalente sin prefijo.
    if let Some(rest) = text.strip_prefix(r"\\?\")
        && is_drive_path(rest)
    {
        return PathBuf::from(rest);
    }
    path
}

/// Quita el prefijo verbatim de Windows. Fuera de Windows no hay prefijo.
#[cfg(not(windows))]
pub fn strip_verbatim_prefix(path: PathBuf) -> PathBuf {
    path
}

#[cfg(windows)]
fn is_drive_path(text: &str) -> bool {
    matches!(text.as_bytes(), [drive, b':', ..] if drive.is_ascii_alphabetic())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_canonical_path_has_no_verbatim_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let canonical = canonicalize(dir.path()).unwrap();
        assert!(canonical.is_absolute());
        assert!(!canonical.to_string_lossy().starts_with(r"\\?\"));
        assert_eq!(
            canonicalize(&canonical).unwrap(),
            canonical,
            "the stripped path resolves to itself"
        );
    }

    #[cfg(windows)]
    #[test]
    fn strips_drive_and_unc_prefixes_only() {
        let strip = |text: &str| strip_verbatim_prefix(PathBuf::from(text));
        assert_eq!(strip(r"\\?\C:\Users\me"), PathBuf::from(r"C:\Users\me"));
        assert_eq!(
            strip(r"\\?\UNC\server\share\x"),
            PathBuf::from(r"\\server\share\x")
        );
        assert_eq!(
            strip(r"\\?\Volume{1234}\x"),
            PathBuf::from(r"\\?\Volume{1234}\x")
        );
        assert_eq!(strip(r"C:\plain"), PathBuf::from(r"C:\plain"));
    }

    #[cfg(not(windows))]
    #[test]
    fn other_systems_keep_the_path() {
        assert_eq!(
            strip_verbatim_prefix(PathBuf::from("/tmp/x")),
            PathBuf::from("/tmp/x")
        );
    }
}
