//! Port de `src/stores/github.test.js`, con el cliente de GitHub real sobre
//! un transporte guionizado y la configuración en memoria.

use gpui_kit::TestAppContext;
use mergeon_backend::Store;
use mergeon_github::{GitHubConfig, GitHubErrorCode, HttpRequest, HttpResponse, TransportError};
use serde_json::{Value, json};

use super::support::TestApp;
use crate::testing::{FakeGitHubTransport, TestBackend};

/// Client id con prefijo de GitHub App: las cuentas tienen instalaciones.
pub(super) fn github_app_config() -> GitHubConfig {
    GitHubConfig::new("Iv1.test-client")
}

fn persisted_account() -> Value {
    json!({
        "id": "7",
        "username": "Spilveru",
        "name": "Spilveru",
        "token": "ghu_token",
        "refreshToken": "ghr_token",
        "activeInstallationId": 10,
        "installations": [{ "id": 10, "accountLogin": "Spilveru" }]
    })
}

fn store_with(accounts: &[Value], active_id: Option<&str>) -> Store {
    let store = TestBackend::memory_store();
    for account in accounts {
        let account = serde_json::from_value(account.clone()).unwrap();
        store.save_github_account(&account).unwrap();
    }
    store.set_active_github_account_id(active_id).unwrap();
    store
}

fn app_with(accounts: &[Value], active_id: Option<&str>, cx: &mut TestAppContext) -> TestApp {
    TestApp::with_backend(
        TestBackend::new()
            .with_store(store_with(accounts, active_id))
            .with_github_config(github_app_config()),
        cx,
    )
}

fn default_app(cx: &mut TestAppContext) -> TestApp {
    app_with(&[persisted_account()], Some("7"), cx)
}

type Response = Result<HttpResponse, TransportError>;

/// GitHub que responde bien a un refresh: token nuevo, usuario e
/// instalaciones.
pub(super) fn successful_login(request: &HttpRequest) -> Response {
    if request.url.contains("/login/oauth/access_token") {
        FakeGitHubTransport::json(
            200,
            json!({ "access_token": "ghu_new", "refresh_token": "ghr_new", "expires_in": 28800 }),
        )
    } else if request.url.contains("/user/installations") {
        FakeGitHubTransport::json(
            200,
            json!({ "installations": [
                { "id": 10, "account": { "login": "Spilveru" }, "repository_selection": "all" }
            ] }),
        )
    } else if request.url.ends_with("/user") {
        FakeGitHubTransport::json(
            200,
            json!({
                "login": "Spilveru",
                "id": 7,
                "name": "Spilveru",
                "avatar_url": "https://example.com/avatar.png"
            }),
        )
    } else {
        FakeGitHubTransport::json(404, json!({ "message": "Not Found" }))
    }
}

fn unreachable(_: &HttpRequest) -> Response {
    FakeGitHubTransport::json(503, json!({ "message": "GitHub is unreachable" }))
}

fn stored_accounts(app: &TestApp) -> Vec<mergeon_store::GitHubAccount> {
    app.backend.storage().get_github_accounts().unwrap()
}

#[gpui_kit::test]
async fn keeps_persisted_accounts_when_the_refresh_fails_temporarily(cx: &mut TestAppContext) {
    let app = default_app(cx);
    app.github.respond_with(unreachable);

    app.model
        .update(cx, |model, cx| model.initialize_github(cx))
        .await;
    cx.run_until_parked();

    app.model.read_with(cx, |model, _| {
        assert_eq!(model.github_accounts().len(), 1);
        assert_eq!(model.active_github_account_id(), Some("7"));
        assert!(!model.github_accounts()[0].requires_reconnect);
        assert!(!model.auth_service_available());
        assert_eq!(
            model.auth_service_message(),
            "Failed to refresh session: GitHub is unreachable"
        );
        assert!(!model.github_initializing());
    });
    assert_eq!(stored_accounts(&app).len(), 1);
}

