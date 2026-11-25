//! `GitHubAccountDialog.test.js`: el alta por device flow y por token.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use gpui_kit::TestAppContext;
use gpui_kit::test::TestWindowExt as _;
use mergeon_app::model_types::notify::NotifyType;
use mergeon_app::testing::{FakeGitHubTransport, TestBackend};
use mergeon_app::{AppEvent, DialogRequest};
use mergeon_github::{AppMode, GitHubConfig};
use serde_json::json;

use super::support::TestApp;
use crate::open_github_account_dialog;

const INSTALL_URL: &str = "https://github.com/apps/app-git-service/installations/new";

fn github_app() -> TestBackend {
    TestBackend::new().with_github_config(
        GitHubConfig::new("Iv1.test")
            .with_app_mode(AppMode::GitHubApp)
            .with_app_slug("app-git-service"),
    )
}

/// GitHub responde al device flow; la lista de instalaciones llega vacía las
/// primeras `empty_installation_answers` veces.
fn script_device_flow(github: &FakeGitHubTransport, empty_installation_answers: usize) {
    let installation_requests = Arc::new(AtomicUsize::new(0));
    github.respond_with(move |request| {
        let url = request.url.as_str();
        if url.ends_with("/login/device/code") {
            return FakeGitHubTransport::json(
                200,
                json!({
                    "device_code": "device-1",
                    "user_code": "ABCD-1234",
                    "verification_uri": "https://github.com/login/device",
                    "expires_in": 900,
                    "interval": 1
                }),
            );
        }
        if url.ends_with("/login/oauth/access_token") {
            return FakeGitHubTransport::json(
                200,
                json!({ "access_token": "ghu_token", "refresh_token": "ghr_token", "expires_in": 28800 }),
            );
        }
        if url.contains("/user/installations") {
            let seen = installation_requests.fetch_add(1, Ordering::SeqCst);
            let installations = if seen < empty_installation_answers {
                json!([])
            } else {
                json!([{ "id": 10, "account": { "login": "Spilveru" } }])
            };
            return FakeGitHubTransport::json(200, json!({ "installations": installations }));
        }
        FakeGitHubTransport::json(
            200,
            json!({
                "id": 7,
                "login": "Spilveru",
                "name": "Spilveru",
                "avatar_url": "https://example.com/avatar.png"
            }),
        )
    });
}

/// Deja pasar un intervalo de consulta del device flow.
fn next_poll(app: &TestApp, cx: &mut TestAppContext) {
    cx.executor().advance_clock(Duration::from_secs(1));
    app.settle(cx);
}

#[gpui_kit::test]
async fn opens_the_install_page_once_and_completes_when_installation_becomes_available(
    cx: &mut TestAppContext,
) {
    let app = TestApp::with_backend(github_app(), cx);
    script_device_flow(&app.github, 2);

    app.in_window(cx, |window, cx| {
        open_github_account_dialog(window, cx);
    });
    assert_eq!(app.dialog_titles(cx), ["Connect GitHub Account"]);

    app.click(cx, "github-device-login");
    // El código queda a la vista mientras se espera la autorización.
    assert_eq!(
        app.label(cx, "github-user-code").as_deref(),
        Some("ABCD-1234")
    );
    assert!(!app.exists(cx, "github-device-login"));
    assert_eq!(
        app.label(cx, "github-auth-status").as_deref(),
        Some("Enter the code in your browser to authorize the app")
    );

    // Primera consulta: autorizado, pero falta instalar la GitHub App.
    next_poll(&app, cx);
    assert_eq!(
        app.label(cx, "github-auth-status").as_deref(),
        Some("Waiting for GitHub App installation...")
    );
    assert!(app.exists(cx, "github-open-install"));

    // Segunda consulta: sigue sin instalar; la página no se vuelve a abrir.
    next_poll(&app, cx);
    // Tercera: ya hay instalación.
    next_poll(&app, cx);

    let opened = app.launcher.launched_args();
    assert_eq!(
        opened,
        [
            "https://github.com/login/device".to_string(),
            INSTALL_URL.to_string()
        ]
    );
    let accounts = app
        .model
        .read_with(cx, |model, _| model.github_accounts().to_vec());
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].username, "Spilveru");
    assert_eq!(accounts[0].active_installation_id, Some(10));
    assert!(app.has_toast(NotifyType::Success, "Account @Spilveru added successfully"));

    // Con la cuenta guardada el diálogo se cierra y se pide el selector.
    assert!(app.dialog_titles(cx).is_empty());
    assert!(
        app.events()
            .contains(&AppEvent::ShowDialog(DialogRequest::GitHubRepositoryPicker))
    );
}

#[gpui_kit::test]
async fn saves_the_account_after_auth_completes_with_an_installation(cx: &mut TestAppContext) {
    let app = TestApp::with_backend(github_app(), cx);
    script_device_flow(&app.github, 0);

    app.in_window(cx, |window, cx| {
        open_github_account_dialog(window, cx);
    });
    app.click(cx, "github-device-login");
    next_poll(&app, cx);

    let accounts = app
        .model
        .read_with(cx, |model, _| model.github_accounts().to_vec());
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].username, "Spilveru");
    assert_eq!(accounts[0].active_installation_id, Some(10));
    assert_eq!(
        app.model.read_with(cx, |model, _| model
            .active_github_account_id()
            .map(str::to_string)),
        Some(accounts[0].id.clone())
    );
    assert!(app.has_toast(NotifyType::Success, "Account @Spilveru added successfully"));
    assert!(app.dialog_titles(cx).is_empty());
    // Solo se abrió la página del código.
    assert_eq!(
        app.launcher.launched_args(),
        ["https://github.com/login/device".to_string()]
    );
}

