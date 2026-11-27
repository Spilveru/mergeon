//! Tests de humo de los flujos de `App.vue` que `App.test.js` no cubría:
//! operación en curso, cancelación, auto-fetch, eventos de operación, push sin
//! remoto, navegación, atajos y recarga.

use std::time::Duration;

use gpui_kit::TestAppContext;
use mergeon_backend::{BackendEvent, OperationEvent, OperationPhase, OperationSummary};
use mergeon_model::keyboard_shortcuts::ShortcutAction;
use mergeon_model::notify::{NotifyType, ToastAction};
use mergeon_model::repositories::RepoLifecycleState;
use mergeon_model::workspace_tabs::{CHANGES_TAB, FileTabDescriptor, WorkspaceSection};
use serde_json::json;

use super::support::{TestApp, TestRepo};
use crate::{
    ActionDefaults, ActionDialogMode, ActionDialogs, AppEvent, AppModel, CHOOSE_EDITOR_ACTION,
    DialogRequest, FetchActionDefaults, FetchRequest, PullActionDefaults, PullRequest,
    PushActionDefaults, PushRequest, RemoteAction, WindowRequest,
};

fn dialogs(app: &TestApp) -> Vec<DialogRequest> {
    app.events()
        .into_iter()
        .filter_map(|event| match event {
            AppEvent::ShowDialog(request) => Some(request),
            _ => None,
        })
        .collect()
}

/// Repositorio a medio merge, con un conflicto en README.
fn conflicted_repository() -> TestRepo {
    let repo = TestRepo::new();
    repo.git(&["checkout", "-b", "feature"]);
    repo.commit_file("README.md", "# Feature\n", "Feature README");
    repo.git(&["checkout", "main"]);
    repo.commit_file("README.md", "# Main\n", "Main README");
    // `git merge` sale con error al quedar en conflicto: es lo que se busca.
    let _ = mergeon_gitcmd::Git::new(&repo.path).raw(["merge", "feature"]);
    repo
}

#[gpui_kit::test]
async fn aborting_the_operation_in_progress_asks_first_and_resyncs(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = conflicted_repository();
    let repo_id = app.open(&repo, cx).await;
    app.model.read_with(cx, |model, _| {
        let repository = model.active_repository().unwrap();
        assert_eq!(
            repository.repo_lifecycle_state,
            RepoLifecycleState::Conflicted
        );
        assert!(
            model
                .repositories()
                .active_repository_should_show_operation_banner()
        );
    });

    // Sin confirmar no se toca nada.
    app.host.answer_confirm(false);
    app.model
        .update(cx, |model, cx| model.handle_sequence_action("abort", cx))
        .await;
    let confirms = app.host.confirms();
    assert_eq!(confirms[0].title.as_deref(), Some("Abort operation"));
    assert_eq!(
        confirms[0].description.as_deref(),
        Some("Abort the current merge operation?")
    );
    assert_eq!(confirms[0].variant.as_deref(), Some("destructive"));
    assert!(repo.path.join(".git/MERGE_HEAD").exists());

    app.host.answer_confirm(true);
    app.clear_events();
    app.model
        .update(cx, |model, cx| model.handle_sequence_action("abort", cx))
        .await;

    assert!(!repo.path.join(".git/MERGE_HEAD").exists());
    app.model.read_with(cx, |model, _| {
        let repository = model.repository(repo_id.as_str()).unwrap();
        assert_eq!(repository.repo_lifecycle_state, RepoLifecycleState::Idle);
        assert_eq!(repository.changes_count, 0);
        assert_eq!(model.busy_sequence_action(), None);
        assert!(
            !model.is_single_flight_running(&AppModel::get_repo_operation_key(
                &repo_id,
                "sequence-action:abort"
            ))
        );
    });
    assert!(!app.recorded_mutations().is_empty());
    assert_eq!(app.host.messages(NotifyType::Success).len(), 1);
}

#[gpui_kit::test]
async fn continuing_with_conflicts_left_warns_and_keeps_the_operation(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = conflicted_repository();
    app.open(&repo, cx).await;

    app.model
        .update(cx, |model, cx| model.handle_sequence_action("continue", cx))
        .await;

    // Continuar no pide confirmación.
    assert!(app.host.confirms().is_empty());
    assert!(repo.path.join(".git/MERGE_HEAD").exists());
    assert_eq!(app.host.messages(NotifyType::Warning).len(), 1);
    app.model.read_with(cx, |model, _| {
        assert_eq!(
            model.active_repository().unwrap().repo_lifecycle_state,
            RepoLifecycleState::Conflicted
        );
    });
}

#[gpui_kit::test]
async fn cancelling_an_operation_that_is_gone_warns(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);

    app.model
        .update(cx, |model, cx| {
            model.handle_cancel_operation("stage-does-not-exist", cx)
        })
        .await;

    assert_eq!(
        app.host.messages(NotifyType::Warning),
        ["This Git operation is no longer cancellable."]
    );
}

