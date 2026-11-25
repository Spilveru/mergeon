//! Port de `src/composables/useGitHubAuthFlow.test.js`.

use std::time::Duration;

use gpui_kit::TestAppContext;
use mergeon_github::{HttpRequest, HttpResponse, TransportError};
use mergeon_model::notify::NotifyType;
use serde_json::json;

use super::github_store::github_app_config;
use super::support::TestApp;
use crate::testing::{FakeGitHubTransport, TestBackend};

const VERIFICATION_URI: &str = "https://github.com/login/device";
const INSTALL_URL: &str = "https://github.com/apps/app-git-service/installations/new";

type Response = Result<HttpResponse, TransportError>;

fn app(cx: &mut TestAppContext) -> TestApp {
    TestApp::with_backend(
        TestBackend::new().with_github_config(github_app_config()),
        cx,
    )
}

fn device_session() -> Response {
    FakeGitHubTransport::json(
        200,
        json!({
            "device_code": "device-1",
            "user_code": "ABCD-1234",
            "verification_uri": VERIFICATION_URI,
            "interval": 1
        }),
    )
}

fn token() -> Response {
    FakeGitHubTransport::json(
        200,
        json!({ "access_token": "ghu_token", "refresh_token": "ghr_token", "expires_in": 28800 }),
    )
}

fn user() -> Response {
    FakeGitHubTransport::json(
        200,
        json!({
            "login": "Spilveru",
            "id": 7,
            "name": "Spilveru",
            "avatar_url": "https://example.com/avatar.png"
        }),
    )
}

fn installations(installed: bool) -> Response {
    let installations = if installed {
        json!([{ "id": 10, "account": { "login": "Spilveru" } }])
    } else {
        json!([])
    };
    FakeGitHubTransport::json(200, json!({ "installations": installations }))
}

fn is(request: &HttpRequest, path: &str) -> bool {
    request.url.contains(path)
}

/// Deja pasar un intervalo de consulta y que corra lo que desencadena.
fn advance(cx: &mut TestAppContext, milliseconds: u64) {
    cx.executor()
        .advance_clock(Duration::from_millis(milliseconds));
    cx.run_until_parked();
}

fn token_requests(app: &TestApp) -> usize {
    app.github
        .requested_urls()
        .iter()
        .filter(|url| url.contains("/login/oauth/access_token"))
        .count()
}

#[gpui_kit::test]
async fn completes_the_device_flow_and_auto_opens_installation_only_once(cx: &mut TestAppContext) {
    let app = app(cx);
    let mut installation_checks = 0;
    app.github.respond_with(move |request| {
        if is(request, "/login/device/code") {
            device_session()
        } else if is(request, "/login/oauth/access_token") {
            token()
        } else if is(request, "/user/installations") {
            installation_checks += 1;
            // La App se instala entre la segunda consulta y la tercera.
            installations(installation_checks > 2)
        } else {
            user()
        }
    });

    let login = app.model.update(cx, |model, cx| model.start_login(cx));
    cx.run_until_parked();
    app.model.read_with(cx, |model, _| {
        let flow = model.github_auth_flow();
        assert!(flow.is_authenticating);
        assert_eq!(
            flow.status_message,
            "Enter the code in your browser to authorize the app"
        );
    });

    advance(cx, 1000);
    app.model.read_with(cx, |model, _| {
        let flow = model.github_auth_flow();
        assert_eq!(flow.install_url, INSTALL_URL);
        assert_eq!(
            flow.status_message,
            "Waiting for GitHub App installation..."
        );
    });
    advance(cx, 1000);
    advance(cx, 1000);

    assert_eq!(login.await.as_deref(), Some("7"));
    // Primero el verification_uri, después la instalación (una sola vez).
    assert_eq!(
        app.launcher.launched_args(),
        [VERIFICATION_URI, INSTALL_URL]
    );
    app.model.read_with(cx, |model, _| {
        let account = &model.github_accounts()[0];
        assert_eq!(account.username, "Spilveru");
        assert_eq!(account.token, "ghu_token");
        assert_eq!(account.refresh_token, "ghr_token");
        assert_eq!(account.active_installation_id, Some(10));
        assert_eq!(model.active_github_account_id(), Some("7"));

        let flow = model.github_auth_flow();
        assert!(!flow.is_authenticating);
        assert_eq!(flow.status_message, "");
        assert_eq!(flow.user_code, "");
    });
    assert!(
        app.host
            .has_message(NotifyType::Success, "Account @Spilveru added successfully")
    );
}

