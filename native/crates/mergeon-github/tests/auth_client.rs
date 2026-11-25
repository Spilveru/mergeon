//! Port de `electron/services/github-auth-client.test.js`.

mod common;

use std::time::{Duration, UNIX_EPOCH};

use common::{
    FakeTransport, github_app_client, json_response, json_response_with_status, oauth_client,
    raw_response,
};
use mergeon_github::{
    GitHubAccount, GitHubClient, GitHubConfig, GitHubErrorCode, HttpMethod, LoginStatus,
    PollLoginResult, TransportError,
};
use serde_json::{Value, json};

fn repository(id: u64, name: &str, owner: &str, description: Value) -> Value {
    json!({
        "id": id,
        "name": name,
        "full_name": format!("{owner}/{name}"),
        "description": description,
        "private": id % 2 == 1,
        "clone_url": format!("https://github.com/{owner}/{name}.git"),
        "html_url": format!("https://github.com/{owner}/{name}"),
        "language": if id == 1 { json!("JS") } else { Value::Null },
        "stargazers_count": if id == 1 { 3 } else { 0 },
        "forks_count": if id == 1 { 1 } else { 0 },
        "updated_at": format!("2026-01-0{id}T00:00:00Z"),
        "owner": { "login": owner, "avatar_url": "https://avatar" }
    })
}

#[test]
fn starts_the_device_flow_without_sending_a_client_secret() {
    let transport = FakeTransport::new();
    transport.respond(json_response(json!({
        "device_code": "device-1",
        "user_code": "ABCD-1234",
        "verification_uri": "https://github.com/login/device",
        "expires_in": 900,
        "interval": 5
    })));

    let session = oauth_client(&transport).start_login().unwrap();

    assert_eq!(session.device_code.as_deref(), Some("device-1"));
    assert_eq!(session.user_code.as_deref(), Some("ABCD-1234"));
    assert_eq!(
        session.verification_uri.as_deref(),
        Some("https://github.com/login/device")
    );
    assert_eq!(session.expires_in, Some(900));
    assert_eq!(session.poll_interval_ms, 5000);

    let calls = transport.calls();
    assert_eq!(calls[0].url, "https://github.com/login/device/code");
    assert_eq!(calls[0].method, HttpMethod::Post);
    assert_eq!(calls[0].header("Accept"), Some("application/json"));
    assert_eq!(calls[0].header("X-GitHub-Api-Version"), Some("2022-11-28"));
    assert_eq!(calls[0].header("Content-Type"), Some("application/json"));
    assert_eq!(calls[0].header("Authorization"), None);
    assert_eq!(calls[0].timeout, Duration::from_millis(15_000));
    // Una OAuth App necesita scope; una GitHub App lo ignora.
    assert_eq!(
        transport.body(0),
        json!({ "client_id": "Ov23liHW92VyuIBmLnUQ", "scope": "repo" })
    );
    assert!(!calls[0].body.as_deref().unwrap().contains("client_secret"));
}

#[test]
fn omits_the_scope_when_the_client_is_a_github_app() {
    let transport = FakeTransport::new();
    transport.respond(json_response(json!({
        "device_code": "device-1",
        "user_code": "ABCD-1234",
        "verification_uri": "https://github.com/login/device",
        "interval": 5
    })));

    let session = github_app_client(&transport).start_login().unwrap();

    assert_eq!(session.expires_in, None);
    assert_eq!(
        transport.body(0),
        json!({ "client_id": "Ov23liHW92VyuIBmLnUQ" })
    );
}

#[test]
fn reports_pending_while_the_user_has_not_authorized_yet() {
    let transport = FakeTransport::new();
    transport.respond(json_response(json!({ "error": "authorization_pending" })));

    let result = oauth_client(&transport).poll_login("device-1").unwrap();

    assert_eq!(
        result,
        PollLoginResult::Pending {
            poll_interval_ms: None
        }
    );
    assert_eq!(
        serde_json::to_value(&result).unwrap(),
        json!({ "status": "pending" })
    );

    assert_eq!(
        transport.calls()[0].url,
        "https://github.com/login/oauth/access_token"
    );
    assert_eq!(
        transport.body(0),
        json!({
            "client_id": "Ov23liHW92VyuIBmLnUQ",
            "device_code": "device-1",
            "grant_type": "urn:ietf:params:oauth:grant-type:device_code"
        })
    );
}

