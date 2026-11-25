//! Estado del working tree. El trabajo lo hace `mergeon_gitcmd`; aquí solo se
//! le pone el contrato de errores de las lecturas.

use serde::{Deserialize, Serialize};
use std::path::Path;

use mergeon_gitcmd::{
    FileStatus, OperationState, StructuredStatus, detect_operation_state, get_structured_status,
};

use crate::error::{QueryResult, read};
use crate::exec;

/// Canal `git:getStatus`.
pub fn get_status(repo_path: &Path) -> QueryResult<StructuredStatus> {
    read("Failed to get status", || {
        Ok(get_structured_status(repo_path)
            .map_err(|error| exec::missing_directory_error(repo_path, error))?)
    })
}

/// Canal `git:getOperationState`: rebase, merge, cherry-pick o revert a medias.
pub fn get_operation_state(repo_path: &Path) -> QueryResult<OperationState> {
    read("Failed to get operation state", || {
        Ok(detect_operation_state(repo_path)
            .map_err(|error| exec::missing_directory_error(repo_path, error))?)
    })
}

/// Resultado de `git:hasUncommittedChanges`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UncommittedChanges {
    pub has_changes: bool,
    pub files: Vec<FileStatus>,
}

/// Canal `git:hasUncommittedChanges`.
pub fn has_uncommitted_changes(repo_path: &Path) -> QueryResult<UncommittedChanges> {
    read("Failed to check uncommitted changes", || {
        let status = exec::open(repo_path)?.status()?;
        Ok(UncommittedChanges {
            has_changes: !status.is_clean(),
            files: status.files,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TestRepo;
    use mergeon_gitcmd::{Git, OperationType};

    #[test]
    fn reads_the_structured_status() {
        let repo = TestRepo::new();
        repo.commit_file("a.txt", "one\n", "first");
        repo.write("a.txt", "two\n");
        repo.write("new file.txt", "x\n");

        let status = get_status(&repo.path).unwrap();

        assert_eq!(status.current.as_deref(), Some("main"));
        assert_eq!(status.modified, ["a.txt"]);
        assert_eq!(status.not_added, ["new file.txt"]);
        assert!(!status.is_clean);
        assert!(status.head_short_hash.is_some());
    }

    #[test]
    fn a_clean_repository_has_no_uncommitted_changes() {
        let repo = TestRepo::new();
        repo.commit_file("a.txt", "one\n", "first");

        assert_eq!(
            has_uncommitted_changes(&repo.path).unwrap(),
            UncommittedChanges::default()
        );
    }

    #[test]
    fn lists_the_files_with_uncommitted_changes() {
        let repo = TestRepo::new();
        repo.commit_file("a.txt", "one\n", "first");
        repo.write("a.txt", "two\n");
        repo.write("b.txt", "new\n");
        repo.git(&["add", "b.txt"]);

        let changes = has_uncommitted_changes(&repo.path).unwrap();

        assert!(changes.has_changes);
        let files: Vec<_> = changes
            .files
            .iter()
            .map(|file| {
                (
                    file.path.as_str(),
                    file.index.as_str(),
                    file.working_dir.as_str(),
                )
            })
            .collect();
        assert_eq!(files, [("a.txt", " ", "M"), ("b.txt", "A", " ")]);

        let json = serde_json::to_value(&changes).unwrap();
        assert_eq!(json["hasChanges"], true);
        assert_eq!(json["files"][0]["working_dir"], "M");
    }

    #[test]
    fn detects_an_operation_in_progress() {
        let repo = TestRepo::new();
        repo.commit_file("a.txt", "base\n", "base");
        assert!(!get_operation_state(&repo.path).unwrap().in_progress);

        repo.git(&["checkout", "-b", "feature"]);
        repo.commit_file("a.txt", "feature\n", "feature");
        repo.git(&["checkout", "main"]);
        repo.commit_file("a.txt", "main\n", "main");
        let merge = Git::new(&repo.path).exec(["merge", "feature"]).unwrap();
        assert!(!merge.success());

        let state = get_operation_state(&repo.path).unwrap();
        assert!(state.in_progress);
        assert_eq!(state.kind, Some(OperationType::Merge));
        assert_eq!(state.label, "Merge in progress");
    }

    #[test]
    fn a_missing_directory_fails_like_simple_git() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("gone");
        let expected = "Cannot use simple-git on a directory that does not exist";

        assert_eq!(
            get_status(&missing).unwrap_err().message(),
            format!("Failed to get status: {expected}")
        );
        assert_eq!(
            get_operation_state(&missing).unwrap_err().message(),
            format!("Failed to get operation state: {expected}")
        );
        assert_eq!(
            has_uncommitted_changes(&missing).unwrap_err().message(),
            format!("Failed to check uncommitted changes: {expected}")
        );
    }

    #[test]
    fn failures_carry_the_read_prefix() {
        let dir = tempfile::tempdir().unwrap();

        let error = get_status(dir.path()).unwrap_err();
        assert!(
            error
                .message()
                .starts_with("Failed to get status: fatal: not a git repository"),
            "{error}"
        );
        let error = get_operation_state(dir.path()).unwrap_err();
        assert!(
            error
                .message()
                .starts_with("Failed to get operation state: fatal: not a git repository"),
            "{error}"
        );
        let error = has_uncommitted_changes(dir.path()).unwrap_err();
        assert!(
            error
                .message()
                .starts_with("Failed to check uncommitted changes: fatal: not a git repository"),
            "{error}"
        );
    }
}