#[gpui_kit::test]
async fn operation_events_feed_the_log_and_the_queue_of_their_repository(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    let repo_id = app.open(&repo, cx).await;
    app.clear_events();
    let event = |phase: OperationPhase| {
        BackendEvent::Operation(OperationEvent {
            operation_id: "fetch-1".to_string(),
            repo_path: repo_id.clone(),
            operation: "fetch".to_string(),
            phase,
            position: 0,
            queue_length: 1,
            active_operation_id: Some("fetch-1".to_string()),
            operations: if phase == OperationPhase::Running {
                vec![OperationSummary {
                    operation_id: Some("fetch-1".to_string()),
                    operation: "fetch".to_string(),
                    phase,
                    position: 0,
                    cancellable: true,
                    started_at: Some("2026-01-01T00:00:01.000Z".to_string()),
                    queued_at: "2026-01-01T00:00:00.000Z".to_string(),
                }]
            } else {
                Vec::new()
            },
            error: None,
        })
    };

    app.model.update(cx, |model, cx| {
        model.handle_backend_event(event(OperationPhase::Running), cx);
    });
    app.model.read_with(cx, |model, _| {
        let queue = model.active_repo_mutation_queue();
        assert_eq!(
            queue.active.and_then(|operation| operation.operation_id),
            Some("fetch-1".to_string())
        );
        assert_eq!(model.operations_log().entries[0].operation_id, "fetch-1");
        assert!(model.operations_log().entries[0].started_at.is_some());
    });

    // Una operación cancelada deja el árbol en un estado que hay que releer.
    app.model.update(cx, |model, cx| {
        model.handle_backend_event(event(OperationPhase::Cancelled), cx);
    });
    app.model.read_with(cx, |model, _| {
        assert_eq!(model.active_repo_mutation_queue().active, None);
        assert_eq!(model.operations_log().unseen_errors, 1);
        assert_eq!(
            model.operations_log().entries[0].outcome.as_deref(),
            Some("cancelled")
        );
    });
    assert!(app.recorded_mutations().is_empty());
    cx.executor().advance_clock(Duration::from_millis(250));
    cx.run_until_parked();
    assert_eq!(app.recorded_mutations(), ["status-watch"]);

    app.model.update(cx, |model, cx| {
        model.mark_operations_seen(cx);
        assert_eq!(model.operations_log().unseen_errors, 0);
        model.clear_operations_log(cx);
        assert!(model.operations_log().entries.is_empty());
    });
}

#[gpui_kit::test]
async fn auto_fetch_is_silent_but_leaves_its_result_in_the_operations_log(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    let origin = repo.publish_to_origin();
    repo.git(&["fetch", "origin"]);
    let repo_id = app.open(&repo, cx).await;
    app.clear_events();
    app.model.read_with(cx, |model, _| {
        assert_eq!(model.default_remote_name(), "origin");
        assert_eq!(model.default_remote_branch_name(), "main");
    });

    // Sin la ventana enfocada no se hace nada.
    app.model
        .update(cx, |model, cx| model.set_window_focused(false, cx));
    app.model
        .update(cx, |model, cx| model.run_auto_fetch(cx))
        .await;
    app.model.read_with(cx, |model, _| {
        assert!(model.operations_log().entries.is_empty())
    });

    // Al volver a la app el auto-fetch está vencido: se pone al día.
    app.model
        .update(cx, |model, cx| model.set_window_focused(true, cx));
    app.wait_for(cx, "the overdue auto-fetch to be logged", |model| {
        model
            .operations_log()
            .entries
            .iter()
            .any(|entry| entry.operation.as_deref() == Some("fetch") && entry.outcome.is_some())
    });
    assert!(app.host.toasts().is_empty());
    // No trajo nada: ni mutación ni relectura.
    assert!(app.recorded_mutations().is_empty());

    // El periódico (lo programa `start`): cuando hay algo nuevo, se relee
    // sin avisos.
    app.model
        .update(cx, |model, cx| model.schedule_auto_fetch(cx));
    let colleague = repo.colleague(&origin);
    colleague.commit_and_push("remote.txt", "remote\n", "Remote change");
    cx.executor().advance_clock(Duration::from_secs(10 * 60));
    app.wait_for(
        cx,
        "the periodic auto-fetch to bring the new commit",
        |model| {
            model
                .repository(repo_id.as_str())
                .and_then(|repository| repository.tracking_status.as_ref())
                .is_some_and(|tracking| tracking.behind == 1)
        },
    );
    assert!(app.host.toasts().is_empty());
    assert_eq!(app.recorded_mutations(), ["fetch"]);

    // Apagado en los ajustes, deja de programarse.
    app.model
        .update(cx, |model, cx| {
            model.save_auto_fetch_settings(false, 10.0, cx)
        })
        .await;
    app.clear_events();
    colleague.commit_and_push("more.txt", "more\n", "More remote changes");
    cx.executor().advance_clock(Duration::from_secs(30 * 60));
    cx.run_until_parked();
    assert!(app.recorded_mutations().is_empty());
}

#[gpui_kit::test]
async fn pushing_without_a_remote_asks_for_its_url_and_retries(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    let remote = repo.root().join("origin.git");
    std::fs::create_dir(&remote).unwrap();
    super::support::git_in(&remote, &["init", "--bare", "-b", "main"]);
    app.open(&repo, cx).await;
    let request = || PushRequest {
        branch: "main".to_string(),
        destination: "origin/main".to_string(),
        push_tags: false,
        force: false,
    };

    // Cancelar el prompt deja el push sin hacer.
    app.host.cancel_prompt();
    app.model
        .update(cx, |model, cx| model.handle_push_submit(request(), cx))
        .await;
    let prompts = app.host.prompts();
    assert_eq!(prompts.len(), 1);
    assert_eq!(prompts[0].title.as_deref(), Some("Configure remote"));
    assert_eq!(prompts[0].pattern.as_deref(), Some("^https?://.+"));
    assert_eq!(repo.git(&["remote"]).trim(), "");

    // Con una URL se añade `origin` (sin la barra final) y se reintenta.
    app.host
        .answer_prompt(" https://example.invalid/demo/repo.git/ ");
    app.model
        .update(cx, |model, cx| model.handle_push_submit(request(), cx))
        .await;
    assert_eq!(
        repo.git(&["remote", "get-url", "origin"]).trim(),
        "https://example.invalid/demo/repo.git"
    );

    // Un destino vacío no hace nada.
    app.host.clear_toasts();
    app.model
        .update(cx, |model, cx| {
            model.handle_push_submit(
                PushRequest {
                    destination: String::new(),
                    ..request()
                },
                cx,
            )
        })
        .await;
    assert!(app.host.toasts().is_empty());
}

