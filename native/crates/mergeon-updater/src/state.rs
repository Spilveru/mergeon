use std::time::Duration;

use serde::{Deserialize, Serialize};

pub const DEFAULT_UPDATE_CHANNEL: &str = "latest";
pub const RELEASES_URL: &str = "https://github.com/Spilveru/mergeon/releases/latest";

/// Canal por el que el proceso principal empujaba el estado al renderer.
pub const UPDATER_EVENT_CHANNEL: &str = "updater:event";

// Re-check periodically so long-running sessions still pick up new releases.
pub const UPDATE_CHECK_INTERVAL: Duration = Duration::from_millis(4 * 60 * 60 * 1000);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UpdateStatus {
    #[default]
    Idle,
    Checking,
    Available,
    Downloading,
    Downloaded,
    Error,
    NotAvailable,
    NotConfigured,
    Unsupported,
}

impl UpdateStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Checking => "checking",
            Self::Available => "available",
            Self::Downloading => "downloading",
            Self::Downloaded => "downloaded",
            Self::Error => "error",
            Self::NotAvailable => "not-available",
            Self::NotConfigured => "not-configured",
            Self::Unsupported => "unsupported",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ErrorPhase {
    Check,
    Download,
}

impl ErrorPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Check => "check",
            Self::Download => "download",
        }
    }
}

/// Payload de `getState()` y de cada `updater:event`.
///
/// `Default` son los valores iniciales del store del renderer
/// (`src/stores/updater.js`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UpdaterState {
    pub status: UpdateStatus,
    pub current_version: Option<String>,
    pub latest_version: Option<String>,
    pub percent: u8,
    pub error: Option<String>,
    pub error_phase: Option<ErrorPhase>,
    pub can_auto_install: bool,
    pub releases_url: String,
    pub updates_configured: bool,
    pub is_packaged: bool,
    pub manual_check: bool,
}

impl Default for UpdaterState {
    fn default() -> Self {
        Self {
            status: UpdateStatus::Idle,
            current_version: None,
            latest_version: None,
            percent: 0,
            error: None,
            error_phase: None,
            can_auto_install: false,
            releases_url: RELEASES_URL.to_string(),
            updates_configured: false,
            is_packaged: false,
            manual_check: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InstallFailureReason {
    /// La plataforma no se instala sola o todavía no hay nada descargado.
    Unsupported,
    /// No hay actualizador (lo devolvía el handler IPC `updater:install`).
    Unavailable,
    /// El instalador falló. No existía en el original, donde el fallo de
    /// `quitAndInstall` solo llegaba como evento de error.
    Failed,
}

impl InstallFailureReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unsupported => "unsupported",
            Self::Unavailable => "unavailable",
            Self::Failed => "failed",
        }
    }
}

/// Resultado de `installUpdate()`: `{ ok: true }` o `{ ok: false, reason }`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallResult {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<InstallFailureReason>,
}

impl InstallResult {
    pub fn ok() -> Self {
        Self {
            ok: true,
            reason: None,
        }
    }

    pub fn failure(reason: InstallFailureReason) -> Self {
        Self {
            ok: false,
            reason: Some(reason),
        }
    }

    pub fn unsupported() -> Self {
        Self::failure(InstallFailureReason::Unsupported)
    }

    pub fn unavailable() -> Self {
        Self::failure(InstallFailureReason::Unavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_the_state_with_the_original_field_names() {
        let state = UpdaterState {
            status: UpdateStatus::NotAvailable,
            current_version: Some("0.9.0".to_string()),
            latest_version: Some("0.9.0".to_string()),
            error_phase: Some(ErrorPhase::Check),
            ..UpdaterState::default()
        };

        assert_eq!(
            serde_json::to_value(&state).unwrap(),
            serde_json::json!({
                "status": "not-available",
                "currentVersion": "0.9.0",
                "latestVersion": "0.9.0",
                "percent": 0,
                "error": null,
                "errorPhase": "check",
                "canAutoInstall": false,
                "releasesUrl": "https://github.com/Spilveru/mergeon/releases/latest",
                "updatesConfigured": false,
                "isPackaged": false,
                "manualCheck": false
            })
        );

        let partial: UpdaterState =
            serde_json::from_str(r#"{"status":"not-configured","percent":7}"#).unwrap();
        assert_eq!(partial.status, UpdateStatus::NotConfigured);
        assert_eq!(partial.percent, 7);
        assert_eq!(partial.releases_url, RELEASES_URL);
    }

    #[test]
    fn status_names_match_the_serialized_form() {
        for status in [
            UpdateStatus::Idle,
            UpdateStatus::Checking,
            UpdateStatus::Available,
            UpdateStatus::Downloading,
            UpdateStatus::Downloaded,
            UpdateStatus::Error,
            UpdateStatus::NotAvailable,
            UpdateStatus::NotConfigured,
            UpdateStatus::Unsupported,
        ] {
            assert_eq!(
                serde_json::to_value(status).unwrap(),
                serde_json::Value::String(status.as_str().to_string())
            );
        }
        assert_eq!(ErrorPhase::Download.as_str(), "download");
    }

    #[test]
    fn serializes_install_results_like_the_ipc_handler() {
        assert_eq!(
            serde_json::to_value(InstallResult::ok()).unwrap(),
            serde_json::json!({ "ok": true })
        );
        assert_eq!(
            serde_json::to_value(InstallResult::unsupported()).unwrap(),
            serde_json::json!({ "ok": false, "reason": "unsupported" })
        );
        assert_eq!(
            serde_json::to_value(InstallResult::unavailable()).unwrap(),
            serde_json::json!({ "ok": false, "reason": "unavailable" })
        );
    }
}
