//! Diálogos de la barra: Fetch, Pull, Push, Create Branch, Operation log y
//! Reflog (`OperationsLogDialog.test.js`, `ReflogDialog.test.js` y el
//! comportamiento que los demás diálogos tenían sin test propio).

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::TestAppContext;
use gpui_kit::test::TestWindowExt as _;
use mergeon_app::{AppModel, CreateBranchSubmission, DialogRequest, LocalChangesMode, SyncRequest};
use mergeon_model::notify::NotifyType;
use mergeon_model::operation_types::OperationResult;
use mergeon_view_toolbar::{
    CreateBranchDialogOptions, handle_dialog_request, open_create_branch_dialog, open_fetch_dialog,
    open_operations_log, open_pull_dialog, open_push_dialog, open_reflog, open_stash_dialog,
};

use crate::support::{TestApp, TestRepo};

/// La operación ya tiene resultado en el log de la sesión.
fn logged(model: &AppModel, operation: &str) -> bool {
    logged_count(model, operation) > 0
}

/// Cuántas operaciones terminadas hay registradas de ese tipo.
fn logged_count(model: &AppModel, operation: &str) -> usize {
    model
        .operations_log()
        .entries
        .iter()
        .filter(|entry| {
            entry.operation.as_deref() == Some(operation)
                && entry
                    .outcome
                    .as_deref()
                    .is_some_and(|outcome| !outcome.is_empty())
        })
        .count()
}

/// Avanza el foco con Tab hasta llegar al control `id`.
fn tab_to(app: &TestApp, cx: &mut TestAppContext, id: &'static str) {
    for _ in 0..16 {
        if app.in_window(cx, |window, _| window.find(id).focused() == Some(true)) {
            return;
        }
        app.in_window(cx, |window, cx| window.press("tab", cx));
    }
    panic!("could not tab to `{id}`");
}

// ---------------------------------------------------------------------------
// Delegación desde la cáscara
// ---------------------------------------------------------------------------

#[gpui_kit::test]
async fn handles_the_dialog_requests_that_belong_to_this_crate(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new().with_origin();
    app.open(&repo, cx).await;

    let handled = app.in_window(cx, |window, cx| {
        [
            DialogRequest::Fetch,
            DialogRequest::Pull,
            DialogRequest::Push,
            DialogRequest::CreateBranch {
                base_reference: "main".to_string(),
                base_label: "main".to_string(),
                has_local_changes: false,
            },
            DialogRequest::OperationsLog,
            DialogRequest::Reflog,
            DialogRequest::Stash { paths: vec![] },
            DialogRequest::Launcher,
            DialogRequest::GitHubAccount,
            DialogRequest::GitHubRepositoryPicker,
        ]
        .map(|request| handle_dialog_request(&request, window, cx))
    });

    assert_eq!(
        handled,
        [
            true, true, true, true, true, true, true, false, false, false
        ]
    );
    assert_eq!(
        app.dialogs(cx),
        [
            "Fetch",
            "Pull",
            "Push",
            "Create Branch",
            "Operation log",
            "Reflog",
            "Stash changes"
        ]
    );
}

/// En el original cada diálogo era un `v-if` sobre una bandera: pedirlo dos
/// veces lo dejaba abierto una sola vez.
#[gpui_kit::test]
async fn asking_twice_for_a_dialog_keeps_a_single_one_open(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new().with_origin();
    app.open(&repo, cx).await;

    let reopened = app.in_window(cx, |window, cx| {
        [
            open_fetch_dialog(window, cx).is_some(),
            open_fetch_dialog(window, cx).is_some(),
            open_reflog(window, cx).is_some(),
            open_reflog(window, cx).is_some(),
        ]
    });

    assert_eq!(reopened, [true, false, true, false]);
    assert_eq!(app.dialogs(cx), ["Fetch", "Reflog"]);
}

// ---------------------------------------------------------------------------
// Fetch
// ---------------------------------------------------------------------------

#[gpui_kit::test]
async fn fetch_brings_every_remote_by_default(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new().with_origin();
    app.open(&repo, cx).await;
    app.in_window(cx, |window, cx| {
        open_fetch_dialog(window, cx);
    });

    app.in_window(cx, |window, _| {
        assert_eq!(window.find("fetch-all-remotes").checked(), Some(true));
        assert_eq!(window.find("fetch-remote").label(), Some("origin"));
    });
    // Con "todos los remotos" el selector queda bloqueado.
    app.click(cx, "fetch-remote");
    assert!(!app.exists(cx, "popup-menu"));

    app.click(cx, "fetch-submit");

    assert!(app.dialogs(cx).is_empty());
    app.wait_for(cx, "the fetch to be logged", |model| logged(model, "fetch"));
}

#[gpui_kit::test]
async fn fetch_lets_choose_one_remote(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new().with_origin();
    repo.git(&[
        "remote",
        "add",
        "upstream",
        &repo.origin_path().to_string_lossy(),
    ]);
    app.open(&repo, cx).await;
    app.in_window(cx, |window, cx| {
        open_fetch_dialog(window, cx);
    });

    app.click(cx, "fetch-all-remotes");
    app.in_window(cx, |window, _| {
        assert_eq!(window.find("fetch-all-remotes").checked(), Some(false));
    });
    app.click(cx, "fetch-remote");
    app.in_window(cx, |window, cx| {
        window.within("popup-menu").click(1usize, cx);
    });

    app.in_window(cx, |window, _| {
        assert_eq!(window.find("fetch-remote").label(), Some("upstream"));
    });
}

