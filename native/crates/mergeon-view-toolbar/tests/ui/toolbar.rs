//! `ToolbarView` (`Toolbar.test.js` y lo que el original dejaba a `App.vue`).

use std::time::Duration;

use gpui_kit::TestAppContext;
use gpui_kit::test::TestWindowExt as _;
use mergeon_backend::ExternalEditorSettings;
use mergeon_model::dialogs::{PromptAnswer, PromptOptions};
use mergeon_model::notify::NotifyType;
use mergeon_model::operation_types::{OperationPhase, QueueOperation};

use crate::support::{TestApp, TestRepo};

// ---------------------------------------------------------------------------
// Toolbar.test.js
// ---------------------------------------------------------------------------

#[gpui_kit::test]
async fn shows_a_detached_label_with_the_short_hash_instead_of_the_literal_head_label(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.git(&["checkout", "-q", "--detach"]);
    let hash = repo.head_short();
    app.open(&repo, cx).await;

    assert_eq!(
        app.label(cx, "toolbar-branch"),
        Some(format!("Detached · {hash}"))
    );
}

#[gpui_kit::test]
async fn shows_the_commit_head_is_parked_on_while_on_a_branch(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    let hash = repo.head_short();
    app.open(&repo, cx).await;

    // El nombre del repositorio ya está en su pestaña: la barra no lo repite.
    assert!(!app.exists(cx, "toolbar-repo-name"));
    assert_eq!(app.label(cx, "toolbar-branch").as_deref(), Some("main"));
    assert_eq!(
        app.label(cx, "toolbar-head"),
        Some(format!("Current commit · HEAD is at {hash}"))
    );
}

#[gpui_kit::test]
async fn does_not_repeat_the_hash_when_the_detached_label_already_carries_it(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.git(&["checkout", "-q", "--detach"]);
    app.open(&repo, cx).await;

    assert!(app.exists(cx, "toolbar-branch"));
    assert!(!app.exists(cx, "toolbar-head"));
}

#[gpui_kit::test]
async fn hides_repository_actions_when_no_repository_is_active(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);

    for id in [
        "toolbar",
        "toolbar-fetch",
        "toolbar-pull",
        "toolbar-push",
        "toolbar-stash",
        "toolbar-reflog",
        "toolbar-new-branch",
        "toolbar-open-editor",
        "toolbar-open-terminal",
        "toolbar-open-explorer",
        "toolbar-branch-button",
    ] {
        assert!(!app.exists(cx, id), "`{id}` should not be drawn");
    }
}

// ---------------------------------------------------------------------------
// Lo que cada botón pide (los `@fetch`, `@pull`... que atendía App.vue)
// ---------------------------------------------------------------------------

#[gpui_kit::test]
async fn fetch_pull_and_push_ask_for_their_dialogs(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new().with_origin();
    app.open(&repo, cx).await;

    for (button, cancel, title) in [
        ("toolbar-fetch", "fetch-cancel", "Fetch"),
        ("toolbar-pull", "pull-cancel", "Pull"),
        ("toolbar-push", "push-cancel", "Push"),
    ] {
        app.click(cx, button);
        assert_eq!(app.dialogs(cx), [title]);
        app.click(cx, cancel);
        assert!(app.dialogs(cx).is_empty(), "{title} should close on Cancel");
    }
}

#[gpui_kit::test]
async fn the_reflog_button_opens_the_reflog(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    app.open(&repo, cx).await;

    app.click(cx, "toolbar-reflog");

    assert_eq!(app.dialogs(cx), ["Reflog"]);
}

#[gpui_kit::test]
async fn new_branch_inspects_the_working_tree_and_opens_the_dialog(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    app.open(&repo, cx).await;

    app.click(cx, "toolbar-new-branch");
    app.wait_for_dialog(cx, "Create Branch");

    // Sin cambios locales no hay nada que decidir sobre ellos.
    app.in_window(cx, |window, cx| window.click("radio-stash", cx));
    app.in_window(cx, |window, cx| {
        window.input("topic", cx);
    });
    app.click(cx, "create-branch-submit");
    app.wait_for(cx, "the new branch", |model| {
        model
            .active_repository()
            .is_some_and(|repository| repository.current_branch.as_deref() == Some("topic"))
    });
    assert_eq!(repo.git(&["branch", "--show-current"]), "topic");
    assert!(repo.git(&["stash", "list"]).is_empty());
}

