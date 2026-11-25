//! Estado, submódulos, identidad, refs, remotos y tags.

use mergeon_gitcmd::get_structured_status;

use super::support::*;
use crate::exec::{get_remotes_verbose, git, with_test_env};
use crate::{
    Code, IdentityValues, Operation, Outcome, RemoteTagOptions, SetIdentityOptions,
    SubmoduleUpdateOptions, add_remote_operation, checkout_tag_operation, create_tag_operation,
    delete_remote_tag_operation, delete_tag_operation, get_git_identity, push_tag_operation,
    remove_remote_operation, rename_remote_operation, resolve_ref_to_commit,
    set_git_identity_operation, set_remote_url_operation, submodule_update_operation,
};

#[test]
fn reports_the_head_commit_on_a_branch_not_only_when_detached() {
    let root = create_temp_root();
    let repo_path = root.path().join("repo");
    init_repository(&repo_path);
    let head = commit_file(&repo_path, "hello\n", "initial commit");

    let status = get_structured_status(&repo_path).unwrap();

    assert_eq!(status.current.as_deref(), Some("main"));
    let short_hash = status.head_short_hash.unwrap();
    assert!(!short_hash.is_empty());
    assert!(head.starts_with(&short_hash));
}

#[test]
fn leaves_the_head_commit_empty_in_a_repository_without_commits() {
    let root = create_temp_root();
    let repo_path = root.path().join("repo");
    init_repository(&repo_path);

    let status = get_structured_status(&repo_path).unwrap();

    assert_eq!(status.head_short_hash, None);
}

#[test]
fn initializes_a_submodule_with_submodule_update_operation_and_reports_its_state() {
    let root = create_temp_root();
    let sub_path = root.path().join("sub");
    let super_path = root.path().join("super");

    init_repository(&sub_path);
    commit_file(&sub_path, "sub content\n", "sub base");

    init_repository(&super_path);
    commit_file(&super_path, "base\n", "base");
    // El protocolo file viene bloqueado por defecto en git moderno; solo para el fixture.
    run_git(
        &super_path,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            path_str(&sub_path),
            "libs/sub",
        ],
    );
    run_git(&super_path, &["commit", "-m", "add submodule"]);

    run_git(&super_path, &["submodule", "deinit", "-f", "libs/sub"]);
    // `-` al inicio: submódulo sin inicializar.
    let before = run_git(&super_path, &["submodule", "status"]);
    assert_eq!(before.lines().count(), 1);
    assert!(before.starts_with('-'), "{before}");

    let result = with_test_env(&[("GIT_ALLOW_PROTOCOL", "file")], || {
        submodule_update_operation(
            &super_path,
            &SubmoduleUpdateOptions {
                paths: vec![" libs/sub ".to_string(), String::new()],
            },
        )
    });

    assert!(result.success, "{}", result.message);
    assert_eq!(result.operation, Operation::SubmoduleUpdate);
    assert_eq!(result.message, "Updated 1 submodule.");
    assert_eq!(result.files_affected, ["libs/sub"]);

    let after = run_git(&super_path, &["submodule", "status"]);
    assert!(after.starts_with(' '), "{after}");
    assert!(after.contains("libs/sub"));
    assert_eq!(
        read_file(&super_path.join("libs/sub/file.txt")),
        "sub content\n"
    );
}

#[test]
fn updating_all_submodules_of_a_repository_without_any_succeeds() {
    let (_root, repo_path) = create_repository();

    let result = submodule_update_operation(&repo_path, &SubmoduleUpdateOptions::default());

    assert!(result.success, "{}", result.message);
    assert_eq!(result.message, "Updated all submodules.");
    assert!(result.files_affected.is_empty());
}

fn identity(name: &str, email: &str) -> IdentityValues {
    IdentityValues {
        name: Some(name.to_string()),
        email: Some(email.to_string()),
    }
}

fn set_identity(scope: &str, name: &str, email: &str) -> SetIdentityOptions {
    SetIdentityOptions {
        scope: scope.to_string(),
        name: Some(name.to_string()),
        email: Some(email.to_string()),
    }
}