#[test]
fn honours_the_slow_down_interval_requested_by_github() {
    let transport = FakeTransport::new();
    transport.respond(json_response(
        json!({ "error": "slow_down", "interval": 12 }),
    ));

    let result = oauth_client(&transport).poll_login("device-1").unwrap();

    assert_eq!(
        serde_json::to_value(&result).unwrap(),
        json!({ "status": "pending", "pollIntervalMs": 12000 })
    );
    assert_eq!(result.poll_interval_ms(), Some(12_000));

    // Sin `interval` en la respuesta, el original sube a 10 segundos.
    transport.respond(json_response(json!({ "error": "slow_down" })));
    let result = oauth_client(&transport).poll_login("device-1").unwrap();
    assert_eq!(result.poll_interval_ms(), Some(10_000));
}

#[test]
fn builds_the_account_with_token_installations_and_expiry_once_authorized() {
    let transport = FakeTransport::new();
    transport.respond_once(json_response(json!({
        "access_token": "ghu_token",
        "refresh_token": "ghr_token",
        "expires_in": 28800
    })));
    transport.respond_once(json_response(json!({
        "id": 7,
        "login": "octocat",
        "name": "The Octocat",
        "avatar_url": "https://avatar"
    })));
    transport.respond_once(json_response(json!({
        "installations": [
            {
                "id": 42,
                "account": { "login": "octocat", "avatar_url": "https://avatar" },
                "repository_selection": "all"
            }
        ]
    })));

    let client = github_app_client(&transport)
        .with_clock(|| UNIX_EPOCH + Duration::from_millis(1_767_225_600_123));
    let result = client.poll_login("device-1").unwrap();

    assert_eq!(result.status(), LoginStatus::Completed);
    assert_eq!(result.status().as_str(), "completed");

    let account = result.account().unwrap();
    assert_eq!(account.id, "7");
    assert_eq!(account.username.as_deref(), Some("octocat"));
    assert_eq!(account.display_name.as_deref(), Some("The Octocat"));
    assert_eq!(account.avatar_url.as_deref(), Some("https://avatar"));
    assert_eq!(account.token, "ghu_token");
    assert_eq!(account.refresh_token.as_deref(), Some("ghr_token"));
    assert_eq!(account.active_installation_id, Some(42));
    assert!(!account.requires_installation);
    // 2026-01-01T00:00:00.123Z más 28800 segundos.
    assert_eq!(
        account.expires_at.as_deref(),
        Some("2026-01-01T08:00:00.123Z")
    );

    let installation = &account.installations[0];
    assert_eq!(installation.id, Some(42));
    assert_eq!(installation.account.as_deref(), Some("octocat"));
    assert_eq!(installation.avatar_url.as_deref(), Some("https://avatar"));
    assert_eq!(installation.repository_selection.as_deref(), Some("all"));

    // El token viaja solo en la cabecera Authorization de las llamadas a la API.
    let calls = transport.calls();
    assert_eq!(calls[0].header("Authorization"), None);
    assert_eq!(calls[1].url, "https://api.github.com/user");
    assert_eq!(calls[1].method, HttpMethod::Get);
    assert_eq!(calls[1].body, None);
    assert_eq!(calls[1].header("Content-Type"), None);
    assert_eq!(calls[1].header("Authorization"), Some("Bearer ghu_token"));
    assert_eq!(calls[2].header("Authorization"), Some("Bearer ghu_token"));
}