#[gpui_kit::test]
async fn cancelling_fetch_runs_nothing(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new().with_origin();
    app.open(&repo, cx).await;
    app.in_window(cx, |window, cx| {
        open_fetch_dialog(window, cx);
    });

    app.click(cx, "fetch-cancel");

    assert!(app.dialogs(cx).is_empty());
    assert!(!app.model.read_with(cx, |model, _| logged(model, "fetch")));
    assert_eq!(
        app.model
            .read_with(cx, |model, _| model.busy_toolbar_operation()),
        None
    );
}

// ---------------------------------------------------------------------------
// Pull
// ---------------------------------------------------------------------------

#[gpui_kit::test]
async fn pull_starts_from_the_upstream_of_the_current_branch(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new().with_origin();
    // origin/main va un commit por delante de main.
    repo.commit_file("CHANGELOG.md", "# Changelog\n", "Add changelog");
    repo.git(&["push", "-q", "origin", "main"]);
    let published = repo.head();
    repo.git(&["reset", "-q", "--hard", "HEAD~1"]);
    app.open(&repo, cx).await;
    app.in_window(cx, |window, cx| {
        open_pull_dialog(window, cx);
    });

    app.in_window(cx, |window, _| {
        assert_eq!(window.find("pull-remote").label(), Some("origin"));
        assert_eq!(window.find("pull-branch").label(), Some("main"));
        assert_eq!(window.find("pull-rebase").checked(), Some(false));
    });
    // Sin cambios locales no hay nada que guardar en un stash.
    app.click(cx, "pull-autostash");
    app.in_window(cx, |window, _| {
        assert_eq!(window.find("pull-autostash").checked(), Some(false));
    });

    app.click(cx, "pull-submit");

    assert!(app.dialogs(cx).is_empty());
    app.wait_until(cx, "the pull to fast-forward main", || {
        repo.head() == published
    });
}

#[gpui_kit::test]
async fn pull_offers_to_stash_only_when_there_are_local_changes(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new().with_origin();
    repo.write("README.md", "# Demo\n\nWork in progress.\n");
    app.open(&repo, cx).await;
    app.in_window(cx, |window, cx| {
        open_pull_dialog(window, cx);
    });

    app.click(cx, "pull-autostash");

    app.in_window(cx, |window, _| {
        assert_eq!(window.find("pull-autostash").checked(), Some(true));
    });
}

// ---------------------------------------------------------------------------
// Push
// ---------------------------------------------------------------------------

#[gpui_kit::test]
async fn push_defaults_to_the_upstream_of_the_current_branch(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new().with_origin();
    repo.commit_file("CHANGELOG.md", "# Changelog\n", "Add changelog");
    app.open(&repo, cx).await;
    app.in_window(cx, |window, cx| {
        open_push_dialog(window, cx);
    });

    app.in_window(cx, |window, _| {
        assert_eq!(window.find("push-branch").label(), Some("main"));
        assert_eq!(
            window.find("push-destination").label(),
            Some("default (origin/main)")
        );
        assert_eq!(window.find("push-tags").checked(), Some(false));
        assert_eq!(window.find("push-force").checked(), Some(false));
    });

    app.click(cx, "push-submit");

    assert!(app.dialogs(cx).is_empty());
    let head = repo.head();
    app.wait_until(cx, "the push to reach origin", || {
        repo.origin_ref("main").as_deref() == Some(head.as_str())
    });
}

#[gpui_kit::test]
async fn push_sends_a_branch_without_upstream_to_its_namesake(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new().with_origin();
    repo.git(&["checkout", "-q", "-b", "topic"]);
    repo.commit_file("topic.txt", "topic\n", "Start topic");
    app.open(&repo, cx).await;
    app.in_window(cx, |window, cx| {
        open_push_dialog(window, cx);
    });

    app.in_window(cx, |window, _| {
        assert_eq!(window.find("push-branch").label(), Some("topic"));
        assert_eq!(
            window.find("push-destination").label(),
            Some("origin/topic")
        );
    });

    app.click(cx, "push-submit");

    let head = repo.head();
    app.wait_until(cx, "the push to create origin/topic", || {
        repo.origin_ref("topic").as_deref() == Some(head.as_str())
    });
}

#[gpui_kit::test]
async fn choosing_another_branch_resets_the_destination(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new().with_origin();
    repo.git(&["branch", "topic"]);
    app.open(&repo, cx).await;
    app.in_window(cx, |window, cx| {
        open_push_dialog(window, cx);
    });

    // Ramas locales en el orden del modelo: main, topic.
    app.click(cx, "push-branch");
    app.in_window(cx, |window, cx| {
        window.within("popup-menu").click(1usize, cx);
    });

    app.in_window(cx, |window, _| {
        assert_eq!(window.find("push-branch").label(), Some("topic"));
        assert_eq!(
            window.find("push-destination").label(),
            Some("origin/topic")
        );
    });
}

// ---------------------------------------------------------------------------
// Create Branch
// ---------------------------------------------------------------------------

type Submissions = Rc<RefCell<Vec<CreateBranchSubmission>>>;