#[gpui_kit::test]
async fn marks_persisted_accounts_for_reconnect_when_the_refresh_confirms_session_expiry(
    cx: &mut TestAppContext,
) {
    let app = default_app(cx);
    app.github.respond_with(|_| {
        FakeGitHubTransport::json(
            200,
            json!({ "error": "bad_refresh_token", "error_description": "Session expired" }),
        )
    });

    app.model
        .update(cx, |model, cx| model.initialize_github(cx))
        .await;
    cx.run_until_parked();

    app.model.read_with(cx, |model, _| {
        assert_eq!(model.github_accounts().len(), 1);
        assert_eq!(model.active_github_account_id(), Some("7"));
        assert!(model.github_accounts()[0].requires_reconnect);
        assert!(model.account_requires_reconnect("7"));
        assert!(model.auth_service_available());
    });
    let stored = stored_accounts(&app);
    assert_eq!(stored.len(), 1);
    assert!(stored[0].requires_reconnect);
}

#[gpui_kit::test]
async fn marks_an_account_as_requiring_reconnect_when_manual_refresh_fails_permanently(
    cx: &mut TestAppContext,
) {
    let app = default_app(cx);
    app.model
        .update(cx, |model, cx| model.load_github_accounts_from_storage(cx))
        .await;
    app.github.respond_with(|_| {
        FakeGitHubTransport::json(401, json!({ "message": "Session not found" }))
    });

    let error = app
        .model
        .update(cx, |model, cx| model.refresh_account("7", cx))
        .await
        .unwrap_err();
    cx.run_until_parked();

    assert_eq!(error.code, GitHubErrorCode::AuthRequired);
    assert_eq!(error.status, 401);
    app.model.read_with(cx, |model, _| {
        assert!(model.github_accounts()[0].requires_reconnect);
        assert_eq!(model.get_git_auth_context(), None);
        assert_eq!(model.git_auth_token(), None);
    });
    assert!(stored_accounts(&app)[0].requires_reconnect);
}

#[gpui_kit::test]
async fn keeps_the_account_usable_when_manual_refresh_fails_temporarily(cx: &mut TestAppContext) {
    let app = default_app(cx);
    app.model
        .update(cx, |model, cx| model.load_github_accounts_from_storage(cx))
        .await;
    app.github.respond_with(unreachable);

    let error = app
        .model
        .update(cx, |model, cx| model.refresh_account("7", cx))
        .await
        .unwrap_err();

    assert_eq!(error.status, 503);
    app.model.read_with(cx, |model, _| {
        assert!(!model.github_accounts()[0].requires_reconnect);
        let context = model.get_git_auth_context().expect("auth context");
        assert_eq!(context.token, "ghu_token");
        assert_eq!(context.installation_id, Some(10));
        assert!(!model.auth_service_available());
        assert_eq!(
            model.auth_service_message(),
            "Failed to refresh session: GitHub is unreachable"
        );
    });
}

#[gpui_kit::test]
async fn clears_reconnect_state_when_the_account_is_added_again(cx: &mut TestAppContext) {
    let app = default_app(cx);
    app.model
        .update(cx, |model, cx| model.load_github_accounts_from_storage(cx))
        .await;
    app.model.update(cx, |model, _| {
        model.github.accounts[0].requires_reconnect = true;
    });

    let account_id = app
        .model
        .update(cx, |model, cx| {
            model.add_account(
                json!({
                    "id": "7",
                    "username": "Spilveru",
                    "name": "Spilveru",
                    "sessionToken": "session-2",
                    "activeInstallationId": 10,
                    "installations": [{ "id": 10, "accountLogin": "Spilveru" }]
                }),
                cx,
            )
        })
        .await
        .unwrap();

    assert_eq!(account_id, "7");
    app.model.read_with(cx, |model, _| {
        assert_eq!(model.github_accounts().len(), 1);
        assert!(!model.github_accounts()[0].requires_reconnect);
        assert_eq!(model.github_accounts()[0].session_token, "session-2");
        // Lo que la cuenta ya tenía y el alta no trae se conserva.
        assert_eq!(model.github_accounts()[0].token, "ghu_token");
    });
    assert!(!stored_accounts(&app)[0].requires_reconnect);
}

