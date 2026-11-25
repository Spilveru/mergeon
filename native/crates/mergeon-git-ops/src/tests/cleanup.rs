//! Limpieza en lote de ramas integradas (sin original).

use super::support::*;
use crate::{
    CleanupBranchRequest, CleanupBranchesOptions, Code, Operation, Outcome,
    cleanup_branches_operation,
};

fn branch_names(repo_path: &std::path::Path) -> Vec<String> {
    run_git(repo_path, &["branch", "--format=%(refname:short)"])
        .lines()
        .map(str::to_string)
        .collect()
}

fn remote_branches(remote_path: &std::path::Path) -> Vec<String> {
    run_git(remote_path, &["for-each-ref", "--format=%(refname:short)"])
        .lines()
        .map(str::to_string)
        .collect()
}

fn request(name: &str, force: bool) -> CleanupBranchRequest {
    CleanupBranchRequest {
        name: name.to_string(),
        force,
    }
}

/// `main` con `merged` (integrada por merge), `ff` (por fast-forward) y
/// `wip` (sin integrar), todas publicadas en `origin` con upstream.
fn fixture() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let root = create_temp_root();
    let repo_path = root.path().join("repo");
    let remote_path = root.path().join("remote.git");
    init_repository(&repo_path);
    init_bare(&remote_path, true);
    commit_file(&repo_path, "base\n", "base");
    run_git(
        &repo_path,
        &["remote", "add", "origin", path_str(&remote_path)],
    );

    run_git(&repo_path, &["checkout", "-q", "-b", "merged"]);
    commit_file_at(&repo_path, "m\n", "merged work", "merged.txt");
    run_git(&repo_path, &["checkout", "-q", "main"]);
    run_git(
        &repo_path,
        &["merge", "-q", "--no-ff", "-m", "merge", "merged"],
    );

    run_git(&repo_path, &["checkout", "-q", "-b", "ff"]);
    commit_file_at(&repo_path, "f\n", "ff work", "ff.txt");
    run_git(&repo_path, &["checkout", "-q", "main"]);
    run_git(&repo_path, &["merge", "-q", "--ff-only", "ff"]);

    run_git(&repo_path, &["checkout", "-q", "-b", "wip"]);
    commit_file_at(&repo_path, "w\n", "wip work", "wip.txt");
    run_git(&repo_path, &["checkout", "-q", "main"]);

    for branch in ["main", "merged", "ff", "wip"] {
        run_git(&repo_path, &["push", "-q", "-u", "origin", branch]);
    }
    (root, repo_path, remote_path)
}

#[test]
fn deletes_merged_branches_and_reports_how_many() {
    let (_root, repo_path, remote_path) = fixture();

    let result = cleanup_branches_operation(
        &repo_path,
        &CleanupBranchesOptions {
            base: "main".to_string(),
            branches: vec![request("merged", false), request("ff", false)],
            ..Default::default()
        },
    );

    assert!(result.success, "{}", result.message);
    assert_eq!(result.operation, Operation::CleanupBranches);
    assert_eq!(result.outcome, Outcome::Completed);
    assert_eq!(result.message, "Deleted 2 branches.");
    assert_eq!(result.refs_changed, Some(true));
    // El log guarda de qué commit era cada una (para recuperarla).
    let summary = result.summary.unwrap();
    assert!(summary.contains("Deleted branch merged (was"), "{summary}");
    assert_eq!(branch_names(&repo_path), ["main", "wip"]);
    // Sin "Also delete remote branches" el remoto no se toca.
    assert_eq!(
        remote_branches(&remote_path),
        ["ff", "main", "merged", "wip"]
    );
}

#[test]
fn refuses_unmerged_branches_unless_forced() {
    let (_root, repo_path, _remote) = fixture();

    let refused = cleanup_branches_operation(
        &repo_path,
        &CleanupBranchesOptions {
            base: "main".to_string(),
            branches: vec![request("merged", false), request("wip", false)],
            ..Default::default()
        },
    );
    assert!(!refused.success);
    assert_eq!(refused.outcome, Outcome::Partial);
    assert_eq!(refused.code, Code::PartialFailure);
    assert_eq!(
        refused.message,
        "Deleted 1 branch. 1 failed:\nwip: not fully merged into main"
    );
    assert_eq!(branch_names(&repo_path), ["ff", "main", "wip"]);

    let forced = cleanup_branches_operation(
        &repo_path,
        &CleanupBranchesOptions {
            base: "main".to_string(),
            branches: vec![request("wip", true)],
            ..Default::default()
        },
    );
    assert!(forced.success, "{}", forced.message);
    assert_eq!(forced.message, "Deleted 1 branch.");
    assert_eq!(branch_names(&repo_path), ["ff", "main"]);
}

#[test]
fn also_deletes_the_remote_branches_when_asked() {
    let (_root, repo_path, remote_path) = fixture();

    let result = cleanup_branches_operation(
        &repo_path,
        &CleanupBranchesOptions {
            base: "main".to_string(),
            branches: vec![request("merged", false), request("ff", false)],
            delete_remote: true,
            auth_token: None,
        },
    );

    assert!(result.success, "{}", result.message);
    assert_eq!(result.message, "Deleted 2 branches and 2 remote branches.");
    assert_eq!(remote_branches(&remote_path), ["main", "wip"]);
}

#[test]
fn skips_the_remote_of_branches_whose_upstream_is_gone() {
    let (_root, repo_path, remote_path) = fixture();
    run_git(&repo_path, &["push", "-q", "origin", "--delete", "merged"]);
    run_git(&repo_path, &["fetch", "-q", "--prune", "origin"]);

    let result = cleanup_branches_operation(
        &repo_path,
        &CleanupBranchesOptions {
            base: "main".to_string(),
            branches: vec![request("merged", false)],
            delete_remote: true,
            auth_token: None,
        },
    );

    assert!(result.success, "{}", result.message);
    assert_eq!(result.message, "Deleted 1 branch.");
    assert_eq!(remote_branches(&remote_path), ["ff", "main", "wip"]);
}

#[test]
fn blocks_without_branches_or_with_an_unknown_base_and_never_deletes_the_current_one() {
    let (_root, repo_path, _remote) = fixture();

    let empty = cleanup_branches_operation(
        &repo_path,
        &CleanupBranchesOptions {
            base: "main".to_string(),
            ..Default::default()
        },
    );
    assert_eq!(empty.outcome, Outcome::Blocked);
    assert_eq!(empty.message, "No branches selected.");

    let unknown = cleanup_branches_operation(
        &repo_path,
        &CleanupBranchesOptions {
            base: "--nope".to_string(),
            branches: vec![request("merged", false)],
            ..Default::default()
        },
    );
    assert_eq!(unknown.outcome, Outcome::Blocked);
    assert_eq!(unknown.message, "Base branch '--nope' does not exist.");

    run_git(&repo_path, &["checkout", "-q", "merged"]);
    let current = cleanup_branches_operation(
        &repo_path,
        &CleanupBranchesOptions {
            base: "main".to_string(),
            branches: vec![
                request("merged", true),
                request("missing", true),
                request("-D", true),
            ],
            ..Default::default()
        },
    );
    assert_eq!(current.outcome, Outcome::Failed);
    assert_eq!(
        current.message,
        "Deleted 0 branches. 3 failed:\nmerged: it is the current branch\nmissing: branch not found\n-D: invalid branch name"
    );
    assert_eq!(branch_names(&repo_path), ["ff", "main", "merged", "wip"]);
}