#[gpui_kit::test]
async fn navigation_between_home_repositories_and_settings(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    let repo_id = app.open(&repo, cx).await;

    app.model.update(cx, |model, cx| {
        model.open_settings("help", cx);
        assert!(model.show_preferences() && model.settings_tab_open());
        assert_eq!(model.settings_section(), "help");

        // Cambiar de vista saca de Settings sin cerrar su pestaña.
        model.handle_change_view(WorkspaceSection::Changes, cx);
        assert!(!model.show_preferences() && model.settings_tab_open());
        assert_eq!(model.active_view(), WorkspaceSection::Changes);
        assert_eq!(model.repository_workspace(&repo_id).active_tab, CHANGES_TAB);

        model.close_settings_tab(cx);
        assert!(!model.settings_tab_open());

        model.show_home_tab(cx);
        assert!(model.active_repository().is_none());
        model.select_repository_tab(0, cx);
        assert_eq!(model.active_repository_index(), Some(0));
        // Al volver al repositorio se recupera la vista que tenía.
        assert_eq!(model.active_view(), WorkspaceSection::Changes);
    });
    cx.run_until_parked();

    let events = app.events();
    assert!(events.contains(&AppEvent::OpenSettings {
        section: "help".to_string()
    }));
    assert!(events.contains(&AppEvent::ViewChanged {
        view: WorkspaceSection::Changes
    }));
}

#[gpui_kit::test]
async fn shortcuts_dispatch_the_same_actions_as_the_original(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);

    // Sin repositorio activo, los atajos de repositorio no hacen nada.
    app.model.update(cx, |model, cx| {
        for action in [
            ShortcutAction::Fetch,
            ShortcutAction::Pull,
            ShortcutAction::Push,
            ShortcutAction::NewBranch,
            ShortcutAction::CloseTab,
        ] {
            model.handle_shortcut(action, cx);
        }
    });
    cx.run_until_parked();
    assert!(app.events().is_empty());

    let repo = TestRepo::new();
    app.open(&repo, cx).await;
    app.clear_events();
    app.model.update(cx, |model, cx| {
        for action in [
            ShortcutAction::OpenRepository,
            ShortcutAction::Fetch,
            ShortcutAction::Pull,
            ShortcutAction::Push,
            ShortcutAction::ShowOperationsLog,
            ShortcutAction::ShowReflog,
            ShortcutAction::SearchCommits,
            ShortcutAction::OpenPreferences,
            ShortcutAction::MinimizeWindow,
            ShortcutAction::ToggleDevTools,
            ShortcutAction::Quit,
        ] {
            model.handle_shortcut(action, cx);
        }
    });
    cx.run_until_parked();

    assert_eq!(
        dialogs(&app),
        [
            DialogRequest::Launcher,
            DialogRequest::Fetch,
            DialogRequest::Pull,
            DialogRequest::Push,
            DialogRequest::OperationsLog,
            DialogRequest::Reflog,
        ]
    );
    let events = app.events();
    assert!(events.contains(&AppEvent::FocusCommitSearch));
    assert!(events.contains(&AppEvent::OpenSettings {
        section: "appearance".to_string()
    }));
    for request in [
        WindowRequest::Minimize,
        WindowRequest::ToggleDevTools,
        WindowRequest::Quit,
    ] {
        assert!(events.contains(&AppEvent::Window(request)));
    }
}

#[gpui_kit::test]
async fn cloning_from_github_needs_an_account_first(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);

    app.host.answer_confirm(false);
    app.model
        .update(cx, |model, cx| model.open_github_picker(cx))
        .await;
    assert_eq!(
        app.host.confirms()[0].title.as_deref(),
        Some("No GitHub accounts")
    );
    assert!(dialogs(&app).is_empty());

    app.host.answer_confirm(true);
    app.model
        .update(cx, |model, cx| model.open_github_picker(cx))
        .await;
    assert_eq!(dialogs(&app), [DialogRequest::GitHubAccount]);

    // Añadida la cuenta desde ese flujo, se pasa al selector de repositorios.
    app.model
        .update(cx, |model, cx| {
            model.add_account(
                json!({ "id": "1", "username": "octocat", "token": "gho" }),
                cx,
            )
        })
        .await
        .unwrap();
    app.clear_events();
    app.model
        .update(cx, |model, cx| model.handle_account_added(cx));
    assert_eq!(dialogs(&app), [DialogRequest::GitHubRepositoryPicker]);

    // Desde Settings, no.
    app.clear_events();
    app.model.update(cx, |model, cx| {
        model.add_account_from_settings(cx);
        model.handle_account_added(cx);
    });
    assert_eq!(dialogs(&app), [DialogRequest::GitHubAccount]);

    app.clear_events();
    app.model
        .update(cx, |model, cx| model.open_github_picker(cx))
        .await;
    assert_eq!(dialogs(&app), [DialogRequest::GitHubRepositoryPicker]);
}

