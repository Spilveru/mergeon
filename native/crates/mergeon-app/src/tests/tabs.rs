//! Pestañas de repositorio: frescura al volver a una oculta, cierre con
//! confirmación, reabrir, teclado, nombres sin ambigüedad y guardado diferido
//! de lo que cambia a cada fotograma.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::TestAppContext;
use mergeon_model::keyboard_shortcuts::ShortcutAction;
use mergeon_model::notify::NotifyType;
use mergeon_model::operation_types::{OperationPhase, QueueOperation};
use mergeon_model::repositories::MutationQueueSnapshot;
use mergeon_model::workspace_tabs::{CHANGES_TAB, FileTabDescriptor, Scroll};

use super::support::{TestApp, TestRepo};
use crate::workspace::WORKSPACE_PERSIST_DELAY;

fn active_path(app: &TestApp, cx: &mut TestAppContext) -> Option<String> {
    app.model.read_with(cx, |model, _| {
        model.active_repository().map(|repo| repo.path.clone())
    })
}

fn select(app: &TestApp, index: usize, cx: &mut TestAppContext) {
    app.model
        .update(cx, |model, cx| model.select_repository_tab(index, cx));
    cx.run_until_parked();
}

/// Mutaciones registradas que recargan el historial al volver a un repo.
fn external_head_changes(app: &TestApp) -> usize {
    app.recorded_mutations()
        .iter()
        .filter(|operation| *operation == "external-head-change")
        .count()
}

// Antes: al volver a la pestaña solo se emitía `hydrate-opened-repository`,
// que la vista de commits no trata como mutación del historial, y la rama no
// cambiaba de nombre: la barra mostraba el HEAD nuevo y el grafo el viejo.
#[gpui_kit::test]
async fn returning_to_a_repository_changed_while_hidden_reloads_its_history(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let first = TestRepo::new();
    let second = TestRepo::new();
    app.open(&first, cx).await;
    app.open(&second, cx).await;
    // `first` quedó oculto: su huella se toma en segundo plano.
    app.wait_for(cx, "the hidden repository fingerprint", |model| {
        model.shell.refs_fingerprints.contains_key(&first.id())
    });

    first.commit_file("outside.txt", "made in a terminal\n", "Commit from outside");
    app.clear_events();
    select(&app, 0, cx);
    app.wait_for(cx, "the history reload", |model| {
        model
            .active_repository()
            .and_then(|repo| repo.last_mutation_operation.as_deref())
            == Some("external-head-change")
    });
    assert_eq!(external_head_changes(&app), 1);

    // Ir y volver sin que nada cambie no recarga.
    select(&app, 1, cx);
    app.wait_for(cx, "the fingerprint of the hidden repository", |model| {
        model.shell.refs_fingerprints.contains_key(&first.id())
    });
    app.clear_events();
    select(&app, 0, cx);
    cx.run_until_parked();
    assert!(
        app.recorded_mutations()
            .contains(&"hydrate-opened-repository".to_string())
    );
    assert_eq!(external_head_changes(&app), 0);
}

#[gpui_kit::test]
async fn closing_a_tab_left_of_the_active_one_keeps_the_active_repository(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repos = [TestRepo::new(), TestRepo::new(), TestRepo::new()];
    for repo in &repos {
        app.open(repo, cx).await;
    }
    select(&app, 1, cx);

    let closed = app
        .model
        .update(cx, |model, cx| {
            model.request_close_repositories(vec![repos[0].id()], cx)
        })
        .await;

    assert!(closed);
    assert_eq!(active_path(&app, cx), Some(repos[1].id()));
    assert!(app.host.confirms().is_empty());
    let stored = app.backend.storage().get_workspace_session();
    assert_eq!(
        stored.active_repository_path.as_deref(),
        Some(repos[1].id().as_str())
    );
}