#[test]
fn asks_for_installation_when_the_app_is_not_installed_anywhere() {
    let transport = FakeTransport::new();
    transport.respond_once(json_response(json!({ "access_token": "ghu_token" })));
    transport.respond_once(json_response(json!({ "id": 7, "login": "octocat" })));
    transport.respond_once(json_response(json!({ "installations": [] })));

    let result = github_app_client(&transport)
        .poll_login("device-1")
        .unwrap();

    assert_eq!(result.status(), LoginStatus::InstallationRequired);
    assert_eq!(
        result.install_url(),
        Some("https://github.com/apps/app-git-service/installations/new")
    );

    let account = result.account().unwrap();
    assert!(account.requires_installation);
    assert_eq!(account.display_name.as_deref(), Some("octocat"));
    assert_eq!(account.avatar_url, None);
    assert_eq!(account.refresh_token, None);
    assert_eq!(account.expires_at, None);
    assert_eq!(account.active_installation_id, None);

    let serialized = serde_json::to_value(&result).unwrap();
    assert_eq!(serialized["status"], "installation_required");
    assert_eq!(
        serialized["installUrl"],
        "https://github.com/apps/app-git-service/installations/new"
    );
    assert_eq!(serialized["account"]["requiresInstallation"], true);
}

#[test]
fn refreshes_with_the_refresh_token_and_no_client_secret() {
    let transport = FakeTransport::new();
    transport.respond_once(json_response(json!({
        "access_token": "ghu_new",
        "refresh_token": "ghr_new",
        "expires_in": 28800
    })));
    transport.respond_once(json_response(json!({ "id": 7, "login": "octocat" })));
    transport.respond_once(json_response(json!({
        "installations": [{ "id": 42, "account": { "login": "octocat" } }]
    })));

    let result = github_app_client(&transport)
        .refresh_session("ghr_token")
        .unwrap();

    assert_eq!(result.account.token, "ghu_new");
    assert_eq!(result.account.refresh_token.as_deref(), Some("ghr_new"));
    assert!(result.account.expires_at.is_some());
    assert_eq!(
        transport.calls()[0].url,
        "https://github.com/login/oauth/access_token"
    );
    assert_eq!(
        transport.body(0),
        json!({
            "client_id": "Ov23liHW92VyuIBmLnUQ",
            "grant_type": "refresh_token",
            "refresh_token": "ghr_token"
        })
    );
    assert!(
        !transport.calls()[0]
            .body
            .as_deref()
            .unwrap()
            .contains("client_secret")
    );
}

#[test]
fn treats_a_rejected_refresh_token_as_a_permanent_failure() {
    let transport = FakeTransport::new();
    transport.respond(json_response(json!({
        "error": "bad_refresh_token",
        "error_description": "Expired"
    })));

    let error = oauth_client(&transport)
        .refresh_session("ghr_token")
        .unwrap_err();

    assert_eq!(error.code, GitHubErrorCode::AuthRequired);
    assert_eq!(error.code.as_str(), "auth_required");
    assert_eq!(error.status, 401);
    assert_eq!(error.message, "Expired");
    assert!(error.is_permanent_auth_failure());
    assert!(!error.is_transient_failure());
}

#[test]
fn accepts_a_personal_access_token_and_flags_the_account() {
    let transport = FakeTransport::new();
    transport.respond_once(json_response(json!({ "id": 9, "login": "octocat" })));
    transport.respond_once(json_response(json!({
        "installations": [{ "id": 42, "account": { "login": "octocat" } }]
    })));

    let result = oauth_client(&transport)
        .authenticate_with_token(" ghp_secret ")
        .unwrap();

    assert_eq!(result.account.token, "ghp_secret");
    assert!(result.account.is_personal_access_token);
    assert_eq!(
        transport.calls()[0].header("Authorization"),
        Some("Bearer ghp_secret")
    );

    let serialized = serde_json::to_value(&result).unwrap();
    assert_eq!(serialized["account"]["isPersonalAccessToken"], true);
}

#[test]
fn maps_a_401_from_github_to_auth_required_so_the_account_is_reconnected() {
    let transport = FakeTransport::new();
    transport.respond(json_response_with_status(
        json!({ "message": "Bad credentials" }),
        401,
    ));

    let error = oauth_client(&transport)
        .authenticate_with_token("ghp_bad")
        .unwrap_err();

    assert_eq!(error.code, GitHubErrorCode::AuthRequired);
    assert_eq!(error.status, 401);
    assert_eq!(error.message, "Bad credentials");
}

