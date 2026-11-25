//! fetch, pull y push contra remotos bare locales.

use mergeon_gitcmd::{OperationContext, get_structured_status, run_with_operation_context};
use std::time::Duration;

use super::support::*;
use crate::{
    Code, FetchOptions, Operation, Outcome, PullOptions, PushOptions, fetch_operation,
    pull_operation, push_operation,
};

fn fetch_origin() -> FetchOptions {
    FetchOptions::default()
}

fn fetch_all() -> FetchOptions {
    FetchOptions {
        all_remotes: true,
        ..Default::default()
    }
}

fn pull_main() -> PullOptions {
    PullOptions {
        branch: Some("main".to_string()),
        ..Default::default()
    }
}

#[test]
fn returns_structured_results_for_selected_and_all_remote_fetches() {
    let root = create_temp_root();
    let repo_path = root.path().join("repo");
    let origin_path = root.path().join("origin.git");
    let backup_path = root.path().join("backup.git");

    init_repository(&repo_path);
    init_bare(&origin_path, false);
    init_bare(&backup_path, false);
    commit_file(&repo_path, "hello\n", "initial commit");
    run_git(
        &repo_path,
        &["remote", "add", "origin", path_str(&origin_path)],
    );
    run_git(
        &repo_path,
        &["remote", "add", "backup", path_str(&backup_path)],
    );

    let selected = fetch_operation(&repo_path, &fetch_origin());
    let all = fetch_operation(&repo_path, &fetch_all());

    // Remotos vacíos: no llega ningún ref nuevo, así que el fetch se anuncia
    // como noop y la UI puede saltarse la recarga del historial.
    assert!(selected.success);
    assert_eq!(selected.operation, Operation::Fetch);
    assert_eq!(selected.outcome, Outcome::Noop);
    assert_eq!(selected.code, Code::AlreadyUpToDate);
    assert_eq!(selected.message, "Already up to date.");
    assert_eq!(selected.refs_changed, Some(false));
    assert_eq!(selected.remote.as_deref(), Some("origin"));
    assert!(!selected.all_remotes);
    assert_eq!(selected.remote_results.len(), 1);
    assert_eq!(selected.summary, None);

    assert!(all.success);
    assert_eq!(all.operation, Operation::Fetch);
    assert_eq!(all.outcome, Outcome::Noop);
    assert_eq!(all.code, Code::AlreadyUpToDate);
    assert_eq!(all.refs_changed, Some(false));
    assert_eq!(all.remote, None);
    assert!(all.all_remotes);
    let mut fetched: Vec<&str> = all
        .remote_results
        .iter()
        .map(|result| result.remote.as_str())
        .collect();
    fetched.sort_unstable();
    assert_eq!(fetched, ["backup", "origin"]);
    assert!(all.remote_results.iter().all(|result| result.success));
}

/// Bare con un commit en `main` empujado desde un clon propio.
fn remote_with_a_commit(root: &std::path::Path, name: &str, message: &str) -> std::path::PathBuf {
    let remote_path = root.join(format!("{name}.git"));
    let author_path = root.join(format!("{name}-author"));
    init_bare(&remote_path, true);
    init_repository(&author_path);
    commit_file(&author_path, &format!("{name}\n"), message);
    run_git(&author_path, &["push", path_str(&remote_path), "main"]);
    remote_path
}

fn remote_tracking_refs(repo_path: &std::path::Path) -> Vec<String> {
    let mut refs: Vec<String> = run_git(
        repo_path,
        &["for-each-ref", "--format=%(refname)", "refs/remotes"],
    )
    .lines()
    // Git moderno también crea `refs/remotes/<remoto>/HEAD` al traer.
    .filter(|name| !name.ends_with("/HEAD"))
    .map(str::to_string)
    .collect();
    refs.sort_unstable();
    refs
}

/*
 * `git.fetch(remoto)` de simple-git ejecuta `git fetch` a secas: en la app
 * Electron elegir otro remoto traía el de por defecto, y "Fetch all" traía N
 * veces el mismo. Cada fetch debe traer su remoto.
 */