#[gpui_kit::test]
async fn asks_before_closing_a_repository_with_a_draft_or_an_operation_running(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let busy = TestRepo::new();
    let drafted = TestRepo::new();
    let busy_id = app.open(&busy, cx).await;
    let drafted_id = app.open(&drafted, cx).await;

    app.model.update(cx, |model, cx| {
        model.set_commit_draft(&drafted_id, "Half a message", cx);
        let push = QueueOperation {
            operation: "push".to_string(),
            phase: OperationPhase::Running,
            ..Default::default()
        };
        model.set_repository_mutation_queue(
            busy_id.as_str(),
            &MutationQueueSnapshot {
                operations: Some(vec![push]),
                ..Default::default()
            },
            cx,
        );
    });

    // Cancelar no cierra nada.
    app.host.answer_confirm(false);
    let closed = app
        .model
        .update(cx, |model, cx| {
            model.request_close_repositories(vec![drafted_id.clone()], cx)
        })
        .await;
    assert!(!closed);
    let confirms = app.host.confirms();
    assert_eq!(confirms.len(), 1);
    assert!(
        confirms[0]
            .description
            .as_deref()
            .is_some_and(|text| text.contains("unfinished commit message")),
        "{confirms:?}"
    );
    app.model.read_with(cx, |model, _| {
        assert_eq!(model.open_repositories().len(), 2);
    });

    app.host.answer_confirm(true);
    let closed = app
        .model
        .update(cx, |model, cx| {
            model.request_close_repositories(vec![busy_id.clone()], cx)
        })
        .await;
    assert!(closed);
    let confirms = app.host.confirms();
    assert!(
        confirms[1]
            .description
            .as_deref()
            .is_some_and(|text| text.contains("still running (push)")),
        "{confirms:?}"
    );

    // El borrador sigue en la sesión y vuelve al reabrir.
    app.host.answer_confirm(true);
    app.model
        .update(cx, |model, cx| {
            model.request_close_repositories(vec![drafted_id.clone()], cx)
        })
        .await;
    app.model
        .update(cx, |model, cx| model.flush_workspaces(cx))
        .await;
    let stored = app.backend.storage().get_workspace_session();
    assert_eq!(
        stored.repository_workspaces[&drafted_id]["commitDraft"],
        "Half a message"
    );
    let reopened = app
        .model
        .update(cx, |model, cx| model.reopen_closed_repository(cx))
        .await;
    assert!(reopened);
    cx.run_until_parked();
    app.model.read_with(cx, |model, _| {
        assert_eq!(
            model.active_repository().map(|repo| repo.id.clone()),
            Some(drafted_id.clone())
        );
        assert_eq!(model.commit_draft(&drafted_id), "Half a message");
    });
}

#[gpui_kit::test]
async fn keyboard_moves_between_repository_tabs_and_closes_the_one_in_view(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let repos = [TestRepo::new(), TestRepo::new(), TestRepo::new()];
    for repo in &repos {
        app.open(repo, cx).await;
    }
    let path = repos[2].id();

    let shortcut = |action, cx: &mut TestAppContext| {
        app.model
            .update(cx, |model, cx| model.handle_shortcut(action, cx));
        cx.run_until_parked();
    };

    shortcut(ShortcutAction::NextRepositoryTab, cx);
    assert_eq!(active_path(&app, cx), Some(repos[0].id()));
    shortcut(ShortcutAction::PreviousRepositoryTab, cx);
    assert_eq!(active_path(&app, cx), Some(repos[2].id()));
    shortcut(ShortcutAction::SelectRepositoryTab(2), cx);
    assert_eq!(active_path(&app, cx), Some(repos[1].id()));
    shortcut(ShortcutAction::SelectRepositoryTab(9), cx);
    assert_eq!(active_path(&app, cx), Some(path.clone()));
    shortcut(ShortcutAction::SelectRepositoryTab(7), cx);
    assert_eq!(active_path(&app, cx), Some(path.clone()));

    // `mod+w` con un archivo abierto cierra el archivo, no el repositorio.
    let file = FileTabDescriptor::working("README.md");
    app.model.update(cx, |model, cx| {
        model.activate_workspace_tab(&path, CHANGES_TAB, cx);
        model.open_workspace_file(&path, &file, false, cx);
    });
    shortcut(ShortcutAction::CloseTab, cx);
    app.model.read_with(cx, |model, _| {
        let workspace = model.repository_workspace(&path);
        assert!(workspace.tabs.is_empty());
        assert_eq!(workspace.active_tab, CHANGES_TAB);
        assert_eq!(model.open_repositories().len(), 3);
    });

    // Sin archivo `mod+w` no hace nada: un `mod+w` de más ya no se lleva el
    // repositorio. Eso es `mod+shift+w`; el último pasa a ser el de la izquierda.
    shortcut(ShortcutAction::CloseTab, cx);
    assert_eq!(active_path(&app, cx), Some(path.clone()));
    app.model.read_with(cx, |model, _| {
        assert_eq!(model.open_repositories().len(), 3);
    });
    shortcut(ShortcutAction::CloseRepository, cx);
    assert_eq!(active_path(&app, cx), Some(repos[1].id()));
    shortcut(ShortcutAction::CloseRepository, cx);
    assert_eq!(active_path(&app, cx), Some(repos[0].id()));

    shortcut(ShortcutAction::ReopenClosedRepository, cx);
    app.wait_for(cx, "the reopened repository", |model| {
        model.open_repositories().len() == 2
    });
    assert_eq!(active_path(&app, cx), Some(repos[1].id()));

    // En Home no hay nada que cerrar.
    app.model.update(cx, |model, cx| model.show_home_tab(cx));
    shortcut(ShortcutAction::CloseTab, cx);
    app.model.read_with(cx, |model, _| {
        assert_eq!(model.open_repositories().len(), 2);
    });
}