#[gpui_kit::test]
async fn persists_refreshed_accounts(cx: &mut TestAppContext) {
    let app = default_app(cx);
    app.model
        .update(cx, |model, cx| model.load_github_accounts_from_storage(cx))
        .await;
    app.github.respond_with(successful_login);

    let refreshed = app
        .model
        .update(cx, |model, cx| model.refresh_account("7", cx))
        .await
        .unwrap()
        .expect("refreshed account");
    cx.run_until_parked();

    assert_eq!(refreshed.token, "ghu_new");
    assert_eq!(refreshed.refresh_token, "ghr_new");
    let stored = stored_accounts(&app);
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].token, "ghu_new");
    assert_eq!(stored[0].installations.len(), 1);
    assert_eq!(stored[0].installations[0].id, 10);
    assert!(!stored[0].requires_reconnect);
    app.model.read_with(cx, |model, _| {
        assert_eq!(model.git_auth_token().as_deref(), Some("ghu_new"));
        assert!(model.auth_service_available());
    });
}

#[gpui_kit::test]
async fn shares_one_refresh_between_simultaneous_requests(cx: &mut TestAppContext) {
    let app = default_app(cx);
    app.model
        .update(cx, |model, cx| model.load_github_accounts_from_storage(cx))
        .await;
    app.github.respond_with(successful_login);

    let (first, second) = app.model.update(cx, |model, cx| {
        (
            model.refresh_account("7", cx),
            model.refresh_account("7", cx),
        )
    });
    let (first, second) = (first.await.unwrap(), second.await.unwrap());

    assert_eq!(first, second);
    let token_requests = app
        .github
        .requested_urls()
        .into_iter()
        .filter(|url| url.contains("/login/oauth/access_token"))
        .count();
    assert_eq!(token_requests, 1);
}

#[gpui_kit::test]
async fn does_not_refresh_personal_tokens_or_accounts_without_a_refresh_token(
    cx: &mut TestAppContext,
) {
    let app = app_with(
        &[
            json!({ "id": "1", "username": "pat", "token": "ghp_token", "refreshToken": "x", "isPersonalAccessToken": true }),
            json!({ "id": "2", "username": "oauth", "token": "gho_token", "refreshToken": "" }),
        ],
        Some("1"),
        cx,
    );
    app.model
        .update(cx, |model, cx| model.initialize_github(cx))
        .await;

    for account_id in ["1", "2", "missing"] {
        let refreshed = app
            .model
            .update(cx, |model, cx| model.refresh_account(account_id, cx))
            .await;
        assert_eq!(refreshed, Ok(None));
    }

    // Solo la cuenta con refresh token llegó a GitHub, y fue al inicializar.
    assert_eq!(app.github.requests().len(), 1);
}

#[gpui_kit::test]
async fn deduplicates_persisted_accounts_with_the_same_username_when_loading_from_storage(
    cx: &mut TestAppContext,
) {
    let app = app_with(
        &[
            json!({
                "id": "account-1",
                "username": "Spilveru",
                "name": "Spilveru",
                "sessionToken": "session-1",
                "activeInstallationId": 10,
                "installations": [{ "id": 10, "accountLogin": "Spilveru" }]
            }),
            json!({
                "id": "account-2",
                "username": "spilveru",
                "name": "Spilveru",
                "sessionToken": "session-2",
                "activeInstallationId": 11,
                "installations": [{ "id": 11, "accountLogin": "Spilveru" }]
            }),
        ],
        Some("account-2"),
        cx,
    );

    app.model
        .update(cx, |model, cx| model.load_github_accounts_from_storage(cx))
        .await;
    cx.run_until_parked();

    app.model.read_with(cx, |model, _| {
        assert_eq!(model.github_accounts().len(), 1);
        assert_eq!(model.github_accounts()[0].id, "account-1");
        // La cuenta activa era la repetida: pasa a serlo la que queda.
        assert_eq!(model.active_github_account_id(), Some("account-1"));
        assert!(model.github_loaded());
    });
    let stored = stored_accounts(&app);
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].id, "account-1");
    assert_eq!(
        app.backend
            .storage()
            .get_active_github_account_id()
            .as_deref(),
        Some("account-1")
    );
}

