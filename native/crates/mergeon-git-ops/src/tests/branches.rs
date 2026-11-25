//! Ramas, checkout y reset.

use mergeon_gitcmd::get_structured_status;

use super::support::*;
use crate::{
    Code, Operation, Outcome, checkout_operation, checkout_remote_branch_operation,
    create_branch_operation, delete_branch_operation, delete_remote_branch_operation,
    delete_remote_branch_with_auth_operation, fast_forward_branch_operation,
    rename_branch_operation, reset_operation, stash_branch_operation,
};

fn branch_names(repo_path: &std::path::Path) -> Vec<String> {
    run_git(repo_path, &["branch", "--format=%(refname:short)"])
        .lines()
        .map(str::to_string)
        .collect()
}

#[test]
fn creates_a_branch_with_structured_results() {
    let (_root, repo_path) = create_repository();

    let result = create_branch_operation(&repo_path, "feature/refactor", None, true);

    assert!(result.success, "{}", result.message);
    assert_eq!(result.code, Code::Ok);
    assert_eq!(result.message, "Created and switched to feature/refactor.");
    assert_eq!(result.branch.as_deref(), Some("feature/refactor"));
    assert_eq!(
        result.status.as_ref().unwrap().current.as_deref(),
        Some("feature/refactor")
    );
    assert_eq!(
        result.operation_state.map(|state| state.in_progress),
        Some(false)
    );
    assert_eq!(result.conflict_state.pending_count, 0);
}

#[test]
fn creates_branches_from_a_start_point_with_or_without_checkout() {
    let (_root, repo_path) = create_repository();
    let base = head(&repo_path);
    commit_file(&repo_path, "second\n", "second");

    let switched = create_branch_operation(&repo_path, " from-base ", Some(&base), true);
    assert!(switched.success, "{}", switched.message);
    assert_eq!(
        switched.message,
        format!("Created from-base from {base} and switched to it.")
    );
    assert_eq!(head(&repo_path), base);

    let from_main = create_branch_operation(&repo_path, "kept", Some("main"), false);
    assert!(from_main.success, "{}", from_main.message);
    assert_eq!(from_main.message, "Created kept from main.");
    // Sin checkout la rama actual no cambia.
    assert_eq!(
        from_main.status.unwrap().current.as_deref(),
        Some("from-base")
    );
    assert_eq!(
        run_git(&repo_path, &["rev-parse", "kept"]).trim(),
        run_git(&repo_path, &["rev-parse", "main"]).trim()
    );

    let plain = create_branch_operation(&repo_path, "plain", None, false);
    assert!(plain.success, "{}", plain.message);
    assert_eq!(plain.message, "Created plain.");

    let duplicate = create_branch_operation(&repo_path, "plain", None, false);
    assert!(!duplicate.success);
    assert_eq!(duplicate.outcome, Outcome::Failed);
    assert_eq!(duplicate.code, Code::GitError);
    assert!(
        duplicate.message.contains("already exists"),
        "{}",
        duplicate.message
    );
    assert_eq!(duplicate.branch.as_deref(), Some("plain"));
}

#[test]
fn checks_out_branches_and_detached_commits() {
    let (_root, repo_path) = create_repository();
    let base = head(&repo_path);
    run_git(&repo_path, &["branch", "feature"]);

    let branch = checkout_operation(&repo_path, "feature");
    assert!(branch.success, "{}", branch.message);
    assert_eq!(branch.code, Code::Ok);
    assert_eq!(branch.message, "Switched to feature.");
    assert_eq!(branch.branch.as_deref(), Some("feature"));

    let detached = checkout_operation(&repo_path, &base);
    assert!(detached.success, "{}", detached.message);
    assert_eq!(detached.code, Code::DetachedHead);
    assert_eq!(
        detached.message,
        format!("Checked out {base} in detached HEAD state.")
    );
    assert_eq!(detached.branch, None);

    let missing = checkout_operation(&repo_path, "no-such-branch");
    assert!(!missing.success);
    assert_eq!(missing.outcome, Outcome::Failed);
    assert_eq!(missing.code, Code::GitError);
    assert_eq!(missing.branch.as_deref(), Some("no-such-branch"));
}

