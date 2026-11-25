//! Stashes.

use mergeon_gitcmd::get_structured_status;
use std::path::Path;

use super::support::*;
use crate::stash::resolve_stash_ref;
use crate::{
    Code, Operation, Outcome, apply_stash_operation, drop_stash_operation, pop_stash_operation,
    stash_branch_operation, stash_operation,
};

fn stash_messages(repo_path: &Path) -> Vec<String> {
    run_git(repo_path, &["stash", "list", "--format=%gs"])
        .lines()
        .map(str::to_string)
        .collect()
}

fn is_full_sha(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[test]
fn creates_a_stash_and_can_pop_it_successfully() {
    let (_root, repo_path) = create_repository();
    std::fs::write(repo_path.join("file.txt"), "stash me\n").unwrap();

    let stash_result = stash_operation(&repo_path, Some("wip"));
    let pop_result = pop_stash_operation(&repo_path, "stash@{0}");

    assert!(stash_result.success, "{}", stash_result.message);
    assert_eq!(stash_result.message, "Changes stashed successfully.");
    assert!(pop_result.success, "{}", pop_result.message);
    assert_eq!(pop_result.code, Code::Ok);
    assert_eq!(pop_result.message, "Popped stash@{0}.");
    assert_eq!(pop_result.stash_ref.as_deref(), Some("stash@{0}"));
    assert_eq!(read_file(&repo_path.join("file.txt")), "stash me\n");
    assert!(stash_messages(&repo_path).is_empty());
}

#[test]
fn stashing_a_clean_tree_is_a_noop() {
    let (_root, repo_path) = create_repository();

    let result = stash_operation(&repo_path, None);

    assert!(!result.success);
    assert_eq!(result.outcome, Outcome::Noop);
    assert_eq!(result.code, Code::NoChanges);
    assert_eq!(result.message, "There are no changes to stash.");
    assert_eq!(result.stash_ref, None);
}

#[test]
fn returns_the_sha_of_the_created_stash_in_stash_ref() {
    let (_root, repo_path) = create_repository();
    std::fs::write(repo_path.join("file.txt"), "stash me\n").unwrap();

    let stash_result = stash_operation(&repo_path, Some("wip"));
    let stash_sha = run_git(&repo_path, &["rev-parse", "refs/stash"]);

    assert!(stash_result.success);
    let stash_ref = stash_result.stash_ref.unwrap();
    assert!(is_full_sha(&stash_ref), "{stash_ref}");
    assert_eq!(stash_ref, stash_sha.trim());
    assert_eq!(stash_messages(&repo_path), ["On main: wip"]);
}

#[test]
fn pops_the_right_stash_by_sha_even_after_another_stash_shifts_the_index() {
    let (_root, repo_path) = create_repository();
    commit_file_at(&repo_path, "other base\n", "other base", "other.txt");

    std::fs::write(repo_path.join("file.txt"), "first change\n").unwrap();
    let first_stash = stash_operation(&repo_path, Some("first"));
    let first_sha = first_stash.stash_ref.unwrap();

    // Un stash externo corre el índice: el primero pasa a ser stash@{1}.
    std::fs::write(repo_path.join("other.txt"), "second change\n").unwrap();
    run_git(&repo_path, &["stash", "push", "-m", "second"]);

    assert_eq!(
        resolve_stash_ref(&repo_path, &first_sha).as_deref(),
        Some("stash@{1}")
    );
    // También con el SHA abreviado y en mayúsculas.
    assert_eq!(
        resolve_stash_ref(&repo_path, &first_sha[..10].to_uppercase()).as_deref(),
        Some("stash@{1}")
    );

    let pop_result = pop_stash_operation(&repo_path, &first_sha);

    assert!(pop_result.success, "{}", pop_result.message);
    assert_eq!(pop_result.message, "Popped stash@{1}.");
    // El resultado hace eco de la referencia pedida, no de la resuelta.
    assert_eq!(pop_result.stash_ref.as_deref(), Some(first_sha.as_str()));
    assert_eq!(read_file(&repo_path.join("file.txt")), "first change\n");

    let stash_list = stash_messages(&repo_path);
    assert_eq!(stash_list.len(), 1);
    assert!(stash_list[0].contains("second"));
}

#[test]
fn stash_references_that_are_not_shas_pass_through() {
    let (_root, repo_path) = create_repository();

    assert_eq!(
        resolve_stash_ref(&repo_path, "  stash@{2} ").as_deref(),
        Some("stash@{2}")
    );
    assert_eq!(resolve_stash_ref(&repo_path, "   "), None);
    // Un SHA sin stashes que consultar no se resuelve.
    assert_eq!(resolve_stash_ref(&repo_path, "deadbeef"), None);
}

#[test]
fn returns_stash_not_found_when_the_stash_sha_no_longer_exists() {
    let (_root, repo_path) = create_repository();

    let result = pop_stash_operation(&repo_path, "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef");

    assert!(!result.success);
    assert_eq!(result.code, Code::StashNotFound);
    assert_eq!(result.outcome, Outcome::Failed);
    assert_eq!(result.message, "The stash no longer exists.");
    assert_eq!(result.operation, Operation::PopStash);

    // Con stashes presentes, un SHA ajeno tampoco se resuelve.
    std::fs::write(repo_path.join("file.txt"), "stash me\n").unwrap();
    stash_operation(&repo_path, None);
    for result in [
        apply_stash_operation(&repo_path, "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef"),
        drop_stash_operation(&repo_path, "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef"),
        stash_branch_operation(
            &repo_path,
            "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
            "recovered",
        ),
    ] {
        assert!(!result.success);
        assert_eq!(result.code, Code::StashNotFound);
    }
    assert_eq!(stash_messages(&repo_path).len(), 1);
}

#[test]
fn returns_stash_conflict_when_applying_a_conflicting_stash() {
    let (_root, repo_path) = create_repository();
    std::fs::write(repo_path.join("file.txt"), "stash version\n").unwrap();
    run_git(&repo_path, &["stash", "push", "-m", "wip"]);
    commit_file(&repo_path, "main version\n", "main version");

    let result = apply_stash_operation(&repo_path, "stash@{0}");

    assert!(!result.success);
    assert_eq!(result.code, Code::StashConflict);
    assert_eq!(result.outcome, Outcome::Conflicted);
    assert!(result.status.as_ref().unwrap().conflict_state.pending_count > 0);
    assert_eq!(result.conflict_files, ["file.txt"]);
    // Aplicar no consume el stash.
    assert_eq!(stash_messages(&repo_path).len(), 1);
}

#[test]
fn applies_a_stash_and_keeps_it_in_the_list() {
    let (_root, repo_path) = create_repository();
    std::fs::write(repo_path.join("file.txt"), "stash me\n").unwrap();
    let stashed = stash_operation(&repo_path, None);
    let sha = stashed.stash_ref.unwrap();

    let result = apply_stash_operation(&repo_path, &sha);

    assert!(result.success, "{}", result.message);
    assert_eq!(result.operation, Operation::ApplyStash);
    assert_eq!(result.message, "Applied stash@{0}.");
    assert_eq!(read_file(&repo_path.join("file.txt")), "stash me\n");
    assert_eq!(stash_messages(&repo_path).len(), 1);
}

#[test]
fn drops_a_stash_with_a_structured_result() {
    let (_root, repo_path) = create_repository();
    std::fs::write(repo_path.join("file.txt"), "stash me\n").unwrap();
    stash_operation(&repo_path, Some("wip"));

    let result = drop_stash_operation(&repo_path, "stash@{0}");

    assert!(result.success, "{}", result.message);
    assert_eq!(result.code, Code::Ok);
    assert_eq!(result.message, "Dropped stash@{0}.");
    assert!(stash_messages(&repo_path).is_empty());
}

#[test]
fn creates_a_branch_from_stash_and_clears_the_stash_entry_on_success() {
    let (_root, repo_path) = create_repository();
    std::fs::write(repo_path.join("file.txt"), "stash me\n").unwrap();
    stash_operation(&repo_path, Some("wip"));

    let result = stash_branch_operation(&repo_path, "stash@{0}", "recover-work");
    let status = get_structured_status(&repo_path).unwrap();

    assert!(result.success, "{}", result.message);
    assert_eq!(result.operation, Operation::StashBranch);
    assert_eq!(
        result.message,
        "Created recover-work from stash@{0} and switched to it."
    );
    assert_eq!(result.branch.as_deref(), Some("recover-work"));
    assert_eq!(result.stash_ref.as_deref(), Some("stash@{0}"));
    assert_eq!(status.current.as_deref(), Some("recover-work"));
    assert!(stash_messages(&repo_path).is_empty());
}

#[test]
fn stash_branch_requires_a_branch_name() {
    let (_root, repo_path) = create_repository();

    let result = stash_branch_operation(&repo_path, " stash@{0} ", " ");

    assert!(!result.success);
    assert_eq!(result.outcome, Outcome::Blocked);
    assert_eq!(result.message, "A branch name is required.");
    assert_eq!(result.branch, None);
    assert_eq!(result.stash_ref.as_deref(), Some("stash@{0}"));
}