#[test]
fn fetches_the_selected_remote_and_every_remote_on_fetch_all() {
    let root = create_temp_root();
    let repo_path = root.path().join("repo");
    init_repository(&repo_path);
    commit_file(&repo_path, "local\n", "local commit");
    let origin_path = remote_with_a_commit(root.path(), "origin", "origin commit");
    let backup_path = remote_with_a_commit(root.path(), "backup", "backup commit");
    run_git(
        &repo_path,
        &["remote", "add", "origin", path_str(&origin_path)],
    );
    run_git(
        &repo_path,
        &["remote", "add", "backup", path_str(&backup_path)],
    );

    let selected = fetch_operation(
        &repo_path,
        &FetchOptions {
            remote: "backup".to_string(),
            ..Default::default()
        },
    );
    assert!(selected.success, "{}", selected.message);
    assert_eq!(selected.outcome, Outcome::Completed);
    assert_eq!(
        selected.message,
        "Fetch completed successfully for 'backup'."
    );
    assert_eq!(
        remote_tracking_refs(&repo_path),
        ["refs/remotes/backup/main"]
    );

    let third_path = remote_with_a_commit(root.path(), "third", "third commit");
    run_git(
        &repo_path,
        &["remote", "add", "third", path_str(&third_path)],
    );

    let all = fetch_operation(&repo_path, &fetch_all());
    assert!(all.success, "{}", all.message);
    assert_eq!(all.outcome, Outcome::Completed);
    assert_eq!(all.message, "Fetch completed successfully for 3 remotes.");
    assert_eq!(
        remote_tracking_refs(&repo_path),
        [
            "refs/remotes/backup/main",
            "refs/remotes/origin/main",
            "refs/remotes/third/main"
        ]
    );
}

#[test]
fn reports_refs_changed_when_a_fetch_actually_moves_remote_refs() {
    let root = create_temp_root();
    let repo_path = root.path().join("repo");
    let contributor_path = root.path().join("contributor");
    let origin_path = root.path().join("origin.git");

    init_repository(&repo_path);
    // El bare necesita HEAD en main para que el clon quede en esa misma rama.
    init_bare(&origin_path, true);
    commit_file(&repo_path, "hello\n", "initial commit");
    run_git(
        &repo_path,
        &["remote", "add", "origin", path_str(&origin_path)],
    );
    run_git(&repo_path, &["push", "-u", "origin", "main"]);

    clone_repository(
        &origin_path,
        &contributor_path,
        "Other User",
        "other@example.com",
    );
    commit_file(&contributor_path, "from upstream\n", "upstream commit");
    run_git(&contributor_path, &["push", "origin", "main"]);

    let with_new_refs = fetch_operation(&repo_path, &fetch_origin());
    assert!(with_new_refs.success);
    assert_eq!(with_new_refs.operation, Operation::Fetch);
    assert_eq!(with_new_refs.outcome, Outcome::Completed);
    assert_eq!(with_new_refs.code, Code::Ok);
    assert_eq!(
        with_new_refs.message,
        "Fetch completed successfully for 'origin'."
    );
    assert_eq!(with_new_refs.refs_changed, Some(true));

    // El segundo fetch ya no trae nada: mismo repo, resultado distinto.
    let without_new_refs = fetch_operation(&repo_path, &fetch_origin());
    assert_eq!(without_new_refs.outcome, Outcome::Noop);
    assert_eq!(without_new_refs.refs_changed, Some(false));
}