#[test]
fn returns_checkout_blocked_when_local_changes_prevent_checkout() {
    let (_root, repo_path) = create_repository();
    run_git(&repo_path, &["checkout", "-b", "feature"]);
    commit_file(&repo_path, "feature\n", "feature");
    run_git(&repo_path, &["checkout", "main"]);
    std::fs::write(repo_path.join("file.txt"), "dirty\n").unwrap();

    let result = checkout_operation(&repo_path, "feature");

    assert!(!result.success);
    assert_eq!(result.code, Code::CheckoutBlocked);
    assert_eq!(result.outcome, Outcome::Blocked);
    assert!(
        result.message.starts_with("Your local changes"),
        "{}",
        result.message
    );
}

#[test]
fn checks_out_a_remote_branch_with_a_structured_result() {
    let root = create_temp_root();
    let remote_path = root.path().join("remote.git");
    let source_repo_path = root.path().join("source");
    let clone_repo_path = root.path().join("clone");
    init_repository(&source_repo_path);

    init_bare(&remote_path, true);
    commit_file(&source_repo_path, "base\n", "base");
    run_git(&source_repo_path, &["checkout", "-b", "feature"]);
    commit_file(&source_repo_path, "feature\n", "feature");
    run_git(&source_repo_path, &["checkout", "main"]);
    run_git(
        &source_repo_path,
        &["remote", "add", "origin", path_str(&remote_path)],
    );
    run_git(&source_repo_path, &["push", "-u", "origin", "main"]);
    run_git(&source_repo_path, &["push", "-u", "origin", "feature"]);
    clone_repository(
        &remote_path,
        &clone_repo_path,
        "Test User",
        "test@example.com",
    );
    run_git(&clone_repo_path, &["fetch", "origin"]);

    let result = checkout_remote_branch_operation(&clone_repo_path, "origin/feature");

    assert!(result.success, "{}", result.message);
    assert_eq!(result.operation, Operation::CheckoutRemoteBranch);
    assert_eq!(
        result.message,
        "Created and switched to feature tracking origin/feature."
    );
    assert_eq!(result.branch.as_deref(), Some("feature"));
    let status = result.status.unwrap();
    assert_eq!(status.current.as_deref(), Some("feature"));
    assert_eq!(status.tracking.as_deref(), Some("origin/feature"));

    // La rama local ya existe: el segundo intento falla con los datos de la petición.
    run_git(&clone_repo_path, &["checkout", "main"]);
    let again = checkout_remote_branch_operation(&clone_repo_path, "origin/feature");
    assert!(!again.success);
    assert_eq!(again.code, Code::GitError);
    assert_eq!(again.branch.as_deref(), Some("feature"));
    assert_eq!(again.remote.as_deref(), Some("origin/feature"));
}

#[test]
fn renames_and_deletes_branches() {
    let (_root, repo_path) = create_repository();
    run_git(&repo_path, &["branch", "old-name"]);

    let renamed = rename_branch_operation(&repo_path, "old-name", "new-name");
    assert!(renamed.success, "{}", renamed.message);
    assert_eq!(renamed.message, "Renamed old-name to new-name.");
    assert_eq!(renamed.branch.as_deref(), Some("new-name"));
    assert_eq!(branch_names(&repo_path), ["main", "new-name"]);

    // Renombrar la rama actual usa la forma corta de `git branch -m`.
    let current = rename_branch_operation(&repo_path, "main", "trunk");
    assert!(current.success, "{}", current.message);
    assert_eq!(current.status.unwrap().current.as_deref(), Some("trunk"));

    let unchanged = rename_branch_operation(&repo_path, "trunk", " trunk ");
    assert!(!unchanged.success);
    assert_eq!(unchanged.outcome, Outcome::Noop);
    assert_eq!(unchanged.code, Code::NoBranchChange);
    assert_eq!(unchanged.message, "Branch is already named trunk.");

    let missing = rename_branch_operation(&repo_path, "trunk", "");
    assert!(!missing.success);
    assert_eq!(missing.outcome, Outcome::Blocked);
    assert_eq!(
        missing.message,
        "Both the current and new branch names are required."
    );
    assert_eq!(missing.branch.as_deref(), Some("trunk"));

    let active = delete_branch_operation(&repo_path, "trunk");
    assert!(!active.success);
    assert_eq!(active.outcome, Outcome::Blocked);
    assert_eq!(active.code, Code::ActiveBranch);
    assert_eq!(
        active.message,
        "Cannot delete the currently checked out branch 'trunk'."
    );

    let deleted = delete_branch_operation(&repo_path, "new-name");
    assert!(deleted.success, "{}", deleted.message);
    assert_eq!(deleted.message, "Deleted branch new-name.");
    assert_eq!(branch_names(&repo_path), ["trunk"]);
}

