//! Los canales con lógica propia y los de red, contra repositorios reales y
//! remotos bare locales (el resto de `git-handlers.test.js`).

use std::path::Path;

use mergeon_git_ops::{Code, FetchOptions, Outcome, PullOptions, PushOptions, SetIdentityOptions};
use mergeon_runtime::OperationPhase;
use serde_json::json;

use super::support::{TestBackend, TestRepo, git_in};
use crate::{
    AddToIgnoreCode, CloneCode, HandlerOperation, InitRepositoryCode, SetupTrackingCode,
    build_mutation_request_key,
};

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

// --- git:fetch / git:pull / git:push ---------------------------------------

#[test]
fn fetch_uses_the_options_and_applies_the_default_remote() {
    let test = TestBackend::new();
    let repo = TestRepo::new();
    let origin = repo.publish_to_origin();
    repo.add_bare_remote("upstream");
    let colleague = repo.colleague(&origin);
    let pushed = colleague.commit_and_push("news.txt", "news\n", "Colleague commit");

    // Sin opciones propias: `origin`.
    let result = test.git().fetch(&repo.path, &FetchOptions::default());

    assert!(result.success(), "{}", result.message());
    let fetched = result.result().unwrap();
    assert_eq!(fetched.remote.as_deref(), Some("origin"));
    assert_eq!(fetched.refs_changed, Some(true));
    assert!(result.operation_id().unwrap().starts_with("fetch-"));
    assert_eq!(
        repo.git(&["rev-parse", "origin/main"]).trim(),
        pushed.as_str()
    );

    // El remoto y el token de las opciones llegan a la operación.
    let result = test.git().fetch(
        &repo.path,
        &FetchOptions {
            remote: "upstream".to_string(),
            auth_token: Some("abc".to_string()),
            ..Default::default()
        },
    );
    assert!(result.success(), "{}", result.message());
    assert_eq!(result.result().unwrap().remote.as_deref(), Some("upstream"));
    assert_eq!(result.outcome(), Some(Outcome::Noop));

    // Un remoto que no existe es un resultado estructurado, no un error.
    let result = test.git().fetch(
        &repo.path,
        &FetchOptions {
            remote: "nowhere".to_string(),
            ..Default::default()
        },
    );
    assert!(!result.success());
    assert_eq!(result.code(), Code::RemoteNotFound.as_str());
    assert_eq!(result.operation(), Some("fetch"));
}

#[test]
fn fetch_and_pull_deduplicate_by_their_options_only() {
    // La clave de fetch y pull es la operación más las opciones, sin la ruta.
    let key = build_mutation_request_key(
        "fetch",
        &json!([serde_json::to_value(FetchOptions {
            auth_token: Some("abc".to_string()),
            ..Default::default()
        })
        .unwrap()]),
    );
    assert_eq!(
        key,
        r#"fetch:[{"allRemotes":false,"authToken":"[fingerprint:1a47e90b]","remote":"origin"}]"#
    );
}

#[test]
fn pull_uses_the_options_and_preserves_its_flags() {
    let test = TestBackend::new();
    let repo = TestRepo::new();
    let origin = repo.publish_to_origin();
    let colleague = repo.colleague(&origin);
    let pushed = colleague.commit_and_push("news.txt", "news\n", "Colleague commit");
    // Un cambio local sin confirmar: `autostash` lo aparta y lo devuelve.
    repo.write("README.md", "# Demo\n\nlocal edit\n");

    let result = test.git().pull(
        &repo.path,
        &PullOptions {
            branch: Some("main".to_string()),
            rebase: true,
            autostash: true,
            ..Default::default()
        },
    );

    assert!(result.success(), "{}", result.message());
    let pulled = result.result().unwrap();
    assert_eq!(pulled.remote.as_deref(), Some("origin"));
    assert_eq!(pulled.branch.as_deref(), Some("main"));
    assert!(pulled.rebase);
    assert!(pulled.autostash);
    assert_eq!(result.operation(), Some("pull"));
    assert!(result.operation_id().unwrap().starts_with("pull-"));
    assert_eq!(repo.head(), pushed);
    assert_eq!(repo.read("news.txt"), "news\n");
    assert_eq!(repo.read("README.md"), "# Demo\n\nlocal edit\n");

    // Nada nuevo: noop.
    let again = test.git().pull(&repo.path, &PullOptions::default());
    assert_eq!(again.outcome(), Some(Outcome::Noop));
    assert_eq!(again.code(), Code::AlreadyUpToDate.as_str());
}

