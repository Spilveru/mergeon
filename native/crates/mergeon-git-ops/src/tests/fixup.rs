//! Fixups dirigidos: `commit --fixup=<sha>` y "Autosquash fixups"
//! (`rebase -i --autosquash` sin editor).

use mergeon_gitcmd::{OperationType, get_structured_status};
use std::path::Path;

use super::support::*;
use crate::{
    Code, CommitOptions, Operation, Outcome, RebaseActionParams, commit_operation,
    rebase_action_operation, sequence_action_operation, stage_operation,
};

fn fixup_into(target: &str) -> CommitOptions {
    CommitOptions {
        fixup: Some(target.to_string()),
        ..Default::default()
    }
}

/// `base`, `feature A` (a.txt) y `feature B` (b.txt).
fn repository_with_two_features() -> (tempfile::TempDir, std::path::PathBuf, String, String) {
    let (root, repo_path) = create_repository();
    let a = commit_file_at(&repo_path, "a\n", "feature A", "a.txt");
    let b = commit_file_at(&repo_path, "b\n", "feature B", "b.txt");
    (root, repo_path, a, b)
}

fn autosquash(repo_path: &Path, commit: &str) -> crate::MutationResult {
    rebase_action_operation(
        repo_path,
        "autosquash",
        commit,
        &RebaseActionParams::default(),
    )
}

#[test]
fn creates_a_fixup_commit_for_an_older_commit() {
    let (_root, repo_path, a, _b) = repository_with_two_features();
    std::fs::write(repo_path.join("a.txt"), "a fixed\n").unwrap();
    stage_operation(&repo_path, "a.txt");

    // El mensaje de la caja se ignora: lo pone git.
    let result = commit_operation(&repo_path, "ignored", &fixup_into(&a[..10]));

    assert!(result.success, "{}", result.message);
    assert_eq!(result.operation, Operation::Commit);
    let created = head(&repo_path);
    assert_eq!(
        result.message,
        format!("Created fixup commit {} for {}.", &created[..7], &a[..7])
    );
    assert_eq!(
        log_subjects(&repo_path),
        ["fixup! feature A", "feature B", "feature A", "base"]
    );
}

#[test]
fn a_fixup_needs_staged_changes_and_a_commit_of_the_branch() {
    let (_root, repo_path, a, _b) = repository_with_two_features();

    let nothing = commit_operation(&repo_path, "", &fixup_into(&a));
    assert!(!nothing.success);
    assert_eq!(nothing.code, Code::NoChanges);

    std::fs::write(repo_path.join("a.txt"), "a fixed\n").unwrap();
    stage_operation(&repo_path, "a.txt");

    let amend = commit_operation(
        &repo_path,
        "",
        &CommitOptions {
            amend: true,
            ..fixup_into(&a)
        },
    );
    assert_eq!(amend.outcome, Outcome::Blocked);

    let unknown = commit_operation(&repo_path, "", &fixup_into("0123456789abcdef"));
    assert_eq!(unknown.outcome, Outcome::Blocked);
    assert_eq!(unknown.message, "Commit 0123456 does not exist.");

    // Un commit de otra rama no sirve de destino.
    run_git(&repo_path, &["stash"]);
    run_git(&repo_path, &["checkout", "-q", "-b", "other", "HEAD~2"]);
    let other = commit_file_at(&repo_path, "o\n", "other work", "o.txt");
    run_git(&repo_path, &["checkout", "-q", "main"]);
    run_git(&repo_path, &["stash", "pop", "--index"]);
    let foreign = commit_operation(&repo_path, "", &fixup_into(&other));
    assert_eq!(foreign.outcome, Outcome::Blocked);
    assert_eq!(
        foreign.message,
        format!(
            "Commit {} is not part of the current branch history.",
            &other[..7]
        )
    );
    assert_eq!(log_subjects(&repo_path)[0], "feature B");
}