#[test]
fn refuses_to_delete_an_unmerged_branch() {
    let fixture = create_diverged_repository();

    let result = delete_branch_operation(&fixture.repo_path, "feature");

    assert!(!result.success);
    assert_eq!(result.outcome, Outcome::Failed);
    assert_eq!(result.code, Code::GitError);
    assert!(
        result.message.contains("not fully merged"),
        "{}",
        result.message
    );
    assert!(branch_names(&fixture.repo_path).contains(&"feature".to_string()));
}

#[test]
fn fast_forwards_the_current_branch_to_its_upstream() {
    let (_root, repo_path) = create_repository();
    run_git(&repo_path, &["checkout", "-b", "ahead"]);
    let tip = commit_file(&repo_path, "ahead\n", "ahead change");

    let not_current = fast_forward_branch_operation(&repo_path, "main", "ahead");
    assert!(!not_current.success);
    assert_eq!(not_current.outcome, Outcome::Blocked);
    assert_eq!(not_current.code, Code::BranchNotCheckedOut);
    assert_eq!(
        not_current.message,
        "Switch to main before fast-forwarding it."
    );
    assert_eq!(not_current.remote.as_deref(), Some("ahead"));

    run_git(&repo_path, &["checkout", "main"]);
    let moved = fast_forward_branch_operation(&repo_path, "main", "ahead");
    assert!(moved.success, "{}", moved.message);
    assert_eq!(moved.outcome, Outcome::Completed);
    assert_eq!(moved.message, "Fast-forwarded main to ahead.");
    assert_eq!(head(&repo_path), tip);

    let again = fast_forward_branch_operation(&repo_path, "main", "ahead");
    assert!(again.success);
    assert_eq!(again.outcome, Outcome::Noop);
    assert_eq!(again.code, Code::AlreadyUpToDate);
    assert_eq!(again.message, "main is already up to date with ahead.");

    let missing = fast_forward_branch_operation(&repo_path, "main", " ");
    assert!(!missing.success);
    assert_eq!(missing.outcome, Outcome::Blocked);
    assert_eq!(missing.branch.as_deref(), Some("main"));
    assert_eq!(missing.remote, None);
}

#[test]
fn fast_forward_fails_when_the_branches_diverged() {
    let fixture = create_diverged_repository();

    let result = fast_forward_branch_operation(&fixture.repo_path, "main", "feature");

    assert!(!result.success);
    assert_eq!(result.outcome, Outcome::Failed);
    assert_eq!(result.code, Code::GitError);
}

#[test]
fn deletes_a_branch_on_the_remote() {
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
    run_git(&repo_path, &["push", "-u", "origin", "main"]);
    run_git(&repo_path, &["push", "origin", "main:feature"]);

    let result = delete_remote_branch_operation(&repo_path, "origin", "feature");

    assert!(result.success, "{}", result.message);
    assert_eq!(result.message, "Deleted origin/feature.");
    assert_eq!(result.branch.as_deref(), Some("feature"));
    assert_eq!(result.remote.as_deref(), Some("origin"));
    assert_eq!(
        run_git(&remote_path, &["for-each-ref", "--format=%(refname)"]).trim(),
        "refs/heads/main"
    );

    let again = delete_remote_branch_operation(&repo_path, "origin", "feature");
    assert!(!again.success);
    assert_eq!(again.outcome, Outcome::Failed);
    assert_eq!(again.code, Code::GitError);
    assert!(
        again.message.contains("remote ref does not exist"),
        "{}",
        again.message
    );
}