#[gpui_kit::test]
async fn terminal_and_explorer_open_on_the_repository(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    app.open(&repo, cx).await;

    app.click(cx, "toolbar-open-terminal");
    app.wait_until(cx, "the terminal", || !app.launcher.launched().is_empty());
    let terminal = app.launcher.launched();
    assert!(
        terminal.iter().any(|launched| {
            launched.cwd.as_deref() == Some(repo.path.as_path())
                || launched.args.iter().any(|arg| arg.contains("demo"))
        }),
        "the terminal should open on the repository: {terminal:?}"
    );

    app.click(cx, "toolbar-open-explorer");
    app.wait_until(cx, "the file explorer", || {
        app.launcher.launched().len() > terminal.len()
    });
    assert!(
        app.launcher
            .launched_args()
            .iter()
            .any(|arg| arg.contains("demo")),
        "the explorer should open the repository folder"
    );
    assert!(app.host.toasts().is_empty());
}

#[gpui_kit::test]
async fn open_in_editor_opens_the_repository_folder_in_the_chosen_editor(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    app.open(&repo, cx).await;
    app.model
        .update(cx, |model, cx| {
            model.save_external_editor(
                ExternalEditorSettings::custom("/opt/editor/run", "--new-window {path}"),
                cx,
            )
        })
        .await;

    app.click(cx, "toolbar-open-editor");
    app.wait_until(cx, "the editor", || !app.launcher.launched().is_empty());

    let launched = app.launcher.launched();
    assert_eq!(launched.len(), 1, "{launched:?}");
    assert_eq!(launched[0].command, "/opt/editor/run");
    assert_eq!(
        launched[0].args,
        ["--new-window".to_string(), repo.path_string()]
    );
    assert!(app.host.toasts().is_empty());
}

#[gpui_kit::test]
async fn stash_opens_its_dialog_and_saves_the_local_changes(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.write("README.md", "# Demo\n\nWork in progress.\n");
    app.open(&repo, cx).await;

    app.click(cx, "toolbar-stash");
    app.wait_for_dialog(cx, "Stash changes");
    // Pedirlo otra vez (el atajo, la paleta) no abre un segundo diálogo.
    app.model
        .update(cx, |model, cx| model.handle_stash(cx))
        .detach();
    assert_eq!(app.dialogs(cx), ["Stash changes"]);

    app.click(cx, "stash-submit");
    app.wait_until(cx, "the stash", || !repo.git(&["stash", "list"]).is_empty());
    assert!(app.host.prompts().is_empty());
}

#[gpui_kit::test]
async fn a_running_toolbar_operation_locks_its_button(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.write("README.md", "# Demo\n\nWork in progress.\n");
    let repo_id = app.open(&repo, cx).await;

    // Un stash en curso que espera una respuesta: la operación sigue viva.
    app.host.hold_answers(true);
    app.model
        .update(cx, |model, cx| {
            model.run_repo_operation_single_flight(&repo_id, "stash", cx, |this, cx| {
                let prompt = this.prompt(PromptOptions::default(), cx);
                cx.spawn(async move |_, _| {
                    prompt.await;
                })
            })
        })
        .detach();
    app.wait_for(cx, "the stash to be in flight", |model| {
        model.busy_toolbar_operation() == Some("stash")
    });

    // Su botón queda bloqueado: el clic no abre el diálogo.
    app.click(cx, "toolbar-stash");
    assert!(app.dialogs(cx).is_empty());

    assert!(app.host.release_prompt(PromptAnswer {
        confirmed: false,
        value: None,
    }));
    app.wait_for(cx, "the stash to settle", |model| {
        model.busy_toolbar_operation().is_none()
    });

    // Y vuelve a responder cuando la operación termina.
    app.click(cx, "toolbar-stash");
    app.wait_for_dialog(cx, "Stash changes");
}

#[gpui_kit::test]
async fn a_sequence_operation_blocks_the_actions_that_mutate(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new().with_merge_conflict();
    app.open(&repo, cx).await;

    for id in [
        "toolbar-fetch",
        "toolbar-pull",
        "toolbar-push",
        "toolbar-stash",
        "toolbar-new-branch",
    ] {
        app.click(cx, id);
    }
    cx.run_until_parked();
    assert!(app.dialogs(cx).is_empty());
    assert!(app.host.prompts().is_empty());

    // Consultar el repositorio y salir de él sigue permitido.
    app.click(cx, "toolbar-open-terminal");
    app.wait_until(cx, "the terminal", || !app.launcher.launched().is_empty());
    app.click(cx, "toolbar-reflog");
    assert_eq!(app.dialogs(cx), ["Reflog"]);
}

// ---------------------------------------------------------------------------
// Indicador de actividad y cancelación
// ---------------------------------------------------------------------------

