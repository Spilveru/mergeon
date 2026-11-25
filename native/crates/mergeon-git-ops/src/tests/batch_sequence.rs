//! Cherry-pick y revert de varios commits a la vez (selección múltiple del
//! historial): orden, mensaje, el commit único de revert y la parada en el
//! primer conflicto con la secuencia lista para continuar, saltar o abortar.

use mergeon_gitcmd::{OperationType, detect_operation_state};

use super::support::*;
use crate::{
    Code, Operation, Outcome, cherry_pick_commits_operation, revert_commits_operation,
    sequence_action_operation,
};

/// `main` con `base`; `feature` con tres commits que tocan archivos propios.
fn repository_with_feature() -> (tempfile::TempDir, std::path::PathBuf, Vec<String>) {
    let (root, repo_path) = create_repository();
    run_git(&repo_path, &["checkout", "-q", "-b", "feature"]);
    let commits = ["one", "two", "three"]
        .iter()
        .map(|name| {
            commit_file_at(
                &repo_path,
                &format!("{name}\n"),
                name,
                &format!("{name}.txt"),
            )
        })
        .collect();
    run_git(&repo_path, &["checkout", "-q", "main"]);
    (root, repo_path, commits)
}

#[test]
fn cherry_picks_several_commits_oldest_first() {
    let (_root, repo_path, commits) = repository_with_feature();

    let result = cherry_pick_commits_operation(&repo_path, &commits);

    assert!(result.success, "{}", result.message);
    assert_eq!(result.operation, Operation::CherryPick);
    assert_eq!(result.message, "Cherry-picked 3 commits onto main.");
    assert_eq!(log_subjects(&repo_path), ["three", "two", "one", "base"]);
}

#[test]
fn reverts_several_commits_newest_first() {
    let (_root, repo_path) = create_repository();
    let one = commit_file_at(&repo_path, "one\n", "one", "one.txt");
    let two = commit_file_at(&repo_path, "two\n", "two", "two.txt");

    let result = revert_commits_operation(&repo_path, &[two, one], false);

    assert!(result.success, "{}", result.message);
    assert_eq!(result.operation, Operation::Revert);
    assert_eq!(result.message, "Reverted 2 commits.");
    assert_eq!(
        log_subjects(&repo_path),
        ["Revert \"one\"", "Revert \"two\"", "two", "one", "base"]
    );
    assert!(!repo_path.join("one.txt").exists() && !repo_path.join("two.txt").exists());
}

#[test]
fn reverts_several_commits_in_a_single_commit() {
    let (_root, repo_path) = create_repository();
    let one = commit_file_at(&repo_path, "one\n", "one", "one.txt");
    let two = commit_file_at(&repo_path, "two\n", "two", "two.txt");

    let result = revert_commits_operation(&repo_path, &[two.clone(), one.clone()], true);

    assert!(result.success, "{}", result.message);
    assert_eq!(result.message, "Reverted 2 commits in a single commit.");
    assert_eq!(
        log_subjects(&repo_path),
        ["Revert 2 commits", "two", "one", "base"]
    );
    let body = run_git(&repo_path, &["log", "-1", "--format=%b"]);
    assert!(body.contains(&format!("- {two} \"two\"")), "{body}");
    assert!(body.contains(&format!("- {one} \"one\"")), "{body}");
    assert!(!repo_path.join("one.txt").exists() && !repo_path.join("two.txt").exists());
}

/// Un solo commit va por la operación de siempre (mismo mensaje).
#[test]
fn a_single_commit_uses_the_plain_operation() {
    let (_root, repo_path, commits) = repository_with_feature();

    let result = cherry_pick_commits_operation(&repo_path, &commits[..1]);

    assert!(result.success, "{}", result.message);
    assert_eq!(log_subjects(&repo_path), ["one", "base"]);
    assert!(!cherry_pick_commits_operation(&repo_path, &[]).success);
}

/// `one` y `three` aplican; `two` choca con `main`. La secuencia se para en
/// `two` con `one` ya aplicado; se puede saltar `two` y seguir con `three`.
#[test]
fn stops_at_the_first_conflict_and_can_skip_it() {
    let (_root, repo_path) = create_repository();
    run_git(&repo_path, &["checkout", "-q", "-b", "feature"]);
    let one = commit_file_at(&repo_path, "one\n", "one", "one.txt");
    let two = commit_file_at(&repo_path, "feature\n", "two", "shared.txt");
    let three = commit_file_at(&repo_path, "three\n", "three", "three.txt");
    run_git(&repo_path, &["checkout", "-q", "main"]);
    commit_file_at(&repo_path, "main\n", "main change", "shared.txt");

    let result = cherry_pick_commits_operation(&repo_path, &[one, two, three]);

    assert!(!result.success);
    assert_eq!(result.outcome, Outcome::Conflicted);
    assert_eq!(result.code, Code::Conflict);
    assert_eq!(
        result.message,
        "Cherry-pick stopped on a conflict after 1 of 3 commits. Resolve it and continue, skip \
         that commit or abort."
    );
    assert_eq!(log_subjects(&repo_path), ["one", "main change", "base"]);
    let state = detect_operation_state(&repo_path).unwrap();
    assert_eq!(state.kind, Some(OperationType::CherryPick));
    assert!(state.can_skip, "a multi-commit cherry-pick can skip");

    let skipped = sequence_action_operation(&repo_path, "skip");
    assert!(skipped.success, "{}", skipped.message);
    assert_eq!(
        log_subjects(&repo_path),
        ["three", "one", "main change", "base"]
    );
    assert!(!detect_operation_state(&repo_path).unwrap().in_progress);
}

#[test]
fn a_conflicting_revert_sequence_can_be_aborted() {
    let (_root, repo_path) = create_repository();
    let one = commit_file_at(&repo_path, "one\n", "one", "shared.txt");
    let two = commit_file_at(&repo_path, "two\n", "two", "other.txt");
    commit_file_at(&repo_path, "later\n", "later", "shared.txt");
    let before = head(&repo_path);

    // `two` se revierte sin problema; `one` choca con `later`.
    let result = revert_commits_operation(&repo_path, &[two, one], false);

    assert_eq!(result.outcome, Outcome::Conflicted);
    assert!(
        result
            .message
            .starts_with("Revert stopped on a conflict after 1 of 2 commits."),
        "{}",
        result.message
    );
    let state = detect_operation_state(&repo_path).unwrap();
    assert_eq!(state.kind, Some(OperationType::Revert));
    assert!(state.can_skip);

    let aborted = sequence_action_operation(&repo_path, "abort");
    assert!(aborted.success, "{}", aborted.message);
    assert_eq!(head(&repo_path), before);
    assert!(!detect_operation_state(&repo_path).unwrap().in_progress);
}

/// Un cherry-pick de un solo commit no tiene secuencia: no se ofrece saltar.
#[test]
fn a_single_conflicting_cherry_pick_cannot_skip() {
    let fixture = create_diverged_repository();

    let result = cherry_pick_commits_operation(
        &fixture.repo_path,
        std::slice::from_ref(&fixture.feature_commit),
    );

    assert_eq!(result.outcome, Outcome::Conflicted);
    assert!(!detect_operation_state(&fixture.repo_path).unwrap().can_skip);
}