#[gpui_kit::test]
async fn opens_a_cloned_repository_and_system_tools(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    let directory = TestRepo::plain_directory();

    app.model
        .update(cx, |model, cx| {
            model.handle_repository_cloned(&directory.path_string(), cx)
        })
        .await;
    assert_eq!(
        app.host.messages(NotifyType::Error),
        ["The cloned directory is not a valid Git repository"]
    );

    app.model
        .update(cx, |model, cx| {
            model.handle_repository_cloned(&repo.path_string(), cx)
        })
        .await;
    cx.run_until_parked();
    app.model.read_with(cx, |model, _| {
        assert_eq!(
            model.active_repository().map(|repo| repo.path.clone()),
            Some(repo.id())
        );
        assert!(model.recent_repositories()[0].is_github);
    });

    app.model
        .update(cx, |model, cx| model.handle_open_explorer(cx))
        .await;
    app.model
        .update(cx, |model, cx| model.handle_open_terminal(cx))
        .await;
    let opened = app
        .model
        .update(cx, |model, cx| {
            model.open_external("https://github.com/demo/repo", cx)
        })
        .await;
    assert_eq!(opened, Ok(()));
    let blocked = app
        .model
        .update(cx, |model, cx| {
            model.open_external("file:///etc/passwd", cx)
        })
        .await;
    assert!(blocked.is_err());

    let launched = app.launcher.launched();
    assert!(launched.len() >= 3);
    assert!(
        app.launcher
            .launched_args()
            .contains(&"https://github.com/demo/repo".to_string())
    );
    assert!(launched.iter().any(|process| {
        process.args.contains(&repo.id()) || process.cwd.as_deref() == Some(repo.path.as_path())
    }));
}

// Ctrl+Shift+E: el archivo de la pestaña activa o, sin ella, el repositorio;
// sin editor detectado, con el programa por defecto del sistema.
#[gpui_kit::test]
async fn the_open_in_editor_shortcut_opens_the_active_file_or_the_repository(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.commit_file("docs/read me.md", "hi\n", "Add docs");
    app.open(&repo, cx).await;
    // Como la da el modelo: con `/` también en Windows.
    let root = repo.id();

    app.model.update(cx, |model, cx| {
        model.handle_shortcut(ShortcutAction::OpenInEditor, cx)
    });
    let launcher = app.launcher.clone();
    app.wait_for(cx, "the repository", |_| launcher.launched().len() == 1);
    assert_eq!(app.launcher.launched()[0].command, "xdg-open");
    assert_eq!(app.launcher.launched()[0].args, std::slice::from_ref(&root));

    let descriptor = FileTabDescriptor::working("docs/read me.md");
    app.model.update(cx, |model, cx| {
        model.open_workspace_file(&root, &descriptor, false, cx);
        model.handle_shortcut(ShortcutAction::OpenInEditor, cx);
    });
    app.wait_for(cx, "the file", |_| launcher.launched().len() == 2);
    assert_eq!(
        app.launcher.launched()[1].args,
        [format!("{root}/docs/read me.md")]
    );
    // Sin editor detectado, se dice con qué se abrió y se ofrece elegir uno
    // (un solo aviso: el segundo reemplaza al primero).
    let toasts = app.host.toasts();
    let last = toasts.last().expect("a notice");
    assert_eq!(last.kind, NotifyType::Info);
    assert_eq!(
        last.title,
        "No external editor was found, so it opened with the system default app."
    );
    assert_eq!(
        last.action,
        Some(ToastAction::custom("Choose editor…", CHOOSE_EDITOR_ACTION))
    );
}

/// Si el programa del sistema tampoco puede abrirlo, el aviso lo explica y
/// "Choose editor…" lleva a Ajustes → Git (donde está "External editor").
#[gpui_kit::test]
async fn open_in_editor_without_any_way_to_open_offers_to_choose_an_editor(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    app.open(&repo, cx).await;
    let root = repo.path.to_string_lossy().into_owned();
    app.launcher
        .fail_with(mergeon_backend::LaunchError::not_found("xdg-open"));

    app.model
        .update(cx, |model, cx| model.open_in_editor(&root, None, cx))
        .await;

    let toasts = app.host.toasts();
    let warning = toasts
        .iter()
        .find(|toast| toast.kind == NotifyType::Warning)
        .expect("a warning");
    assert_eq!(
        warning.full_message,
        "No external editor was found, and the system couldn't open the file either (Failed to open path)."
    );
    assert_eq!(
        warning.action,
        Some(ToastAction::custom("Choose editor…", CHOOSE_EDITOR_ACTION))
    );

    app.model.update(cx, |model, cx| {
        assert!(model.run_toast_action(CHOOSE_EDITOR_ACTION, cx));
        assert!(model.show_preferences());
        assert_eq!(model.settings_section(), "git");
    });
}

#[gpui_kit::test]
async fn a_file_that_left_the_working_tree_is_not_opened(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    app.open(&repo, cx).await;
    let root = repo.path.to_string_lossy().into_owned();

    app.model
        .update(cx, |model, cx| {
            model.open_working_tree_file_in_editor(&root, "gone/file.txt", Some(3), cx)
        })
        .await;

    assert!(app.launcher.launched().is_empty());
    assert!(app.host.has_message(
        NotifyType::Warning,
        "gone/file.txt no longer exists in the working tree."
    ));
}

// Antes el botón y Ctrl+R recargaban sin preguntar.
#[gpui_kit::test]
async fn the_reload_button_asks_before_reloading(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    app.open(&repo, cx).await;
    app.clear_events();

    app.host.answer_confirm(false);
    let reloaded = app
        .model
        .update(cx, |model, cx| model.request_reload_application(cx))
        .await;
    cx.run_until_parked();
    assert!(!reloaded);
    assert!(
        !app.events()
            .contains(&AppEvent::Window(WindowRequest::Reloaded))
    );
    let confirms = app.host.confirms();
    assert_eq!(confirms.len(), 1);
    assert_eq!(confirms[0].title.as_deref(), Some("Reload Mergeon?"));

    app.host.answer_confirm(true);
    let reloaded = app
        .model
        .update(cx, |model, cx| model.request_reload_application(cx))
        .await;
    cx.run_until_parked();
    assert!(reloaded);
    assert!(
        app.events()
            .contains(&AppEvent::Window(WindowRequest::Reloaded))
    );
}