#[test]
fn reads_and_writes_the_git_identity_per_scope_without_touching_the_real_global_config() {
    let (root, repo_path) = create_repository();

    // Config global falsa: jamás tocar la del usuario.
    let fake_global_path = root.path().join("fake-gitconfig");
    std::fs::write(&fake_global_path, "").unwrap();

    with_test_env(
        &[("GIT_CONFIG_GLOBAL", path_str(&fake_global_path))],
        || {
            let global_result = set_git_identity_operation(
                &repo_path,
                &set_identity("global", "Global User", "global@example.com"),
            );
            assert!(global_result.success, "{}", global_result.message);
            assert_eq!(global_result.operation, Operation::SetIdentity);
            assert_eq!(global_result.message, "Global git identity updated.");

            // init_repository ya seteó identidad local (Test User); la limpiamos.
            let clear_local =
                set_git_identity_operation(&repo_path, &set_identity("local", "", ""));
            assert!(clear_local.success, "{}", clear_local.message);
            assert_eq!(clear_local.message, "Repository git identity updated.");

            let identity_after_clear = get_git_identity(&repo_path);
            assert_eq!(
                identity_after_clear.global,
                identity("Global User", "global@example.com")
            );
            assert_eq!(identity_after_clear.local, IdentityValues::default());

            // Limpiar otra vez lo que ya no existe tampoco es un error.
            assert!(set_git_identity_operation(&repo_path, &set_identity("local", "", "")).success);

            set_git_identity_operation(
                &repo_path,
                &set_identity("local", "  Repo User  ", "repo@example.com"),
            );
            assert_eq!(
                get_git_identity(&repo_path).local,
                identity("Repo User", "repo@example.com")
            );

            let bad_scope = set_git_identity_operation(
                &repo_path,
                &SetIdentityOptions {
                    scope: "system".to_string(),
                    name: Some("x".to_string()),
                    email: None,
                },
            );
            assert!(!bad_scope.success);
            assert_eq!(bad_scope.outcome, Outcome::Blocked);
            assert_eq!(bad_scope.code, Code::GitError);
            assert_eq!(bad_scope.message, "Unsupported identity scope 'system'.");
        },
    );

    let written = std::fs::read_to_string(&fake_global_path).unwrap();
    assert!(written.contains("Global User"), "{written}");
}

#[test]
fn resolves_branches_remote_refs_and_annotated_tags_to_a_commit_hash() {
    let root = create_temp_root();
    let repo_path = root.path().join("repo");
    let remote_path = root.path().join("remote.git");

    init_repository(&repo_path);
    init_bare(&remote_path, true);
    let base_hash = commit_file(&repo_path, "base\n", "base");
    run_git(
        &repo_path,
        &["remote", "add", "origin", path_str(&remote_path)],
    );
    run_git(&repo_path, &["push", "-u", "origin", "main"]);
    run_git(&repo_path, &["tag", "-a", "v9.9.9", "-m", "annotated tag"]);
    let feature_hash = commit_file(&repo_path, "feature\n", "feature commit");

    assert_eq!(
        resolve_ref_to_commit(&repo_path, "refs/heads/main").unwrap(),
        feature_hash
    );
    assert_eq!(
        resolve_ref_to_commit(&repo_path, " refs/remotes/origin/main ").unwrap(),
        base_hash
    );
    // El tag anotado apunta a un objeto tag; ^{commit} lo pela hasta el commit.
    assert_eq!(
        resolve_ref_to_commit(&repo_path, "refs/tags/v9.9.9").unwrap(),
        base_hash
    );

    assert!(resolve_ref_to_commit(&repo_path, "refs/heads/no-existe").is_err());
    // Un nombre con pinta de opción no se interpreta como tal.
    assert!(resolve_ref_to_commit(&repo_path, "--help").is_err());
    assert_eq!(
        resolve_ref_to_commit(&repo_path, "").unwrap_err().message,
        "A ref is required."
    );
}