#[gpui_kit::test]
async fn names_repositories_apart_and_says_which_one_a_notification_is_about(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let first = TestRepo::new();
    let second = TestRepo::new();
    app.open(&first, cx).await;

    // Con un solo repositorio abierto el aviso no cambia.
    first.write("one.txt", "1\n");
    app.model
        .update(cx, |model, cx| model.stage_all(&first.id(), cx))
        .await;
    let single = app.host.messages(NotifyType::Success);

    app.open(&second, cx).await;
    // Los dos se llaman "repo": los distingue su carpeta temporal.
    let labels = app
        .model
        .read_with(cx, |model, _| model.repository_tab_labels());
    let first_parent = first
        .root()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert_eq!(labels[0].name, "repo");
    assert_eq!(labels[0].suffix.as_deref(), Some(first_parent.as_str()));
    assert_ne!(labels[0].suffix, labels[1].suffix);

    app.host.clear_toasts();
    first.write("two.txt", "2\n");
    app.model
        .update(cx, |model, cx| model.stage_all(&first.id(), cx))
        .await;
    let labelled = app.host.messages(NotifyType::Success);
    assert_eq!(labelled.len(), 1, "{labelled:?}");
    assert_eq!(
        labelled[0],
        format!("repo ({first_parent}): {}", single[0]),
        "{single:?}"
    );
}

// Antes: cada paso de la rueda guardaba toda la configuración (con fsync) y
// avisaba a todas las vistas.
#[gpui_kit::test]
async fn scrolling_saves_the_session_once_after_it_settles_and_repaints_nothing(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    let path = app.open(&repo, cx).await;
    let file = FileTabDescriptor::working("README.md");
    let file_id = file.id();
    app.model.update(cx, |model, cx| {
        model.open_workspace_file(&path, &file, false, cx);
    });
    cx.run_until_parked();

    let notifications = Rc::new(Cell::new(0));
    let _observer = cx.update(|cx| {
        let notifications = notifications.clone();
        cx.observe(&app.model, move |_, _| {
            notifications.set(notifications.get() + 1);
        })
    });
    let stored_top = |app: &TestApp| {
        app.backend
            .storage()
            .get_workspace_session()
            .repository_workspaces[&path]["tabs"][0]["scroll"]["top"]
            .as_f64()
    };

    app.model.update(cx, |model, cx| {
        for step in 1..=30 {
            model.remember_workspace_scroll(
                &path,
                &file_id,
                Scroll {
                    top: f64::from(step) * 40.0,
                    left: 0.0,
                },
                cx,
            );
        }
        model.set_workspace_selected_commit(&path, Some("abc123"), cx);
    });
    cx.run_until_parked();
    assert_eq!(notifications.get(), 0);
    assert_eq!(stored_top(&app), Some(0.0));
    assert!(
        app.model
            .read_with(cx, |model, _| model.has_pending_workspace_persist())
    );

    cx.executor()
        .advance_clock(WORKSPACE_PERSIST_DELAY + Duration::from_millis(50));
    cx.run_until_parked();
    assert_eq!(stored_top(&app), Some(1200.0));
    assert_eq!(
        app.backend
            .storage()
            .get_workspace_session()
            .repository_workspaces[&path]["selectedCommit"],
        "abc123"
    );
    assert!(
        !app.model
            .read_with(cx, |model, _| model.has_pending_workspace_persist())
    );
}

#[gpui_kit::test]
async fn closes_other_tabs_and_tabs_to_the_right_from_the_context_menu(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repos = [
        TestRepo::new(),
        TestRepo::new(),
        TestRepo::new(),
        TestRepo::new(),
    ];
    for repo in &repos {
        app.open(repo, cx).await;
    }
    // Activa la última; "Close to the right" desde la segunda la cierra.
    let closed = app
        .model
        .update(cx, |model, cx| {
            model.close_repositories_to_the_right(&repos[1].id(), cx)
        })
        .await;
    assert!(closed);
    cx.run_until_parked();
    assert_eq!(active_path(&app, cx), Some(repos[1].id()));
    app.model.read_with(cx, |model, _| {
        assert_eq!(model.open_repositories().len(), 2);
        // Se recuerdan para reabrirlas, la última cerrada al final.
        assert_eq!(model.closed_repositories(), [repos[2].id(), repos[3].id()]);
    });

    app.model
        .update(cx, |model, cx| {
            model.close_other_repositories(&repos[0].id(), cx)
        })
        .await;
    cx.run_until_parked();
    assert_eq!(active_path(&app, cx), Some(repos[0].id()));
    app.model.read_with(cx, |model, _| {
        assert_eq!(model.open_repositories().len(), 1);
    });
}