#[gpui_kit::test]
async fn reloading_the_application_restores_the_session_from_scratch(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    let repo_id = app.open(&repo, cx).await;
    app.model.update(cx, |model, cx| {
        model.set_workspace_selected_commit(&repo_id, Some("abc123"), cx);
    });
    app.clear_events();

    app.model
        .update(cx, |model, cx| model.reload_application(cx))
        .await;
    cx.run_until_parked();

    assert!(
        app.events()
            .contains(&AppEvent::Window(WindowRequest::Reloaded))
    );
    app.model.read_with(cx, |model, _| {
        // La sesión guardada vuelve a abrir el repositorio, con sus pestañas.
        assert_eq!(model.open_repositories().len(), 1);
        assert_eq!(
            model.active_repository().map(|repo| repo.id.clone()),
            Some(repo_id.clone())
        );
        assert_eq!(
            model
                .repository_workspace(&repo_id)
                .selected_commit
                .as_deref(),
            Some("abc123")
        );
        assert!(model.active_repository().unwrap().status.is_some());
        assert_eq!(model.active_status_watch_path(), Some(repo_id.as_str()));
    });
}

#[gpui_kit::test]
async fn direct_fetch_pull_push_execute_without_dialogs_and_fall_back_when_unsafe(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    app.model
        .update(cx, |model, cx| model.set_skip_action_dialogs(true, cx))
        .await;
    assert!(
        app.model
            .read_with(cx, |model, _| model.skip_action_dialogs())
    );
    assert!(app.backend.storage().get_settings().skip_action_dialogs);

    // 1. Repository with NO remotes: Fetch, Pull, and Push all fall back to dialogs.
    let no_remote = TestRepo::new();
    app.open(&no_remote, cx).await;
    app.clear_events();
    app.model.update(cx, |model, cx| {
        model.handle_shortcut(ShortcutAction::Fetch, cx);
        model.handle_shortcut(ShortcutAction::Pull, cx);
        model.handle_shortcut(ShortcutAction::Push, cx);
    });
    cx.run_until_parked();
    assert_eq!(
        dialogs(&app),
        [
            DialogRequest::Fetch,
            DialogRequest::Pull,
            DialogRequest::Push
        ]
    );

    // 2. Repository WITH origin and upstream: Fetch, Pull, and Push all execute directly.
    let synced = TestRepo::new();
    let origin = synced.add_bare_remote("origin");
    synced.git(&["push", "-q", "-u", "origin", "main"]);
    synced.commit_file("ahead.txt", "local\n", "Local commit");
    app.open(&synced, cx).await;
    app.clear_events();

    app.model.update(cx, |model, cx| {
        model.handle_shortcut(ShortcutAction::Fetch, cx)
    });
    app.wait_for(cx, "direct fetch recorded", |model| {
        model
            .operations_log()
            .entries
            .iter()
            .any(|entry| entry.operation.as_deref() == Some("fetch"))
    });
    assert!(dialogs(&app).is_empty());

    let local_head = synced.head();
    app.model.update(cx, |model, cx| {
        model.handle_shortcut(ShortcutAction::Push, cx)
    });
    app.wait_for(cx, "direct push updated origin", |_| {
        super::support::git_in(&origin, &["rev-parse", "main"]).trim() == local_head
    });
    assert!(dialogs(&app).is_empty());

    app.model.update(cx, |model, cx| {
        model.handle_shortcut(ShortcutAction::Pull, cx)
    });
    app.wait_for(cx, "direct pull recorded", |model| {
        model
            .operations_log()
            .entries
            .iter()
            .any(|entry| entry.operation.as_deref() == Some("pull"))
    });
    assert!(dialogs(&app).is_empty());

    // 3. Local-only branch without upstream or matching remote branch:
    //    Pull and Push fall back to dialogs, while Fetch still runs directly.
    synced.git(&["checkout", "-q", "-b", "topic"]);
    let repo_id = app
        .model
        .read_with(cx, |model, _| model.active_repository().unwrap().id.clone());
    app.model
        .update(cx, |model, cx| {
            model.sync_repository_read_model(
                repo_id.as_str(),
                crate::SyncRequest::new()
                    .status(true)
                    .branches(true)
                    .remotes(true),
                cx,
            )
        })
        .await;
    cx.run_until_parked();
    app.clear_events();

    app.model.update(cx, |model, cx| {
        model.handle_shortcut(ShortcutAction::Pull, cx);
        model.handle_shortcut(ShortcutAction::Push, cx);
    });
    cx.run_until_parked();
    assert_eq!(dialogs(&app), [DialogRequest::Pull, DialogRequest::Push]);

    // 4. Detached HEAD: Pull and Push fall back to dialogs.
    synced.git(&["checkout", "-q", "--detach"]);
    app.model
        .update(cx, |model, cx| {
            model.sync_repository_read_model(
                repo_id.as_str(),
                crate::SyncRequest::new()
                    .status(true)
                    .branches(true)
                    .remotes(true),
                cx,
            )
        })
        .await;
    cx.run_until_parked();
    app.clear_events();

    app.model.update(cx, |model, cx| {
        model.handle_shortcut(ShortcutAction::Pull, cx);
        model.handle_shortcut(ShortcutAction::Push, cx);
    });
    cx.run_until_parked();
    assert_eq!(dialogs(&app), [DialogRequest::Pull, DialogRequest::Push]);
}