#[test]
fn adds_renames_updates_and_removes_remotes_with_structured_results() {
    let (_root, repo_path) = create_repository();

    let add_result = add_remote_operation(&repo_path, "origin", "https://example.com/repo.git");
    assert!(add_result.success, "{}", add_result.message);
    assert_eq!(add_result.operation, Operation::AddRemote);
    assert_eq!(add_result.message, "Remote 'origin' added successfully.");
    assert_eq!(add_result.remote.as_deref(), Some("origin"));

    let duplicate = add_remote_operation(&repo_path, "origin", "https://example.com/repo.git");
    assert!(!duplicate.success);
    assert_eq!(duplicate.outcome, Outcome::Failed);
    assert_eq!(duplicate.message, "remote origin already exists.");

    let rename_result = rename_remote_operation(&repo_path, "origin", "upstream");
    assert!(rename_result.success, "{}", rename_result.message);
    assert_eq!(
        rename_result.message,
        "Remote 'origin' renamed to 'upstream'."
    );
    assert_eq!(rename_result.remote.as_deref(), Some("upstream"));

    let set_url_result =
        set_remote_url_operation(&repo_path, "upstream", "https://example.com/other.git");
    assert!(set_url_result.success, "{}", set_url_result.message);
    assert_eq!(
        set_url_result.message,
        "Updated the URL of remote 'upstream'."
    );

    let remotes = get_remotes_verbose(&git(&repo_path)).unwrap();
    assert_eq!(remotes.len(), 1);
    assert_eq!(remotes[0].name, "upstream");
    assert_eq!(remotes[0].fetch, "https://example.com/other.git");
    assert_eq!(remotes[0].push, "https://example.com/other.git");

    let remove_result = remove_remote_operation(&repo_path, "upstream");
    assert!(remove_result.success, "{}", remove_result.message);
    assert_eq!(remove_result.message, "Remote 'upstream' removed.");
    assert!(get_remotes_verbose(&git(&repo_path)).unwrap().is_empty());
}

#[test]
fn blocks_remote_operations_with_missing_arguments() {
    let (_root, repo_path) = create_repository();

    let result = remove_remote_operation(&repo_path, "");
    assert!(!result.success);
    assert_eq!(result.outcome, Outcome::Blocked);
    assert_eq!(result.message, "A remote name is required.");
    assert_eq!(result.remote, None);

    let rename_result = rename_remote_operation(&repo_path, "origin", "");
    assert!(!rename_result.success);
    assert_eq!(rename_result.outcome, Outcome::Blocked);
    assert_eq!(rename_result.remote.as_deref(), Some("origin"));

    let add_result = add_remote_operation(&repo_path, "origin", "  ");
    assert!(!add_result.success);
    assert_eq!(add_result.message, "A remote name and URL are required.");

    let set_url_result = set_remote_url_operation(&repo_path, "", "https://example.com/a.git");
    assert!(!set_url_result.success);
    assert_eq!(set_url_result.outcome, Outcome::Blocked);
}

#[test]
fn creates_checks_out_and_deletes_tags_with_structured_results() {
    let (_root, repo_path) = create_repository();

    let create_result = create_tag_operation(&repo_path, "v1.0.0", None, None);
    let checkout_result = checkout_tag_operation(&repo_path, "v1.0.0");
    let delete_result = delete_tag_operation(&repo_path, "v1.0.0");

    assert!(create_result.success, "{}", create_result.message);
    assert_eq!(create_result.message, "Created tag v1.0.0.");
    assert_eq!(create_result.tag.as_deref(), Some("v1.0.0"));
    assert!(checkout_result.success, "{}", checkout_result.message);
    assert_eq!(checkout_result.code, Code::DetachedHead);
    assert_eq!(
        checkout_result.message,
        "Checked out tag v1.0.0 in detached HEAD state."
    );
    assert_eq!(
        checkout_result.status.unwrap().current.as_deref(),
        Some("HEAD")
    );
    assert!(delete_result.success, "{}", delete_result.message);
    assert_eq!(delete_result.message, "Deleted tag v1.0.0.");
    assert_eq!(run_git(&repo_path, &["tag", "--list"]).trim(), "");
}