/// Una rama borrada en el remoto deja de figurar como `origin/x` tras el
/// fetch (antes quedaba para siempre en el sidebar y en el grafo); la rama
/// local con el mismo nombre se conserva.
#[test]
fn fetch_prunes_remote_branches_that_no_longer_exist() {
    let root = create_temp_root();
    let repo_path = root.path().join("repo");
    let origin_path = root.path().join("origin.git");

    init_repository(&repo_path);
    init_bare(&origin_path, true);
    commit_file(&repo_path, "hello\n", "initial commit");
    run_git(
        &repo_path,
        &["remote", "add", "origin", path_str(&origin_path)],
    );
    run_git(&repo_path, &["push", "-u", "origin", "main"]);
    run_git(&repo_path, &["branch", "feature/old"]);
    run_git(&repo_path, &["push", "origin", "feature/old"]);
    assert_eq!(
        remote_tracking_refs(&repo_path),
        [
            "refs/remotes/origin/feature/old",
            "refs/remotes/origin/main"
        ]
    );

    run_git(&origin_path, &["branch", "-D", "feature/old"]);
    let fetched = fetch_operation(&repo_path, &fetch_origin());

    assert!(fetched.success, "{}", fetched.message);
    assert_eq!(fetched.refs_changed, Some(true));
    assert_eq!(
        remote_tracking_refs(&repo_path),
        ["refs/remotes/origin/main"]
    );
    assert_eq!(
        run_git(&repo_path, &["branch", "--list", "feature/old"]).trim(),
        "feature/old"
    );
}

#[test]
fn fetch_blocks_on_unknown_remotes_and_reports_missing_ones() {
    let (_root, repo_path) = create_repository();

    let unknown = fetch_operation(&repo_path, &fetch_origin());
    assert!(!unknown.success);
    assert_eq!(unknown.outcome, Outcome::Blocked);
    assert_eq!(unknown.code, Code::RemoteNotFound);
    assert_eq!(
        unknown.message,
        "Remote 'origin' not found. Please configure the remote first."
    );
    assert_eq!(unknown.refs_changed, Some(false));

    let none = fetch_operation(&repo_path, &fetch_all());
    assert!(none.success);
    assert_eq!(none.outcome, Outcome::Noop);
    assert_eq!(none.code, Code::NoRemotes);
    assert_eq!(none.message, "No Git remotes are configured.");
    assert!(none.all_remotes);
    assert_eq!(none.refs_changed, Some(false));
}

#[test]
fn fetch_reports_remotes_that_cannot_be_reached() {
    let (root, repo_path) = create_repository();
    let missing = root.path().join("missing.git");
    run_git(&repo_path, &["remote", "add", "origin", path_str(&missing)]);

    let selected = fetch_operation(&repo_path, &fetch_origin());
    assert!(!selected.success);
    assert_eq!(selected.outcome, Outcome::Failed);
    assert_eq!(selected.code, Code::GitError);
    assert!(
        selected
            .message
            .contains("does not appear to be a git repository"),
        "{}",
        selected.message
    );
    assert!(!selected.message.starts_with("fatal:"));
    assert_eq!(selected.remote.as_deref(), Some("origin"));
    assert_eq!(selected.refs_changed, None);

    let all = fetch_operation(&repo_path, &fetch_all());
    assert!(!all.success);
    assert_eq!(all.outcome, Outcome::Failed);
    assert_eq!(all.code, Code::GitError);
    // El motivo viaja en el mensaje: el toast lo resume y "Details" lo muestra.
    assert!(
        all.message.starts_with("Fetch failed for 'origin'.\n")
            && all
                .message
                .contains("does not appear to be a git repository"),
        "{}",
        all.message
    );
    assert_eq!(all.remote_results.len(), 1);
    let failure = &all.remote_results[0];
    assert!(!failure.success);
    assert_eq!(failure.remote, "origin");
    assert_eq!(failure.code, Some(Code::GitError));
    assert!(
        failure
            .message
            .as_deref()
            .is_some_and(|message| !message.is_empty())
    );
}

#[test]
fn returns_a_structured_result_when_pull_completes_without_output() {
    let root = create_temp_root();
    let repo_path = root.path().join("repo");
    let remote_path = root.path().join("remote.git");

    init_repository(&repo_path);
    init_bare(&remote_path, false);
    commit_file(&repo_path, "hello\n", "initial commit");
    run_git(
        &repo_path,
        &["remote", "add", "origin", path_str(&remote_path)],
    );
    run_git(&repo_path, &["push", "-u", "origin", "main"]);

    let result = pull_operation(&repo_path, &pull_main());

    assert!(result.success);
    assert_eq!(result.operation, Operation::Pull);
    assert_eq!(result.outcome, Outcome::Noop);
    assert_eq!(result.code, Code::AlreadyUpToDate);
    assert_eq!(result.message, "Already up to date.");
    assert_eq!(result.remote.as_deref(), Some("origin"));
    assert_eq!(result.branch.as_deref(), Some("main"));
    assert_eq!(result.refs_changed, Some(false));
    assert!(!result.rebase);
}

