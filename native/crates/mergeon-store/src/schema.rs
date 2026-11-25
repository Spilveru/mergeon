//! Esquema de la configuración: defaults y validación.
//!
//! `electron-store` valida con Ajv (`useDefaults: true`) el documento completo
//! en cada escritura. Aquí se reproduce esa validación para este esquema
//! concreto, con los mismos mensajes (`Config schema violation: ...`).

use serde_json::{Map, Value, json};

use crate::error::StoreError;

pub(crate) const GITHUB_ACCOUNTS: &str = "githubAccounts";
pub(crate) const ACTIVE_GITHUB_ACCOUNT_ID: &str = "activeGithubAccountId";
pub(crate) const RECENT_REPOSITORIES: &str = "recentRepositories";
pub(crate) const WORKSPACE_SESSION: &str = "workspaceSession";
pub(crate) const SETTINGS: &str = "settings";
pub(crate) const STARRED_BRANCHES: &str = "starredBranches";

/// Claves del esquema, en el orden en que las declara `store.js`.
pub(crate) const SCHEMA_KEYS: [&str; 6] = [
    GITHUB_ACCOUNTS,
    ACTIVE_GITHUB_ACCOUNT_ID,
    RECENT_REPOSITORIES,
    WORKSPACE_SESSION,
    SETTINGS,
    STARRED_BRANCHES,
];

/// `DEFAULT_SIDEBAR_SECTIONS`.
pub fn default_sidebar_sections() -> Value {
    json!({
        "starred": true,
        "branches": true,
        "remotes": false,
        "tags": false,
        "stashes": false,
        "submodules": false
    })
}

/// `DEFAULT_COMMIT_VIEW_SETTINGS`.
pub fn default_commit_view_settings() -> Value {
    json!({
        "historyWidth": null,
        // Lista de archivos frente al diff dentro de una pestaña de archivo.
        "fileListWidth": null,
        // Cómo se prefiere leer un diff. Es del usuario, no de cada sesión.
        "diffViewMode": "unified",
        "ignoreWhitespace": false
    })
}

/// `DEFAULT_SETTINGS`.
pub fn default_settings() -> Value {
    json!({
        "theme": "light",
        "autoFetch": true,
        "maxRecentRepos": 10,
        // Ancho de la barra lateral (ramas, remotos, tags). null = el de diseño.
        "sidebarWidth": null,
        "sidebarSections": default_sidebar_sections(),
        "commitView": default_commit_view_settings()
    })
}

/// Default de `workspaceSession` en el esquema. Al validar se le añade además
/// `repositoryWorkspaces: {}`, que es el default de esa propiedad.
pub fn default_workspace_session() -> Value {
    json!({
        "openRepositories": [],
        "activeRepositoryPath": null
    })
}

/// El documento completo con todos los defaults, tal como queda un archivo
/// recién creado.
pub fn default_config() -> Value {
    let mut root = Map::new();
    apply_defaults(&mut root);
    Value::Object(root)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum JsonType {
    String,
    Number,
    Boolean,
    Array,
    Object,
    Null,
}

impl JsonType {
    fn name(self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Number => "number",
            Self::Boolean => "boolean",
            Self::Array => "array",
            Self::Object => "object",
            Self::Null => "null",
        }
    }

    fn matches(self, value: &Value) -> bool {
        match self {
            Self::String => value.is_string(),
            Self::Number => value.is_number(),
            Self::Boolean => value.is_boolean(),
            Self::Array => value.is_array(),
            Self::Object => value.is_object(),
            Self::Null => value.is_null(),
        }
    }
}

use JsonType::{Array, Boolean, Null, Number, Object, String as Str};

type Properties = &'static [(&'static str, &'static [JsonType])];

const GITHUB_ACCOUNT_PROPERTIES: Properties = &[
    ("id", &[Str]),
    ("username", &[Str]),
    ("name", &[Str]),
    ("email", &[Str]),
    ("avatarUrl", &[Str]),
    ("token", &[Str]),
    ("tokenCipher", &[Str]),
    ("refreshToken", &[Str]),
    ("refreshTokenCipher", &[Str]),
    // Legado del backend propio: solo sirve para detectar cuentas antiguas
    // y pedir que se reconecten.
    ("sessionToken", &[Str]),
    ("sessionTokenCipher", &[Str]),
    ("isPersonalAccessToken", &[Boolean]),
    // Los tokens de OAuth App y los personales no caducan: llegan sin fecha.
    ("expiresAt", &[Str, Null]),
    ("installUrl", &[Str, Null]),
    ("requiresInstallation", &[Boolean]),
    ("requiresReconnect", &[Boolean]),
    ("activeInstallationId", &[Number, Null]),
    ("installations", &[Array]),
    ("addedAt", &[Str]),
];

const RECENT_REPOSITORY_PROPERTIES: Properties = &[
    ("id", &[Str]),
    ("name", &[Str]),
    ("path", &[Str]),
    ("lastOpened", &[Str]),
    ("currentBranch", &[Str]),
    ("isGitHub", &[Boolean]),
    ("remoteUrl", &[Str]),
];