#[test]
fn autosquash_folds_the_fixup_into_its_target() {
    let (_root, repo_path, a, _b) = repository_with_two_features();
    std::fs::write(repo_path.join("a.txt"), "a fixed\n").unwrap();
    stage_operation(&repo_path, "a.txt");
    assert!(commit_operation(&repo_path, "", &fixup_into(&a)).success);
    // Un cambio sin commitear no impide el autosquash (`--autostash`).
    std::fs::write(repo_path.join("file.txt"), "local edit\n").unwrap();

    let result = autosquash(&repo_path, &a);

    assert!(result.success, "{}", result.message);
    assert_eq!(result.operation, Operation::RebaseAction);
    assert_eq!(result.message, "Squashed 1 fixup commit into its target.");
    assert_eq!(log_subjects(&repo_path), ["feature B", "feature A", "base"]);
    // El cambio del fixup vive ahora en "feature A".
    assert_eq!(run_git(&repo_path, &["show", "HEAD~1:a.txt"]), "a fixed\n");
    assert_eq!(
        run_git(&repo_path, &["show", "--name-only", "--format=", "HEAD~1"]).trim(),
        "a.txt"
    );
    assert_eq!(read_file(&repo_path.join("file.txt")), "local edit\n");
    let status = get_structured_status(&repo_path).unwrap();
    assert!(!status.operation_state.in_progress);
}

#[test]
fn autosquash_without_fixups_is_a_noop() {
    let (_root, repo_path, a, _b) = repository_with_two_features();
    let before = head(&repo_path);

    let result = autosquash(&repo_path, &a);

    assert!(!result.success);
    assert_eq!(result.outcome, Outcome::Noop);
    assert_eq!(
        result.message,
        format!("There are no fixup commits from {} up to HEAD.", &a[..7])
    );
    assert_eq!(head(&repo_path), before);
}

#[test]
fn autosquash_from_the_root_commit() {
    let (_root, repo_path, _a, _b) = repository_with_two_features();
    let root = run_git(&repo_path, &["rev-list", "--max-parents=0", "HEAD"])
        .trim()
        .to_string();
    std::fs::write(repo_path.join("file.txt"), "base fixed\n").unwrap();
    stage_operation(&repo_path, "file.txt");
    assert!(commit_operation(&repo_path, "", &fixup_into(&root)).success);

    let result = autosquash(&repo_path, &root);

    assert!(result.success, "{}", result.message);
    assert_eq!(log_subjects(&repo_path), ["feature B", "feature A", "base"]);
    assert_eq!(
        run_git(&repo_path, &["show", "HEAD~2:file.txt"]),
        "base fixed\n"
    );
}

#[test]
fn a_conflicting_autosquash_stops_in_a_rebase_that_can_be_aborted() {
    let (_root, repo_path) = create_repository();
    let first = commit_file(&repo_path, "first\n", "first edit");
    commit_file(&repo_path, "second\n", "second edit");
    // El fixup parte de "second": movido tras "first" no encaja.
    std::fs::write(repo_path.join("file.txt"), "fixed\n").unwrap();
    stage_operation(&repo_path, "file.txt");
    assert!(commit_operation(&repo_path, "", &fixup_into(&first)).success);
    let before = head(&repo_path);

    let result = autosquash(&repo_path, &first);

    assert!(!result.success);
    assert_eq!(result.operation, Operation::RebaseAction);
    assert_eq!(result.outcome, Outcome::Conflicted);
    assert_eq!(result.conflict_files, ["file.txt"]);
    assert_eq!(
        result.operation_state.and_then(|state| state.kind),
        Some(OperationType::Rebase)
    );

    // El mismo "Abort" que cualquier rebase parado.
    let aborted = sequence_action_operation(&repo_path, "abort");
    assert!(aborted.success, "{}", aborted.message);
    assert_eq!(head(&repo_path), before);
    let status = get_structured_status(&repo_path).unwrap();
    assert!(!status.operation_state.in_progress);
}