/// Remoto con un commit base, clon local con tracking y un colega que ya
/// empujó `colleague_file` con `colleague_content`.
fn repository_behind_a_colleague(
    colleague_file: &str,
    colleague_content: &str,
) -> (tempfile::TempDir, std::path::PathBuf) {
    let root = create_temp_root();
    let repo_path = root.path().join("repo");
    let remote_path = root.path().join("remote.git");
    let colleague_path = root.path().join("colleague");

    init_repository(&repo_path);
    // `-b main` en el bare: sin eso el HEAD del remoto puede quedar en master
    // y el clon del colega nace en la rama equivocada.
    init_bare(&remote_path, true);
    commit_file(&repo_path, "base\n", "initial commit");
    run_git(
        &repo_path,
        &["remote", "add", "origin", path_str(&remote_path)],
    );
    run_git(&repo_path, &["push", "-u", "origin", "main"]);

    clone_repository(
        &remote_path,
        &colleague_path,
        "Colleague",
        "colleague@example.com",
    );
    commit_file_at(
        &colleague_path,
        colleague_content,
        "remote commit",
        colleague_file,
    );
    run_git(&colleague_path, &["push", "origin", "main"]);

    (root, repo_path)
}

#[test]
fn merges_a_divergent_pull_instead_of_failing_on_the_reconcile_hint() {
    // El colega avanza el remoto tocando OTRO archivo; el local avanza el suyo.
    let (_root, repo_path) = repository_behind_a_colleague("remote.txt", "remote change\n");
    commit_file_at(&repo_path, "local change\n", "local commit", "local.txt");

    // Sin estrategia explícita, git moderno aborta aquí con
    // "Need to specify how to reconcile divergent branches".
    let result = pull_operation(&repo_path, &pull_main());

    assert!(result.success, "{}", result.message);
    assert_eq!(result.outcome, Outcome::Completed);
    assert_eq!(result.refs_changed, Some(true));
    assert!(result.summary.is_some());
    let parents = run_git(&repo_path, &["rev-list", "--parents", "-1", "HEAD"]);
    assert_eq!(parents.trim().split(' ').count(), 3);
}

#[test]
fn pull_with_rebase_keeps_a_linear_history() {
    let (_root, repo_path) = repository_behind_a_colleague("remote.txt", "remote change\n");
    commit_file_at(&repo_path, "local change\n", "local commit", "local.txt");

    let result = pull_operation(
        &repo_path,
        &PullOptions {
            rebase: true,
            ..pull_main()
        },
    );

    assert!(result.success, "{}", result.message);
    assert!(result.rebase);
    assert_eq!(
        log_subjects(&repo_path),
        ["local commit", "remote commit", "initial commit"]
    );
    let parents = run_git(&repo_path, &["rev-list", "--parents", "-1", "HEAD"]);
    assert_eq!(parents.trim().split(' ').count(), 2);
}

#[test]
fn pull_reports_conflicts_with_the_repository_state() {
    // El colega y el local cambian la misma línea del mismo archivo.
    let (_root, repo_path) = repository_behind_a_colleague("file.txt", "remote\n");
    commit_file(&repo_path, "local\n", "local commit");

    let result = pull_operation(&repo_path, &pull_main());

    assert!(!result.success);
    assert_eq!(result.operation, Operation::Pull);
    assert_eq!(result.outcome, Outcome::Conflicted);
    assert_eq!(result.code, Code::Conflict);
    assert_eq!(result.conflict_files, ["file.txt"]);
    assert_eq!(result.conflict_state.pending_count, 1);
    assert!(
        result
            .operation_state
            .is_some_and(|state| state.in_progress)
    );
}