fn open_create_branch(
    app: &TestApp,
    cx: &mut TestAppContext,
    options: CreateBranchDialogOptions,
) -> Submissions {
    let submissions: Submissions = Rc::new(RefCell::new(Vec::new()));
    app.in_window(cx, {
        let submissions = submissions.clone();
        move |window, cx| {
            open_create_branch_dialog(
                options.on_submit(move |submission, _, _| {
                    submissions.borrow_mut().push(submission);
                }),
                window,
                cx,
            );
        }
    });
    submissions
}

#[gpui_kit::test]
async fn create_branch_needs_a_name(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let submissions = open_create_branch(&app, cx, CreateBranchDialogOptions::new("main"));

    app.click(cx, "create-branch-submit");
    // Solo espacios tampoco es un nombre.
    app.in_window(cx, |window, cx| window.input("   ", cx));
    app.click(cx, "create-branch-submit");
    app.in_window(cx, |window, cx| window.press("enter", cx));

    assert!(submissions.borrow().is_empty());
    assert_eq!(app.dialogs(cx), ["Create Branch"]);
}

#[gpui_kit::test]
async fn create_branch_submits_the_trimmed_name_with_the_default_choices(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let submissions = open_create_branch(&app, cx, CreateBranchDialogOptions::new("main"));

    // El campo ya tiene el foco (`autofocus`).
    app.in_window(cx, |window, cx| window.input("  feature/payments ", cx));
    app.click(cx, "create-branch-submit");

    assert_eq!(
        *submissions.borrow(),
        [CreateBranchSubmission {
            branch_name: "feature/payments".to_string(),
            checkout_after_create: true,
            local_changes_mode: LocalChangesMode::Keep,
        }]
    );
    assert!(app.dialogs(cx).is_empty());
}

#[gpui_kit::test]
async fn enter_in_the_name_field_submits(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let submissions = open_create_branch(
        &app,
        cx,
        CreateBranchDialogOptions::new("v1.0.0").suggested_name("hotfix/1.0.1"),
    );

    app.in_window(cx, |window, cx| window.press("enter", cx));

    assert_eq!(submissions.borrow().len(), 1);
    assert_eq!(submissions.borrow()[0].branch_name, "hotfix/1.0.1");
    assert!(app.dialogs(cx).is_empty());
}

#[gpui_kit::test]
async fn create_branch_carries_the_chosen_options(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let submissions = open_create_branch(
        &app,
        cx,
        CreateBranchDialogOptions::new("main").has_local_changes(true),
    );

    app.in_window(cx, |window, cx| window.input("topic", cx));
    app.click(cx, "create-branch-checkout");
    app.click(cx, "radio-stash");
    app.click(cx, "create-branch-submit");

    assert_eq!(
        *submissions.borrow(),
        [CreateBranchSubmission {
            branch_name: "topic".to_string(),
            checkout_after_create: false,
            local_changes_mode: LocalChangesMode::Stash,
        }]
    );
}

#[gpui_kit::test]
async fn local_changes_cannot_be_chosen_without_local_changes(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let submissions = open_create_branch(&app, cx, CreateBranchDialogOptions::new("main"));

    app.in_window(cx, |window, cx| window.input("topic", cx));
    app.click(cx, "radio-discard");
    app.click(cx, "create-branch-submit");

    assert_eq!(
        submissions.borrow()[0].local_changes_mode,
        LocalChangesMode::Keep
    );
}

/// `<label for="create-branch-name">`.
#[gpui_kit::test]
async fn clicking_the_name_label_focuses_the_field(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let submissions = open_create_branch(&app, cx, CreateBranchDialogOptions::new("main"));

    // Con el foco fuera del campo, lo que se teclea no llega al nombre.
    app.in_window(cx, |window, cx| window.focus_next(cx));
    app.in_window(cx, |window, cx| window.input("lost", cx));
    app.click(cx, "create-branch-submit");
    assert!(submissions.borrow().is_empty());

    app.click(cx, "create-branch-name-label");
    app.in_window(cx, |window, cx| window.input("topic", cx));
    app.click(cx, "create-branch-submit");

    assert_eq!(submissions.borrow().len(), 1);
    assert_eq!(submissions.borrow()[0].branch_name, "topic");
}

#[gpui_kit::test]
async fn cancelling_create_branch_submits_nothing(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let submissions = open_create_branch(&app, cx, CreateBranchDialogOptions::new("main"));

    app.in_window(cx, |window, cx| window.input("topic", cx));
    app.click(cx, "create-branch-cancel");

    assert!(submissions.borrow().is_empty());
    assert!(app.dialogs(cx).is_empty());
}

// ---------------------------------------------------------------------------
// OperationsLogDialog.test.js
// ---------------------------------------------------------------------------

fn result(id: &str, operation: &str, outcome: &str, message: &str) -> OperationResult {
    OperationResult {
        success: outcome == "completed",
        operation: Some(operation.to_string()),
        operation_id: Some(id.to_string()),
        outcome: Some(outcome.to_string()),
        code: Some("GIT_ERROR".to_string()),
        message: Some(message.to_string()),
        status: None,
    }
}

fn record(app: &TestApp, cx: &mut TestAppContext, repo_id: &str, result: OperationResult) {
    app.model.update(cx, |model, cx| {
        model.apply_repository_operation_result(repo_id, &result, cx);
    });
}

fn unseen_errors(app: &TestApp, cx: &mut TestAppContext) -> usize {
    app.model
        .read_with(cx, |model, _| model.operations_log().unseen_errors)
}

