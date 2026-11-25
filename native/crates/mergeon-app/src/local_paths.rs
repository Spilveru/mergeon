//! Rutas de repositorios escritas o pegadas a mano (el campo de ruta del
//! lanzador "Open repository", y el plan B cuando el selector de carpetas del
//! sistema no aparece: en Linux sin `xdg-desktop-portal` GPUI espera para
//! siempre).

use std::path::{Path, PathBuf};

use gpui_kit::{Context, Task};
use mergeon_backend::RepositoryKind;

use crate::AppModel;

/// `~/proyecto` → la carpeta personal + `proyecto`. Solo `~` solo o seguido
/// de un separador (`~otro/x` se deja tal cual).
pub fn expand_home(path: &str) -> PathBuf {
    match path.strip_prefix("~") {
        Some(rest) if rest.is_empty() || rest.starts_with(['/', '\\']) => ["HOME", "USERPROFILE"]
            .into_iter()
            .filter_map(std::env::var_os)
            .find(|home| !home.is_empty())
            .map(PathBuf::from)
            .map(|home| home.join(rest.trim_start_matches(['/', '\\'])))
            .unwrap_or_else(|| PathBuf::from(path)),
        _ => PathBuf::from(path),
    }
}

/// Quita espacios y las comillas con que suele pegarse una ruta copiada del
/// explorador o de una terminal (`"C:\code\web"`, `'~/code/web'`).
pub fn clean_typed_path(text: &str) -> &str {
    let text = text.trim();
    for quote in ['"', '\''] {
        if let Some(inner) = text
            .strip_prefix(quote)
            .and_then(|rest| rest.strip_suffix(quote))
        {
            return inner.trim();
        }
    }
    text
}

/// Por qué una ruta escrita no se puede abrir.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypedPathError {
    Empty,
    Missing,
    NotADirectory,
    NotARepository,
    /// Sin permiso u otro error al leerla.
    Unreadable(String),
}

impl TypedPathError {
    /// El texto que se enseña bajo el campo.
    pub fn message(&self) -> String {
        match self {
            TypedPathError::Empty => "Enter the path to a repository.".to_string(),
            TypedPathError::Missing => "That folder doesn't exist.".to_string(),
            TypedPathError::NotADirectory => "That's a file, not a folder.".to_string(),
            TypedPathError::NotARepository => "That folder isn't a Git repository.".to_string(),
            TypedPathError::Unreadable(message) => format!("Can't read that folder: {message}"),
        }
    }
}

/// Comprueba en disco (y con git) una ruta escrita: debe existir, ser una
/// carpeta y estar dentro de un repositorio. Devuelve la ruta a abrir (la
/// raíz canónica del repositorio). Bloquea: llamarla fuera del hilo de UI.
pub fn check_typed_repository_path(
    text: &str,
    inspect: impl FnOnce(&Path) -> mergeon_backend::RepositoryInspection,
) -> Result<PathBuf, TypedPathError> {
    let text = clean_typed_path(text);
    if text.is_empty() {
        return Err(TypedPathError::Empty);
    }
    let path = expand_home(text);
    match std::fs::metadata(&path) {
        Ok(metadata) if !metadata.is_dir() => return Err(TypedPathError::NotADirectory),
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(TypedPathError::Missing);
        }
        Err(error) => return Err(TypedPathError::Unreadable(error.to_string())),
    }
    let inspection = inspect(&path);
    if inspection.valid {
        return Ok(inspection
            .canonical_path
            .or(inspection.path)
            .map(PathBuf::from)
            .unwrap_or(path));
    }
    Err(match inspection.kind {
        RepositoryKind::MissingPath => TypedPathError::Missing,
        RepositoryKind::AccessError | RepositoryKind::UnknownError => {
            TypedPathError::Unreadable(inspection.message)
        }
        _ => TypedPathError::NotARepository,
    })
}

impl AppModel {
    /// [`check_typed_repository_path`] en un hilo de fondo.
    pub fn check_typed_repository_path(
        &mut self,
        text: &str,
        cx: &mut Context<Self>,
    ) -> Task<Result<PathBuf, TypedPathError>> {
        let text = text.to_string();
        self.blocking(cx, move |backend| {
            check_typed_repository_path(&text, |path| backend.git().inspect_repository(path))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> PathBuf {
        ["HOME", "USERPROFILE"]
            .into_iter()
            .filter_map(std::env::var_os)
            .find(|home| !home.is_empty())
            .map(PathBuf::from)
            .expect("a home folder")
    }

    #[test]
    fn expands_the_home_folder_in_a_typed_path() {
        assert_eq!(expand_home("~"), home());
        assert_eq!(expand_home("~/code/demo"), home().join("code/demo"));
        assert_eq!(expand_home("/srv/repo"), PathBuf::from("/srv/repo"));
        assert_eq!(expand_home("~other/x"), PathBuf::from("~other/x"));
    }

    #[test]
    fn cleans_pasted_paths() {
        assert_eq!(clean_typed_path("  /srv/repo \n"), "/srv/repo");
        assert_eq!(clean_typed_path("\"/srv/my repo\""), "/srv/my repo");
        assert_eq!(clean_typed_path("'~/code/web'"), "~/code/web");
        assert_eq!(clean_typed_path("\""), "\"");
    }

    #[test]
    fn checks_that_a_typed_path_is_a_repository() {
        let root = tempfile::tempdir().unwrap();
        let inspect = |path: &Path| mergeon_git_query::inspect_repository(path);

        assert_eq!(
            check_typed_repository_path("   ", inspect),
            Err(TypedPathError::Empty)
        );
        let missing = root.path().join("missing");
        assert_eq!(
            check_typed_repository_path(&missing.to_string_lossy(), inspect),
            Err(TypedPathError::Missing)
        );
        let file = root.path().join("file.txt");
        std::fs::write(&file, "x").unwrap();
        assert_eq!(
            check_typed_repository_path(&file.to_string_lossy(), inspect),
            Err(TypedPathError::NotADirectory)
        );
        let plain = root.path().join("plain");
        std::fs::create_dir(&plain).unwrap();
        assert_eq!(
            check_typed_repository_path(&plain.to_string_lossy(), inspect),
            Err(TypedPathError::NotARepository)
        );

        let repo = root.path().join("my repo");
        std::fs::create_dir(&repo).unwrap();
        mergeon_gitcmd::Git::new(&repo)
            .raw(["init", "-b", "main"])
            .unwrap();
        let opened =
            check_typed_repository_path(&format!("\"{}\"", repo.to_string_lossy()), inspect)
                .unwrap();
        assert_eq!(
            std::fs::canonicalize(opened).unwrap(),
            std::fs::canonicalize(&repo).unwrap()
        );
        assert_eq!(
            TypedPathError::NotARepository.message(),
            "That folder isn't a Git repository."
        );
    }
}