fn queue_operation(id: &str, operation: &str, phase: OperationPhase) -> QueueOperation {
    QueueOperation {
        operation_id: Some(id.to_string()),
        operation: operation.to_string(),
        phase,
        cancellable: true,
        ..Default::default()
    }
}

#[gpui_kit::test]
async fn the_indicator_stays_hidden_while_nothing_is_running(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    app.open(&repo, cx).await;

    assert!(!app.exists(cx, "operation-indicator"));
}

#[gpui_kit::test]
async fn the_indicator_summarizes_a_sequence_operation(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new().with_merge_conflict();
    app.open(&repo, cx).await;

    assert_eq!(
        app.label(cx, "operation-indicator").as_deref(),
        Some("Merge in progress")
    );
}

#[gpui_kit::test]
async fn the_indicator_lists_the_queue_and_cancels_from_its_popover(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    let repo_id = app.open(&repo, cx).await;

    app.model.update(cx, |model, cx| {
        model.apply_repository_operation_event(
            repo_id.as_str(),
            &[
                queue_operation("fetch-1", "fetch", OperationPhase::Running),
                queue_operation("pull-1", "pull", OperationPhase::Queued),
            ],
            cx,
        );
    });
    assert_eq!(
        app.label(cx, "operation-indicator").as_deref(),
        Some("2 git operations active or queued")
    );
    assert!(!app.exists(cx, "mutation-queue"));

    // El popover se abre al dejar el cursor encima.
    app.in_window(cx, |window, cx| window.hover("operation-indicator", cx));
    cx.executor().advance_clock(Duration::from_millis(200));
    assert_eq!(
        app.label(cx, "queue-row-fetch-1").as_deref(),
        Some("Running Fetch")
    );
    assert_eq!(
        app.label(cx, "queue-row-pull-1").as_deref(),
        Some("Queued #1 Pull")
    );

    // La operación ya no existe en la cola del backend: el modelo lo avisa.
    app.click(cx, "queue-remove-pull-1");
    app.wait_until(cx, "the cancellation warning", || {
        app.host.has_message(
            NotifyType::Warning,
            "This Git operation is no longer cancellable.",
        )
    });
}

// ---------------------------------------------------------------------------
// `@container repository-content (max-width: 900px)`
// ---------------------------------------------------------------------------

#[gpui_kit::test]
async fn a_narrow_toolbar_moves_the_repository_chip_to_a_second_row(cx: &mut TestAppContext) {
    let wide = TestApp::new(cx);
    let repo = TestRepo::new();
    wide.open(&repo, cx).await;
    let (actions, chip) = wide.in_window(cx, |window, _| {
        (
            window.find("toolbar-fetch").bounds(),
            window.find("toolbar-branch-button").bounds(),
        )
    });
    assert!(
        !wide
            .shell
            .read_with(cx, |shell, cx| shell.toolbar.read(cx).is_compact())
    );
    assert!(chip.top() < actions.bottom(), "one row when there is room");
}

#[gpui_kit::test]
async fn a_narrow_window_uses_the_two_row_toolbar(cx: &mut TestAppContext) {
    let narrow = TestApp::with_width(700., cx);
    let repo = TestRepo::new();
    narrow.open(&repo, cx).await;
    let (actions, chip, right) = narrow.in_window(cx, |window, _| {
        (
            window.find("toolbar-fetch").bounds(),
            window.find("toolbar-branch-button").bounds(),
            window.find("toolbar-open-explorer").bounds(),
        )
    });

    assert!(
        narrow
            .shell
            .read_with(cx, |shell, cx| shell.toolbar.read(cx).is_compact())
    );
    assert!(chip.top() >= actions.bottom(), "the chip drops below");
    assert!(
        right.top() < chip.top(),
        "the right buttons stay on the first row"
    );
}

#[gpui_kit::test]
async fn the_narrow_toolbar_keeps_the_search_inside_the_window(cx: &mut TestAppContext) {
    let narrow = TestApp::with_width(852., cx);
    let repo = TestRepo::new();
    narrow.open(&repo, cx).await;
    let (actions, search, explorer) = narrow.in_window(cx, |window, _| {
        (
            window.find("toolbar-fetch").bounds(),
            window.find("toolbar-search").bounds(),
            window.find("toolbar-open-explorer").bounds(),
        )
    });

    assert!(search.top() >= actions.bottom(), "the search drops below");
    assert!(f32::from(search.right()) <= 852., "search: {search:?}");
    assert!(
        f32::from(explorer.right()) <= 852.,
        "explorer: {explorer:?}"
    );
}

// ---------------------------------------------------------------------------
// Señal: contadores, buscador y barra de estado
// ---------------------------------------------------------------------------