#[test]
fn returns_remote_not_found_when_pushing_without_origin() {
    let root = create_temp_root();
    let repo_path = root.path().join("repo");

    init_repository(&repo_path);
    commit_file(&repo_path, "hello\n", "initial commit");

    let result = push_operation(&repo_path, &PushOptions::default());

    assert!(!result.success);
    assert_eq!(result.outcome, Outcome::Blocked);
    assert_eq!(result.code, Code::RemoteNotFound);
    assert_eq!(result.remote.as_deref(), Some("origin"));
    assert_eq!(result.branch.as_deref(), Some("main"));
}

#[test]
fn sets_upstream_on_the_first_successful_push() {
    let root = create_temp_root();
    let repo_path = root.path().join("repo");
    let remote_path = root.path().join("remote.git");

    init_repository(&repo_path);
    init_bare(&remote_path, false);
    commit_file(&repo_path, "hello\n", "initial commit");
    run_git(
        &repo_path,
        &["remote", "add", "origin", path_str(&remote_path)],
    );

    let result = push_operation(&repo_path, &PushOptions::default());
    let status = get_structured_status(&repo_path).unwrap();

    assert!(result.success, "{}", result.message);
    assert_eq!(result.code, Code::UpstreamCreated);
    assert_eq!(result.outcome, Outcome::Completed);
    assert_eq!(
        result.message,
        "Push completed and upstream set to origin/main."
    );
    assert!(result.upstream_set);
    assert_eq!(result.branch.as_deref(), Some("main"));
    assert_eq!(status.tracking.as_deref(), Some("origin/main"));
}

/*
 * La app Electron no sabía leer el upstream (`git.branch()` de simple-git no
 * trae `tracking`): todo push llevaba `-u` y se anunciaba como
 * UPSTREAM_CREATED. Con upstream ya configurado no se pasa `-u` (que lo
 * movería) y el resultado es un push normal.
 */
#[test]
fn only_sets_the_upstream_when_the_branch_has_none() {
    let root = create_temp_root();
    let repo_path = root.path().join("repo");
    let remote_path = root.path().join("remote.git");
    let backup_path = root.path().join("backup.git");

    init_repository(&repo_path);
    init_bare(&remote_path, false);
    init_bare(&backup_path, false);
    commit_file(&repo_path, "hello\n", "initial commit");
    run_git(
        &repo_path,
        &["remote", "add", "origin", path_str(&remote_path)],
    );
    run_git(
        &repo_path,
        &["remote", "add", "backup", path_str(&backup_path)],
    );

    let first = push_operation(&repo_path, &PushOptions::default());
    assert!(first.success, "{}", first.message);
    assert_eq!(first.code, Code::UpstreamCreated);
    assert!(first.upstream_set);

    commit_file(&repo_path, "again\n", "second commit");
    let second = push_operation(&repo_path, &PushOptions::default());
    assert!(second.success, "{}", second.message);
    assert_eq!(second.outcome, Outcome::Completed);
    assert_eq!(second.code, Code::Ok);
    assert!(!second.upstream_set);
    assert_ne!(
        second.message,
        "Push completed and upstream set to origin/main."
    );

    // Empujar a otro remoto no mueve el upstream que ya había.
    let to_backup = push_operation(
        &repo_path,
        &PushOptions {
            remote: "backup".to_string(),
            ..Default::default()
        },
    );
    assert!(to_backup.success, "{}", to_backup.message);
    assert!(!to_backup.upstream_set);
    assert_eq!(
        get_structured_status(&repo_path)
            .unwrap()
            .tracking
            .as_deref(),
        Some("origin/main")
    );
    assert_eq!(
        run_git(&backup_path, &["rev-parse", "main"]).trim(),
        head(&repo_path)
    );

    let again = push_operation(&repo_path, &PushOptions::default());
    assert!(again.success, "{}", again.message);
    assert!(!again.upstream_set);
    assert_ne!(again.code, Code::UpstreamCreated);
}