#[gpui_kit::test]
async fn custom_keyboard_shortcuts_persist_emit_events_and_reset(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    app.clear_events();

    // OpenShortcutsHelp opens Settings → Shortcuts.
    app.model.update(cx, |model, cx| {
        model.handle_shortcut(ShortcutAction::OpenShortcutsHelp, cx);
    });
    assert!(app.events().contains(&AppEvent::OpenSettings {
        section: "shortcuts".to_string()
    }));

    app.clear_events();
    app.model
        .update(cx, |model, cx| {
            model.set_keyboard_shortcut(ShortcutAction::Fetch, "ctrl+shift+down", cx)
        })
        .await;
    assert!(app.events().contains(&AppEvent::ShortcutsChanged));
    assert_eq!(
        app.model.read_with(cx, |model, _| model
            .keyboard_shortcuts()
            .get("fetch")
            .cloned()),
        Some("ctrl+shift+down".to_string())
    );
    assert_eq!(
        app.backend
            .storage()
            .get_settings()
            .keyboard_shortcuts
            .get("fetch")
            .map(String::as_str),
        Some("ctrl+shift+down")
    );

    // Unbind Pull with empty string.
    app.model
        .update(cx, |model, cx| {
            model.set_keyboard_shortcut(ShortcutAction::Pull, "", cx)
        })
        .await;
    assert_eq!(
        app.model.read_with(cx, |model, _| model
            .keyboard_shortcuts()
            .get("pull")
            .cloned()),
        Some(String::new())
    );

    // Reset single action.
    app.model
        .update(cx, |model, cx| {
            model.reset_keyboard_shortcut(ShortcutAction::Fetch, cx)
        })
        .await;
    assert!(!app.model.read_with(cx, |model, _| {
        model.keyboard_shortcuts().contains_key("fetch")
    }));
    assert!(app.model.read_with(cx, |model, _| {
        model.keyboard_shortcuts().contains_key("pull")
    }));

    // Reset all omits the key from persisted settings.
    app.model
        .update(cx, |model, cx| model.reset_all_keyboard_shortcuts(cx))
        .await;
    assert!(
        app.model
            .read_with(cx, |model, _| model.keyboard_shortcuts().is_empty())
    );
    assert!(
        app.backend
            .storage()
            .get_settings()
            .keyboard_shortcuts
            .is_empty()
    );
}

#[gpui_kit::test]
async fn keyboard_shortcuts_run_fetch_pull_push_directly_even_when_skip_action_dialogs_is_false(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    assert!(
        !app.model
            .read_with(cx, |model, _| model.skip_action_dialogs())
    );

    // 1. Repository WITH origin and upstream:
    //    Toolbar/palette paths (`handle_fetch`/`handle_shortcut`) open dialogs when skip is false.
    let synced = TestRepo::new();
    let origin = synced.add_bare_remote("origin");
    synced.git(&["push", "-q", "-u", "origin", "main"]);
    synced.commit_file("ahead.txt", "local\n", "Local commit");
    app.open(&synced, cx).await;
    app.clear_events();

    app.model.update(cx, |model, cx| {
        model.handle_fetch(cx);
        model.handle_pull(cx);
        model.handle_push(cx);
        model.handle_shortcut(ShortcutAction::Fetch, cx);
    });
    cx.run_until_parked();
    assert_eq!(
        dialogs(&app),
        [
            DialogRequest::Fetch,
            DialogRequest::Pull,
            DialogRequest::Push,
            DialogRequest::Fetch,
        ]
    );

    // 2. Keyboard shortcuts (`handle_keyboard_shortcut`) execute directly even with skip == false.
    app.clear_events();
    app.model.update(cx, |model, cx| {
        model.handle_keyboard_shortcut(ShortcutAction::Fetch, cx);
    });
    app.wait_for(cx, "direct keyboard fetch recorded", |model| {
        model
            .operations_log()
            .entries
            .iter()
            .any(|entry| entry.operation.as_deref() == Some("fetch"))
    });
    assert!(dialogs(&app).is_empty());

    let local_head = synced.head();
    app.model.update(cx, |model, cx| {
        model.handle_keyboard_shortcut(ShortcutAction::Push, cx);
    });
    app.wait_for(cx, "direct keyboard push updated origin", |_| {
        super::support::git_in(&origin, &["rev-parse", "main"]).trim() == local_head
    });
    assert!(dialogs(&app).is_empty());

    app.model.update(cx, |model, cx| {
        model.handle_keyboard_shortcut(ShortcutAction::Pull, cx);
    });
    app.wait_for(cx, "direct keyboard pull recorded", |model| {
        model
            .operations_log()
            .entries
            .iter()
            .any(|entry| entry.operation.as_deref() == Some("pull"))
    });
    assert!(dialogs(&app).is_empty());

    // 3. Unsafe targets still fall back to dialogs on keyboard shortcuts:
    //    no remotes, branch without upstream, and detached HEAD.
    let no_remote = TestRepo::new();
    app.open(&no_remote, cx).await;
    app.clear_events();
    app.model.update(cx, |model, cx| {
        model.handle_keyboard_shortcut(ShortcutAction::Fetch, cx);
        model.handle_keyboard_shortcut(ShortcutAction::Pull, cx);
        model.handle_keyboard_shortcut(ShortcutAction::Push, cx);
    });
    cx.run_until_parked();
    assert_eq!(
        dialogs(&app),
        [
            DialogRequest::Fetch,
            DialogRequest::Pull,
            DialogRequest::Push,
        ]
    );

    app.model
        .update(cx, |model, cx| model.select_repository_tab(0, cx));
    cx.run_until_parked();
    let repo_id = app
        .model
        .read_with(cx, |model, _| model.active_repository().unwrap().id.clone());

    synced.git(&["checkout", "-q", "-b", "topic"]);
    app.model
        .update(cx, |model, cx| {
            model.sync_repository_read_model(
                repo_id.as_str(),
                crate::SyncRequest::new()
                    .status(true)
                    .branches(true)
                    .remotes(true),
                cx,
            )
        })
        .await;
    cx.run_until_parked();
    app.clear_events();
    app.model.update(cx, |model, cx| {
        model.handle_keyboard_shortcut(ShortcutAction::Pull, cx);
        model.handle_keyboard_shortcut(ShortcutAction::Push, cx);
    });
    cx.run_until_parked();
    assert_eq!(dialogs(&app), [DialogRequest::Pull, DialogRequest::Push]);

    synced.git(&["checkout", "-q", "--detach"]);
    app.model
        .update(cx, |model, cx| {
            model.sync_repository_read_model(
                repo_id.as_str(),
                crate::SyncRequest::new()
                    .status(true)
                    .branches(true)
                    .remotes(true),
                cx,
            )
        })
        .await;
    cx.run_until_parked();
    app.clear_events();
    app.model.update(cx, |model, cx| {
        model.handle_keyboard_shortcut(ShortcutAction::Pull, cx);
        model.handle_keyboard_shortcut(ShortcutAction::Push, cx);
    });
    cx.run_until_parked();
    assert_eq!(dialogs(&app), [DialogRequest::Pull, DialogRequest::Push]);
}