#[gpui_kit::test]
async fn exposes_the_user_code_so_the_dialog_can_show_it(cx: &mut TestAppContext) {
    let app = app(cx);
    app.github.respond_with(|request| {
        if is(request, "/login/device/code") {
            device_session()
        } else {
            FakeGitHubTransport::json(200, json!({ "error": "authorization_pending" }))
        }
    });

    let login = app.model.update(cx, |model, cx| model.start_login(cx));
    cx.run_until_parked();

    app.model.read_with(cx, |model, _| {
        let flow = model.github_auth_flow();
        assert_eq!(flow.user_code, "ABCD-1234");
        assert_eq!(flow.verification_uri, VERIFICATION_URI);
    });

    // Un segundo alta mientras hay uno en curso no hace nada.
    let second = app.model.update(cx, |model, cx| model.start_login(cx));
    assert_eq!(second.await, None);

    // El código se puede copiar y la página volver a abrir.
    assert!(app.model.update(cx, |model, cx| model.copy_user_code(cx)));
    assert_eq!(
        cx.read_from_clipboard().and_then(|item| item.text()),
        Some("ABCD-1234".to_string())
    );
    assert!(
        app.model
            .update(cx, |model, cx| model.open_verification_page(cx))
            .await
    );
    assert_eq!(
        app.launcher.launched_args(),
        [VERIFICATION_URI, VERIFICATION_URI]
    );

    app.model.update(cx, |model, cx| model.cancel_login(cx));
    advance(cx, 1000);
    assert_eq!(login.await, None);
}

#[gpui_kit::test]
async fn honours_a_slow_down_interval_without_failing(cx: &mut TestAppContext) {
    let app = app(cx);
    let mut polls = 0;
    app.github.respond_with(move |request| {
        if is(request, "/login/device/code") {
            device_session()
        } else if is(request, "/login/oauth/access_token") {
            polls += 1;
            if polls == 1 {
                FakeGitHubTransport::json(200, json!({ "error": "slow_down", "interval": 5 }))
            } else {
                token()
            }
        } else if is(request, "/user/installations") {
            installations(true)
        } else {
            user()
        }
    });

    let login = app.model.update(cx, |model, cx| model.start_login(cx));
    cx.run_until_parked();

    advance(cx, 1000);
    assert_eq!(token_requests(&app), 1);
    // GitHub pidió esperar 5 s: antes de eso no se vuelve a consultar.
    advance(cx, 4999);
    assert_eq!(token_requests(&app), 1);
    advance(cx, 1);
    assert_eq!(token_requests(&app), 2);

    assert_eq!(login.await.as_deref(), Some("7"));
}

#[gpui_kit::test]
async fn cancels_the_polling_flow_cleanly(cx: &mut TestAppContext) {
    let app = app(cx);
    app.github.respond_with(|request| {
        if is(request, "/login/device/code") {
            device_session()
        } else if is(request, "/login/oauth/access_token") {
            token()
        } else if is(request, "/user/installations") {
            installations(true)
        } else {
            user()
        }
    });

    let login = app.model.update(cx, |model, cx| model.start_login(cx));
    app.model.update(cx, |model, cx| model.cancel_login(cx));
    cx.run_until_parked();
    advance(cx, 1000);

    assert_eq!(login.await, None);
    app.model.read_with(cx, |model, _| {
        assert!(model.github_accounts().is_empty());
        assert!(!model.github_auth_flow().is_authenticating);
    });
    assert!(app.host.toasts().is_empty());
}

#[gpui_kit::test]
async fn a_login_cancelled_while_polling_does_not_add_the_account(cx: &mut TestAppContext) {
    let app = app(cx);
    app.github.respond_with(|request| {
        if is(request, "/login/device/code") {
            device_session()
        } else if is(request, "/login/oauth/access_token") {
            token()
        } else if is(request, "/user/installations") {
            installations(true)
        } else {
            user()
        }
    });

    let login = app.model.update(cx, |model, cx| model.start_login(cx));
    cx.run_until_parked();
    // El usuario cierra el diálogo mientras se espera la siguiente consulta;
    // la consulta ya programada llega a hacerse, pero su respuesta se descarta.
    app.model.update(cx, |model, cx| model.cancel_login(cx));
    advance(cx, 1000);

    assert_eq!(login.await, None);
    app.model.read_with(cx, |model, _| {
        assert!(model.github_accounts().is_empty());
        assert_eq!(model.github_auth_flow().user_code, "");
    });
}