// Las rutas son el `instancePath` de Ajv sin la barra inicial, que es como las
// muestra `conf`.
fn child_path(base: &str, segment: &str) -> String {
    if base.is_empty() {
        segment.to_string()
    } else {
        format!("{base}/{segment}")
    }
}

fn check_type(path: &str, value: &Value, types: &[JsonType], errors: &mut Vec<String>) -> bool {
    if types.iter().any(|kind| kind.matches(value)) {
        return true;
    }

    let names: Vec<&str> = types.iter().map(|kind| kind.name()).collect();
    errors.push(format!("`{path}` must be {}", names.join(",")));
    false
}

fn check_object(path: &str, value: &Value, properties: Properties, errors: &mut Vec<String>) {
    if !check_type(path, value, &[Object], errors) {
        return;
    }

    for (name, types) in properties {
        if let Some(property) = value.get(name) {
            check_type(&child_path(path, name), property, types, errors);
        }
    }
}

fn check_array_of_objects(
    path: &str,
    value: &Value,
    properties: Properties,
    errors: &mut Vec<String>,
) {
    if !check_type(path, value, &[Array], errors) {
        return;
    }

    for (index, item) in value.as_array().into_iter().flatten().enumerate() {
        check_object(
            &child_path(path, &index.to_string()),
            item,
            properties,
            errors,
        );
    }
}

fn check_workspace_session(path: &str, value: &mut Value, errors: &mut Vec<String>) {
    if !check_type(path, value, &[Object], errors) {
        return;
    }

    if let Some(session) = value.as_object_mut() {
        // Defaults de las propiedades (Ajv `useDefaults`).
        session
            .entry("repositoryWorkspaces")
            .or_insert_with(|| json!({}));
        session.entry("activeRepositoryPath").or_insert(Value::Null);
    }

    if let Some(workspaces) = value.get("repositoryWorkspaces") {
        check_type(
            &child_path(path, "repositoryWorkspaces"),
            workspaces,
            &[Object],
            errors,
        );
    }

    if let Some(open) = value.get("openRepositories") {
        let open_path = child_path(path, "openRepositories");
        if check_type(&open_path, open, &[Array], errors) {
            for (index, item) in open.as_array().into_iter().flatten().enumerate() {
                check_type(
                    &child_path(&open_path, &index.to_string()),
                    item,
                    &[Str],
                    errors,
                );
            }
        }
    }

    if let Some(active) = value.get("activeRepositoryPath") {
        check_type(
            &child_path(path, "activeRepositoryPath"),
            active,
            &[Str, Null],
            errors,
        );
    }
}

/// Valida el valor de una clave del esquema (aplicando los defaults anidados)
/// y devuelve los errores. Una clave que no es del esquema siempre es válida.
pub(crate) fn validate_key(key: &str, value: &mut Value) -> Vec<String> {
    let mut errors = Vec::new();
    match key {
        GITHUB_ACCOUNTS => {
            check_array_of_objects(key, value, GITHUB_ACCOUNT_PROPERTIES, &mut errors)
        }
        ACTIVE_GITHUB_ACCOUNT_ID => {
            check_type(key, value, &[Str, Null], &mut errors);
        }
        RECENT_REPOSITORIES => {
            check_array_of_objects(key, value, RECENT_REPOSITORY_PROPERTIES, &mut errors)
        }
        WORKSPACE_SESSION => check_workspace_session(key, value, &mut errors),
        SETTINGS | STARRED_BRANCHES => {
            check_type(key, value, &[Object], &mut errors);
        }
        _ => {}
    }
    errors
}

/// `true` si un elemento de `githubAccounts` o `recentRepositories` cumple el
/// esquema de sus elementos.
pub(crate) fn is_valid_item(key: &str, item: &Value) -> bool {
    let properties = match key {
        GITHUB_ACCOUNTS => GITHUB_ACCOUNT_PROPERTIES,
        RECENT_REPOSITORIES => RECENT_REPOSITORY_PROPERTIES,
        _ => return true,
    };
    let mut errors = Vec::new();
    check_object("", item, properties, &mut errors);
    errors.is_empty()
}

fn default_for(key: &str) -> Value {
    match key {
        GITHUB_ACCOUNTS | RECENT_REPOSITORIES => json!([]),
        WORKSPACE_SESSION => default_workspace_session(),
        SETTINGS => default_settings(),
        STARRED_BRANCHES => json!({}),
        _ => Value::Null,
    }
}

fn apply_defaults(root: &mut Map<String, Value>) {
    for key in SCHEMA_KEYS {
        if !root.contains_key(key) {
            root.insert(key.to_string(), default_for(key));
        }
    }

    if let Some(session) = root.get_mut(WORKSPACE_SESSION) {
        // Solo para completar sus defaults anidados; los errores no interesan aquí.
        check_workspace_session(WORKSPACE_SESSION, session, &mut Vec::new());
    }
}

