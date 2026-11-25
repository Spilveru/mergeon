//! Validación de rutas que llegan de la interfaz: siempre relativas al
//! repositorio y sin salirse de él.

use std::path::{Component, Path, PathBuf};

use crate::text::js_trim;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathError {
    NotRelative,
    OutsideRepository,
}

impl PathError {
    pub fn message(self) -> &'static str {
        match self {
            PathError::NotRelative => "The file path must be relative to the repository.",
            PathError::OutsideRepository => "The file path must stay inside the repository.",
        }
    }
}

impl std::fmt::Display for PathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for PathError {}

impl From<PathError> for crate::error::Cause {
    fn from(error: PathError) -> Self {
        crate::error::Cause(error.message().to_string())
    }
}

fn has_drive_prefix(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'/'
}

/// `normalizeRepoRelativePath`: separadores `/`, sin `./` inicial, sin `..`.
pub fn normalize_repo_relative_path(file_path: &str) -> Result<String, PathError> {
    let replaced = file_path.replace('\\', "/");
    let raw_path = js_trim(&replaced);

    if raw_path.starts_with('/') || has_drive_prefix(raw_path) {
        return Err(PathError::NotRelative);
    }

    let normalized = raw_path.strip_prefix("./").unwrap_or(raw_path);

    if normalized.is_empty() || normalized.split('/').any(|segment| segment == "..") {
        return Err(PathError::OutsideRepository);
    }

    Ok(normalized.to_string())
}

/// `path.resolve(path)`: absoluta y sin `.` ni `..`, sin tocar el disco.
pub(crate) fn lexical_absolute(path: &Path) -> PathBuf {
    let mut resolved = PathBuf::new();
    if path.is_relative()
        && let Ok(current) = std::env::current_dir()
    {
        resolved.push(current);
    }
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                resolved.push(component.as_os_str());
            }
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
        }
    }
    resolved
}

/// `assertPathInsideRepository`: ruta absoluta de `file_path` dentro del repo.
pub fn assert_path_inside_repository(
    repo_path: &Path,
    file_path: &str,
) -> Result<PathBuf, PathError> {
    let root = lexical_absolute(repo_path);
    let target = lexical_absolute(&root.join(file_path));

    if !target.starts_with(&root) {
        return Err(PathError::OutsideRepository);
    }

    Ok(target)
}

/// `normalizeRepositoryFilePath`: la ruta normalizada, ya comprobada contra el repo.
pub fn normalize_repository_file_path(
    repo_path: &Path,
    file_path: &str,
) -> Result<String, PathError> {
    let normalized = normalize_repo_relative_path(file_path)?;
    assert_path_inside_repository(repo_path, &normalized)?;
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_separators_and_the_leading_dot() {
        assert_eq!(
            normalize_repo_relative_path("src\\app.js").unwrap(),
            "src/app.js"
        );
        assert_eq!(
            normalize_repo_relative_path("  ./src/app.js ").unwrap(),
            "src/app.js"
        );
        assert_eq!(
            normalize_repo_relative_path("año de diseño.md").unwrap(),
            "año de diseño.md"
        );
    }

    #[test]
    fn rejects_absolute_paths() {
        for path in [
            "/etc/passwd",
            "C:/Windows/system.ini",
            "c:\\temp\\x",
            "\\\\server\\share",
        ] {
            let error = normalize_repo_relative_path(path).unwrap_err();
            assert_eq!(error, PathError::NotRelative, "{path}");
            assert_eq!(
                error.to_string(),
                "The file path must be relative to the repository."
            );
        }
    }

    #[test]
    fn rejects_empty_paths_and_parent_segments() {
        for path in ["", "   ", "./", "../secret", "src/../../secret", "a/.."] {
            let error = normalize_repo_relative_path(path).unwrap_err();
            assert_eq!(error, PathError::OutsideRepository, "{path:?}");
            assert_eq!(
                error.to_string(),
                "The file path must stay inside the repository."
            );
        }
    }

    #[test]
    fn resolves_the_target_inside_the_repository() {
        let dir = tempfile::tempdir().unwrap();
        let target = assert_path_inside_repository(dir.path(), "src/app.js").unwrap();
        assert_eq!(target, dir.path().join("src").join("app.js"));
        assert_eq!(
            assert_path_inside_repository(dir.path(), "../outside"),
            Err(PathError::OutsideRepository)
        );
        assert_eq!(
            normalize_repository_file_path(dir.path(), ".\\src\\app.js").unwrap(),
            "src/app.js"
        );
    }
}