#[gpui_kit::test]
async fn lists_operations_with_outcome_and_expands_the_full_message_on_click(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    let repo_id = app.open(&repo, cx).await;
    record(
        &app,
        cx,
        &repo_id,
        result(
            "push-1",
            "push",
            "failed",
            "remote: permission denied\nfatal: unable to push",
        ),
    );
    app.in_window(cx, |window, cx| {
        open_operations_log(window, cx);
    });

    assert_eq!(
        app.label(cx, "ops-log-row-push-1").as_deref(),
        Some("Push · demo · failed")
    );
    assert!(!app.exists(cx, "ops-log-message-push-1"));

    app.click(cx, "ops-log-row-push-1");
    assert_eq!(
        app.label(cx, "ops-log-message-push-1").as_deref(),
        Some("remote: permission denied\nfatal: unable to push")
    );

    // Otro clic la pliega.
    app.click(cx, "ops-log-row-push-1");
    assert!(!app.exists(cx, "ops-log-message-push-1"));
}

#[gpui_kit::test]
async fn filters_to_errors_only_and_marks_errors_as_seen_when_opened(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    let repo_id = app.open(&repo, cx).await;
    record(&app, cx, &repo_id, result("a", "fetch", "completed", "ok"));
    record(&app, cx, &repo_id, result("b", "pull", "failed", "bad"));
    assert_eq!(unseen_errors(&app, cx), 1);

    app.in_window(cx, |window, cx| {
        open_operations_log(window, cx);
    });
    // Abrir marca los errores como vistos
    assert_eq!(unseen_errors(&app, cx), 0);

    assert!(app.exists(cx, "ops-log-row-a"));
    app.click(cx, "ops-log-only-errors");
    assert!(!app.exists(cx, "ops-log-row-a"));
    assert_eq!(
        app.label(cx, "ops-log-row-b").as_deref(),
        Some("Pull · demo · failed")
    );
}

#[gpui_kit::test]
async fn shows_an_empty_state_when_there_is_nothing_logged(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    app.in_window(cx, |window, cx| {
        open_operations_log(window, cx);
    });

    assert_eq!(
        app.label(cx, "ops-log-empty").as_deref(),
        Some("No operations yet.")
    );
    app.click(cx, "ops-log-only-errors");
    assert_eq!(
        app.label(cx, "ops-log-empty").as_deref(),
        Some("No failed operations this session.")
    );
}

#[gpui_kit::test]
async fn clear_empties_the_log(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    let repo_id = app.open(&repo, cx).await;
    record(&app, cx, &repo_id, result("a", "fetch", "completed", "ok"));
    app.in_window(cx, |window, cx| {
        open_operations_log(window, cx);
    });

    app.click(cx, "ops-log-clear");

    assert!(!app.exists(cx, "ops-log-row-a"));
    assert!(app.exists(cx, "ops-log-empty"));
    assert!(
        app.model
            .read_with(cx, |model, _| model.operations_log().entries.is_empty())
    );
}

// ---------------------------------------------------------------------------
// ReflogDialog.test.js
// ---------------------------------------------------------------------------

#[gpui_kit::test]
async fn the_operation_log_builds_only_the_visible_rows(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    let repo_id = app.open(&repo, cx).await;
    for ix in 0..200 {
        record(
            &app,
            cx,
            &repo_id,
            result(&format!("op-{ix}"), "push", "failed", &format!("boom {ix}")),
        );
    }
    app.in_window(cx, |window, cx| {
        open_operations_log(window, cx);
    });

    // Las más nuevas, arriba, se construyen; las de abajo no.
    assert!(app.exists(cx, "ops-log-row-op-199"));
    assert!(!app.exists(cx, "ops-log-row-op-0"));
    let list = app.in_window(cx, |window, _| {
        window.find("ops-log-list").bounds().size.height
    });
    assert!(list <= gpui_kit::px(440.), "{list:?}");

    // Una fila virtual se despliega con un clic.
    app.click(cx, "ops-log-row-op-198");
    assert_eq!(
        app.label(cx, "ops-log-message-op-198").as_deref(),
        Some("boom 198")
    );

    // Lo que llega con el diálogo abierto aparece arriba y lo desplegado
    // sigue desplegado.
    record(
        &app,
        cx,
        &repo_id,
        result("op-200", "fetch", "completed", "done"),
    );
    assert_eq!(
        app.label(cx, "ops-log-row-op-200").as_deref(),
        Some("Fetch · demo · completed")
    );
    assert!(app.exists(cx, "ops-log-message-op-198"));

    // "Only errors" deja fuera la que terminó bien.
    app.click(cx, "ops-log-only-errors");
    assert!(!app.exists(cx, "ops-log-row-op-200"));
    assert!(app.exists(cx, "ops-log-row-op-199"));
}

/// Un repositorio cuyo reflog es: `HEAD@{0}` reset a "Initial commit",
/// `HEAD@{1}` el commit "Lost work" y `HEAD@{2}` el commit inicial.
fn repo_with_lost_work() -> (TestRepo, String) {
    let repo = TestRepo::new();
    repo.commit_file("work.txt", "important\n", "Lost work");
    let lost = repo.head();
    repo.git(&["reset", "-q", "--hard", "HEAD~1"]);
    (repo, lost)
}

