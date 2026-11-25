//! Notificación del sistema al terminar una operación de red larga con la
//! ventana sin foco.

use std::time::Duration;

use gpui_kit::TestAppContext;

use super::support::{TestApp, TestRepo};
use crate::PushRequest;

fn push_main() -> PushRequest {
    PushRequest {
        branch: "main".to_string(),
        destination: "origin/main".to_string(),
        push_tags: false,
        force: false,
    }
}

async fn push(app: &TestApp, cx: &mut TestAppContext) {
    app.model
        .update(cx, |model, cx| model.handle_push_submit(push_main(), cx))
        .await;
    cx.run_until_parked();
}

#[gpui_kit::test]
async fn a_long_push_that_ends_without_focus_posts_a_system_notification(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.publish_to_origin();
    repo.commit_file("a.txt", "a\n", "A");
    app.open(&repo, cx).await;
    // Como la cáscara al arrancar: sin identidad no se publica nada.
    cx.update(|cx| cx.set_app_identity("io.github.spilveru.Mergeon", "Mergeon"));
    app.model.update(cx, |model, cx| {
        assert!(model.desktop_notifications_enabled());
        // Toda operación cuenta como larga.
        model.set_desktop_notification_threshold(Duration::ZERO);
        model.set_window_focused(false, cx);
    });

    push(&app, cx).await;
    let shown = cx.shown_system_notifications();
    assert_eq!(shown.len(), 1, "{shown:?}");
    assert_eq!(shown[0].title.as_ref(), "Push origin/main finished");
    assert_eq!(shown[0].body.as_ref(), "repo");

    // Con la ventana enfocada no se notifica.
    repo.commit_file("b.txt", "b\n", "B");
    app.model
        .update(cx, |model, cx| model.set_window_focused(true, cx));
    push(&app, cx).await;
    assert_eq!(cx.shown_system_notifications().len(), 1);

    // Apagadas en Ajustes, tampoco.
    repo.commit_file("c.txt", "c\n", "C");
    app.model.update(cx, |model, cx| {
        model.set_window_focused(false, cx);
        model.set_desktop_notifications_enabled(false, cx).detach();
    });
    push(&app, cx).await;
    assert_eq!(cx.shown_system_notifications().len(), 1);
}

#[gpui_kit::test]
async fn a_short_push_is_not_notified(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.publish_to_origin();
    repo.commit_file("a.txt", "a\n", "A");
    app.open(&repo, cx).await;
    cx.update(|cx| cx.set_app_identity("io.github.spilveru.Mergeon", "Mergeon"));
    app.model
        .update(cx, |model, cx| model.set_window_focused(false, cx));

    // El umbral por defecto (5 s) no se alcanza con un remoto local.
    push(&app, cx).await;
    assert!(cx.shown_system_notifications().is_empty());
}

#[gpui_kit::test]
async fn the_setting_is_on_by_default_and_persists(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    app.model
        .update(cx, |model, cx| model.load_app_settings(true, cx))
        .await;
    app.model.read_with(cx, |model, _| {
        assert!(model.desktop_notifications_enabled())
    });

    app.model
        .update(cx, |model, cx| {
            model.set_desktop_notifications_enabled(false, cx)
        })
        .await;
    cx.run_until_parked();
    app.model.update(cx, |model, _| {
        // Se relee de lo guardado.
        model.settings.desktop_notifications = true;
    });
    app.model
        .update(cx, |model, cx| model.load_app_settings(true, cx))
        .await;
    app.model.read_with(cx, |model, _| {
        assert!(!model.desktop_notifications_enabled())
    });
}
