//! Worktrees con git real: crear con rama nueva y existente, quitar (limpio,
//! con cambios y forzado), bloquear, podar y el checkout de una rama que ya
//! está en otro worktree.

use std::path::{Path, PathBuf};

use super::support::*;
use crate::{
    AddWorktreeOptions, Code, Operation, Outcome, add_worktree_operation, checkout_operation,
    lock_worktree_operation, prune_worktrees_operation, remove_worktree_operation,
    unlock_worktree_operation,
};

fn sibling(repo_path: &Path, name: &str) -> PathBuf {
    repo_path.parent().unwrap().join(name)
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn worktree_list(repo_path: &Path) -> String {
    run_git(repo_path, &["worktree", "list", "--porcelain"])
}

fn add_new(repo_path: &Path, folder: &Path, branch: &str) -> crate::MutationResult {
    add_worktree_operation(
        repo_path,
        &AddWorktreeOptions {
            path: text(folder),
            new_branch: Some(branch.to_string()),
            ..Default::default()
        },
    )
}

#[test]
fn creates_a_worktree_with_a_new_branch_from_a_start_point() {
    let (_root, repo_path) = create_repository();
    let base = head(&repo_path);
    commit_file(&repo_path, "next\n", "next");
    let folder = sibling(&repo_path, "repo feature ñ");

    let result = add_worktree_operation(
        &repo_path,
        &AddWorktreeOptions {
            path: text(&folder),
            new_branch: Some("feature".into()),
            start_point: Some(base.clone()),
            ..Default::default()
        },
    );

    assert!(result.success, "{}", result.message);
    assert_eq!(result.operation, Operation::WorktreeAdd);
    assert_eq!(
        result.message,
        "Created worktree repo feature ñ with new branch feature."
    );
    assert_eq!(head(&folder), base);
    assert_eq!(
        run_git(&folder, &["symbolic-ref", "--short", "HEAD"]).trim(),
        "feature"
    );
}

#[test]
fn creates_a_worktree_for_an_existing_branch() {
    let (_root, repo_path) = create_repository();
    run_git(&repo_path, &["branch", "topic"]);
    let folder = sibling(&repo_path, "repo-topic");

    let result = add_worktree_operation(
        &repo_path,
        &AddWorktreeOptions {
            path: text(&folder),
            branch: Some("topic".into()),
            ..Default::default()
        },
    );

    assert!(result.success, "{}", result.message);
    assert!(worktree_list(&repo_path).contains("branch refs/heads/topic"));
}

#[test]
fn refuses_a_branch_that_is_checked_out_elsewhere_and_bad_input() {
    let (_root, repo_path) = create_repository();
    let folder = sibling(&repo_path, "repo-main");

    let result = add_worktree_operation(
        &repo_path,
        &AddWorktreeOptions {
            path: text(&folder),
            branch: Some("main".into()),
            ..Default::default()
        },
    );

    assert!(!result.success);
    assert_eq!(result.outcome, Outcome::Blocked);
    assert_eq!(result.code, Code::BranchInOtherWorktree);
    assert!(
        result
            .message
            .starts_with("Branch 'main' is already checked out in the worktree at "),
        "{}",
        result.message
    );

    let no_target = add_worktree_operation(
        &repo_path,
        &AddWorktreeOptions {
            path: text(&folder),
            ..Default::default()
        },
    );
    assert_eq!(no_target.outcome, Outcome::Blocked);
    let no_path = add_new(&repo_path, Path::new(""), "x");
    assert_eq!(no_path.message, "Choose a folder for the worktree.");
}

#[test]
fn removes_a_clean_worktree_and_refuses_one_with_changes_unless_forced() {
    let (_root, repo_path) = create_repository();
    let clean = sibling(&repo_path, "repo-clean");
    let dirty = sibling(&repo_path, "repo-dirty");
    assert!(add_new(&repo_path, &clean, "clean").success);
    assert!(add_new(&repo_path, &dirty, "dirty").success);
    std::fs::write(dirty.join("wip.txt"), "wip\n").unwrap();

    let removed = remove_worktree_operation(&repo_path, &text(&clean), false);
    assert!(removed.success, "{}", removed.message);
    assert_eq!(removed.message, "Removed worktree repo-clean.");
    assert!(!clean.exists());
    // La rama se queda.
    assert!(run_git(&repo_path, &["branch", "--list", "clean"]).contains("clean"));

    let refused = remove_worktree_operation(&repo_path, &text(&dirty), false);
    assert!(!refused.success);
    assert_eq!(refused.code, Code::WorktreeHasChanges);
    assert!(dirty.join("wip.txt").exists());

    let forced = remove_worktree_operation(&repo_path, &text(&dirty), true);
    assert!(forced.success, "{}", forced.message);
    assert!(!dirty.exists());
}

#[test]
fn locks_unlocks_and_prunes() {
    let (_root, repo_path) = create_repository();
    let locked = sibling(&repo_path, "repo-locked");
    let gone = sibling(&repo_path, "repo-gone");
    assert!(add_new(&repo_path, &locked, "locked").success);
    assert!(add_new(&repo_path, &gone, "gone").success);

    let lock = lock_worktree_operation(&repo_path, &text(&locked), Some("usb disk"));
    assert!(lock.success, "{}", lock.message);
    assert!(worktree_list(&repo_path).contains("locked usb disk"));
    let refused = remove_worktree_operation(&repo_path, &text(&locked), false);
    assert_eq!(refused.code, Code::WorktreeLocked);

    std::fs::remove_dir_all(&gone).unwrap();
    std::fs::remove_dir_all(&locked).unwrap();
    let pruned = prune_worktrees_operation(&repo_path);
    assert!(pruned.success, "{}", pruned.message);
    // El bloqueado no se poda.
    assert_eq!(pruned.message, "Pruned 1 stale worktree.");
    assert!(worktree_list(&repo_path).contains("repo-locked"));
    let nothing = prune_worktrees_operation(&repo_path);
    assert_eq!(nothing.outcome, Outcome::Noop);

    let unlock = unlock_worktree_operation(&repo_path, &text(&locked));
    assert!(unlock.success, "{}", unlock.message);
    assert!(
        prune_worktrees_operation(&repo_path)
            .message
            .starts_with("Pruned 1")
    );
    assert!(!worktree_list(&repo_path).contains("repo-locked"));
}

#[test]
fn checking_out_a_branch_that_lives_in_another_worktree_says_so() {
    let (_root, repo_path) = create_repository();
    let folder = sibling(&repo_path, "repo-busy");
    assert!(add_new(&repo_path, &folder, "busy").success);

    let result = checkout_operation(&repo_path, "busy");

    assert!(!result.success);
    assert_eq!(result.outcome, Outcome::Blocked);
    assert_eq!(result.code, Code::BranchInOtherWorktree);
    assert!(result.message.contains("repo-busy"), "{}", result.message);
}
