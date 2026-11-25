use std::fmt;

use serde::{Deserialize, Serialize};

const REDACTED: &str = "<redacted>";

fn is_false(value: &bool) -> bool {
    !*value
}

/// Respuesta de `startGitHubLogin`: el código que el usuario introduce en
/// `verification_uri`.
///
/// Los campos son opcionales porque el original copia la respuesta de GitHub
/// sin validarla; quien inicia el flujo debe comprobar que vengan `device_code`
/// y `verification_uri` (como hace `useGitHubAuthFlow`).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceLoginSession {
    #[serde(default)]
    pub device_code: Option<String>,
    #[serde(default)]
    pub user_code: Option<String>,
    #[serde(default)]
    pub verification_uri: Option<String>,
    /// Segundos de validez del código.
    #[serde(default)]
    pub expires_in: Option<u64>,
    pub poll_interval_ms: u64,
}

// El device code basta para canjear el token: no se imprime.
impl fmt::Debug for DeviceLoginSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeviceLoginSession")
            .field("device_code", &self.device_code.as_ref().map(|_| REDACTED))
            .field("user_code", &self.user_code)
            .field("verification_uri", &self.verification_uri)
            .field("expires_in", &self.expires_in)
            .field("poll_interval_ms", &self.poll_interval_ms)
            .finish()
    }
}

/// Respuesta del canal `github:auth:start`: la sesión del device flow más el
/// client id público.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthStartResult {
    #[serde(flatten)]
    pub session: DeviceLoginSession,
    pub client_id: String,
}

/// Instalación de la GitHub App visible para el usuario.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitHubInstallation {
    #[serde(default)]
    pub id: Option<u64>,
    /// Login de la cuenta donde está instalada la App.
    #[serde(default)]
    pub account: Option<String>,
    #[serde(default)]
    pub avatar_url: Option<String>,
    #[serde(default)]
    pub repository_selection: Option<String>,
}

/// Cuenta de GitHub tal como la arma `buildAccount`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitHubAccount {
    pub id: String,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub avatar_url: Option<String>,
    pub token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// Caducidad del token en ISO 8601 (UTC), o `None` si no caduca.
    #[serde(default)]
    pub expires_at: Option<String>,
    #[serde(default)]
    pub installations: Vec<GitHubInstallation>,
    #[serde(default)]
    pub active_installation_id: Option<u64>,
    #[serde(default)]
    pub install_url: Option<String>,
    #[serde(default)]
    pub requires_installation: bool,
    /// Solo se marca (y se serializa) en las cuentas dadas de alta con un
    /// token personal.
    #[serde(default, skip_serializing_if = "is_false")]
    pub is_personal_access_token: bool,
}

// Los tokens nunca se imprimen.
impl fmt::Debug for GitHubAccount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GitHubAccount")
            .field("id", &self.id)
            .field("username", &self.username)
            .field("display_name", &self.display_name)
            .field("avatar_url", &self.avatar_url)
            .field("token", &REDACTED)
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| REDACTED),
            )
            .field("expires_at", &self.expires_at)
            .field("installations", &self.installations)
            .field("active_installation_id", &self.active_installation_id)
            .field("install_url", &self.install_url)
            .field("requires_installation", &self.requires_installation)
            .field("is_personal_access_token", &self.is_personal_access_token)
            .finish()
    }
}

/// Respuesta de `refreshSession` y `authenticateWithToken`: `{ account }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountResult {
    pub account: GitHubAccount,
}

/// Estado del device flow (`status` en la respuesta de `pollGitHubLogin`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoginStatus {
    Pending,
    Completed,
    InstallationRequired,
}

impl LoginStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Completed => "completed",
            Self::InstallationRequired => "installation_required",
        }
    }
}

/// Respuesta de `pollGitHubLogin`. Se serializa con la misma forma que el
/// objeto JS: `{ status, pollIntervalMs? }` o `{ status, account, installUrl }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum PollLoginResult {
    /// El usuario todavía no autoriza. `poll_interval_ms` viene cuando GitHub
    /// pide bajar el ritmo (`slow_down`).
    Pending {
        #[serde(
            rename = "pollIntervalMs",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        poll_interval_ms: Option<u64>,
    },
    Completed {
        account: Box<GitHubAccount>,
        #[serde(rename = "installUrl", default)]
        install_url: Option<String>,
    },
    /// Autorizado, pero la GitHub App no está instalada en ninguna cuenta.
    InstallationRequired {
        account: Box<GitHubAccount>,
        #[serde(rename = "installUrl", default)]
        install_url: Option<String>,
    },
}

impl PollLoginResult {
    pub fn status(&self) -> LoginStatus {
        match self {
            Self::Pending { .. } => LoginStatus::Pending,
            Self::Completed { .. } => LoginStatus::Completed,
            Self::InstallationRequired { .. } => LoginStatus::InstallationRequired,
        }
    }

    pub fn account(&self) -> Option<&GitHubAccount> {
        match self {
            Self::Pending { .. } => None,
            Self::Completed { account, .. } | Self::InstallationRequired { account, .. } => {
                Some(account)
            }
        }
    }

    pub fn install_url(&self) -> Option<&str> {
        match self {
            Self::Pending { .. } => None,
            Self::Completed { install_url, .. }
            | Self::InstallationRequired { install_url, .. } => install_url.as_deref(),
        }
    }

    pub fn poll_interval_ms(&self) -> Option<u64> {
        match self {
            Self::Pending { poll_interval_ms } => *poll_interval_ms,
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepositoryOwner {
    #[serde(default)]
    pub login: Option<String>,
    #[serde(default)]
    pub avatar_url: Option<String>,
}

/// Repositorio normalizado, con los nombres de campo que usa el renderer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitHubRepository {
    #[serde(default)]
    pub id: Option<u64>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub full_name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub private: Option<bool>,
    #[serde(default)]
    pub clone_url: Option<String>,
    #[serde(default)]
    pub html_url: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub stars: Option<u64>,
    #[serde(default)]
    pub forks: Option<u64>,
    #[serde(default)]
    pub updated_at: Option<String>,
    pub owner: RepositoryOwner,
}

/// Respuesta de `getRepositories`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepositoryPage {
    pub repositories: Vec<GitHubRepository>,
    pub total_count: u64,
    #[serde(default)]
    pub installation_id: Option<u64>,
}

/// Respuesta del canal `github:searchRepositories`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepositorySearchResult {
    pub total_count: u64,
    pub repositories: Vec<GitHubRepository>,
}

/// Contexto de autenticación que el renderer pasa a los canales de
/// repositorios (`getGitAuthContext` en `src/stores/github.js`).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthContext {
    pub token: String,
    #[serde(default)]
    pub installation_id: Option<u64>,
}

impl AuthContext {
    pub fn new(token: impl Into<String>, installation_id: Option<u64>) -> Self {
        Self {
            token: token.into(),
            installation_id,
        }
    }
}

impl fmt::Debug for AuthContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AuthContext")
            .field("token", &REDACTED)
            .field("installation_id", &self.installation_id)
            .finish()
    }
}
