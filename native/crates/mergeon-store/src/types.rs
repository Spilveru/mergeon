//! Formas de los datos guardados, con los nombres de campo del JSON.
//!
//! La lectura es tolerante: un campo conocido con un tipo inesperado toma su
//! default en vez de invalidar el objeto entero, y las claves desconocidas se
//! conservan en `extra`.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};

fn lenient<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned + Default,
{
    let value = Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(value).unwrap_or_default())
}

fn lenient_vec<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    Ok(match Value::deserialize(deserializer)? {
        Value::Array(items) => items
            .into_iter()
            .filter_map(|item| serde_json::from_value(item).ok())
            .collect(),
        _ => Vec::new(),
    })
}

/// Una instalación de la GitHub App visible para la cuenta
/// (`fetchInstallations` en `github-auth-client.js`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitHubInstallation {
    #[serde(default, deserialize_with = "lenient")]
    pub id: u64,
    /// Login de la cuenta u organización donde está instalada.
    #[serde(default, deserialize_with = "lenient")]
    pub account: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    pub avatar_url: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    pub repository_selection: Option<String>,
    /// Claves que este tipo no conoce; se conservan.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Una cuenta de GitHub (`githubAccounts[]`).
///
/// En el archivo solo se guardan los `*Cipher`; `token`, `refresh_token` y
/// `session_token` son los valores descifrados que devuelve el store. Al
/// guardar, un token en claro no vacío manda sobre su `*Cipher`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitHubAccount {
    #[serde(default, deserialize_with = "lenient")]
    pub id: String,
    #[serde(default, deserialize_with = "lenient")]
    pub username: String,
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub name: Option<String>,
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub email: Option<String>,
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub avatar_url: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    pub token: String,
    #[serde(default, deserialize_with = "lenient")]
    pub token_cipher: String,
    #[serde(default, deserialize_with = "lenient")]
    pub refresh_token: String,
    #[serde(default, deserialize_with = "lenient")]
    pub refresh_token_cipher: String,
    /// Legado del backend propio: solo sirve para detectar cuentas antiguas
    /// y pedir que se reconecten.
    #[serde(default, deserialize_with = "lenient")]
    pub session_token: String,
    #[serde(default, deserialize_with = "lenient")]
    pub session_token_cipher: String,
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub is_personal_access_token: Option<bool>,
    /// Los tokens de OAuth App y los personales no caducan: llegan sin fecha.
    #[serde(default, deserialize_with = "lenient")]
    pub expires_at: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    pub install_url: Option<String>,
    /// Solo las cuentas de GitHub App necesitan instalación. `None` = la cuenta
    /// no trae la marca y quien la consume la deduce.
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub requires_installation: Option<bool>,
    #[serde(default, deserialize_with = "lenient")]
    pub requires_reconnect: bool,
    #[serde(default, deserialize_with = "lenient")]
    pub active_installation_id: Option<u64>,
    #[serde(default, deserialize_with = "lenient_vec")]
    pub installations: Vec<GitHubInstallation>,
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub added_at: Option<String>,
    /// Claves que este tipo no conoce (p. ej. `displayName`); se conservan.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Un repositorio reciente (`recentRepositories[]`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentRepository {
    #[serde(default, deserialize_with = "lenient")]
    pub id: String,
    #[serde(default, deserialize_with = "lenient")]
    pub name: String,
    #[serde(default, deserialize_with = "lenient")]
    pub path: String,
    /// Fecha ISO de la última apertura. La pone el store al añadirlo.
    #[serde(default, deserialize_with = "lenient")]
    pub last_opened: String,
    #[serde(default, deserialize_with = "lenient")]
    pub current_branch: String,
    #[serde(default, rename = "isGitHub", deserialize_with = "lenient")]
    pub is_github: bool,
    #[serde(default, deserialize_with = "lenient")]
    pub remote_url: String,
    /// Claves que este tipo no conoce; se conservan.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Qué repositorios estaban abiertos y cuál estaba activo (`workspaceSession`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSession {
    /// Estado de la interfaz por repositorio (pestañas abiertas, commit
    /// seleccionado...), por ruta. El store no lo interpreta.
    #[serde(default, deserialize_with = "lenient")]
    pub repository_workspaces: Map<String, Value>,
    #[serde(default, deserialize_with = "lenient_vec")]
    pub open_repositories: Vec<String>,
    #[serde(default, deserialize_with = "lenient")]
    pub active_repository_path: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn account_uses_the_json_field_names_and_keeps_unknown_keys() {
        let account: GitHubAccount = serde_json::from_value(json!({
            "id": "7",
            "username": "octocat",
            "displayName": "The Octocat",
            "avatarUrl": "https://avatar",
            "tokenCipher": "plain:gho_token",
            "refreshToken": null,
            "expiresAt": null,
            "activeInstallationId": 42,
            "installations": [
                { "id": 42, "account": "octocat", "avatarUrl": null, "repositorySelection": "all" },
                "garbage"
            ],
            "requiresReconnect": false
        }))
        .unwrap();

        assert_eq!(account.id, "7");
        assert_eq!(account.avatar_url.as_deref(), Some("https://avatar"));
        assert_eq!(account.token_cipher, "plain:gho_token");
        assert_eq!(account.refresh_token, "");
        assert_eq!(account.active_installation_id, Some(42));
        assert_eq!(account.installations.len(), 1);
        assert_eq!(
            account.installations[0].repository_selection.as_deref(),
            Some("all")
        );
        assert_eq!(account.requires_installation, None);
        assert_eq!(account.extra["displayName"], "The Octocat");

        let json = serde_json::to_value(&account).unwrap();
        assert_eq!(json["displayName"], "The Octocat");
        assert_eq!(json["avatarUrl"], "https://avatar");
        assert_eq!(json["expiresAt"], json!(null));
        assert_eq!(json["activeInstallationId"], 42);
        // Lo que el esquema no admite como null no se escribe si falta.
        assert!(json.get("name").is_none());
        assert!(json.get("requiresInstallation").is_none());
    }

    #[test]
    fn recent_repository_keeps_the_original_casing_of_is_github() {
        let repo = RecentRepository {
            id: "/repo".into(),
            name: "repo".into(),
            path: "/repo".into(),
            current_branch: "main".into(),
            is_github: true,
            ..Default::default()
        };

        let json = serde_json::to_value(&repo).unwrap();
        assert_eq!(json["isGitHub"], true);
        assert_eq!(json["currentBranch"], "main");
        assert_eq!(json["remoteUrl"], "");

        let back: RecentRepository =
            serde_json::from_value(json!({ "path": "/x", "isGitHub": "yes", "pinned": true }))
                .unwrap();
        assert_eq!(back.path, "/x");
        assert!(!back.is_github);
        assert_eq!(back.extra["pinned"], true);
    }

    #[test]
    fn workspace_session_reads_leniently() {
        let session: WorkspaceSession = serde_json::from_value(json!({
            "openRepositories": ["/a", 5, "/b"],
            "activeRepositoryPath": 3,
            "repositoryWorkspaces": []
        }))
        .unwrap();

        assert_eq!(session.open_repositories, vec!["/a", "/b"]);
        assert_eq!(session.active_repository_path, None);
        assert!(session.repository_workspaces.is_empty());
    }
}