#[gpui_kit::test]
async fn remembered_action_defaults_persist_and_feed_direct_requests(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    let _origin = repo.add_bare_remote("origin");
    repo.git(&["push", "-q", "-u", "origin", "main"]);
    app.open(&repo, cx).await;

    // Initial defaults: all_remotes = true, rebase/autostash/push_tags/force = false.
    app.model.read_with(cx, |model, _| {
        assert_eq!(model.action_defaults(), ActionDefaults::default());
        assert_eq!(
            model.direct_fetch_request(),
            Some(FetchRequest {
                remote: "origin".to_string(),
                all_remotes: true,
            })
        );
        assert_eq!(
            model.direct_pull_request(),
            Some(PullRequest {
                remote: "origin".to_string(),
                branch: Some("main".to_string()),
                rebase: false,
                autostash: false,
            })
        );
        assert_eq!(
            model.direct_push_request(),
            Some(PushRequest {
                branch: "main".to_string(),
                destination: "origin/main".to_string(),
                push_tags: false,
                force: false,
            })
        );
    });

    // Remember non-default options and verify persistence + direct request propagation.
    app.model
        .update(cx, |model, cx| model.remember_fetch_defaults(false, cx))
        .await;
    app.model
        .update(cx, |model, cx| model.remember_pull_defaults(true, true, cx))
        .await;
    app.model
        .update(cx, |model, cx| model.remember_push_defaults(true, cx))
        .await;

    let expected = ActionDefaults {
        fetch: FetchActionDefaults { all_remotes: false },
        pull: PullActionDefaults {
            rebase: true,
            autostash: true,
        },
        push: PushActionDefaults { push_tags: true },
    };
    assert_eq!(
        app.model.read_with(cx, |model, _| model.action_defaults()),
        expected
    );
    assert_eq!(
        app.backend.storage().get_settings().action_defaults,
        expected
    );

    // Force reload from storage to confirm `load_app_settings` restores `action_defaults`.
    app.model
        .update(cx, |model, cx| model.load_app_settings(true, cx))
        .await;

    app.model.read_with(cx, |model, _| {
        assert_eq!(model.action_defaults(), expected);
        assert_eq!(
            model.direct_fetch_request(),
            Some(FetchRequest {
                remote: "origin".to_string(),
                all_remotes: false,
            })
        );
        assert_eq!(
            model.direct_pull_request(),
            Some(PullRequest {
                remote: "origin".to_string(),
                branch: Some("main".to_string()),
                rebase: true,
                autostash: true,
            })
        );
        assert_eq!(
            model.direct_push_request(),
            Some(PushRequest {
                branch: "main".to_string(),
                destination: "origin/main".to_string(),
                push_tags: true,
                force: false,
            })
        );
    });
}