/// `main` con un commit propio sin publicar y otro de `origin/main` sin
/// traer.
fn diverged_repo() -> TestRepo {
    let repo = TestRepo::new().with_origin();
    repo.commit_file("remote.txt", "remote\n", "Remote work");
    repo.git(&["push", "-q", "origin", "main"]);
    repo.git(&["reset", "-q", "--hard", "HEAD~1"]);
    repo.commit_file("local.txt", "local\n", "Local work");
    repo
}

#[gpui_kit::test]
async fn pull_and_push_count_what_is_behind_and_ahead(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = diverged_repo();
    app.open(&repo, cx).await;

    assert_eq!(app.label(cx, "toolbar-pull-count").as_deref(), Some("1"));
    assert_eq!(app.label(cx, "toolbar-push-count").as_deref(), Some("1"));
    // Solo en las insignias: el botón de la rama no los repite.
    assert!(!app.exists(cx, "toolbar-branch-sync"));
    // Los contadores van dentro de su botón.
    let (pull, count) = app.in_window(cx, |window, _| {
        (
            window.find("toolbar-pull").bounds(),
            window.find("toolbar-pull-count").bounds(),
        )
    });
    assert!(pull.contains(&count.center()));
}

#[gpui_kit::test]
async fn a_branch_in_sync_shows_no_counters(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new().with_origin();
    app.open(&repo, cx).await;

    for id in [
        "toolbar-pull-count",
        "toolbar-push-count",
        "toolbar-branch-sync",
    ] {
        assert!(!app.exists(cx, id), "`{id}` should not be drawn");
    }
}

#[gpui_kit::test]
async fn the_search_field_opens_the_commit_search(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    app.open(&repo, cx).await;
    app.model.update(cx, |model, cx| {
        model.handle_change_view(mergeon_model::workspace_tabs::WorkspaceSection::Changes, cx);
    });
    let focused = std::rc::Rc::new(std::cell::Cell::new(0));
    cx.update(|cx| {
        let focused = focused.clone();
        cx.subscribe(&app.model, move |_, event: &mergeon_app::AppEvent, _| {
            if matches!(event, mergeon_app::AppEvent::FocusCommitSearch) {
                focused.set(focused.get() + 1);
            }
        })
        .detach();
    });

    assert_eq!(
        app.label(cx, "toolbar-search").as_deref(),
        Some("Search commits, authors, hashes…")
    );
    app.click(cx, "toolbar-search");

    assert_eq!(focused.get(), 1);
    assert_eq!(
        app.model.read_with(cx, |model, _| model.active_view()),
        mergeon_model::workspace_tabs::WorkspaceSection::Commits
    );
}

#[gpui_kit::test]
async fn the_status_bar_only_shows_what_is_not_already_on_screen(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    assert!(
        !app.exists(cx, "status-bar"),
        "no repository, no status bar"
    );

    let repo = diverged_repo();
    app.open(&repo, cx).await;

    // El repositorio (pestaña), la rama y su ventaja/retraso (barra) y la
    // versión de git (About) no se repiten.
    for id in [
        "status-repo",
        "status-branch",
        "status-git-version",
        "toolbar-branch-sync",
    ] {
        assert!(!app.exists(cx, id), "`{id}` should not be drawn");
    }
    assert_eq!(
        app.label(cx, "status-upstream").as_deref(),
        Some("origin/main")
    );
    // Nadie ha hablado con el remoto en esta sesión: no se inventa una hora.
    assert!(!app.exists(cx, "status-fetched"));
    // Sin historial que le dé la cáscara, no hay posición.
    assert!(!app.exists(cx, "status-history"));
}

/// La rama de la barra es un botón: pide a la cáscara el selector de ramas.
#[gpui_kit::test]
async fn the_branch_button_asks_for_the_branch_picker(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    app.open(&repo, cx).await;
    let asked = std::rc::Rc::new(std::cell::Cell::new(0));
    let toolbar = app.shell.read_with(cx, |shell, _| shell.toolbar.clone());
    cx.update(|cx| {
        let asked = asked.clone();
        cx.subscribe(
            &toolbar,
            move |_, event: &mergeon_view_toolbar::ToolbarEvent, _| {
                if *event == mergeon_view_toolbar::ToolbarEvent::OpenBranchPicker {
                    asked.set(asked.get() + 1);
                }
            },
        )
        .detach();
    });

    app.click(cx, "toolbar-branch-button");

    assert_eq!(asked.get(), 1);
}