#[test]
fn pushes_to_a_differently_named_branch_and_includes_tags() {
    let root = create_temp_root();
    let repo_path = root.path().join("repo");
    let remote_path = root.path().join("remote.git");

    init_repository(&repo_path);
    init_bare(&remote_path, false);
    commit_file(&repo_path, "hello\n", "initial commit");
    run_git(&repo_path, &["tag", "v1.0.0"]);
    run_git(
        &repo_path,
        &["remote", "add", "origin", path_str(&remote_path)],
    );

    let result = push_operation(
        &repo_path,
        &PushOptions {
            target_branch: Some("release".to_string()),
            push_tags: true,
            ..Default::default()
        },
    );

    assert!(result.success, "{}", result.message);
    assert_eq!(result.code, Code::Ok);
    assert_eq!(
        result.message,
        "Push completed successfully, including tags."
    );
    // Con otro nombre en el remoto no se fija upstream.
    assert!(!result.upstream_set);
    assert_eq!(get_structured_status(&repo_path).unwrap().tracking, None);

    let remote_refs = run_git(&remote_path, &["for-each-ref", "--format=%(refname)"]);
    let mut remote_refs: Vec<&str> = remote_refs.lines().collect();
    remote_refs.sort_unstable();
    assert_eq!(remote_refs, ["refs/heads/release", "refs/tags/v1.0.0"]);
}

#[test]
fn push_is_blocked_on_a_detached_head() {
    let (_root, repo_path) = create_repository();
    let commit = head(&repo_path);
    run_git(&repo_path, &["checkout", "--detach", &commit]);

    let result = push_operation(&repo_path, &PushOptions::default());

    assert!(!result.success);
    assert_eq!(result.outcome, Outcome::Blocked);
    assert_eq!(result.code, Code::NoCurrentBranch);
    assert_eq!(result.branch, None);
    assert_eq!(result.remote.as_deref(), Some("origin"));
}

#[test]
fn a_rejected_push_is_a_structured_failure() {
    let (_root, repo_path) = repository_behind_a_colleague("remote.txt", "remote change\n");
    commit_file_at(&repo_path, "local change\n", "local commit", "local.txt");

    // El remoto tiene un commit que el local no: el push no es fast-forward.
    let result = push_operation(&repo_path, &PushOptions::default());

    assert!(!result.success);
    assert_eq!(result.operation, Operation::Push);
    assert_eq!(result.outcome, Outcome::Failed);
    assert_eq!(result.code, Code::GitError);
    assert!(result.message.contains("rejected"), "{}", result.message);
    assert_eq!(result.branch.as_deref(), Some("main"));
    assert!(!result.upstream_set);
}

#[test]
fn option_like_remote_names_never_reach_git() {
    let (_root, repo_path) = create_repository();

    let result = pull_operation(
        &repo_path,
        &PullOptions {
            remote: Some("--upload-pack=touch pwned".to_string()),
            ..pull_main()
        },
    );

    assert!(!result.success);
    assert_eq!(result.outcome, Outcome::Failed);
    assert_eq!(result.code, Code::GitError);
    assert_eq!(
        result.message,
        "Use of --upload-pack or --receive-pack is not permitted without enabling allowUnsafePack"
    );
    assert!(!repo_path.join("pwned").exists());
}

#[test]
fn a_cancelled_operation_reports_the_control_code() {
    let (root, repo_path) = create_repository();
    let remote_path = root.path().join("remote.git");
    init_bare(&remote_path, false);
    run_git(
        &repo_path,
        &["remote", "add", "origin", path_str(&remote_path)],
    );

    let context = OperationContext::new(Some("fetch-1".to_string()), Duration::from_secs(30));
    context.cancel();
    let (fetch, push) = run_with_operation_context(context, || {
        (
            fetch_operation(&repo_path, &fetch_origin()),
            push_operation(&repo_path, &PushOptions::default()),
        )
    });

    for result in [&fetch, &push] {
        assert!(!result.success);
        assert_eq!(result.outcome, Outcome::Cancelled);
        assert_eq!(result.code, Code::Cancelled);
        assert_eq!(result.message, "Git operation was cancelled.");
        // Con la operación cancelada tampoco se puede leer el estado.
        assert_eq!(
            result
                .status
                .as_ref()
                .and_then(|status| status.current.as_deref()),
            Some("")
        );
    }
    assert_eq!(fetch.refs_changed, None);
}
