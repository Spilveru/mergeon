//! Inspección de una ruta: ¿es un repositorio Git utilizable?

use serde::{Deserialize, Serialize};
use std::path::Path;

use mergeon_gitcmd::{Git, GitError};

use crate::fs_util::{node_error_message, realpath};
use crate::paths::lexical_absolute;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RepositoryKind {
    GitRepository,
    NotGitRepository,
    MissingPath,
    AccessError,
    UnknownError,
}

impl RepositoryKind {
    pub fn as_str(self) -> &'static str {
        match self {
            RepositoryKind::GitRepository => "git-repository",
            RepositoryKind::NotGitRepository => "not-git-repository",
            RepositoryKind::MissingPath => "missing-path",
            RepositoryKind::AccessError => "access-error",
            RepositoryKind::UnknownError => "unknown-error",
        }
    }
}

/// Resultado de `inspectGitRepository`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepositoryInspection {
    pub valid: bool,
    pub kind: RepositoryKind,
    pub message: String,
    /// Igual que `canonical_path`; solo viene en repositorios válidos.
    pub path: Option<String>,
    pub canonical_path: Option<String>,
}

fn inspection(
    valid: bool,
    kind: RepositoryKind,
    message: impl Into<String>,
) -> RepositoryInspection {
    RepositoryInspection {
        valid,
        kind,
        message: message.into(),
        path: None,
        canonical_path: None,
    }
}

pub(crate) fn is_not_git_repository_message(message: &str) -> bool {
    let message = message.to_lowercase();
    message.contains("not a git repository") || message.contains("must be run in a work tree")
}

fn unknown_message(message: &str) -> &str {
    if message.is_empty() {
        "Unknown error"
    } else {
        message
    }
}

fn git_failure(error: &GitError) -> RepositoryInspection {
    if is_not_git_repository_message(&error.message) {
        return inspection(
            false,
            RepositoryKind::NotGitRepository,
            "This directory is not under Git source control.",
        );
    }

    // El original también miraba `error.code` (EACCES/EPERM), pero un error de
    // git nunca lo trae: los fallos de permisos llegan como texto de git.
    inspection(
        false,
        RepositoryKind::UnknownError,
        format!(
            "Unable to inspect this repository: {}",
            unknown_message(&error.message)
        ),
    )
}

/// Canal `git:inspectRepository`. Nunca falla: el motivo va en `kind`.
pub fn inspect_repository(repo_path: &Path) -> RepositoryInspection {
    if repo_path.as_os_str().is_empty() {
        return inspection(
            false,
            RepositoryKind::UnknownError,
            "The selected directory could not be inspected.",
        );
    }

    match std::fs::metadata(repo_path) {
        Ok(metadata) if !metadata.is_dir() => {
            return inspection(
                false,
                RepositoryKind::MissingPath,
                "The selected path is not a directory.",
            );
        }
        Ok(_) => {}
        Err(error) => {
            return match error.kind() {
                std::io::ErrorKind::NotFound => inspection(
                    false,
                    RepositoryKind::MissingPath,
                    "The selected directory no longer exists.",
                ),
                std::io::ErrorKind::PermissionDenied => inspection(
                    false,
                    RepositoryKind::AccessError,
                    "You do not have permission to access this directory.",
                ),
                _ => inspection(
                    false,
                    RepositoryKind::UnknownError,
                    format!(
                        "Unable to inspect this directory: {}",
                        node_error_message(&error, "stat", repo_path)
                    ),
                ),
            };
        }
    }

    if let Err(error) = Git::new(repo_path).status() {
        return git_failure(&error);
    }

    // La ruta resuelta sigue siendo válida si realpath no está disponible.
    let canonical_path = realpath(repo_path)
        .unwrap_or_else(|_| lexical_absolute(repo_path))
        .to_string_lossy()
        .into_owned();

    RepositoryInspection {
        valid: true,
        kind: RepositoryKind::GitRepository,
        message: "Git repository available.".to_string(),
        path: Some(canonical_path.clone()),
        canonical_path: Some(canonical_path),
    }
}

/// Canal `git:isValidRepository`.
pub fn is_valid_repository(repo_path: &Path) -> bool {
    inspect_repository(repo_path).valid
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TestRepo;

    #[test]
    fn a_repository_is_valid_and_reports_its_canonical_path() {
        let repo = TestRepo::new();

        let result = inspect_repository(&repo.path);

        assert!(result.valid);
        assert_eq!(result.kind, RepositoryKind::GitRepository);
        assert_eq!(result.message, "Git repository available.");
        let canonical = mergeon_gitcmd::paths::canonicalize(&repo.path).unwrap();
        assert_eq!(
            result.canonical_path.as_deref(),
            Some(&*canonical.to_string_lossy())
        );
        assert_eq!(result.path, result.canonical_path);
        assert!(is_valid_repository(&repo.path));
    }

    #[test]
    fn a_plain_directory_is_not_a_git_repository() {
        let dir = tempfile::tempdir().unwrap();

        let result = inspect_repository(dir.path());

        assert!(!result.valid);
        assert_eq!(result.kind, RepositoryKind::NotGitRepository);
        assert_eq!(
            result.message,
            "This directory is not under Git source control."
        );
        assert_eq!(result.path, None);
        assert_eq!(result.canonical_path, None);
        assert!(!is_valid_repository(dir.path()));
    }

    #[test]
    fn missing_paths_and_files_are_reported_as_missing() {
        let dir = tempfile::tempdir().unwrap();

        let missing = inspect_repository(&dir.path().join("gone"));
        assert_eq!(missing.kind, RepositoryKind::MissingPath);
        assert_eq!(missing.message, "The selected directory no longer exists.");

        let file = dir.path().join("file.txt");
        std::fs::write(&file, "x").unwrap();
        let not_a_directory = inspect_repository(&file);
        assert_eq!(not_a_directory.kind, RepositoryKind::MissingPath);
        assert_eq!(
            not_a_directory.message,
            "The selected path is not a directory."
        );
    }

    #[test]
    fn an_empty_path_cannot_be_inspected() {
        let result = inspect_repository(Path::new(""));
        assert_eq!(result.kind, RepositoryKind::UnknownError);
        assert_eq!(
            result.message,
            "The selected directory could not be inspected."
        );
    }

    #[test]
    fn serializes_with_the_wire_names() {
        let json = serde_json::to_value(inspect_repository(Path::new(""))).unwrap();
        assert_eq!(json["kind"], "unknown-error");
        assert_eq!(json["valid"], false);
        assert_eq!(json["canonicalPath"], serde_json::Value::Null);
        assert_eq!(json["path"], serde_json::Value::Null);
        assert_eq!(RepositoryKind::GitRepository.as_str(), "git-repository");
    }
}