#[gpui_kit::test]
async fn cancelling_abandons_the_login_in_progress(cx: &mut TestAppContext) {
    let app = TestApp::with_backend(github_app(), cx);
    script_device_flow(&app.github, 0);

    app.in_window(cx, |window, cx| {
        open_github_account_dialog(window, cx);
    });
    app.click(cx, "github-device-login");
    assert!(
        app.model
            .read_with(cx, |model, _| model.github_auth_flow().is_authenticating)
    );

    app.click(cx, "github-account-cancel");

    assert!(app.dialog_titles(cx).is_empty());
    app.model.read_with(cx, |model, _| {
        let flow = model.github_auth_flow();
        assert!(!flow.is_authenticating);
        assert!(flow.user_code.is_empty());
    });
    // Lo que aún llegue del intento abandonado se descarta.
    next_poll(&app, cx);
    assert!(
        app.model
            .read_with(cx, |model, _| model.github_accounts().is_empty())
    );
}

#[gpui_kit::test]
async fn a_rejected_token_shows_the_error_and_keeps_the_dialog_open(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    app.github
        .respond_with(|_| FakeGitHubTransport::json(401, json!({ "message": "Bad credentials" })));

    let dialog = app.in_window(cx, open_github_account_dialog);
    // El formulario del token empieza plegado.
    assert!(!app.exists(cx, "github-token-login"));
    app.click(cx, "github-token-toggle");
    assert!(app.exists(cx, "github-token-login"));

    // Sin token el botón no hace nada.
    app.click(cx, "github-token-login");
    assert!(app.github.requests().is_empty());

    app.in_window(cx, |window, cx| {
        dialog.update(cx, |dialog, cx| {
            dialog.token.update(cx, |state, cx| {
                state.set_value("ghp_exampleexampleexample", window, cx);
            });
        });
    });
    app.click(cx, "github-token-login");

    assert_eq!(
        app.label(cx, "github-auth-error").as_deref(),
        Some("Failed to authenticate with token: Bad credentials")
    );
    assert!(app.has_toast(
        NotifyType::Error,
        "Authentication failed: Failed to authenticate with token: Bad credentials"
    ));
    assert_eq!(app.dialog_titles(cx), ["Connect GitHub Account"]);
    assert!(
        app.model
            .read_with(cx, |model, _| model.github_accounts().is_empty())
    );
}

#[gpui_kit::test]
async fn a_valid_token_adds_the_account_and_closes(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    app.github.respond_with(|request| {
        assert_eq!(request.header("authorization"), Some("Bearer ghp_valid"));
        FakeGitHubTransport::json(
            200,
            json!({ "id": 9, "login": "octocat", "name": "The Octocat" }),
        )
    });

    let dialog = app.in_window(cx, open_github_account_dialog);
    app.click(cx, "github-token-toggle");
    app.in_window(cx, |window, cx| {
        dialog.update(cx, |dialog, cx| {
            dialog
                .token
                .update(cx, |state, cx| state.set_value("ghp_valid", window, cx));
        });
        // Enter dentro del campo conecta, como el botón.
        dialog.update(cx, |dialog, cx| {
            dialog.token.update(cx, |state, cx| state.focus(window, cx));
        });
        window.press("enter", cx);
    });
    app.settle(cx);

    let accounts = app
        .model
        .read_with(cx, |model, _| model.github_accounts().to_vec());
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].username, "octocat");
    assert_eq!(accounts[0].name.as_deref(), Some("The Octocat"));
    assert_eq!(accounts[0].is_personal_access_token, Some(true));
    assert!(app.dialog_titles(cx).is_empty());
}

#[gpui_kit::test]
async fn an_account_added_from_settings_does_not_ask_for_the_picker(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    app.github
        .respond_with(|_| FakeGitHubTransport::json(200, json!({ "id": 9, "login": "octocat" })));
    // `addAccountFromSettings`
    app.model
        .update(cx, |model, cx| model.add_account_from_settings(cx));

    let dialog = app.in_window(cx, open_github_account_dialog);
    app.click(cx, "github-token-toggle");
    app.in_window(cx, |window, cx| {
        dialog.update(cx, |dialog, cx| {
            dialog
                .token
                .update(cx, |state, cx| state.set_value("ghp_valid", window, cx));
        });
    });
    app.click(cx, "github-token-login");

    assert_eq!(
        app.model
            .read_with(cx, |model, _| model.github_accounts().len()),
        1
    );
    assert!(app.dialog_titles(cx).is_empty());
    assert!(
        !app.events()
            .contains(&AppEvent::ShowDialog(DialogRequest::GitHubRepositoryPicker))
    );
}

#[gpui_kit::test]
async fn opening_twice_keeps_a_single_dialog_and_the_token_form_state(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);

    let first = app.in_window(cx, open_github_account_dialog);
    let second = app.in_window(cx, open_github_account_dialog);
    assert_eq!(first, second);
    assert_eq!(app.dialog_titles(cx), ["Connect GitHub Account"]);

    // `showTokenForm` no se reinicia al cerrar.
    app.click(cx, "github-token-toggle");
    app.click(cx, "github-account-cancel");
    app.in_window(cx, |window, cx| {
        open_github_account_dialog(window, cx);
    });
    assert!(app.exists(cx, "github-token-login"));
}
