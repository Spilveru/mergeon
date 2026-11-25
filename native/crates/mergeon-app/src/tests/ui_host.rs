//! La interfaz global inyectada: avisos antes de tener interfaz, respuestas
//! por defecto y preguntas que se quedan abiertas.

use gpui_kit::{Task, TestAppContext};
use mergeon_model::dialogs::{ConfirmAnswer, ConfirmOptions, PromptAnswer, PromptOptions};
use mergeon_model::notify::{NotifyOptions, NotifyType, ToastId};

use super::support::{TestApp, TestRepo};
use crate::AppModel;
use crate::tasks::flow;
use crate::testing::{RecordingUiHost, TestBackend};

#[gpui_kit::test]
async fn toasts_requested_before_the_shell_installs_its_host_are_delivered_later(
    cx: &mut TestAppContext,
) {
    cx.executor().allow_parking();
    let (backend, events) = TestBackend::new().build();
    let model = cx.update(|cx| AppModel::init(backend, events, cx));
    cx.update(|cx| assert_eq!(AppModel::try_global(cx), Some(AppModel::global(cx))));

    // Sin interfaz: los avisos esperan y las preguntas se dan por cerradas.
    let (confirmed, prompted, directory) = model.update(cx, |model, cx| {
        model.notify_warning("GitHub is unreachable", cx);
        model.notify_warning("GitHub is unreachable", cx);
        model.notify_error("Something failed", cx);
        (
            model.confirm(ConfirmOptions::default(), cx),
            model.prompt(PromptOptions::default(), cx),
            model.pick_directory(cx),
        )
    });
    assert!(!confirmed.await);
    assert!(!prompted.await.confirmed);
    assert_eq!(directory.await, None);

    let host = RecordingUiHost::new();
    model.update(cx, |model, cx| model.set_ui_host(host.clone(), cx));

    // El aviso repetido cuenta una vez.
    assert_eq!(
        host.messages(NotifyType::Warning),
        ["GitHub is unreachable"]
    );
    assert_eq!(host.messages(NotifyType::Error), ["Something failed"]);
}

#[gpui_kit::test]
async fn notifications_are_trimmed_deduplicated_and_can_be_owned_by_id(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);

    app.model.update(cx, |model, cx| {
        let first = model.notify_success("Saved", cx);
        // El mismo mensaje enseguida es el mismo aviso.
        assert_eq!(model.notify_success("Saved", cx), first);
        model.notify_error_with(
            "Could not save",
            &"Error invoking remote method 'x': disk full",
            cx,
        );
        // Con id propio el aviso se reescribe, no se deduplica.
        for message in ["Checking...", "Checking..."] {
            model.notify(
                NotifyType::Info,
                message,
                NotifyOptions {
                    id: Some(ToastId::from("progress")),
                    ..Default::default()
                },
                cx,
            );
        }
        model.report_persistence_error("Failed to save the layout", &"read-only", cx);
    });

    let toasts = app.host.toasts();
    assert_eq!(toasts.len(), 5);
    assert_eq!(toasts[1].title, "Could not save");
    assert_eq!(
        toasts[1].full_message,
        "Error invoking remote method 'x': disk full"
    );
    assert_eq!(toasts[2].id, toasts[3].id);
    assert_eq!(
        toasts[4].title,
        "Failed to save the layout. The change may not persist after restarting."
    );

    app.model.update(cx, |model, cx| {
        assert_eq!(model.notification_details(), None);
        model.open_notification_details(&toasts[1], cx);
        assert_eq!(
            model.notification_details(),
            Some("Error invoking remote method 'x': disk full")
        );
        model.close_notification_details(cx);
        assert_eq!(model.notification_details(), None);
    });
}

#[gpui_kit::test]
async fn confirmations_with_an_option_and_dismissed_dialogs(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);

    app.host.answer_confirm_with_option(true, true);
    let answer = app
        .model
        .update(cx, |model, cx| {
            model.confirm_with_option(
                ConfirmOptions {
                    title: Some("Delete branch".to_string()),
                    option_label: Some("Also delete on the remote".to_string()),
                    ..Default::default()
                },
                cx,
            )
        })
        .await;
    assert_eq!(
        answer,
        ConfirmAnswer::WithOption {
            confirmed: true,
            checked: true
        }
    );
    assert_eq!(
        app.host.option_confirms()[0].option_label.as_deref(),
        Some("Also delete on the remote")
    );

    // Sin respuesta programada, la confirmación responde lo configurado.
    app.host.confirm_by_default(true);
    assert!(
        app.model
            .update(cx, |model, cx| model.confirm(ConfirmOptions::default(), cx))
            .await
    );
}

/// Un flujo de la barra de herramientas que pregunta antes de guardar un
/// stash (como hacía el botón Stash antes de tener diálogo propio).
fn stash_after_prompt(app: &TestApp, repo: &TestRepo, cx: &mut TestAppContext) -> Task<()> {
    let repo_id = repo.id();
    let repo_path = repo.path_string();
    app.model.update(cx, |model, cx| {
        model.run_repo_operation_single_flight(&repo_id, "stash", cx, move |this, cx| {
            let prompt = this.prompt(
                PromptOptions {
                    title: Some("Create stash".to_string()),
                    ..Default::default()
                },
                cx,
            );
            flow(cx, async move |this, cx| {
                if !prompt.await.confirmed {
                    return;
                }
                this.update(cx, |this, cx| this.stash(&repo_path, None, cx))
                    .await;
            })
        })
    })
}

#[gpui_kit::test]
async fn a_flow_stays_busy_while_its_dialog_is_open(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.write("README.md", "# Demo\n\nwork in progress\n");
    app.open(&repo, cx).await;
    app.host.hold_answers(true);

    let stash = stash_after_prompt(&app, &repo, cx);
    cx.run_until_parked();

    // El prompt sigue abierto: la barra de herramientas muestra la operación
    // en curso y un segundo clic no abre otro prompt.
    app.model.read_with(cx, |model, _| {
        assert_eq!(model.busy_toolbar_operation(), Some("stash"));
    });
    let second = stash_after_prompt(&app, &repo, cx);
    cx.run_until_parked();
    assert_eq!(app.host.prompts().len(), 1);

    assert!(app.host.release_prompt(PromptAnswer {
        confirmed: true,
        value: Some(String::new()),
    }));
    stash.await;
    second.await;

    assert_eq!(repo.git(&["stash", "list"]).lines().count(), 1);
    app.model.read_with(cx, |model, _| {
        assert_eq!(model.busy_toolbar_operation(), None)
    });
    assert!(!app.host.release_prompt(PromptAnswer {
        confirmed: false,
        value: None,
    }));
}

#[gpui_kit::test]
async fn shutting_down_flushes_the_session_and_stops_the_backend(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    let repo_id = app.open(&repo, cx).await;

    app.model.update(cx, |model, cx| {
        model.set_workspace_selected_commit(&repo_id, Some("abc123"), cx);
    });
    app.model.update(cx, |model, cx| model.shutdown(cx)).await;

    let session = app.backend.storage().get_workspace_session();
    assert_eq!(session.open_repositories, std::slice::from_ref(&repo_id));
    assert_eq!(
        session.repository_workspaces[&repo_id]["selectedCommit"],
        "abc123"
    );
}