#[test]
fn push_uses_the_options_and_publishes_the_branch() {
    let test = TestBackend::new();
    let repo = TestRepo::new();
    let origin = repo.add_bare_remote("origin");
    repo.commit_file("feature.txt", "feature\n", "Add feature");

    let result = test.git().push(
        &repo.path,
        &PushOptions {
            branch: Some("main".to_string()),
            ..Default::default()
        },
    );

    assert!(result.success(), "{}", result.message());
    let pushed = result.result().unwrap();
    assert_eq!(pushed.branch.as_deref(), Some("main"));
    assert_eq!(pushed.remote.as_deref(), Some("origin"));
    assert!(pushed.upstream_set);
    assert_eq!(result.operation(), Some("push"));
    assert_eq!(git_in(&origin, &["rev-parse", "main"]).trim(), repo.head());

    // Un push que git rechaza vuelve como resultado de la operación.
    let result = test.git().push(
        &repo.path,
        &PushOptions {
            remote: "nowhere".to_string(),
            ..Default::default()
        },
    );
    assert!(!result.success());
    assert_eq!(result.code(), Code::RemoteNotFound.as_str());
    assert_eq!(result.result().unwrap().remote.as_deref(), Some("nowhere"));
}

// --- git:deleteRemoteBranch -------------------------------------------------