async fn open_reflog_dialog(app: &TestApp, repo: &TestRepo, cx: &mut TestAppContext) {
    app.open(repo, cx).await;
    app.in_window(cx, |window, cx| {
        open_reflog(window, cx);
    });
    // La lectura corre en segundo plano.
    app.wait_for_element(cx, "reflog-row-HEAD@{0}");
}

#[gpui_kit::test]
async fn lists_reflog_entries_with_selector_hash_and_subject(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let (repo, lost) = repo_with_lost_work();
    open_reflog_dialog(&app, &repo, cx).await;

    assert_eq!(
        app.label(cx, "reflog-row-HEAD@{0}"),
        Some(format!(
            "HEAD@{{0}} {} reset: moving to HEAD~1",
            repo.head_short()
        ))
    );
    assert_eq!(
        app.label(cx, "reflog-row-HEAD@{1}"),
        Some(format!("HEAD@{{1}} {} commit: Lost work", &lost[..7]))
    );
    assert!(app.exists(cx, "reflog-row-HEAD@{2}"));
    assert!(!app.exists(cx, "reflog-row-HEAD@{3}"));
}

#[gpui_kit::test]
async fn creates_a_branch_at_an_entry_through_the_prompt(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let (repo, lost) = repo_with_lost_work();
    open_reflog_dialog(&app, &repo, cx).await;

    app.host.answer_prompt(" rescued ");
    app.click(cx, "reflog-branch-HEAD@{1}");
    app.wait_until(cx, "the rescued branch", || {
        repo.try_git(&["rev-parse", "-q", "--verify", "rescued"])
            .is_ok()
    });

    assert_eq!(repo.git(&["rev-parse", "rescued"]), lost);
    // Crear la rama no cambia de rama ni cierra el diálogo.
    assert_eq!(repo.git(&["branch", "--show-current"]), "main");
    assert_eq!(app.dialogs(cx), ["Reflog"]);

    let prompts = app.host.prompts();
    assert_eq!(prompts.len(), 1);
    assert_eq!(
        prompts[0].title,
        Some(format!("Create branch at {}", &lost[..7]))
    );
    assert_eq!(prompts[0].description.as_deref(), Some("commit: Lost work"));
    assert_eq!(prompts[0].placeholder.as_deref(), Some("recovered-work"));

    // La rama nueva llega a la barra lateral.
    app.wait_for(cx, "the branch list to be read again", |model| {
        model.active_repository().is_some_and(|repository| {
            repository
                .branches
                .iter()
                .any(|branch| branch.name == "rescued")
        })
    });
}

#[gpui_kit::test]
async fn a_cancelled_prompt_creates_no_branch(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let (repo, _) = repo_with_lost_work();
    open_reflog_dialog(&app, &repo, cx).await;

    app.host.cancel_prompt();
    app.click(cx, "reflog-branch-HEAD@{1}");
    app.wait_until(cx, "the prompt", || app.host.prompts().len() == 1);
    cx.run_until_parked();

    assert_eq!(repo.git(&["branch", "--format=%(refname:short)"]), "main");
}

#[gpui_kit::test]
async fn hard_resets_to_an_entry_after_destructive_confirmation(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let (repo, lost) = repo_with_lost_work();
    open_reflog_dialog(&app, &repo, cx).await;

    app.host.answer_confirm(true);
    app.click(cx, "reflog-reset-HEAD@{1}");
    app.wait_until(cx, "the reset", || repo.head() == lost);

    let confirms = app.host.confirms();
    assert_eq!(confirms.len(), 1);
    assert_eq!(confirms[0].title.as_deref(), Some("Reset here"));
    assert_eq!(
        confirms[0].description,
        Some(format!(
            "Hard-reset the current branch to {} (HEAD@{{1}})? Uncommitted changes will be lost.",
            &lost[..7]
        ))
    );
    assert_eq!(confirms[0].variant.as_deref(), Some("destructive"));

    // Tras el reset el diálogo se cierra solo.
    app.wait_for_no_dialogs(cx);
    assert_eq!(repo.git(&["branch", "--show-current"]), "main");
}

#[gpui_kit::test]
async fn does_not_reset_when_the_confirmation_is_declined(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let (repo, _) = repo_with_lost_work();
    let before = repo.head();
    open_reflog_dialog(&app, &repo, cx).await;

    app.host.answer_confirm(false);
    app.click(cx, "reflog-reset-HEAD@{1}");
    app.wait_until(cx, "the confirmation", || app.host.confirms().len() == 1);
    cx.run_until_parked();

    assert_eq!(repo.head(), before);
    assert_eq!(app.dialogs(cx), ["Reflog"]);
}

#[gpui_kit::test]
async fn an_unreadable_reflog_is_reported_and_shown_empty(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    app.open(&repo, cx).await;
    // El repositorio desaparece con su pestaña aún abierta.
    std::fs::remove_dir_all(repo.path.join(".git")).expect("remove .git");

    app.in_window(cx, |window, cx| {
        open_reflog(window, cx);
    });
    app.wait_for_label(cx, "reflog-state", "No reflog entries.");

    assert!(
        app.host
            .messages(NotifyType::Error)
            .iter()
            .any(|message| !message.is_empty()),
        "the failure should be announced"
    );
}