#[test]
fn maps_timeouts_and_network_failures_to_transient_codes() {
    let transport = FakeTransport::new();
    let client = oauth_client(&transport);

    transport.respond(Err(TransportError::Timeout));
    let error = client.authenticate_with_token("ghp_x").unwrap_err();
    assert_eq!(error.code, GitHubErrorCode::GithubTimeout);
    assert_eq!(error.status, 504);
    assert_eq!(error.message, "GitHub request timed out");
    assert!(error.is_transient_failure());

    transport.respond(Err(TransportError::Network(
        "connect ECONNREFUSED".to_string(),
    )));
    let error = client.authenticate_with_token("ghp_x").unwrap_err();
    assert_eq!(error.code, GitHubErrorCode::GithubUnavailable);
    assert_eq!(error.status, 503);
    assert_eq!(
        error.message,
        "GitHub is unreachable at https://api.github.com"
    );
    assert!(error.is_transient_failure());

    transport.respond(Err(TransportError::Cancelled));
    let error = client.authenticate_with_token("ghp_x").unwrap_err();
    assert_eq!(error.code, GitHubErrorCode::GithubCancelled);
    assert_eq!(error.status, 499);
    assert_eq!(error.message, "GitHub request was cancelled");
}

#[test]
fn normalizes_repositories_and_filters_them_by_query() {
    let transport = FakeTransport::new();
    transport.respond(json_response(json!({
        "total_count": 2,
        "repositories": [
            repository(1, "alpha", "octocat", json!("first")),
            repository(2, "beta", "octocat", Value::Null)
        ]
    })));

    let result = oauth_client(&transport)
        .get_repositories("ghu_token", Some(42), 1, 100, Some("alpha"))
        .unwrap();

    assert_eq!(result.installation_id, Some(42));
    assert_eq!(result.total_count, 2);
    assert_eq!(
        serde_json::to_value(&result.repositories).unwrap(),
        json!([{
            "id": 1,
            "name": "alpha",
            "fullName": "octocat/alpha",
            "description": "first",
            "private": true,
            "cloneUrl": "https://github.com/octocat/alpha.git",
            "htmlUrl": "https://github.com/octocat/alpha",
            "language": "JS",
            "stars": 3,
            "forks": 1,
            "updatedAt": "2026-01-01T00:00:00Z",
            "owner": { "login": "octocat", "avatarUrl": "https://avatar" }
        }])
    );

    let calls = transport.calls();
    assert_eq!(
        calls[0].url,
        "https://api.github.com/user/installations/42/repositories?page=1&per_page=100"
    );
    assert_eq!(calls[0].header("Authorization"), Some("Bearer ghu_token"));

    // El filtro mira también la descripción y no distingue mayúsculas.
    let by_description = oauth_client(&transport)
        .get_repositories("ghu_token", Some(42), 1, 100, Some("  FIRST "))
        .unwrap();
    assert_eq!(by_description.repositories.len(), 1);

    let unfiltered = oauth_client(&transport)
        .get_repositories("ghu_token", Some(42), 1, 100, None)
        .unwrap();
    assert_eq!(unfiltered.repositories.len(), 2);
    assert_eq!(unfiltered.repositories[1].description, None);
    assert_eq!(unfiltered.repositories[1].language, None);
    assert_eq!(unfiltered.repositories[1].private, Some(false));
}

#[test]
fn paginates_installations_so_accounts_with_many_orgs_are_not_truncated() {
    let many: Vec<Value> = (0..100)
        .map(|i| json!({ "id": i + 1, "account": { "login": format!("org-{i}") } }))
        .collect();

    let transport = FakeTransport::new();
    transport.respond_once(json_response(json!({ "access_token": "ghu_token" })));
    transport.respond_once(json_response(json!({ "id": 7, "login": "octocat" })));
    transport.respond_once(json_response(json!({ "installations": many })));
    transport.respond_once(json_response(json!({
        "installations": [{ "id": 101, "account": { "login": "last" } }]
    })));

    let result = github_app_client(&transport)
        .poll_login("device-1")
        .unwrap();

    let account = result.account().unwrap();
    assert_eq!(account.installations.len(), 101);
    assert_eq!(account.active_installation_id, Some(1));
    assert_eq!(account.installations[100].account.as_deref(), Some("last"));

    // Sin per_page GitHub devuelve solo 30 y el resto desaparece en silencio.
    let calls = transport.calls();
    assert_eq!(
        calls[2].url,
        "https://api.github.com/user/installations?page=1&per_page=100"
    );
    assert_eq!(
        calls[3].url,
        "https://api.github.com/user/installations?page=2&per_page=100"
    );
}