#[gpui_kit::test]
async fn keeps_an_oauth_app_account_usable_even_though_it_has_no_installations(
    cx: &mut TestAppContext,
) {
    let app = default_app(cx);
    app.model
        .update(cx, |model, cx| model.load_github_accounts_from_storage(cx))
        .await;

    // Una OAuth App (o un token personal) nunca tiene instalaciones: eso es lo
    // normal, no un "falta instalar la App".
    app.model
        .update(cx, |model, cx| {
            model.add_account(
                json!({
                    "id": "account-2",
                    "username": "Octocat",
                    "token": "gho_token",
                    "refreshToken": "",
                    "requiresInstallation": false,
                    "installations": [],
                    "activeInstallationId": null
                }),
                cx,
            )
        })
        .await
        .unwrap();

    app.model.read_with(cx, |model, _| {
        let account = model.github_account_by_id("account-2").unwrap();
        assert_eq!(account.requires_installation, Some(false));
        assert!(!model.account_requires_installation("account-2"));
        // Ya había una cuenta activa: la nueva no la desplaza.
        assert_eq!(model.active_github_account_id(), Some("7"));
        assert!(model.has_github_accounts());
    });
    assert_eq!(stored_accounts(&app).len(), 2);
}

#[gpui_kit::test]
async fn still_flags_a_github_app_account_that_is_not_installed_anywhere(cx: &mut TestAppContext) {
    let app = default_app(cx);
    app.model
        .update(cx, |model, cx| model.load_github_accounts_from_storage(cx))
        .await;

    app.model
        .update(cx, |model, cx| {
            model.add_account(
                json!({
                    "id": "account-3",
                    "username": "NeedsInstall",
                    "token": "ghu_token",
                    "requiresInstallation": true,
                    "installations": [],
                    "activeInstallationId": null
                }),
                cx,
            )
        })
        .await
        .unwrap();

    app.model.read_with(cx, |model, _| {
        assert!(model.account_requires_installation("account-3"));
    });
}

#[gpui_kit::test]
async fn the_first_account_becomes_active_and_removing_it_moves_on(cx: &mut TestAppContext) {
    let app = app_with(&[], None, cx);
    app.model
        .update(cx, |model, cx| model.initialize_github(cx))
        .await;
    app.model.read_with(cx, |model, _| {
        assert!(!model.has_github_accounts());
        assert_eq!(model.get_git_auth_context(), None);
    });

    for (id, username) in [("1", "first"), ("2", "second")] {
        app.model
            .update(cx, |model, cx| {
                model.add_account(
                    json!({ "id": id, "username": username, "token": format!("gho_{username}") }),
                    cx,
                )
            })
            .await
            .unwrap();
    }
    app.model.read_with(cx, |model, _| {
        assert_eq!(model.active_github_account_id(), Some("1"));
        assert_eq!(model.git_auth_token().as_deref(), Some("gho_first"));
    });

    app.model
        .update(cx, |model, cx| model.remove_account("1", cx))
        .await
        .unwrap();

    app.model.read_with(cx, |model, _| {
        assert_eq!(model.github_accounts().len(), 1);
        assert_eq!(model.active_github_account_id(), Some("2"));
    });
    assert_eq!(stored_accounts(&app).len(), 1);
    assert_eq!(
        app.backend
            .storage()
            .get_active_github_account_id()
            .as_deref(),
        Some("2")
    );

    // Un id que no es de ninguna cuenta no cambia la activa.
    app.model
        .update(cx, |model, cx| model.set_active_account(Some("nope"), cx))
        .await
        .unwrap();
    app.model.read_with(cx, |model, _| {
        assert_eq!(model.active_github_account_id(), Some("2"));
    });
}

#[gpui_kit::test]
async fn changes_the_active_installation_of_an_account(cx: &mut TestAppContext) {
    let mut account = persisted_account();
    account["installations"] = json!([{ "id": 10 }, { "id": 11, "account": "acme" }]);
    let app = app_with(&[account], Some("7"), cx);
    app.model
        .update(cx, |model, cx| model.load_github_accounts_from_storage(cx))
        .await;

    app.model
        .update(cx, |model, cx| {
            model.set_active_installation("7", Some(11), cx)
        })
        .await
        .unwrap();

    app.model.read_with(cx, |model, _| {
        let installation = model.active_github_installation().expect("installation");
        assert_eq!(installation.id, 11);
        assert_eq!(installation.account.as_deref(), Some("acme"));
        assert_eq!(
            model.get_git_auth_context().unwrap().installation_id,
            Some(11)
        );
    });
    assert_eq!(stored_accounts(&app)[0].active_installation_id, Some(11));
}