#[gpui_kit::test]
async fn the_reflog_builds_only_the_visible_rows(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    for ix in 0..120 {
        repo.git(&["commit", "-q", "--allow-empty", "-m", &format!("step {ix}")]);
    }
    open_reflog_dialog(&app, &repo, cx).await;

    // Se piden 100 entradas; solo las de arriba se construyen.
    assert!(app.exists(cx, "reflog-row-HEAD@{1}"));
    assert!(!app.exists(cx, "reflog-row-HEAD@{99}"));

    // La lista no pasa de su `max-height` ni estira el diálogo.
    let (list, content) = app.in_window(cx, |window, _| {
        (
            window.find("reflog-list").bounds().size.height,
            window.find("reflog-content").bounds().size.height,
        )
    });
    assert!(list <= gpui_kit::px(460.), "{list:?}");
    assert!(content <= gpui_kit::px(470.), "{content:?}");
}

// ---------------------------------------------------------------------------
// Stash
// ---------------------------------------------------------------------------

/// Archivos del último stash, con los que no se siguen.
fn stashed_files(repo: &TestRepo) -> Vec<String> {
    let mut files: Vec<String> = repo
        .git(&[
            "stash",
            "show",
            "--include-untracked",
            "--name-only",
            "stash@{0}",
        ])
        .lines()
        .map(str::to_string)
        .collect();
    files.sort();
    files
}

fn checked(app: &TestApp, cx: &mut TestAppContext, id: &'static str) -> Option<bool> {
    app.in_window(cx, |window, _| window.find(id).checked())
}

#[gpui_kit::test]
async fn stash_summarizes_what_it_will_save_and_follows_its_options(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.write("README.md", "# Demo\n\nWork in progress.\n");
    repo.write("notes draft.txt", "draft\n");
    app.open(&repo, cx).await;

    app.click(cx, "toolbar-stash");
    app.wait_for_dialog(cx, "Stash changes");
    assert_eq!(
        app.label(cx, "stash-summary").as_deref(),
        Some("1 file will be stashed:")
    );
    assert_eq!(
        app.label(cx, "stash-summary:README.md").as_deref(),
        Some("README.md (unstaged)")
    );
    assert_eq!(checked(&app, cx, "stash-include-untracked"), Some(false));

    app.click(cx, "stash-include-untracked");
    assert_eq!(
        app.label(cx, "stash-summary").as_deref(),
        Some("2 files will be stashed:")
    );

    app.click(cx, "stash-submit");
    app.wait_until(cx, "the stash", || !repo.git(&["stash", "list"]).is_empty());
    assert!(app.dialogs(cx).is_empty());
    assert_eq!(stashed_files(&repo), ["README.md", "notes draft.txt"]);
    app.wait_for(cx, "the new stash in the sidebar", |model| {
        model
            .active_repository()
            .is_some_and(|repository| repository.stashes.len() == 1)
    });
}

#[gpui_kit::test]
async fn only_staged_changes_turns_off_the_other_options(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.write("README.md", "# Demo\n\nStaged.\n");
    repo.git(&["add", "README.md"]);
    repo.write("other.txt", "untracked\n");
    app.open(&repo, cx).await;
    app.in_window(cx, |window, cx| {
        open_stash_dialog(Vec::new(), window, cx);
    });

    app.click(cx, "stash-include-untracked");
    app.click(cx, "stash-keep-index");
    assert_eq!(checked(&app, cx, "stash-keep-index"), Some(true));

    app.click(cx, "stash-staged-only");
    assert_eq!(checked(&app, cx, "stash-staged-only"), Some(true));
    assert_eq!(checked(&app, cx, "stash-include-untracked"), Some(false));
    assert_eq!(checked(&app, cx, "stash-keep-index"), Some(false));
    // Apagadas mientras "Only staged changes" siga marcada.
    app.click(cx, "stash-keep-index");
    assert_eq!(checked(&app, cx, "stash-keep-index"), Some(false));
    assert_eq!(
        app.label(cx, "stash-summary:README.md").as_deref(),
        Some("README.md (staged)")
    );

    app.click(cx, "stash-submit");
    app.wait_until(cx, "the stash", || !repo.git(&["stash", "list"]).is_empty());
    assert_eq!(stashed_files(&repo), ["README.md"]);
    assert_eq!(repo.git(&["status", "--porcelain"]).trim(), "?? other.txt");
}

#[gpui_kit::test]
async fn stashing_selected_files_keeps_the_rest(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.write("README.md", "# Demo\n\nKeep me.\n");
    repo.write("new notes.txt", "draft\n");
    app.open(&repo, cx).await;
    app.in_window(cx, |window, cx| {
        open_stash_dialog(vec!["new notes.txt".to_string()], window, cx);
    });

    assert_eq!(app.dialogs(cx), ["Stash selected files"]);
    // Una ruta que no se sigue solo se guarda con "Include untracked files".
    assert_eq!(checked(&app, cx, "stash-include-untracked"), Some(true));
    app.click(cx, "stash-include-untracked");
    assert_eq!(checked(&app, cx, "stash-include-untracked"), Some(true));
    assert_eq!(
        app.label(cx, "stash-summary").as_deref(),
        Some("1 file will be stashed:")
    );

    app.click(cx, "stash-submit");
    app.wait_until(cx, "the stash", || !repo.git(&["stash", "list"]).is_empty());
    assert_eq!(stashed_files(&repo), ["new notes.txt"]);
    assert_eq!(repo.git(&["status", "--porcelain"]).trim(), "M README.md");
}