#[test]
fn stops_paginating_installations_after_ten_pages() {
    let full_page: Vec<Value> = (0..100).map(|i| json!({ "id": i + 1 })).collect();

    let transport = FakeTransport::new();
    transport.respond_once(json_response(json!({ "access_token": "ghu_token" })));
    transport.respond_once(json_response(json!({ "id": 7, "login": "octocat" })));
    transport.respond(json_response(json!({ "installations": full_page })));

    let result = github_app_client(&transport)
        .poll_login("device-1")
        .unwrap();

    assert_eq!(result.account().unwrap().installations.len(), 1000);
    assert_eq!(transport.call_count(), 12);
}

#[test]
fn lists_repositories_by_affiliation_when_the_account_has_no_installation() {
    let transport = FakeTransport::new();
    transport.respond(json_response(json!([repository(
        5,
        "shared",
        "other-owner",
        Value::Null
    )])));

    let result = oauth_client(&transport)
        .get_repositories("ghp_token", None, 1, 100, None)
        .unwrap();

    let url = &transport.calls()[0].url;
    assert_eq!(
        url,
        "https://api.github.com/user/repos?page=1&per_page=100\
         &affiliation=owner%2Ccollaborator%2Corganization_member&sort=updated"
    );
    assert_eq!(
        result.repositories[0].full_name.as_deref(),
        Some("other-owner/shared")
    );
    assert_eq!(result.total_count, 1);
    assert_eq!(result.installation_id, None);
    assert_eq!(
        serde_json::to_value(&result).unwrap()["installationId"],
        Value::Null
    );

    // Un id de instalación 0 es falsy en el original: también lista por afiliación.
    oauth_client(&transport)
        .get_repositories("ghp_token", Some(0), 2, 50, None)
        .unwrap();
    assert!(
        transport.calls()[1]
            .url
            .starts_with("https://api.github.com/user/repos?page=2&per_page=50&")
    );
}

#[test]
fn does_not_query_installations_for_personal_access_tokens() {
    let transport = FakeTransport::new();
    transport.respond_once(json_response(json!({ "id": 9, "login": "octocat" })));

    // Ni siquiera con un cliente de GitHub App se consultan las instalaciones.
    let result = github_app_client(&transport)
        .authenticate_with_token("ghp_secret")
        .unwrap();

    assert_eq!(transport.call_count(), 1);
    assert!(!result.account.requires_installation);
    assert!(result.account.installations.is_empty());
}

#[test]
fn skips_installations_entirely_for_an_oauth_app_client() {
    let transport = FakeTransport::new();
    transport.respond_once(json_response(json!({ "access_token": "gho_token" })));
    transport.respond_once(json_response(json!({ "id": 7, "login": "octocat" })));

    let result = oauth_client(&transport).poll_login("device-1").unwrap();

    // Sin instalaciones, pero tampoco pide instalar nada: lista por afiliación.
    assert_eq!(transport.call_count(), 2);
    assert_eq!(result.status(), LoginStatus::Completed);

    let account = result.account().unwrap();
    assert!(account.installations.is_empty());
    assert!(!account.requires_installation);
    assert_eq!(account.install_url, None);
    assert_eq!(result.install_url(), None);

    assert_eq!(
        serde_json::to_value(&result).unwrap(),
        json!({
            "status": "completed",
            "account": {
                "id": "7",
                "username": "octocat",
                "displayName": "octocat",
                "avatarUrl": null,
                "token": "gho_token",
                "refreshToken": null,
                "expiresAt": null,
                "installations": [],
                "activeInstallationId": null,
                "installUrl": null,
                "requiresInstallation": false
            },
            "installUrl": null
        })
    );
}

// --- Casos sin test en el original: códigos y mensajes que consume el renderer.

