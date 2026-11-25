use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Código de error del cliente de GitHub (`error.code` en el original).
///
/// GitHub puede devolver códigos propios en el device flow (`access_denied`,
/// `device_flow_disabled`...) que el original propaga tal cual: esos quedan en
/// `Other`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum GitHubErrorCode {
    /// `github_error`: respuesta no exitosa de GitHub.
    GithubError,
    /// `invalid_response`: el cuerpo no es un objeto JSON.
    InvalidResponse,
    /// `auth_required`: el token ya no sirve y hay que reconectar la cuenta.
    AuthRequired,
    /// `github_cancelled`: la petición se canceló desde fuera.
    GithubCancelled,
    /// `github_timeout`: la petición superó el tiempo máximo.
    GithubTimeout,
    /// `github_unavailable`: no se pudo llegar a GitHub.
    GithubUnavailable,
    /// `expired`: el código del device flow caducó.
    Expired,
    /// `invalid_token`: no se entregó un token personal.
    InvalidToken,
    /// `github_auth_error`: código por defecto de los canales `github:*` para
    /// errores sin código propio.
    GithubAuthError,
    /// `rate_limited`: se agotó el cupo de la API (o GitHub pidió esperar);
    /// cuándo se renueva lo dice [`crate::ResponseCache::rate_limit`].
    RateLimited,
    /// Cualquier otro código entregado por GitHub.
    Other(String),
}

impl GitHubErrorCode {
    pub fn as_str(&self) -> &str {
        match self {
            Self::GithubError => "github_error",
            Self::InvalidResponse => "invalid_response",
            Self::AuthRequired => "auth_required",
            Self::GithubCancelled => "github_cancelled",
            Self::GithubTimeout => "github_timeout",
            Self::GithubUnavailable => "github_unavailable",
            Self::Expired => "expired",
            Self::InvalidToken => "invalid_token",
            Self::GithubAuthError => "github_auth_error",
            Self::RateLimited => "rate_limited",
            Self::Other(code) => code,
        }
    }

    pub fn from_code(code: &str) -> Self {
        match code {
            "github_error" => Self::GithubError,
            "invalid_response" => Self::InvalidResponse,
            "auth_required" => Self::AuthRequired,
            "github_cancelled" => Self::GithubCancelled,
            "github_timeout" => Self::GithubTimeout,
            "github_unavailable" => Self::GithubUnavailable,
            "expired" => Self::Expired,
            "invalid_token" => Self::InvalidToken,
            "github_auth_error" => Self::GithubAuthError,
            "rate_limited" => Self::RateLimited,
            other => Self::Other(other.to_string()),
        }
    }
}

impl fmt::Display for GitHubErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for GitHubErrorCode {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for GitHubErrorCode {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let code = String::deserialize(deserializer)?;
        Ok(Self::from_code(&code))
    }
}

/// Error del cliente de GitHub: mismo trío `message` / `code` / `status` que
/// el `Error` que construía `createAuthServiceError`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitHubError {
    pub message: String,
    pub code: GitHubErrorCode,
    pub status: u16,
}

impl GitHubError {
    pub fn new(message: impl Into<String>, code: GitHubErrorCode, status: u16) -> Self {
        Self {
            message: message.into(),
            code,
            status,
        }
    }

    /// Equivalente a `toStructuredGitHubError(error, prefix)`: antepone el
    /// prefijo del canal al mensaje y conserva código y estado.
    pub fn with_prefix(self, prefix: &str) -> Self {
        if prefix.is_empty() {
            return self;
        }

        Self {
            message: format!("{prefix}: {}", self.message),
            code: self.code,
            status: self.status,
        }
    }

    /// El store trata estos fallos como permanentes y pide reconectar la
    /// cuenta (`isPermanentRefreshFailure` en `src/stores/github.js`).
    pub fn is_permanent_auth_failure(&self) -> bool {
        if matches!(self.code, GitHubErrorCode::AuthRequired)
            || self.code.as_str() == "session_expired"
        {
            return true;
        }

        let message = self.message.to_lowercase();
        if message.contains("session revoked")
            || message.contains("session expired")
            || message.contains("session not found")
        {
            return true;
        }

        self.status == 401
    }

    /// Fallos pasajeros: GitHub no responde o está saturado
    /// (`isTransientAuthFailure` en `src/stores/github.js`).
    pub fn is_transient_failure(&self) -> bool {
        matches!(
            self.code,
            GitHubErrorCode::GithubUnavailable | GitHubErrorCode::GithubTimeout
        ) || self.code.as_str() == "github_api_error"
            || self.status == 0
            || self.status == 408
            || self.status == 429
            || self.status >= 500
    }
}

impl fmt::Display for GitHubError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for GitHubError {}

pub type GitHubResult<T> = Result<T, GitHubError>;