#[gpui_kit::test]
async fn cancelling_stash_saves_nothing(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.write("README.md", "# Demo\n\nWork in progress.\n");
    app.open(&repo, cx).await;
    app.click(cx, "toolbar-stash");
    app.wait_for_dialog(cx, "Stash changes");

    app.click(cx, "stash-cancel");

    assert!(app.dialogs(cx).is_empty());
    assert!(repo.git(&["stash", "list"]).is_empty());
    assert!(!app.model.read_with(cx, |model, _| logged(model, "stash")));
}

// ---------------------------------------------------------------------------
// Enter, foco por teclado y recordar opciones (Issue #15)
// ---------------------------------------------------------------------------

#[gpui_kit::test]
async fn enter_submits_fetch_pull_and_push_dialogs(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new().with_origin();
    repo.commit_file("CHANGELOG.md", "# Changelog\n", "Add changelog");
    repo.git(&["push", "-q", "origin", "main"]);
    let published = repo.head();
    repo.git(&["reset", "-q", "--hard", "HEAD~1"]);
    app.open(&repo, cx).await;

    app.in_window(cx, |window, cx| {
        open_fetch_dialog(window, cx);
    });
    app.in_window(cx, |window, cx| window.press("enter", cx));
    assert!(app.dialogs(cx).is_empty());
    app.wait_for(cx, "the fetch to be logged", |model| logged(model, "fetch"));
    app.wait_for(cx, "the fetch to settle", |model| {
        model.busy_toolbar_operation().is_none()
    });

    app.in_window(cx, |window, cx| {
        open_pull_dialog(window, cx);
    });
    app.in_window(cx, |window, cx| window.press("enter", cx));
    assert!(app.dialogs(cx).is_empty());
    app.wait_until(cx, "the pull to fast-forward main", || {
        repo.head() == published
    });
    app.wait_for(cx, "the pull to settle", |model| {
        model.busy_toolbar_operation().is_none()
    });

    repo.commit_file("NOTES.md", "notes\n", "Add notes");
    let head = repo.head();
    app.in_window(cx, |window, cx| {
        open_push_dialog(window, cx);
    });
    app.in_window(cx, |window, cx| window.press("enter", cx));
    assert!(app.dialogs(cx).is_empty());
    app.wait_until(cx, "the push to reach origin", || {
        repo.origin_ref("main").as_deref() == Some(head.as_str())
    });
}

#[gpui_kit::test]
async fn tabbing_to_fetch_submit_and_pressing_enter_submits_once(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new().with_origin();
    app.open(&repo, cx).await;
    app.in_window(cx, |window, cx| {
        open_fetch_dialog(window, cx);
    });

    tab_to(&app, cx, "fetch-submit");
    app.in_window(cx, |window, cx| window.press("enter", cx));

    assert!(app.dialogs(cx).is_empty());
    app.wait_for(cx, "the fetch to be logged", |model| logged(model, "fetch"));
    app.wait_for(cx, "the fetch to settle", |model| {
        model.busy_toolbar_operation().is_none()
    });
    assert_eq!(
        app.model
            .read_with(cx, |model, _| logged_count(model, "fetch")),
        1
    );
}

#[gpui_kit::test]
async fn tabbing_to_fetch_cancel_and_pressing_enter_closes_without_fetching(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new().with_origin();
    app.open(&repo, cx).await;
    app.in_window(cx, |window, cx| {
        open_fetch_dialog(window, cx);
    });

    tab_to(&app, cx, "fetch-cancel");
    app.in_window(cx, |window, cx| window.press("enter", cx));

    assert!(app.dialogs(cx).is_empty());
    assert!(!app.model.read_with(cx, |model, _| logged(model, "fetch")));
    assert_eq!(
        app.model
            .read_with(cx, |model, _| model.busy_toolbar_operation()),
        None
    );
}

#[gpui_kit::test]
async fn fetch_remembers_all_remotes_on_confirm_and_not_on_cancel(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new().with_origin();
    app.open(&repo, cx).await;

    // Cancelar tras cambiar la casilla no guarda nada.
    app.in_window(cx, |window, cx| {
        open_fetch_dialog(window, cx);
    });
    app.click(cx, "fetch-all-remotes");
    assert_eq!(checked(&app, cx, "fetch-all-remotes"), Some(false));
    app.click(cx, "fetch-cancel");
    assert!(
        app.model
            .read_with(cx, |model, _| model.action_defaults().fetch.all_remotes)
    );

    // Confirmar con `all_remotes = false` lo recuerda al reabrir.
    app.in_window(cx, |window, cx| {
        open_fetch_dialog(window, cx);
    });
    assert_eq!(checked(&app, cx, "fetch-all-remotes"), Some(true));
    app.click(cx, "fetch-all-remotes");
    app.click(cx, "fetch-submit");
    app.wait_for(cx, "the fetch to be logged", |model| logged(model, "fetch"));
    assert!(
        !app.model
            .read_with(cx, |model, _| model.action_defaults().fetch.all_remotes)
    );

    // Al reabrir llega desmarcado; volver a marcar y cancelar no lo pisa.
    app.in_window(cx, |window, cx| {
        open_fetch_dialog(window, cx);
    });
    assert_eq!(checked(&app, cx, "fetch-all-remotes"), Some(false));
    app.click(cx, "fetch-all-remotes");
    assert_eq!(checked(&app, cx, "fetch-all-remotes"), Some(true));
    app.click(cx, "fetch-cancel");
    assert!(
        !app.model
            .read_with(cx, |model, _| model.action_defaults().fetch.all_remotes)
    );
}