/// Completa los defaults que falten y valida el documento entero. Devuelve los
/// errores en el orden en que los daría Ajv.
pub(crate) fn apply_defaults_and_validate(root: &mut Map<String, Value>) -> Vec<String> {
    apply_defaults(root);

    let mut errors = Vec::new();
    for key in SCHEMA_KEYS {
        if let Some(value) = root.get_mut(key) {
            errors.extend(validate_key(key, value));
        }
    }
    errors
}

pub(crate) fn schema_violation(errors: &[String]) -> StoreError {
    StoreError::Schema(format!("Config schema violation: {}", errors.join("; ")))
}

/// Valida un documento de configuración completo contra el esquema, sin
/// modificarlo.
pub fn validate_config(config: &Value) -> Result<(), StoreError> {
    let Some(root) = config.as_object() else {
        return Err(schema_violation(&["`` must be object".to_string()]));
    };

    let errors = apply_defaults_and_validate(&mut root.clone());
    if errors.is_empty() {
        Ok(())
    } else {
        Err(schema_violation(&errors))
    }
}

/// Valida una cuenta contra el esquema de los elementos de `githubAccounts`:
/// es lo que decide si `save_github_account` la acepta.
pub fn validate_github_account(account: &Value) -> Result<(), StoreError> {
    let mut errors = Vec::new();
    check_object("", account, GITHUB_ACCOUNT_PROPERTIES, &mut errors);
    if errors.is_empty() {
        Ok(())
    } else {
        Err(schema_violation(&errors))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn errors_of(mut config: Value) -> Vec<String> {
        let root = config.as_object_mut().expect("object");
        apply_defaults_and_validate(root)
    }

    #[test]
    fn an_empty_document_gets_every_default() {
        assert_eq!(
            default_config(),
            json!({
                "githubAccounts": [],
                "activeGithubAccountId": null,
                "recentRepositories": [],
                "workspaceSession": {
                    "openRepositories": [],
                    "activeRepositoryPath": null,
                    "repositoryWorkspaces": {}
                },
                "settings": {
                    "theme": "light",
                    "autoFetch": true,
                    "maxRecentRepos": 10,
                    "sidebarWidth": null,
                    "sidebarSections": {
                        "starred": true,
                        "branches": true,
                        "remotes": false,
                        "tags": false,
                        "stashes": false,
                        "submodules": false
                    },
                    "commitView": {
                        "historyWidth": null,
                        "fileListWidth": null,
                        "diffViewMode": "unified",
                        "ignoreWhitespace": false
                    }
                },
                "starredBranches": {}
            })
        );
    }

    #[test]
    fn nested_workspace_session_defaults_do_not_invent_open_repositories() {
        let mut root = json!({ "workspaceSession": {} });
        let errors = apply_defaults_and_validate(root.as_object_mut().unwrap());
        assert!(errors.is_empty());
        assert_eq!(
            root["workspaceSession"],
            json!({ "repositoryWorkspaces": {}, "activeRepositoryPath": null })
        );
    }

    // Los mensajes esperados salen de ejecutar Ajv (la versión que usa `conf`)
    // con el esquema de `store.js` sobre estos mismos documentos.
    #[test]
    fn reports_the_same_messages_as_ajv() {
        assert_eq!(
            errors_of(json!({
                "githubAccounts": [
                    { "expiresAt": 123, "id": null, "activeInstallationId": "x", "installations": {} },
                    5
                ],
                "activeGithubAccountId": 3
            })),
            vec![
                "`githubAccounts/0/id` must be string",
                "`githubAccounts/0/expiresAt` must be string,null",
                "`githubAccounts/0/activeInstallationId` must be number,null",
                "`githubAccounts/0/installations` must be array",
                "`githubAccounts/1` must be object",
                "`activeGithubAccountId` must be string,null",
            ]
        );

        assert_eq!(
            errors_of(json!({
                "recentRepositories": {},
                "settings": [],
                "starredBranches": null,
                "workspaceSession": {
                    "openRepositories": [1, "a"],
                    "activeRepositoryPath": 4,
                    "repositoryWorkspaces": []
                }
            })),
            vec![
                "`recentRepositories` must be array",
                "`workspaceSession/repositoryWorkspaces` must be object",
                "`workspaceSession/openRepositories/0` must be string",
                "`workspaceSession/activeRepositoryPath` must be string,null",
                "`settings` must be object",
                "`starredBranches` must be object",
            ]
        );

        assert_eq!(
            errors_of(json!({ "workspaceSession": [] })),
            vec!["`workspaceSession` must be object"]
        );

        assert_eq!(
            errors_of(json!({ "recentRepositories": [{ "isGitHub": "no", "path": 1 }] })),
            vec![
                "`recentRepositories/0/path` must be string",
                "`recentRepositories/0/isGitHub` must be boolean",
            ]
        );
    }

    #[test]
    fn validate_config_does_not_touch_the_document() {
        let config = json!({ "settings": {} });
        assert!(validate_config(&config).is_ok());
        assert_eq!(config, json!({ "settings": {} }));

        let error = validate_config(&json!([])).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Config schema violation: `` must be object"
        );

        let error = validate_config(&json!({ "settings": 1, "starredBranches": [] })).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Config schema violation: `settings` must be object; `starredBranches` must be object"
        );
    }
}