#[test]
fn creates_an_annotated_tag_with_a_message() {
    let (_root, repo_path) = create_repository();
    let base = head(&repo_path);
    commit_file(&repo_path, "second\n", "second");

    let result = create_tag_operation(
        &repo_path,
        "v1.1.0",
        Some("HEAD"),
        Some("Annotated release tag"),
    );
    let tag_message = run_git(&repo_path, &["tag", "-n99", "v1.1.0"]);

    assert!(result.success, "{}", result.message);
    assert!(tag_message.contains("Annotated release tag"));

    // Sobre un commit concreto, no solo HEAD.
    let on_base = create_tag_operation(&repo_path, "v1.0.0", Some(&base), None);
    assert!(on_base.success, "{}", on_base.message);
    assert_eq!(
        resolve_ref_to_commit(&repo_path, "refs/tags/v1.0.0").unwrap(),
        base
    );
}

#[test]
fn reports_tags_that_already_exist_or_are_missing() {
    let (_root, repo_path) = create_repository();
    create_tag_operation(&repo_path, "v1.0.0", None, None);

    let duplicate = create_tag_operation(&repo_path, "v1.0.0", None, None);
    assert!(!duplicate.success);
    assert_eq!(duplicate.outcome, Outcome::Blocked);
    assert_eq!(duplicate.code, Code::TagAlreadyExists);
    assert_eq!(duplicate.message, "tag 'v1.0.0' already exists");
    assert_eq!(duplicate.tag.as_deref(), Some("v1.0.0"));

    let missing = delete_tag_operation(&repo_path, "v9");
    assert!(!missing.success);
    assert_eq!(missing.outcome, Outcome::Failed);
    assert_eq!(missing.code, Code::GitError);

    let checkout = checkout_tag_operation(&repo_path, "v9");
    assert!(!checkout.success);
    assert_eq!(checkout.operation, Operation::CheckoutTag);
    assert_eq!(checkout.tag.as_deref(), Some("v9"));
}

#[test]
fn pushes_and_deletes_a_tag_on_the_remote() {
    let (root, repo_path) = create_repository();
    let remote_path = root.path().join("remote.git");
    init_bare(&remote_path, true);
    run_git(
        &repo_path,
        &["remote", "add", "origin", path_str(&remote_path)],
    );
    create_tag_operation(&repo_path, "v1.0.0", None, None);
    let remote_tags = || run_git(&remote_path, &["tag", "--list"]).trim().to_string();

    let pushed = push_tag_operation(&repo_path, "v1.0.0", &RemoteTagOptions::default());
    assert!(pushed.success, "{}", pushed.message);
    assert_eq!(pushed.operation, Operation::PushTag);
    assert_eq!(pushed.message, "Pushed tag v1.0.0 to origin.");
    assert_eq!(pushed.tag.as_deref(), Some("v1.0.0"));
    assert_eq!(pushed.remote.as_deref(), Some("origin"));
    assert_eq!(remote_tags(), "v1.0.0");

    let deleted = delete_remote_tag_operation(&repo_path, "v1.0.0", &RemoteTagOptions::default());
    assert!(deleted.success, "{}", deleted.message);
    assert_eq!(deleted.operation, Operation::DeleteRemoteTag);
    assert_eq!(deleted.message, "Deleted tag v1.0.0 on origin.");
    assert_eq!(remote_tags(), "");
    // El tag local sigue existiendo.
    assert_eq!(run_git(&repo_path, &["tag", "--list"]).trim(), "v1.0.0");

    let unknown_remote = push_tag_operation(
        &repo_path,
        "v1.0.0",
        &RemoteTagOptions {
            remote: "nowhere".to_string(),
            auth_token: None,
        },
    );
    assert!(!unknown_remote.success);
    assert_eq!(unknown_remote.code, Code::GitError);
    assert_eq!(unknown_remote.remote.as_deref(), Some("nowhere"));
}