// La app Electron borraba la rama remota sin el token de GitHub que sí usan
// push, fetch y pull. Un `pre-push` comprueba que el helper de credenciales de
// GitHub está activo sin exponer el token en `http.extraHeader` ni en `argv`.
#[test]
fn delete_remote_branch_with_auth_sends_the_token_to_the_remote() {
    let test = TestBackend::new();
    let repo = TestRepo::new();
    let origin = repo.publish_to_origin();
    repo.git(&["push", "origin", "main:old", "main:older"]);
    let helper = repo.root().join("helper.txt");
    let header = repo.root().join("header.txt");
    let hook = repo.path.join(".git").join("hooks").join("pre-push");
    std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
    std::fs::write(
        &hook,
        format!(
            "#!/bin/sh\ngit config --get-urlmatch credential.helper https://github.com/x.git > '{}'\ngit config --get-urlmatch http.extraHeader https://github.com/x.git > '{}'\nexit 0\n",
            text(&helper),
            text(&header)
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let result = test.git().delete_remote_branch_with_auth(
        &repo.path,
        "origin",
        "old",
        Some("gho_secret_abc"),
    );
    assert!(result.success(), "{}", result.message());
    assert_eq!(result.operation(), Some("delete-remote-branch"));
    let helper_value = std::fs::read_to_string(&helper).unwrap();
    assert!(
        helper_value.contains("mergeon-cred-"),
        "expected ephemeral credential helper, got: {helper_value:?}"
    );
    assert!(
        !helper_value.contains("gho_secret_abc"),
        "token leaked into credential.helper config: {helper_value:?}"
    );
    assert_eq!(std::fs::read_to_string(&header).unwrap(), "");

    let result = test
        .git()
        .delete_remote_branch(&repo.path, "origin", "older");
    assert!(result.success(), "{}", result.message());
    assert!(
        !std::fs::read_to_string(&helper)
            .unwrap()
            .contains("mergeon-cred-")
    );
    assert_eq!(std::fs::read_to_string(&header).unwrap(), "");
    assert_eq!(
        git_in(&origin, &["for-each-ref", "--format=%(refname)"]).trim(),
        "refs/heads/main"
    );
}

// --- git:setupTracking ------------------------------------------------------

#[test]
fn setup_tracking_returns_structured_outcomes_for_noop_and_missing_remote_branches() {
    let test = TestBackend::new();
    let repo = TestRepo::new();
    repo.publish_to_origin();
    repo.git(&["branch", "feature"]);

    let tracked = test.git().setup_tracking(&repo.path, Some("main"));
    assert_eq!(
        tracked.to_json(),
        json!({
            "success": true,
            "operation": "setup-tracking",
            "outcome": "noop",
            "code": "ALREADY_TRACKING",
            "message": "main already tracks origin/main.",
            "branch": "main",
            "remote": "origin/main",
            "upstreamSet": true,
            "alreadyTracked": true,
            "operationId": tracked.operation_id().unwrap()
        })
    );
    assert_eq!(
        tracked.result().unwrap().operation,
        HandlerOperation::SetupTracking
    );

    let missing = test.git().setup_tracking(&repo.path, Some("feature"));
    assert_eq!(
        missing.to_json(),
        json!({
            "success": false,
            "operation": "setup-tracking",
            "outcome": "blocked",
            "code": "REMOTE_BRANCH_NOT_FOUND",
            "message": "No remote branch found for origin/feature.",
            "branch": "feature",
            "remote": "origin/feature",
            "upstreamSet": false,
            "alreadyTracked": false,
            "hasRemote": false,
            "operationId": missing.operation_id().unwrap()
        })
    );
    assert_eq!(
        missing.result().unwrap().code,
        SetupTrackingCode::RemoteBranchNotFound
    );
}

#[test]
fn setup_tracking_configures_the_upstream_when_the_remote_branch_exists() {
    let test = TestBackend::new();
    let repo = TestRepo::new();
    repo.publish_to_origin();
    // Publicada sin `-u`: existe `origin/feature` pero la rama no la sigue.
    repo.git(&["checkout", "-b", "feature"]);
    repo.git(&["push", "origin", "feature"]);

    // Sin nombre de rama se usa la actual.
    let result = test.git().setup_tracking(&repo.path, None);

    assert!(result.success(), "{}", result.message());
    let tracking = result.result().unwrap();
    assert_eq!(tracking.code, SetupTrackingCode::Ok);
    assert_eq!(tracking.outcome, Outcome::Completed);
    assert_eq!(
        tracking.message,
        "Tracking set up for feature -> origin/feature."
    );
    assert_eq!(tracking.branch.as_deref(), Some("feature"));
    assert_eq!(tracking.remote.as_deref(), Some("origin/feature"));
    assert!(tracking.upstream_set);
    assert!(!tracking.already_tracked);
    assert_eq!(tracking.has_remote, None);
    assert_eq!(
        repo.git(&["rev-parse", "--abbrev-ref", "feature@{upstream}"])
            .trim(),
        "origin/feature"
    );

    // La segunda vez ya no hay nada que hacer.
    let again = test.git().setup_tracking(&repo.path, Some("feature"));
    assert_eq!(again.code(), "ALREADY_TRACKING");
    assert_eq!(again.outcome(), Some(Outcome::Noop));
}

#[test]
fn setup_tracking_reports_a_handler_failure_when_the_repository_is_gone() {
    let test = TestBackend::new();
    let directory = tempfile::tempdir().unwrap();
    let missing = directory.path().join("gone");

    let result = test.git().setup_tracking(&missing, Some("main"));

    assert!(!result.success());
    assert_eq!(result.code(), "IPC_ERROR");
    assert_eq!(
        result.message(),
        "Failed to set up tracking: Cannot use simple-git on a directory that does not exist"
    );
    assert_eq!(
        result.failure().unwrap().field("branch"),
        Some(&json!("main"))
    );
}

// --- git:initRepository -----------------------------------------------------

#[test]
fn init_repository_initializes_a_plain_directory_once() {
    let test = TestBackend::new();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("project");
    std::fs::create_dir(&path).unwrap();
    assert!(!test.git().is_valid_repository(&path));

    let created = test.git().init_repository(&path);

    assert_eq!(
        created.to_json(),
        json!({
            "success": true,
            "operation": "init-repository",
            "outcome": "completed",
            "code": "OK",
            "message": "Initialized Git repository.",
            "operationId": created.operation_id().unwrap()
        })
    );
    assert!(
        created
            .operation_id()
            .unwrap()
            .starts_with("init-repository-")
    );
    assert!(path.join(".git").is_dir());
    assert!(test.git().is_valid_repository(&path));
    assert_eq!(
        test.events.phases_of(created.operation_id().unwrap()),
        [
            OperationPhase::Queued,
            OperationPhase::Running,
            OperationPhase::Completed
        ]
    );

    let again = test.git().init_repository(&path);
    assert!(again.success());
    assert_eq!(again.outcome(), Some(Outcome::Noop));
    assert_eq!(
        again.result().unwrap().code,
        InitRepositoryCode::AlreadyInitialized
    );
    assert_eq!(again.message(), "Git repository already initialized.");
}

#[test]
fn init_repository_refuses_paths_that_are_not_directories() {
    let test = TestBackend::new();
    let directory = tempfile::tempdir().unwrap();

    let missing = test.git().init_repository(directory.path().join("gone"));
    assert!(!missing.success());
    assert_eq!(missing.outcome(), Some(Outcome::Failed));
    assert_eq!(missing.code(), "MISSING_PATH");
    assert_eq!(
        missing.message(),
        "The selected directory no longer exists."
    );
    assert!(!directory.path().join("gone").exists());

    let file = directory.path().join("file.txt");
    std::fs::write(&file, "x").unwrap();
    let not_a_directory = test.git().init_repository(&file);
    assert_eq!(
        not_a_directory.result().unwrap().code,
        InitRepositoryCode::MissingPath
    );
    assert_eq!(
        not_a_directory.message(),
        "The selected path is not a directory."
    );
}

// --- git:addToIgnore --------------------------------------------------------

#[test]
fn add_to_ignore_creates_and_appends_to_gitignore() {
    let test = TestBackend::new();
    let repo = TestRepo::new();

    let created = test.git().add_to_ignore(&repo.path, "dist");
    assert_eq!(
        created.to_json(),
        json!({
            "success": true,
            "operation": "add-to-ignore",
            "outcome": "completed",
            "code": "OK",
            "message": "Added dist to .gitignore.",
            "filePath": "dist",
            "operationId": created.operation_id().unwrap()
        })
    );
    assert_eq!(repo.read(".gitignore"), "dist\n");

    // La ruta se normaliza antes de escribirla.
    let nested = test.git().add_to_ignore(&repo.path, ".\\build\\out.log ");
    assert_eq!(nested.result().unwrap().file_path, "build/out.log");
    assert_eq!(repo.read(".gitignore"), "dist\nbuild/out.log\n");

    // Un archivo sin salto de línea final lo recibe antes de la entrada nueva.
    repo.write(".gitignore", "node_modules");
    let appended = test.git().add_to_ignore(&repo.path, "coverage");
    assert!(appended.success());
    assert_eq!(repo.read(".gitignore"), "node_modules\ncoverage\n");
}

#[test]
fn add_to_ignore_does_nothing_when_the_path_is_already_ignored() {
    let test = TestBackend::new();
    let repo = TestRepo::new();
    // Finales de línea de Windows y espacios alrededor.
    repo.write(".gitignore", "node_modules\r\n  dist  \r\n");

    let result = test.git().add_to_ignore(&repo.path, "./dist");

    assert!(result.success());
    let ignored = result.result().unwrap();
    assert_eq!(ignored.code, AddToIgnoreCode::AlreadyIgnored);
    assert_eq!(ignored.outcome, Outcome::Noop);
    assert_eq!(ignored.message, "dist is already ignored.");
    assert_eq!(ignored.file_path, "dist");
    assert_eq!(repo.read(".gitignore"), "node_modules\r\n  dist  \r\n");
}

#[test]
fn add_to_ignore_reports_handler_failures_with_the_file_path() {
    let test = TestBackend::new();
    let repo = TestRepo::new();

    let outside = test.git().add_to_ignore(&repo.path, "../secret");
    assert_eq!(
        outside.to_json(),
        json!({
            "success": false,
            "operation": "add-to-ignore",
            "outcome": "failed",
            "code": "IPC_ERROR",
            "message": "Failed to update .gitignore: The file path must stay inside the repository.",
            "status": null,
            "operationState": null,
            "conflictState": null,
            "branch": null,
            "remote": null,
            "tag": null,
            "stashRef": null,
            "commit": null,
            "mode": null,
            "filePath": "../secret",
            "filesAffected": []
        })
    );

    let absolute = test.git().add_to_ignore(&repo.path, "/etc/passwd");
    assert_eq!(
        absolute.message(),
        "Failed to update .gitignore: The file path must be relative to the repository."
    );
    assert!(!repo.path.join(".gitignore").exists());

    // Un `.gitignore` que no se puede leer como archivo.
    if cfg!(unix) {
        std::fs::create_dir(repo.path.join(".gitignore")).unwrap();
        let unreadable = test.git().add_to_ignore(&repo.path, "dist");
        assert_eq!(unreadable.code(), "IPC_ERROR");
        assert_eq!(
            unreadable.message(),
            "Failed to update .gitignore: EISDIR: illegal operation on a directory, read"
        );
        let failed = test.events.operations().pop().unwrap();
        assert_eq!(failed.phase, OperationPhase::Failed);
        assert_eq!(failed.error.unwrap().code, "EISDIR");
    }
}

// --- git:clone --------------------------------------------------------------

#[test]
fn clone_copies_a_local_repository_and_reports_its_canonical_path() {
    let test = TestBackend::new();
    let source = TestRepo::new();
    let origin = source.publish_to_origin();
    let target = source.root().join("clones").join("demo");
    std::fs::create_dir(source.root().join("clones")).unwrap();

    // Espacios y barras finales en la URL se recortan.
    let url = format!("  {}//  ", text(&origin));
    let result = test
        .git()
        .clone(&url, &format!(" {} ", text(&target)), None);

    assert!(result.success, "{}", result.message);
    assert_eq!(result.operation, HandlerOperation::Clone);
    assert_eq!(result.outcome, Outcome::Completed);
    assert_eq!(result.code, CloneCode::Ok);
    assert_eq!(result.message, "Repository cloned successfully.");
    assert_eq!(result.path.as_deref(), Some(text(&target).as_str()));
    assert_eq!(result.canonical_path, result.path);
    let operation_id = result.operation_id.clone().expect("operation id");
    assert!(operation_id.starts_with("clone-"));
    assert_eq!(
        serde_json::to_value(&result).unwrap(),
        json!({
            "success": true,
            "operation": "clone",
            "outcome": "completed",
            "code": "OK",
            "message": "Repository cloned successfully.",
            "path": text(&target),
            "canonicalPath": text(&target),
            "operationId": operation_id
        })
    );

    assert_eq!(
        std::fs::read_to_string(target.join("README.md"))
            .unwrap()
            .replace("\r\n", "\n"),
        "# Demo\n"
    );
    assert_eq!(
        git_in(&target, &["remote", "get-url", "origin"]).trim(),
        text(&origin)
    );
    assert!(test.git().is_valid_repository(&target));

    // La cola es la del destino.
    let events = test.events.operations();
    assert_eq!(
        events.iter().map(|event| event.phase).collect::<Vec<_>>(),
        [
            OperationPhase::Queued,
            OperationPhase::Running,
            OperationPhase::Completed
        ]
    );
    assert!(events.iter().all(|event| event.operation == "clone"
        && event.operation_id == operation_id
        && event.repo_path == text(&target)));
}

#[test]
fn clone_with_a_token_authenticates_without_storing_it() {
    let test = TestBackend::new();
    let source = TestRepo::new();
    let origin = source.publish_to_origin();
    let target = source.root().join("with-token");

    let result = test
        .git()
        .clone(&text(&origin), &text(&target), Some("gho_secret"));

    assert!(result.success, "{}", result.message);
    // El token viaja como cabecera del comando, no queda en el clon.
    let config = std::fs::read_to_string(target.join(".git/config")).unwrap();
    assert!(!config.contains("gho_secret"));
    assert!(!config.contains("extraHeader"));
}

#[test]
fn clone_requires_a_url_and_a_destination() {
    let test = TestBackend::new();

    let no_url = test.git().clone("   ", " relative/target ", None);
    assert_eq!(
        serde_json::to_value(&no_url).unwrap(),
        json!({
            "success": false,
            "operation": "clone",
            "outcome": "blocked",
            "code": "ARGUMENTS_REQUIRED",
            "message": "A repository URL and clone destination are required.",
            "path": "relative/target",
            "canonicalPath": text(&std::path::absolute("relative/target").unwrap())
        })
    );

    let no_target = test.git().clone("https://example.com/repo.git", "", None);
    assert_eq!(no_target.code, CloneCode::ArgumentsRequired);
    assert_eq!(no_target.path, None);
    assert_eq!(no_target.canonical_path, None);
    assert_eq!(no_target.operation_id, None);

    // Ni siquiera llega a la cola.
    assert!(test.events.operations().is_empty());
}

#[test]
fn clone_reports_git_failures_as_a_structured_result() {
    let test = TestBackend::new();
    let directory = tempfile::tempdir().unwrap();
    let missing_source = directory.path().join("no-such-repo.git");
    let target = directory.path().join("target");

    let result = test
        .git()
        .clone(&text(&missing_source), &text(&target), None);

    assert!(!result.success);
    assert_eq!(result.outcome, Outcome::Failed);
    assert_eq!(result.code, CloneCode::GitError);
    assert!(
        result.message.starts_with("Failed to clone repository: "),
        "{}",
        result.message
    );
    // Con `C:` en la ruta, git en Windows la toma por una URL y lo dice de
    // otro modo.
    assert!(
        result.message.contains("does not exist")
            || (cfg!(windows)
                && result
                    .message
                    .contains("does not appear to be a git repository")),
        "{}",
        result.message
    );
    assert_eq!(result.path.as_deref(), Some(text(&target).as_str()));
    assert_eq!(result.canonical_path, result.path);
    assert!(result.operation_id.is_some());

    let failed = test.events.operations().pop().unwrap();
    assert_eq!(failed.phase, OperationPhase::Failed);
    assert_eq!(failed.error.unwrap().code, "GIT_ERROR");

    // Una URL con pinta de opción es una ruta más, no una opción de git.
    let result = test.git().clone(
        "--upload-pack=touch /tmp/pwned",
        &text(&directory.path().join("other")),
        None,
    );
    assert_eq!(result.code, CloneCode::GitError);
    assert!(!directory.path().join("other").exists());
}

#[test]
fn clone_failures_never_repeat_the_credentials_of_the_url() {
    let test = TestBackend::new();
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("target");

    // Nadie escucha en el puerto 9 de la máquina: falla enseguida.
    let result = test.git().clone(
        "https://alice:s3cr3t-token@127.0.0.1:9/team/repo.git",
        &text(&target),
        None,
    );

    assert!(!result.success);
    assert!(!result.message.contains("s3cr3t"), "{}", result.message);
    assert!(!result.message.contains("alice"), "{}", result.message);
    let failed = test.events.operations().pop().unwrap();
    let error = failed.error.expect("error");
    assert!(!error.message.contains("s3cr3t"), "{}", error.message);
}

// --- git:setIdentity / lecturas envueltas -----------------------------------

#[test]
fn set_identity_writes_the_repository_identity_outside_the_queue() {
    let test = TestBackend::new();
    let repo = TestRepo::new();

    let result = test.git().set_identity(
        &repo.path,
        &SetIdentityOptions {
            name: Some("  Ada Lovelace ".to_string()),
            email: Some("ada@example.com".to_string()),
            ..Default::default()
        },
    );

    assert!(result.success, "{}", result.message);
    assert_eq!(result.message, "Repository git identity updated.");
    let identity = test.git().get_identity(&repo.path).unwrap();
    assert_eq!(identity.local.name.as_deref(), Some("Ada Lovelace"));
    assert_eq!(identity.local.email.as_deref(), Some("ada@example.com"));
    // No pasa por la cola: no hay eventos de operación.
    assert!(test.events.operations().is_empty());

    let unsupported = test.git().set_identity(
        &repo.path,
        &SetIdentityOptions {
            scope: "system".to_string(),
            ..Default::default()
        },
    );
    assert!(!unsupported.success);
    assert_eq!(unsupported.message, "Unsupported identity scope 'system'.");
}

#[test]
fn resolve_ref_wraps_its_errors_with_the_channel_prefix() {
    let test = TestBackend::new();
    let repo = TestRepo::new();
    repo.git(&["tag", "-a", "v1", "-m", "release"]);

    assert_eq!(
        test.git().resolve_ref(&repo.path, "main").unwrap(),
        repo.head()
    );
    // Un tag anotado se pela hasta su commit.
    assert_eq!(
        test.git().resolve_ref(&repo.path, " v1 ").unwrap(),
        repo.head()
    );

    let empty = test.git().resolve_ref(&repo.path, "  ").unwrap_err();
    assert_eq!(
        empty.message(),
        "Failed to resolve the ref: A ref is required."
    );
    let unknown = test.git().resolve_ref(&repo.path, "nope").unwrap_err();
    assert!(
        unknown.message().starts_with("Failed to resolve the ref: "),
        "{}",
        unknown.message()
    );
    let gone = test
        .git()
        .resolve_ref(repo.path.join("gone"), "main")
        .unwrap_err();
    assert_eq!(
        gone.message(),
        "Failed to resolve the ref: Cannot use simple-git on a directory that does not exist"
    );
}

#[test]
fn reads_throw_with_the_channel_prefix_and_never_touch_the_queue() {
    let test = TestBackend::new();
    let directory = tempfile::tempdir().unwrap();

    let error = test.git().get_status(directory.path()).unwrap_err();
    assert!(
        error.message().starts_with("Failed to get status: "),
        "{}",
        error.message()
    );
    let inspection = test.git().inspect_repository(directory.path());
    assert!(!inspection.valid);
    assert_eq!(
        inspection.message,
        "This directory is not under Git source control."
    );
    assert!(test.events.operations().is_empty());
}

// --- git:operation-progress ------------------------------------------------

/// Archivos poco comprimibles para que git tenga objetos que contar y enviar.
fn write_payload(dir: &Path, prefix: &str, files: usize) {
    // Semilla distinta por prefijo: contenidos repetidos no serían objetos nuevos.
    let mut seed = prefix
        .bytes()
        .fold(0x9e37_79b9_7f4a_7c15_u64, |seed, byte| {
            seed.rotate_left(5) ^ u64::from(byte)
        });
    for index in 0..files {
        let mut text = String::with_capacity(4096);
        while text.len() < 4096 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            text.push_str(&format!("{seed:016x}\n"));
        }
        std::fs::write(dir.join(format!("{prefix}{index}.txt")), text).unwrap();
    }
}

/// Fases de progreso que llegaron para `operation_id`, comprobando que todas
/// llegan antes del evento final de la operación.
fn progress_phases(test: &TestBackend, operation_id: &str) -> Vec<String> {
    let events = test.events.all();
    let finished = events
        .iter()
        .position(|event| {
            matches!(event, crate::BackendEvent::Operation(event)
                if event.operation_id == operation_id && event.phase.is_terminal())
        })
        .expect("final event");
    let mut phases = Vec::new();
    for (index, event) in events.iter().enumerate() {
        if let crate::BackendEvent::OperationProgress(update) = event
            && update.operation_id == operation_id
        {
            assert!(index < finished, "progress after the final event");
            phases.push(update.progress.phase.clone());
        }
    }
    phases
}

#[test]
fn fetch_push_and_clone_report_git_progress_before_finishing() {
    let test = TestBackend::new();
    let repo = TestRepo::new();
    let origin = repo.publish_to_origin();

    // push: los objetos nuevos se cuentan, comprimen y escriben.
    write_payload(&repo.path, "push-", 60);
    repo.git(&["add", "."]);
    repo.git(&["commit", "-q", "-m", "Payload"]);
    let pushed = test.git().push(
        &repo.path,
        &PushOptions {
            remote: "origin".to_string(),
            branch: Some("main".to_string()),
            ..Default::default()
        },
    );
    assert!(pushed.success(), "{}", pushed.message());
    let phases = progress_phases(&test, pushed.operation_id().unwrap());
    assert!(
        phases.iter().any(|phase| phase == "Writing objects"),
        "{phases:?}"
    );
    // El progreso no se cuela en el mensaje del resultado.
    assert!(!pushed.message().contains('%'), "{}", pushed.message());

    // fetch: lo que otro empujó llega con "Receiving objects" (con menos
    // de `transfer.unpackLimit`, 100 objetos, git los desempaqueta sin
    // informar).
    let colleague = repo.colleague(&origin);
    write_payload(&colleague.path, "fetch-", 150);
    git_in(&colleague.path, &["add", "."]);
    git_in(&colleague.path, &["commit", "-q", "-m", "More payload"]);
    git_in(&colleague.path, &["push", "-q", "origin", "HEAD"]);
    let fetched = test.git().fetch(&repo.path, &FetchOptions::default());
    assert!(fetched.success(), "{}", fetched.message());
    let phases = progress_phases(&test, fetched.operation_id().unwrap());
    assert!(
        phases.iter().any(|phase| phase == "Receiving objects"),
        "{phases:?}"
    );

    // clone por `file://` (una ruta local se copia sin transferir objetos).
    let target = repo.root().join("progress-clone");
    let url = format!("file://{}", text(&origin));
    let cloned = test.git().clone(&url, &text(&target), None);
    assert!(cloned.success, "{}", cloned.message);
    let phases = progress_phases(&test, cloned.operation_id.as_deref().unwrap());
    assert!(
        phases.iter().any(|phase| phase == "Receiving objects"),
        "{phases:?}"
    );
}
