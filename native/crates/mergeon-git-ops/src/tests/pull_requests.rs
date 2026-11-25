//! Traer la rama de un pull request (`fetch_pull_request_operation`) contra
//! un remoto bare local que imita los refs `refs/pull/<n>/head` de GitHub.

use super::support::*;
use crate::{
    Code, FetchPullRequestOptions, Operation, Outcome, PullRequestHead,
    fetch_pull_request_operation, pull_request_branch_name,
};

/// Un remoto con `main`, una rama `feat/search` y `refs/pull/7/head` (la
/// rama de un fork), y un clon que todavía no conoce ninguna de las dos.
fn remote_with_pull_requests() -> (tempfile::TempDir, std::path::PathBuf, String, String) {
    let root = create_temp_root();
    let origin = root.path().join("origin.git");
    let author = root.path().join("author");
    let clone = root.path().join("clone");
    init_bare(&origin, true);
    init_repository(&author);
    commit_file(&author, "base\n", "initial commit");
    run_git(&author, &["remote", "add", "origin", path_str(&origin)]);
    run_git(&author, &["push", "origin", "main"]);
    clone_repository(&origin, &clone, "Test User", "test@example.com");

    run_git(&author, &["checkout", "-b", "feat/search"]);
    let same_repo = commit_file(&author, "search\n", "add search");
    run_git(&author, &["push", "origin", "feat/search"]);
    run_git(&author, &["checkout", "-b", "fork-work", "main"]);
    let fork = commit_file(&author, "fork\n", "fork work");
    run_git(&author, &["push", "origin", "HEAD:refs/pull/7/head"]);

    (root, clone, same_repo, fork)
}

fn options(head: PullRequestHead) -> FetchPullRequestOptions {
    FetchPullRequestOptions {
        remote: "origin".to_string(),
        head,
        auth_token: None,
    }
}

#[test]
fn fetches_the_branch_of_a_pull_request_from_the_same_repository() {
    let (_root, clone, same_repo, _) = remote_with_pull_requests();

    let result = fetch_pull_request_operation(
        &clone,
        &options(PullRequestHead::SameRepository {
            branch: "feat/search".to_string(),
        }),
    );

    assert!(result.success, "{}", result.message);
    assert_eq!(result.operation, Operation::Fetch);
    assert_eq!(result.outcome, Outcome::Completed);
    assert_eq!(result.refs_changed, Some(true));
    assert_eq!(result.message, "Fetched 'feat/search' from 'origin'.");
    assert_eq!(
        run_git(&clone, &["rev-parse", "refs/remotes/origin/feat/search"]).trim(),
        same_repo
    );
    // No se crea ninguna rama local: eso lo hace el checkout.
    assert!(
        run_git(&clone, &["branch", "--list", "feat/search"])
            .trim()
            .is_empty()
    );
}

#[test]
fn fetches_a_pull_request_from_a_fork_into_its_own_local_branch() {
    let (_root, clone, _, fork) = remote_with_pull_requests();

    let result =
        fetch_pull_request_operation(&clone, &options(PullRequestHead::Fork { number: 7 }));

    assert!(result.success, "{}", result.message);
    assert_eq!(result.message, "Fetched pull request #7 into 'pr-7'.");
    assert_eq!(pull_request_branch_name(7), "pr-7");
    assert_eq!(run_git(&clone, &["rev-parse", "pr-7"]).trim(), fork);
    // Sigue en main: traer no cambia de rama.
    assert_eq!(
        run_git(&clone, &["branch", "--show-current"]).trim(),
        "main"
    );

    // Otra vez, sin novedades: se anuncia que no cambió nada.
    let again = fetch_pull_request_operation(&clone, &options(PullRequestHead::Fork { number: 7 }));
    assert!(again.success);
    assert_eq!(again.refs_changed, Some(false));
}

#[test]
fn reports_a_missing_remote_or_pull_request() {
    let (_root, clone, _, _) = remote_with_pull_requests();

    let missing_remote = fetch_pull_request_operation(
        &clone,
        &FetchPullRequestOptions {
            remote: "upstream".to_string(),
            ..options(PullRequestHead::Fork { number: 7 })
        },
    );
    assert!(!missing_remote.success);
    assert_eq!(missing_remote.code, Code::RemoteNotFound);

    let missing_pull =
        fetch_pull_request_operation(&clone, &options(PullRequestHead::Fork { number: 99 }));
    assert!(!missing_pull.success);
    assert!(
        run_git(&clone, &["branch", "--list", "pr-99"])
            .trim()
            .is_empty()
    );
}

#[test]
fn refuses_branch_names_that_would_change_the_refspec() {
    let (_root, clone, _, _) = remote_with_pull_requests();

    for branch in ["a:refs/heads/main", "-upload-pack=x", "*"] {
        let result = fetch_pull_request_operation(
            &clone,
            &options(PullRequestHead::SameRepository {
                branch: branch.to_string(),
            }),
        );
        assert!(!result.success, "{branch}");
        assert_eq!(result.outcome, Outcome::Blocked);
    }
}
