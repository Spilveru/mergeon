// Port de `electron/store-schema.test.js`.
//
// El esquema se valida al escribir: si una cuenta trae un campo con un tipo que
// no acepta, `save_github_account` falla y el login falla al final del todo.
// Estos casos cubren las formas reales que produce `github-auth-client.js`.

use std::sync::Arc;

use mergeon_store::{
    GitHubAccount, PlainTextCipher, Store, StoreError, StoreOptions, validate_github_account,
};
use serde_json::{Map, Value, json};

fn account_from(overrides: Value) -> Value {
    let mut account = json!({
        "id": "7",
        "username": "octocat",
        "name": "The Octocat",
        "email": "",
        "avatarUrl": "https://avatar",
        "tokenCipher": "plain:gho_token",
        "refreshTokenCipher": "",
        "sessionTokenCipher": "",
        "installations": [],
        "activeInstallationId": null,
        "installUrl": null,
        "requiresInstallation": false,
        "requiresReconnect": false,
        "addedAt": "2026-07-26T00:00:00.000Z"
    });

    let fields = account.as_object_mut().expect("object");
    for (key, value) in overrides.as_object().cloned().unwrap_or_else(Map::new) {
        fields.insert(key, value);
    }
    account
}

fn memory_store() -> Store {
    Store::in_memory(StoreOptions::new(Arc::new(PlainTextCipher)))
}

/// La cuenta pasa el esquema y además el store la guarda y la devuelve.
fn assert_accepted(account: Value) -> GitHubAccount {
    assert!(validate_github_account(&account).is_ok());

    let typed: GitHubAccount = serde_json::from_value(account).expect("typed account");
    let store = memory_store();
    let saved = store.save_github_account(&typed).expect("save");
    assert_eq!(
        store.get_github_accounts().expect("accounts"),
        vec![saved.clone()]
    );
    saved
}

#[test]
fn accepts_an_oauth_app_account_whose_token_never_expires() {
    // Sin `expires_in`, el cliente emite expiresAt null.
    let saved = assert_accepted(account_from(json!({ "expiresAt": null })));
    assert_eq!(saved.expires_at, None);
    assert_eq!(saved.token, "gho_token");
}

#[test]
fn accepts_a_personal_access_token_account() {
    let saved = assert_accepted(account_from(
        json!({ "expiresAt": null, "isPersonalAccessToken": true }),
    ));
    assert_eq!(saved.is_personal_access_token, Some(true));
}

#[test]
fn accepts_a_github_app_account_with_expiry_and_installations() {
    let saved = assert_accepted(account_from(json!({
        "expiresAt": "2026-07-26T08:00:00.000Z",
        "refreshTokenCipher": "plain:ghr_token",
        "installations": [{ "id": 42, "account": "octocat" }],
        "activeInstallationId": 42,
        "installUrl": "https://github.com/apps/app-git-service/installations/new"
    })));

    assert_eq!(
        saved.expires_at.as_deref(),
        Some("2026-07-26T08:00:00.000Z")
    );
    assert_eq!(saved.refresh_token, "ghr_token");
    assert_eq!(saved.active_installation_id, Some(42));
    assert_eq!(saved.installations.len(), 1);
    assert_eq!(saved.installations[0].id, 42);
    assert_eq!(saved.installations[0].account.as_deref(), Some("octocat"));
}

#[test]
fn still_rejects_a_wrong_type_so_the_guard_keeps_its_value() {
    let error = validate_github_account(&account_from(json!({ "expiresAt": 123 })))
        .expect_err("schema violation");
    assert_eq!(
        error.to_string(),
        "Config schema violation: `expiresAt` must be string,null"
    );
}

#[test]
fn the_store_refuses_to_write_an_account_that_violates_the_schema() {
    // Con tipos de Rust un campo conocido no puede traer otro tipo; la única
    // vía es `extra`, y la validación al escribir la sigue cortando.
    let mut account: GitHubAccount =
        serde_json::from_value(account_from(json!({ "expiresAt": null }))).expect("typed account");
    account.extra.insert("expiresAt".into(), json!(123));

    let store = memory_store();
    let error = store.save_github_account(&account).expect_err("rejected");

    assert!(matches!(error, StoreError::Schema(_)));
    assert_eq!(
        error.to_string(),
        "Config schema violation: `githubAccounts/0/expiresAt` must be string,null"
    );
    assert!(store.get_github_accounts().expect("accounts").is_empty());
}