#[gpui_kit::test]
async fn pull_remembers_rebase_and_model_sync_preserves_it(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new().with_origin();
    let repo_id = app.open(&repo, cx).await;

    // Cancelar no guarda la preferencia.
    app.in_window(cx, |window, cx| {
        open_pull_dialog(window, cx);
    });
    app.click(cx, "pull-rebase");
    assert_eq!(checked(&app, cx, "pull-rebase"), Some(true));
    app.click(cx, "pull-cancel");
    assert!(
        !app.model
            .read_with(cx, |model, _| model.action_defaults().pull.rebase)
    );

    // Confirmar recuerda `rebase = true`.
    app.in_window(cx, |window, cx| {
        open_pull_dialog(window, cx);
    });
    assert_eq!(checked(&app, cx, "pull-rebase"), Some(false));
    app.click(cx, "pull-rebase");
    app.click(cx, "pull-submit");
    app.wait_for(cx, "the pull to be logged", |model| logged(model, "pull"));
    app.wait_for(cx, "the pull to settle", |model| {
        model.busy_toolbar_operation().is_none()
    });
    assert!(
        app.model
            .read_with(cx, |model, _| model.action_defaults().pull.rebase)
    );

    // Al reabrir llega marcado y un refresco de rama/destino no lo borra.
    app.in_window(cx, |window, cx| {
        open_pull_dialog(window, cx);
    });
    assert_eq!(checked(&app, cx, "pull-rebase"), Some(true));

    repo.git(&["checkout", "-q", "-b", "topic"]);
    repo.git(&["push", "-q", "-u", "origin", "topic"]);
    app.model
        .update(cx, |model, cx| {
            model.sync_repository_read_model(
                repo_id.as_str(),
                SyncRequest::new().status(true).branches(true).remotes(true),
                cx,
            )
        })
        .await;
    cx.run_until_parked();

    app.in_window(cx, |window, _| {
        assert_eq!(window.find("pull-branch").label(), Some("topic"));
        assert_eq!(window.find("pull-rebase").checked(), Some(true));
    });
}

#[gpui_kit::test]
async fn push_remembers_push_tags_and_resets_force_push(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new().with_origin();
    repo.commit_file("CHANGELOG.md", "# Changelog\n", "Add changelog");
    app.open(&repo, cx).await;

    // Cancelar no guarda `push_tags`.
    app.in_window(cx, |window, cx| {
        open_push_dialog(window, cx);
    });
    app.click(cx, "push-tags");
    app.click(cx, "push-force");
    app.click(cx, "push-cancel");
    assert!(
        !app.model
            .read_with(cx, |model, _| model.action_defaults().push.push_tags)
    );

    // Confirmar con ambos marcados solo recuerda `push_tags`, nunca `force`.
    app.in_window(cx, |window, cx| {
        open_push_dialog(window, cx);
    });
    app.click(cx, "push-tags");
    app.click(cx, "push-force");
    assert_eq!(checked(&app, cx, "push-tags"), Some(true));
    assert_eq!(checked(&app, cx, "push-force"), Some(true));
    app.click(cx, "push-submit");

    let head = repo.head();
    app.wait_until(cx, "the push to reach origin", || {
        repo.origin_ref("main").as_deref() == Some(head.as_str())
    });
    assert!(
        app.model
            .read_with(cx, |model, _| model.action_defaults().push.push_tags)
    );

    app.in_window(cx, |window, cx| {
        open_push_dialog(window, cx);
    });
    assert_eq!(checked(&app, cx, "push-tags"), Some(true));
    assert_eq!(checked(&app, cx, "push-force"), Some(false));
}

#[gpui_kit::test]
async fn enter_in_create_branch_from_another_control_submits_once(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let submissions = open_create_branch(
        &app,
        cx,
        CreateBranchDialogOptions::new("main").suggested_name("feature/enter"),
    );

    tab_to(&app, cx, "create-branch-checkout");
    app.in_window(cx, |window, cx| window.press("enter", cx));

    assert_eq!(submissions.borrow().len(), 1);
    assert_eq!(submissions.borrow()[0].branch_name, "feature/enter");
    assert!(app.dialogs(cx).is_empty());
}

#[gpui_kit::test]
async fn enter_in_stash_submits_once(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.write("README.md", "# Demo\n\nWork in progress.\n");
    app.open(&repo, cx).await;
    app.in_window(cx, |window, cx| {
        open_stash_dialog(Vec::new(), window, cx);
    });

    app.in_window(cx, |window, cx| window.input("WIP enter", cx));
    app.in_window(cx, |window, cx| window.press("enter", cx));

    assert!(app.dialogs(cx).is_empty());
    app.wait_until(cx, "the stash", || !repo.git(&["stash", "list"]).is_empty());
    app.wait_for(cx, "the stash to settle", |model| {
        model.busy_toolbar_operation().is_none()
    });
    let stashes = repo.git(&["stash", "list"]);
    assert_eq!(stashes.lines().count(), 1, "{stashes}");
    assert!(stashes.contains("WIP enter"), "{stashes}");
    assert_eq!(
        app.model
            .read_with(cx, |model, _| logged_count(model, "stash")),
        1
    );
    assert!(
        app.host.messages(NotifyType::Error).is_empty(),
        "{:?}",
        app.host.messages(NotifyType::Error)
    );
}
