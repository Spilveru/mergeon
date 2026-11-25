//! Lectura de archivos del working tree.

use std::io::Read;
use std::path::Path;

use crate::error::{Inner, QueryResult, read};
use crate::fs_util::node_error_message;
use crate::paths::{assert_path_inside_repository, normalize_repo_relative_path};

/// El resolvedor de conflictos no abre archivos más grandes que esto. Es más
/// alto que el tope de `blame` y `get_file_at_commit` (1 MiB) para que entren
/// los conflictos habituales en lockfiles (`package-lock.json`, `Cargo.lock`…).
pub const MAX_READ_FILE_BYTES: u64 = 5 * 1024 * 1024;

fn too_large_message(size: u64) -> String {
    format!(
        "The file is too large to open here ({:.1} MB; the limit is {} MB).",
        size as f64 / (1024.0 * 1024.0),
        MAX_READ_FILE_BYTES / (1024 * 1024)
    )
}

/*
 * El tamaño se mira en el archivo ya abierto (sin carrera entre medir y leer)
 * y antes de leer nada; aun así la lectura se corta en el tope por si el
 * archivo crece entre medias. Los errores siguen siendo los de `fs.readFile`
 * de Node (`open`, `EISDIR` al leer un directorio).
 */
fn read_capped_utf8(path: &Path) -> Inner<String> {
    let file =
        std::fs::File::open(path).map_err(|error| node_error_message(&error, "open", path))?;
    if let Ok(metadata) = file.metadata()
        && metadata.is_file()
        && metadata.len() > MAX_READ_FILE_BYTES
    {
        return Err(too_large_message(metadata.len()).into());
    }

    let mut bytes = Vec::new();
    file.take(MAX_READ_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| node_error_message(&error, "read", path))?;
    if bytes.len() as u64 > MAX_READ_FILE_BYTES {
        return Err(too_large_message(bytes.len() as u64).into());
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// [`read_capped_utf8`] para un archivo que puede no existir (`None`): en un
/// conflicto de borrado, el working tree ya no lo tiene.
pub(crate) fn read_capped_utf8_optional(path: &Path) -> Inner<Option<String>> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        _ => read_capped_utf8(path).map(Some),
    }
}

/// Canal `git:readFile`: contenido de un archivo del repo (para resolver
/// conflictos). Un archivo de más de [`MAX_READ_FILE_BYTES`] es un error.
pub fn read_file(repo_path: &Path, file_path: &str) -> QueryResult<String> {
    read("Failed to read file", || {
        let full_path =
            assert_path_inside_repository(repo_path, &normalize_repo_relative_path(file_path)?)?;
        read_capped_utf8(&full_path)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_file_inside_the_repository() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::write(
            dir.path().join("src").join("app.js"),
            "<<<<<<< HEAD\naño\n=======\n",
        )
        .unwrap();

        assert_eq!(
            read_file(dir.path(), "src\\app.js").unwrap(),
            "<<<<<<< HEAD\naño\n=======\n"
        );
    }

    #[test]
    fn refuses_paths_outside_the_repository() {
        let dir = tempfile::tempdir().unwrap();

        assert_eq!(
            read_file(dir.path(), "../secret").unwrap_err().message(),
            "Failed to read file: The file path must stay inside the repository."
        );
        assert_eq!(
            read_file(dir.path(), "/etc/passwd").unwrap_err().message(),
            "Failed to read file: The file path must be relative to the repository."
        );
    }

    #[test]
    fn refuses_files_above_the_limit_without_reading_them() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("big.txt"), vec![b'x'; 10 * 1024 * 1024]).unwrap();
        std::fs::write(
            dir.path().join("limit.txt"),
            vec![b'y'; MAX_READ_FILE_BYTES as usize],
        )
        .unwrap();

        assert_eq!(
            read_file(dir.path(), "big.txt").unwrap_err().message(),
            "Failed to read file: The file is too large to open here (10.0 MB; the limit is 5 MB)."
        );
        assert_eq!(
            read_file(dir.path(), "limit.txt").unwrap().len(),
            MAX_READ_FILE_BYTES as usize
        );
    }

    #[test]
    fn a_missing_file_fails_like_node() {
        let dir = tempfile::tempdir().unwrap();

        assert_eq!(
            read_file(dir.path(), "gone.txt").unwrap_err().message(),
            format!(
                "Failed to read file: ENOENT: no such file or directory, open '{}'",
                dir.path().join("gone.txt").display()
            )
        );
    }
}
