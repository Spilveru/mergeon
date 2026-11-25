//! Port de `electron/ipc/github-handlers.test.js`.
//!
//! El original mockea el módulo `github-auth-client.js`; aquí los canales se
//! ejercitan de punta a punta con el transporte falso, y lo que el test JS
//! comprobaba sobre los argumentos de `getRepositories` se comprueba sobre la
//! petición que sale hacia GitHub.

mod common;

use common::{FakeTransport, json_response, json_response_with_status, oauth_client};
use mergeon_github::{
    AuthContext, DEFAULT_LIST_PER_PAGE, DEFAULT_SEARCH_PER_PAGE, GitHubClient, GitHubConfig,
    GitHubErrorCode, TransportError,
};
use serde_json::{Value, json};

fn repository(id: u64, full_name: &str) -> Value {
    let (owner, name) = full_name.split_once('/').unwrap();
    json!({
        "id": id,
        "name": name,
        "full_name": full_name,
        "description": null,
        "private": false,
        "clone_url": format!("https://github.com/{full_name}.git"),
        "html_url": format!("https://github.com/{full_name}"),
        "language": null,
        "stargazers_count": 0,
        "forks_count": 0,
        "updated_at": "2026-01-01T00:00:00Z",
        "owner": { "login": owner, "avatar_url": "https://avatar" }
    })
}

#[test]
fn exposes_the_public_client_id_alongside_the_device_code() {
    let transport = FakeTransport::new();
    transport.respond(json_response(json!({
        "device_code": "device-1",
        "user_code": "ABCD-1234",
        "verification_uri": "https://github.com/login/device",
        "interval": 5
    })));

    let client = GitHubClient::with_transport(GitHubConfig::new("Iv1test"), transport.clone());
    let result = client.auth_start().unwrap();

    assert_eq!(result.session.device_code.as_deref(), Some("device-1"));
    assert_eq!(result.session.user_code.as_deref(), Some("ABCD-1234"));
    assert_eq!(result.client_id, "Iv1test");

    // La sesión y el client id viajan en un mismo objeto plano.
    assert_eq!(
        serde_json::to_value(&result).unwrap(),
        json!({
            "deviceCode": "device-1",
            "userCode": "ABCD-1234",
            "verificationUri": "https://github.com/login/device",
            "expiresIn": null,
            "pollIntervalMs": 5000,
            "clientId": "Iv1test"
        })
    );
}

#[test]
fn preserves_code_and_status_when_github_refresh_fails() {
    let transport = FakeTransport::new();
    transport.respond(json_response_with_status(
        json!({ "message": "Bad credentials" }),
        401,
    ));

    let error = oauth_client(&transport)
        .auth_refresh("ghr_token")
        .unwrap_err();

    assert_eq!(error.code, GitHubErrorCode::AuthRequired);
    assert_eq!(error.status, 401);
    assert_eq!(error.message, "Failed to refresh session: Bad credentials");
    assert_eq!(
        error.to_string(),
        "Failed to refresh session: Bad credentials"
    );
    assert_eq!(
        serde_json::to_value(&error).unwrap(),
        json!({
            "message": "Failed to refresh session: Bad credentials",
            "code": "auth_required",
            "status": 401
        })
    );
}

#[test]
fn authenticates_with_a_personal_access_token() {
    let transport = FakeTransport::new();
    transport.respond(json_response(json!({ "id": 9, "login": "octocat" })));

    let result = oauth_client(&transport).auth_token("ghp_secret").unwrap();

    assert_eq!(result.account.username.as_deref(), Some("octocat"));
    assert!(result.account.is_personal_access_token);
    assert_eq!(
        transport.calls()[0].header("Authorization"),
        Some("Bearer ghp_secret")
    );
}

#[test]
fn returns_repository_lists_through_the_shared_repository_loader_for_list_and_search_handlers() {
    let transport = FakeTransport::new();
    transport.respond_once(json_response(json!({
        "total_count": 1,
        "repositories": [repository(1, "demo/repo")]
    })));
    transport.respond_once(json_response(json!({
        "total_count": 2,
        "repositories": [repository(2, "demo/search-repo"), repository(3, "demo/other")]
    })));

    let client = oauth_client(&transport);
    let auth_context = AuthContext::new("ghu_token", Some(7));

    // El canal de listado devuelve solo el array de repositorios.
    let listed = client
        .get_user_repositories(&auth_context, 1, DEFAULT_LIST_PER_PAGE)
        .unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, Some(1));
    assert_eq!(listed[0].full_name.as_deref(), Some("demo/repo"));

    // El de búsqueda devuelve `{ totalCount, repositories }`, ya filtrado.
    let found = client
        .search_repositories(&auth_context, "search", 1, DEFAULT_SEARCH_PER_PAGE)
        .unwrap();
    assert_eq!(found.total_count, 2);
    assert_eq!(found.repositories.len(), 1);
    assert_eq!(
        found.repositories[0].full_name.as_deref(),
        Some("demo/search-repo")
    );

    let serialized = serde_json::to_value(&found).unwrap();
    let mut keys: Vec<&str> = serialized
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, ["repositories", "totalCount"]);

    let calls = transport.calls();
    assert_eq!(
        calls[0].url,
        "https://api.github.com/user/installations/7/repositories?page=1&per_page=100"
    );
    assert_eq!(
        calls[1].url,
        "https://api.github.com/user/installations/7/repositories?page=1&per_page=30"
    );
    assert_eq!(calls[0].header("Authorization"), Some("Bearer ghu_token"));
    assert_eq!(calls[1].header("Authorization"), Some("Bearer ghu_token"));
}

#[test]
fn prefixes_the_error_message_of_every_channel() {
    let transport = FakeTransport::new();
    transport.respond(Err(TransportError::Timeout));

    let client = oauth_client(&transport);
    let auth_context = AuthContext::new("ghu_token", None);

    let errors = [
        client.auth_start().unwrap_err(),
        client.auth_poll("device-1").unwrap_err(),
        client.auth_refresh("ghr_token").unwrap_err(),
        client.auth_token("ghp_secret").unwrap_err(),
        client
            .get_user_repositories(&auth_context, 1, 100)
            .unwrap_err(),
        client
            .search_repositories(&auth_context, "query", 1, 30)
            .unwrap_err(),
    ];

    let messages: Vec<&str> = errors.iter().map(|error| error.message.as_str()).collect();
    assert_eq!(
        messages,
        [
            "Failed to start GitHub login: GitHub request timed out",
            "Failed to poll GitHub login: GitHub request timed out",
            "Failed to refresh session: GitHub request timed out",
            "Failed to authenticate with token: GitHub request timed out",
            "Failed to fetch repositories: GitHub request timed out",
            "Search failed: GitHub request timed out",
        ]
    );

    for error in &errors {
        assert_eq!(error.code, GitHubErrorCode::GithubTimeout);
        assert_eq!(error.status, 504);
    }
}

#[test]
fn accepts_the_auth_context_shape_sent_by_the_renderer() {
    let with_installation: AuthContext =
        serde_json::from_value(json!({ "token": "ghu_token", "installationId": 7 })).unwrap();
    assert_eq!(with_installation, AuthContext::new("ghu_token", Some(7)));

    let without_installation: AuthContext =
        serde_json::from_value(json!({ "token": "ghp_token", "installationId": null })).unwrap();
    assert_eq!(without_installation, AuthContext::new("ghp_token", None));

    assert!(!format!("{with_installation:?}").contains("ghu_token"));
}