#[gpui_kit::test]
async fn per_action_dialog_modes_control_toolbar_and_keyboard_independently(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let synced = TestRepo::new();
    let origin = synced.add_bare_remote("origin");
    synced.git(&["push", "-q", "-u", "origin", "main"]);
    synced.commit_file("ahead.txt", "local\n", "Local commit");
    app.open(&synced, cx).await;

    // Set Push to Always while Fetch and Pull stay at Toolbar.
    app.model
        .update(cx, |model, cx| {
            model.set_action_dialog_mode(RemoteAction::Push, ActionDialogMode::Always, cx)
        })
        .await;
    app.model.read_with(cx, |model, _| {
        assert!(!model.skip_action_dialogs());
        assert_eq!(
            model.action_dialog_mode(RemoteAction::Fetch),
            ActionDialogMode::Toolbar
        );
        assert_eq!(
            model.action_dialog_mode(RemoteAction::Pull),
            ActionDialogMode::Toolbar
        );
        assert_eq!(
            model.action_dialog_mode(RemoteAction::Push),
            ActionDialogMode::Always
        );
    });
    let stored = app.backend.storage().get_settings();
    assert!(!stored.skip_action_dialogs);
    assert_eq!(
        stored.action_dialogs,
        ActionDialogs {
            fetch: None,
            pull: None,
            push: Some(ActionDialogMode::Always),
        }
    );

    // Keyboard Push opens DialogRequest::Push (Always), while keyboard Fetch & Pull run directly (Toolbar).
    app.clear_events();
    app.model.update(cx, |model, cx| {
        model.handle_keyboard_shortcut(ShortcutAction::Push, cx);
    });
    cx.run_until_parked();
    assert_eq!(dialogs(&app), [DialogRequest::Push]);
    assert_ne!(
        super::support::git_in(&origin, &["rev-parse", "main"]).trim(),
        synced.head()
    );

    app.clear_events();
    app.model.update(cx, |model, cx| {
        model.handle_keyboard_shortcut(ShortcutAction::Fetch, cx);
    });
    app.wait_for(cx, "direct keyboard fetch recorded", |model| {
        model
            .operations_log()
            .entries
            .iter()
            .any(|entry| entry.operation.as_deref() == Some("fetch"))
    });
    assert!(dialogs(&app).is_empty());

    app.model.update(cx, |model, cx| {
        model.handle_keyboard_shortcut(ShortcutAction::Pull, cx);
    });
    app.wait_for(cx, "direct keyboard pull recorded", |model| {
        model
            .operations_log()
            .entries
            .iter()
            .any(|entry| entry.operation.as_deref() == Some("pull"))
    });
    assert!(dialogs(&app).is_empty());

    // Setting Fetch and Pull to Never with Push = Always canonicalizes to skipActionDialogs: true + push: always.
    app.model
        .update(cx, |model, cx| {
            model.set_action_dialog_mode(RemoteAction::Fetch, ActionDialogMode::Never, cx)
        })
        .await;
    app.model
        .update(cx, |model, cx| {
            model.set_action_dialog_mode(RemoteAction::Pull, ActionDialogMode::Never, cx)
        })
        .await;
    let stored = app.backend.storage().get_settings();
    assert!(stored.skip_action_dialogs);
    assert_eq!(
        stored.action_dialogs,
        ActionDialogs {
            fetch: None,
            pull: None,
            push: Some(ActionDialogMode::Always),
        }
    );
    assert!(
        !app.model
            .read_with(cx, |model, _| model.skip_action_dialogs())
    );

    // Reloading settings restores per-action modes.
    app.model
        .update(cx, |model, cx| model.load_app_settings(true, cx))
        .await;
    app.model.read_with(cx, |model, _| {
        assert_eq!(
            model.action_dialog_mode(RemoteAction::Fetch),
            ActionDialogMode::Never
        );
        assert_eq!(
            model.action_dialog_mode(RemoteAction::Pull),
            ActionDialogMode::Never
        );
        assert_eq!(
            model.action_dialog_mode(RemoteAction::Push),
            ActionDialogMode::Always
        );
    });

    // set_skip_action_dialogs(true) sets all 3 to Never and removes actionDialogs.
    app.model
        .update(cx, |model, cx| model.set_skip_action_dialogs(true, cx))
        .await;
    let stored = app.backend.storage().get_settings();
    assert!(stored.skip_action_dialogs);
    assert!(stored.action_dialogs.is_empty());
    assert!(
        app.model
            .read_with(cx, |model, _| model.skip_action_dialogs())
    );

    // set_skip_action_dialogs(false) resets all 3 to Toolbar and removes both keys.
    app.model
        .update(cx, |model, cx| model.set_skip_action_dialogs(false, cx))
        .await;
    let stored = app.backend.storage().get_settings();
    assert!(!stored.skip_action_dialogs);
    assert!(stored.action_dialogs.is_empty());
}

#[gpui_kit::test]
async fn remotes_containing_slashes_resolve_in_defaults_pull_and_push(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    let bare = repo.root().join("team-origin.git");
    std::fs::create_dir(&bare).unwrap();
    super::support::git_in(&bare, &["init", "--bare", "-b", "main"]);
    repo.git(&["remote", "add", "team/origin", &bare.to_string_lossy()]);
    repo.git(&["checkout", "-q", "-b", "feature/foo"]);
    repo.git(&["push", "-q", "-u", "team/origin", "feature/foo"]);
    let new_head = repo.commit_file("slash.txt", "ok\n", "Commit on slashed remote");
    app.open(&repo, cx).await;

    app.model.read_with(cx, |model, _| {
        let remotes = model.repositories().active_repository_remotes();
        let team_origin = remotes
            .iter()
            .find(|remote| remote.name == "team/origin")
            .expect("team/origin remote");
        assert!(
            team_origin.branches.iter().any(|b| b == "feature/foo"),
            "{:?}",
            team_origin.branches
        );
        assert_eq!(model.default_remote_name(), "team/origin");
        assert_eq!(model.default_remote_branch_name(), "feature/foo");
        assert_eq!(
            model.direct_pull_request(),
            Some(PullRequest {
                remote: "team/origin".to_string(),
                branch: Some("feature/foo".to_string()),
                rebase: false,
                autostash: false,
            })
        );
        assert_eq!(
            model.direct_push_request(),
            Some(PushRequest {
                branch: "feature/foo".to_string(),
                destination: "team/origin/feature/foo".to_string(),
                push_tags: false,
                force: false,
            })
        );
    });

    app.model
        .update(cx, |model, cx| {
            model.handle_push_submit(
                PushRequest {
                    branch: "feature/foo".to_string(),
                    destination: "team/origin/feature/foo".to_string(),
                    push_tags: false,
                    force: false,
                },
                cx,
            )
        })
        .await;
    assert_eq!(
        super::support::git_in(&bare, &["rev-parse", "feature/foo"]).trim(),
        new_head
    );
}