/// Instala un `pre-push` que guarda el `credential.helper` configurado para
/// `github.com` y comprueba que no se filtre `http.extraHeader`.
fn record_auth_helper_on_push(
    repo_path: &std::path::Path,
    helper_output: &std::path::Path,
    header_output: &std::path::Path,
) {
    let hook = repo_path.join(".git").join("hooks").join("pre-push");
    std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
    std::fs::write(
        &hook,
        format!(
            "#!/bin/sh\ngit config --get-urlmatch credential.helper https://github.com/x.git > '{}'\ngit config --get-urlmatch http.extraHeader https://github.com/x.git > '{}'\nexit 0\n",
            path_str(helper_output),
            path_str(header_output)
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

// La app Electron borraba la rama remota sin el token que sí usan push, fetch
// y pull: con un remoto HTTPS privado el borrado fallaba por credenciales.
#[test]
fn deletes_a_remote_branch_with_the_auth_token() {
    let root = create_temp_root();
    let repo_path = root.path().join("repo");
    let remote_path = root.path().join("remote.git");
    let helper_path = root.path().join("helper.txt");
    let header_path = root.path().join("header.txt");
    init_repository(&repo_path);
    init_bare(&remote_path, true);
    commit_file(&repo_path, "base\n", "base");
    run_git(
        &repo_path,
        &["remote", "add", "origin", path_str(&remote_path)],
    );
    run_git(&repo_path, &["push", "-u", "origin", "main"]);
    run_git(
        &repo_path,
        &["push", "origin", "main:feature", "main:other"],
    );
    record_auth_helper_on_push(&repo_path, &helper_path, &header_path);

    let result = delete_remote_branch_with_auth_operation(
        &repo_path,
        "origin",
        "feature",
        Some("gho_secret_abc"),
    );

    assert!(result.success, "{}", result.message);
    assert_eq!(result.message, "Deleted origin/feature.");
    let helper = read_file(&helper_path);
    assert!(helper.contains("mergeon-cred-"), "{helper:?}");
    assert!(!helper.contains("gho_secret_abc"), "{helper:?}");
    assert_eq!(read_file(&header_path), "");

    // Sin token, como hasta ahora: sin helper efímero ni cabecera.
    let result = delete_remote_branch_operation(&repo_path, "origin", "other");
    assert!(result.success, "{}", result.message);
    assert!(!read_file(&helper_path).contains("mergeon-cred-"));
    assert_eq!(read_file(&header_path), "");
    assert_eq!(
        run_git(&remote_path, &["for-each-ref", "--format=%(refname)"]).trim(),
        "refs/heads/main"
    );
}

#[test]
fn resets_to_a_previous_commit_with_structured_results() {
    let (_root, repo_path) = create_repository();
    let base_commit = head(&repo_path);
    commit_file(&repo_path, "changed\n", "second");

    let result = reset_operation(&repo_path, &base_commit, "mixed");

    assert!(result.success, "{}", result.message);
    assert_eq!(result.code, Code::Ok);
    assert_eq!(
        result.message,
        format!("Mixed reset to {}.", &base_commit[..7])
    );
    assert_eq!(result.mode.as_deref(), Some("mixed"));
    assert_eq!(result.commit.as_deref(), Some(base_commit.as_str()));
    assert_eq!(head(&repo_path), base_commit);
    let status = result.status.as_ref().unwrap();
    let file = status
        .files
        .iter()
        .find(|file| file.path == "file.txt")
        .unwrap();
    assert_eq!(file.working_dir, "M");
    assert_eq!(
        result.operation_state.map(|state| state.in_progress),
        Some(false)
    );
    assert_eq!(result.conflict_state.pending_count, 0);
}

#[test]
fn supports_hard_reset_with_the_same_structured_contract() {
    let (_root, repo_path) = create_repository();
    let base_commit = head(&repo_path);
    commit_file(&repo_path, "changed\n", "second");

    let result = reset_operation(&repo_path, &base_commit, "hard");

    assert!(result.success, "{}", result.message);
    assert_eq!(result.code, Code::Ok);
    assert_eq!(result.operation, Operation::Reset);
    assert_eq!(result.mode.as_deref(), Some("hard"));
    assert_eq!(result.outcome, Outcome::Completed);
    assert_eq!(result.commit.as_deref(), Some(base_commit.as_str()));
    assert!(result.status.as_ref().unwrap().is_clean);
    assert_eq!(
        result.operation_state.map(|state| state.in_progress),
        Some(false)
    );
    assert_eq!(result.conflict_state.pending_count, 0);
    assert_eq!(head(&repo_path), base_commit);
    assert_eq!(read_file(&repo_path.join("file.txt")), "base\n");
}

#[test]
fn blocks_reset_modes_outside_the_allowlist() {
    let (_root, repo_path) = create_repository();
    let base_hash = head(&repo_path);

    let result = reset_operation(&repo_path, &base_hash, "hard --exec=whoami");

    assert!(!result.success);
    assert_eq!(result.outcome, Outcome::Blocked);
    assert_eq!(result.code, Code::GitError);
    assert_eq!(
        result.message,
        "Unsupported reset mode 'hard --exec=whoami'."
    );
    assert_eq!(result.mode.as_deref(), Some("hard --exec=whoami"));
}

#[test]
fn blocks_branch_mutations_when_required_arguments_are_missing() {
    let (_root, repo_path) = create_repository();

    let create_result = create_branch_operation(&repo_path, "", None, true);
    assert!(!create_result.success);
    assert_eq!(create_result.operation, Operation::CreateBranch);
    assert_eq!(create_result.outcome, Outcome::Blocked);
    assert_eq!(create_result.code, Code::GitError);
    assert_eq!(create_result.message, "A branch name is required.");
    assert_eq!(create_result.branch, None);
    assert!(create_result.files_affected.is_empty());

    let delete_remote_result = delete_remote_branch_operation(&repo_path, "origin", " ");
    assert!(!delete_remote_result.success);
    assert_eq!(
        delete_remote_result.operation,
        Operation::DeleteRemoteBranch
    );
    assert_eq!(delete_remote_result.outcome, Outcome::Blocked);
    assert_eq!(delete_remote_result.code, Code::GitError);
    assert_eq!(
        delete_remote_result.message,
        "A remote and branch name are required."
    );
    assert_eq!(delete_remote_result.branch, None);
    assert_eq!(delete_remote_result.remote.as_deref(), Some("origin"));
    assert!(delete_remote_result.files_affected.is_empty());

    let stash_branch_result = stash_branch_operation(&repo_path, "", "new-branch");
    assert!(!stash_branch_result.success);
    assert_eq!(stash_branch_result.operation, Operation::StashBranch);
    assert_eq!(stash_branch_result.outcome, Outcome::Blocked);
    assert_eq!(stash_branch_result.code, Code::GitError);
    assert_eq!(
        stash_branch_result.message,
        "A stash reference is required."
    );
    assert_eq!(stash_branch_result.branch.as_deref(), Some("new-branch"));
    assert_eq!(stash_branch_result.stash_ref, None);
    assert!(stash_branch_result.files_affected.is_empty());

    let delete_result = delete_branch_operation(&repo_path, "  ");
    assert!(!delete_result.success);
    assert_eq!(delete_result.message, "A branch name is required.");

    // Nada de lo anterior tocó el repositorio.
    assert_eq!(branch_names(&repo_path), ["main"]);
    assert!(get_structured_status(&repo_path).unwrap().is_clean);
}