/// La barra en una fila mide 40px.
#[gpui_kit::test]
async fn the_toolbar_is_forty_pixels_tall(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    app.open(&repo, cx).await;
    let toolbar = app.in_window(cx, |window, _| window.find("toolbar").bounds());
    assert_eq!(
        toolbar.size.height,
        mergeon_view_toolbar::TOOLBAR_HEIGHT,
        "{toolbar:?}"
    );
}

#[gpui_kit::test]
async fn the_status_bar_shows_the_history_position_it_is_given(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    app.open(&repo, cx).await;

    app.shell.update(cx, |shell, cx| {
        shell.status_bar.update(cx, |bar, cx| {
            bar.set_history(
                Some(mergeon_view_toolbar::HistoryPosition {
                    selected: Some(27),
                    total: 1284,
                }),
                cx,
            );
        });
    });

    assert_eq!(
        app.label(cx, "status-history").as_deref(),
        Some("27 / 1,284 commits")
    );
}

#[gpui_kit::test]
async fn the_status_bar_remembers_the_last_fetch(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new().with_origin();
    app.open(&repo, cx).await;

    app.click(cx, "toolbar-fetch");
    app.click(cx, "fetch-submit");
    app.wait_for_label(cx, "status-fetched", "fetched just now");

    // Vaciar el log de operaciones no le borra la memoria.
    app.model
        .update(cx, |model, cx| model.clear_operations_log(cx));
    assert_eq!(
        app.label(cx, "status-fetched").as_deref(),
        Some("fetched just now")
    );

    // Un clic vuelve a traer: el mismo diálogo que el botón Fetch.
    app.click(cx, "status-fetched");
    assert_eq!(app.dialogs(cx), ["Fetch"]);
}

// ---------------------------------------------------------------------------
// Deshacer / rehacer
// ---------------------------------------------------------------------------

/// Hace commit de `a.txt` por el modelo (entra en la pila de deshacer).
async fn commit_through_the_app(app: &TestApp, repo: &TestRepo, cx: &mut TestAppContext) {
    repo.write("a.txt", "a\n");
    repo.git(&["add", "a.txt"]);
    let path = repo.path_string();
    let result = app
        .model
        .update(cx, |model, cx| {
            model.commit(
                &path,
                "Fix login",
                mergeon_backend::CommitOptions::default(),
                true,
                cx,
            )
        })
        .await;
    assert!(result.success(), "{}", result.message());
    cx.run_until_parked();
}

#[gpui_kit::test]
async fn undo_and_redo_buttons_follow_the_history_of_the_repository(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    app.open(&repo, cx).await;
    assert!(app.exists(cx, "toolbar-undo"));
    assert!(app.exists(cx, "toolbar-redo"));

    // Sin nada que deshacer están apagados: pulsarlos no hace nada.
    app.click(cx, "toolbar-undo");
    app.click(cx, "toolbar-redo");
    cx.run_until_parked();
    assert!(app.host.toasts().is_empty(), "{:?}", app.host.toasts());

    let before = repo.head();
    commit_through_the_app(&app, &repo, cx).await;
    let after = repo.head();
    let path = repo.path_string();
    app.wait_for(cx, "la entrada", |model| {
        model.undo_action_label(&path, false).as_deref() == Some("Undo commit 'Fix login'")
    });

    app.click(cx, "toolbar-undo");
    app.wait_until(cx, "el deshacer", || repo.head() == before);
    app.wait_for(cx, "el aviso", |_| {
        app.host
            .has_message(NotifyType::Success, "Undid commit 'Fix login'.")
    });

    app.click(cx, "toolbar-redo");
    app.wait_until(cx, "el rehacer", || repo.head() == after);
}

#[gpui_kit::test]
async fn the_undo_buttons_are_off_during_a_sequence_operation(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    app.open(&repo, cx).await;
    commit_through_the_app(&app, &repo, cx).await;
    let head = repo.head();
    // Un merge a medias desde la terminal.
    let repo = repo.with_merge_conflict();
    let merging = repo.head();
    app.model
        .update(cx, |model, cx| {
            let id = model.active_repository().unwrap().id.clone();
            model.sync_repository_read_model(
                id.as_str(),
                mergeon_app::SyncRequest::new().status(true),
                cx,
            )
        })
        .await;
    app.wait_for(cx, "el merge", |model| !model.can_undo(false));

    app.click(cx, "toolbar-undo");
    cx.run_until_parked();
    assert_eq!(repo.head(), merging);
    assert_ne!(repo.head(), head);
    assert!(
        app.host.messages(NotifyType::Info).is_empty(),
        "{:?}",
        app.host.messages(NotifyType::Info)
    );
}