#[gpui_kit::test]
async fn fails_clearly_when_github_returns_an_unexpected_session_shape(cx: &mut TestAppContext) {
    let app = app(cx);
    app.github.respond_with(|_| {
        FakeGitHubTransport::json(
            200,
            json!({ "authUrl": "https://github.com/login/oauth/authorize", "attemptId": "attempt-1" }),
        )
    });

    let login = app.model.update(cx, |model, cx| model.start_login(cx));

    assert_eq!(login.await, None);
    app.model.read_with(cx, |model, _| {
        assert_eq!(
            model.github_auth_flow().validation_error,
            "Unexpected response from GitHub. Restart the app and try again."
        );
        assert!(!model.github_auth_flow().is_authenticating);
        assert!(model.github_accounts().is_empty());
    });
    assert!(app.launcher.launched().is_empty());
}

#[gpui_kit::test]
async fn surfaces_github_errors_without_adding_an_account(cx: &mut TestAppContext) {
    let app = app(cx);
    app.github
        .respond_with(|_| Err(TransportError::Network("connection refused".to_string())));

    let login = app.model.update(cx, |model, cx| model.start_login(cx));

    assert_eq!(login.await, None);
    let expected = "Failed to start GitHub login: GitHub is unreachable at https://api.github.com";
    app.model.read_with(cx, |model, _| {
        assert_eq!(model.github_auth_flow().validation_error, expected);
        assert!(model.github_accounts().is_empty());
    });
    assert_eq!(
        app.host.messages(NotifyType::Error),
        [format!("Authentication failed: {expected}")]
    );
}

#[gpui_kit::test]
async fn adds_an_account_from_a_personal_access_token(cx: &mut TestAppContext) {
    let app = app(cx);
    app.github.respond_with(|_| user());

    let account_id = app
        .model
        .update(cx, |model, cx| model.login_with_token("ghp_secret", cx))
        .await;

    assert_eq!(account_id.as_deref(), Some("7"));
    app.model.read_with(cx, |model, _| {
        let account = &model.github_accounts()[0];
        assert_eq!(account.token, "ghp_secret");
        assert_eq!(account.is_personal_access_token, Some(true));
        assert_eq!(account.refresh_token, "");
        // Un token personal no pertenece a ninguna instalación.
        assert!(!model.account_requires_installation("7"));
        assert!(!model.github_auth_flow().is_authenticating);
    });
    // El token viajó en la cabecera, no en la URL.
    let requests = app.github.requests();
    assert_eq!(
        requests[0].header("authorization"),
        Some("Bearer ghp_secret")
    );
}

#[gpui_kit::test]
async fn reports_a_token_that_github_rejects(cx: &mut TestAppContext) {
    let app = app(cx);
    app.github
        .respond_with(|_| FakeGitHubTransport::json(401, json!({ "message": "Bad credentials" })));

    let account_id = app
        .model
        .update(cx, |model, cx| model.login_with_token("ghp_wrong", cx))
        .await;

    assert_eq!(account_id, None);
    let expected = "Failed to authenticate with token: Bad credentials";
    app.model.read_with(cx, |model, _| {
        assert_eq!(model.github_auth_flow().validation_error, expected);
        assert!(!model.github_auth_flow().is_authenticating);
    });
    assert_eq!(
        app.host.messages(NotifyType::Error),
        [format!("Authentication failed: {expected}")]
    );
}

#[gpui_kit::test]
async fn opens_the_install_page_explicitly_when_requested(cx: &mut TestAppContext) {
    let app = app(cx);

    // Sin página de instalación (ni código) no hay nada que abrir.
    assert!(
        !app.model
            .update(cx, |model, cx| model.open_install_page(cx))
            .await
    );
    assert!(!app.model.update(cx, |model, cx| model.copy_user_code(cx)));

    app.model.update(cx, |model, _| {
        model.github.auth_flow.install_url = INSTALL_URL.to_string();
    });

    assert!(
        app.model
            .update(cx, |model, cx| model.open_install_page(cx))
            .await
    );
    assert_eq!(app.launcher.launched_args(), [INSTALL_URL]);
}