#[test]
fn reports_device_flow_errors_with_the_code_sent_by_github() {
    let transport = FakeTransport::new();
    let client = oauth_client(&transport);

    transport.respond(json_response(json!({
        "error": "device_flow_disabled",
        "error_description": "Device Flow must be explicitly enabled for this App"
    })));
    let error = client.start_login().unwrap_err();
    assert_eq!(error.code.as_str(), "device_flow_disabled");
    assert_eq!(error.status, 400);
    assert_eq!(
        error.message,
        "Device Flow must be explicitly enabled for this App"
    );

    transport.respond(json_response(json!({ "error": "expired_token" })));
    let error = client.poll_login("device-1").unwrap_err();
    assert_eq!(error.code, GitHubErrorCode::Expired);
    assert_eq!(error.status, 400);
    assert_eq!(error.message, "expired_token");

    transport.respond(json_response(json!({
        "error": "access_denied",
        "error_description": "The user has denied your application access."
    })));
    let error = client.poll_login("device-1").unwrap_err();
    assert_eq!(error.code.as_str(), "access_denied");
    assert_eq!(error.status, 400);
    assert_eq!(
        error.message,
        "The user has denied your application access."
    );

    // Un error de refresh que no es permanente conserva su código y va con 400.
    transport.respond(json_response(json!({ "error": "temporarily_unavailable" })));
    let error = client.refresh_session("ghr_token").unwrap_err();
    assert_eq!(error.code.as_str(), "temporarily_unavailable");
    assert_eq!(error.status, 400);
}

#[test]
fn validates_missing_tokens_without_touching_the_network() {
    let transport = FakeTransport::new();
    let client = oauth_client(&transport);

    let error = client.refresh_session("").unwrap_err();
    assert_eq!(error.code, GitHubErrorCode::AuthRequired);
    assert_eq!(error.status, 401);
    assert_eq!(error.message, "Missing refresh token");

    let error = client.authenticate_with_token("   ").unwrap_err();
    assert_eq!(error.code, GitHubErrorCode::InvalidToken);
    assert_eq!(error.code.as_str(), "invalid_token");
    assert_eq!(error.status, 400);
    assert_eq!(error.message, "A GitHub token is required");

    assert_eq!(transport.call_count(), 0);
}

#[test]
fn maps_unsuccessful_and_malformed_responses() {
    let transport = FakeTransport::new();
    let client = oauth_client(&transport);

    transport.respond(json_response_with_status(json!({}), 403));
    let error = client
        .get_repositories("ghu_token", Some(42), 1, 100, None)
        .unwrap_err();
    assert_eq!(error.code, GitHubErrorCode::GithubError);
    assert_eq!(error.status, 403);
    assert_eq!(error.message, "GitHub request failed: 403");

    transport.respond(json_response_with_status(
        json!({ "message": "Server Error" }),
        500,
    ));
    let error = client
        .get_repositories("ghu_token", None, 1, 100, None)
        .unwrap_err();
    assert_eq!(error.code, GitHubErrorCode::GithubError);
    assert_eq!(error.status, 500);
    assert_eq!(error.message, "Server Error");
    assert!(error.is_transient_failure());

    transport.respond(raw_response("<html>Bad Gateway</html>", 502));
    let error = client.start_login().unwrap_err();
    assert_eq!(error.code, GitHubErrorCode::InvalidResponse);
    assert_eq!(error.status, 502);
    assert_eq!(error.message, "Invalid response from GitHub");
}

#[test]
fn uses_the_injected_configuration() {
    let transport = FakeTransport::new();
    transport.respond_once(json_response(json!({ "device_code": "d", "interval": 7 })));
    transport.respond_once(json_response(json!({ "access_token": "ghu_token" })));
    transport.respond_once(json_response(json!({ "id": 7, "login": "octocat" })));
    transport.respond_once(json_response(json!({ "installations": [] })));
    transport.respond_once(Err(TransportError::Network("refused".to_string())));

    let config = GitHubConfig::new("Iv1.custom")
        .with_app_slug("custom-app")
        .with_base_url("http://127.0.0.1:1/site/")
        .with_api_url("http://127.0.0.1:1/api")
        .with_timeout(Duration::from_secs(3));
    let client = GitHubClient::with_transport(config, transport.clone());
    assert_eq!(client.client_id(), "Iv1.custom");

    let session = client.start_login().unwrap();
    assert_eq!(session.poll_interval_ms, 7000);
    // El prefijo `Iv` es de GitHub App: sin scope.
    assert_eq!(transport.body(0), json!({ "client_id": "Iv1.custom" }));

    let result = client.poll_login("d").unwrap();
    assert_eq!(
        result.install_url(),
        Some("http://127.0.0.1:1/site/apps/custom-app/installations/new")
    );

    let error = client.authenticate_with_token("ghp_x").unwrap_err();
    assert_eq!(
        error.message,
        "GitHub is unreachable at http://127.0.0.1:1/api"
    );

    let calls = transport.calls();
    assert_eq!(calls[0].url, "http://127.0.0.1:1/site/login/device/code");
    assert_eq!(calls[0].timeout, Duration::from_secs(3));
    assert_eq!(
        calls[1].url,
        "http://127.0.0.1:1/site/login/oauth/access_token"
    );
    assert_eq!(calls[2].url, "http://127.0.0.1:1/api/user");
    assert_eq!(
        calls[3].url,
        "http://127.0.0.1:1/api/user/installations?page=1&per_page=100"
    );
}

#[test]
fn never_prints_tokens_in_debug_output() {
    let transport = FakeTransport::new();
    transport.respond_once(json_response(json!({
        "access_token": "ghu_secret_access",
        "refresh_token": "ghr_secret_refresh",
        "expires_in": 28800
    })));
    transport.respond_once(json_response(json!({ "id": 7, "login": "octocat" })));
    transport.respond_once(json_response(json!({
        "device_code": "secret-device-code",
        "user_code": "ABCD-1234",
        "verification_uri": "https://github.com/login/device"
    })));

    let client = oauth_client(&transport);
    let result = client.refresh_session("ghr_secret_old").unwrap();
    let session = client.start_login().unwrap();

    let printed = format!(
        "{result:?} {session:?} {:?} {client:?} {:?}",
        transport.calls(),
        json_response(json!({ "access_token": "ghu_secret_access" }))
    );

    for secret in [
        "ghu_secret_access",
        "ghr_secret_refresh",
        "ghr_secret_old",
        "secret-device-code",
    ] {
        assert!(!printed.contains(secret), "{secret} leaked: {printed}");
    }
    assert!(printed.contains("octocat"));
    assert!(printed.contains("ABCD-1234"));
}

#[test]
fn round_trips_results_through_json_and_is_shareable_across_threads() {
    fn assert_send_sync<T: Send + Sync + Clone>() {}
    assert_send_sync::<GitHubClient>();

    let transport = FakeTransport::new();
    transport.respond_once(json_response(json!({
        "access_token": "ghu_token",
        "refresh_token": "ghr_token",
        "expires_in": 28800
    })));
    transport.respond_once(json_response(json!({ "id": 7, "login": "octocat" })));
    transport.respond_once(json_response(json!({
        "installations": [{ "id": 42, "account": { "login": "octocat" } }]
    })));

    let result = github_app_client(&transport)
        .poll_login("device-1")
        .unwrap();
    let serialized = serde_json::to_string(&result).unwrap();
    assert_eq!(
        serde_json::from_str::<PollLoginResult>(&serialized).unwrap(),
        result
    );

    for pending in [
        json!({ "status": "pending" }),
        json!({ "status": "pending", "pollIntervalMs": 12000 }),
    ] {
        let parsed: PollLoginResult = serde_json::from_value(pending.clone()).unwrap();
        assert_eq!(serde_json::to_value(&parsed).unwrap(), pending);
    }

    // Una cuenta guardada por la app Electron trae campos extra del store.
    let stored: GitHubAccount = serde_json::from_value(json!({
        "id": "7",
        "username": "octocat",
        "name": "The Octocat",
        "email": "",
        "token": "ghp_token",
        "refreshToken": "",
        "isPersonalAccessToken": true,
        "installUrl": null,
        "requiresInstallation": false,
        "installations": [],
        "activeInstallationId": null,
        "addedAt": "2026-01-01T00:00:00.000Z",
        "requiresReconnect": false
    }))
    .unwrap();
    assert_eq!(stored.username.as_deref(), Some("octocat"));
    assert!(stored.is_personal_access_token);
    assert_eq!(stored.display_name, None);
}
