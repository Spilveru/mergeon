//! Máquina de estados de `createAppUpdater`, sin hilos ni red.
//!
//! En el original los eventos venían de `autoUpdater` (electron-updater); aquí
//! los produce el motor de `AppUpdater` y entran por `handle`.

use crate::error_summary::summarize_updater_error;
use crate::state::{ErrorPhase, RELEASES_URL, UpdateStatus, UpdaterState};

/// Eventos del motor de actualización, los mismos que emitía `autoUpdater`.
#[derive(Debug, Clone, PartialEq)]
pub enum UpdateEvent {
    /// `checking-for-update`
    CheckingForUpdate,
    /// `update-available`
    UpdateAvailable { version: Option<String> },
    /// `update-not-available`
    UpdateNotAvailable { version: Option<String> },
    /// `download-progress`
    DownloadProgress { percent: f64 },
    /// `update-downloaded`
    UpdateDownloaded { version: Option<String> },
    /// `error`
    Error { message: String },
}

/// Qué hacer tras `begin_check`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckStart {
    /// No se consulta nada: build de desarrollo o sin canal. Lleva el payload emitido.
    Blocked(UpdaterState),
    /// Hay que consultar el feed.
    Ready,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateMachineConfig {
    /// `app.getVersion()`.
    pub current_version: Option<String>,
    /// `app.isPackaged`.
    pub is_packaged: bool,
    pub updates_configured: bool,
    pub can_auto_install: bool,
    pub releases_url: String,
}

impl Default for StateMachineConfig {
    fn default() -> Self {
        Self {
            current_version: None,
            is_packaged: false,
            updates_configured: false,
            can_auto_install: false,
            releases_url: RELEASES_URL.to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct UpdaterStateMachine {
    state: UpdaterState,
    pending_manual_check: bool,
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|text| !text.is_empty())
}

impl UpdaterStateMachine {
    pub fn new(config: StateMachineConfig) -> Self {
        Self {
            state: UpdaterState {
                status: UpdateStatus::Idle,
                current_version: non_empty(config.current_version),
                latest_version: None,
                percent: 0,
                error: None,
                error_phase: None,
                can_auto_install: config.can_auto_install,
                releases_url: config.releases_url,
                updates_configured: config.updates_configured,
                is_packaged: config.is_packaged,
                manual_check: false,
            },
            pending_manual_check: false,
        }
    }

    /// `getState()`.
    pub fn state(&self) -> UpdaterState {
        self.state.clone()
    }

    pub fn status(&self) -> UpdateStatus {
        self.state.status
    }

    /// Condición de `installUpdate()`: plataforma auto-instalable y descarga lista.
    pub fn can_install(&self) -> bool {
        self.state.can_auto_install && self.state.status == UpdateStatus::Downloaded
    }

    fn reset_manual_check(&mut self) {
        self.pending_manual_check = false;
        self.state.manual_check = false;
    }

    /// Preámbulo de `checkForUpdates({ manual })`.
    pub fn begin_check(&mut self, manual: bool) -> CheckStart {
        if !self.state.is_packaged || !self.state.updates_configured {
            self.state.status = if self.state.is_packaged {
                UpdateStatus::NotConfigured
            } else {
                UpdateStatus::Unsupported
            };
            self.state.error = None;
            self.state.error_phase = None;
            self.state.latest_version = None;
            self.state.manual_check = manual;
            let payload = self.state.clone();
            self.reset_manual_check();
            return CheckStart::Blocked(payload);
        }

        self.pending_manual_check = self.pending_manual_check || manual;
        CheckStart::Ready
    }

    /// Aplica un evento del motor y devuelve el payload que se emite
    /// (`updater:event`). El estado puede diferir del payload justo después:
    /// `manualCheck` se limpia tras emitir, como en el original.
    pub fn handle(&mut self, event: UpdateEvent) -> UpdaterState {
        match event {
            UpdateEvent::CheckingForUpdate => {
                self.state.status = UpdateStatus::Checking;
                self.state.error = None;
                self.state.error_phase = None;
                self.state.manual_check = self.pending_manual_check;
                self.state.clone()
            }
            UpdateEvent::UpdateAvailable { version } => {
                self.state.status = UpdateStatus::Available;
                self.state.latest_version =
                    non_empty(version).or_else(|| self.state.latest_version.take());
                self.state.percent = 0;
                self.state.error = None;
                self.state.error_phase = None;
                self.state.manual_check = self.pending_manual_check;
                self.state.clone()
            }
            UpdateEvent::UpdateNotAvailable { version } => {
                let manual_check = self.pending_manual_check;
                self.state.status = UpdateStatus::NotAvailable;
                self.state.latest_version = non_empty(version)
                    .or_else(|| self.state.current_version.clone())
                    .or_else(|| self.state.latest_version.take());
                self.state.percent = 0;
                self.state.error = None;
                self.state.error_phase = None;
                self.state.manual_check = manual_check;
                let payload = self.state.clone();
                if manual_check {
                    self.reset_manual_check();
                }
                payload
            }
            UpdateEvent::DownloadProgress { percent } => {
                self.state.status = UpdateStatus::Downloading;
                self.state.percent = round_percent(percent);
                self.state.error_phase = None;
                self.state.clone()
            }
            UpdateEvent::UpdateDownloaded { version } => {
                self.state.status = UpdateStatus::Downloaded;
                self.state.latest_version =
                    non_empty(version).or_else(|| self.state.latest_version.take());
                self.state.percent = 100;
                self.state.error_phase = None;
                self.state.manual_check = self.pending_manual_check;
                let payload = self.state.clone();
                self.reset_manual_check();
                payload
            }
            UpdateEvent::Error { message } => {
                let manual_check = self.pending_manual_check;
                let has_newer_version = self
                    .state
                    .latest_version
                    .as_ref()
                    .is_some_and(|latest| Some(latest) != self.state.current_version.as_ref());
                let error_phase = if self.state.status == UpdateStatus::Available
                    || self.state.status == UpdateStatus::Downloading
                    || has_newer_version
                {
                    ErrorPhase::Download
                } else {
                    ErrorPhase::Check
                };
                self.state.status = UpdateStatus::Error;
                self.state.error = Some(summarize_updater_error(&message));
                self.state.error_phase = Some(error_phase);
                self.state.manual_check = manual_check;
                let payload = self.state.clone();
                if manual_check {
                    self.reset_manual_check();
                }
                payload
            }
        }
    }
}

/// `Math.round(progress?.percent || 0)`, acotado a 0–100.
fn round_percent(percent: f64) -> u8 {
    if !percent.is_finite() {
        return 0;
    }
    // `Math.round` redondea el .5 hacia arriba; con valores positivos coincide
    // con `floor(x + 0.5)`.
    (percent + 0.5).floor().clamp(0.0, 100.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error_summary::tests::GITHUB_404_MESSAGE;

    fn machine(current_version: Option<&str>, can_auto_install: bool) -> UpdaterStateMachine {
        UpdaterStateMachine::new(StateMachineConfig {
            current_version: current_version.map(str::to_string),
            is_packaged: true,
            updates_configured: true,
            can_auto_install,
            ..StateMachineConfig::default()
        })
    }

    fn version(text: &str) -> Option<String> {
        Some(text.to_string())
    }

    #[test]
    fn starts_idle_with_the_configured_flags() {
        let state = machine(Some("0.9.0"), true).state();
        assert_eq!(
            state,
            UpdaterState {
                status: UpdateStatus::Idle,
                current_version: version("0.9.0"),
                latest_version: None,
                percent: 0,
                error: None,
                error_phase: None,
                can_auto_install: true,
                releases_url: RELEASES_URL.to_string(),
                updates_configured: true,
                is_packaged: true,
                manual_check: false,
            }
        );
        // `app.getVersion() || null`: una versión vacía es nula.
        assert_eq!(machine(Some(""), false).state().current_version, None);
    }

    #[test]
    fn preserves_the_latest_version_from_update_not_available_for_manual_checks() {
        let mut machine = machine(Some("0.1.6"), false);

        assert_eq!(machine.begin_check(true), CheckStart::Ready);
        let checking = machine.handle(UpdateEvent::CheckingForUpdate);
        assert_eq!(checking.status, UpdateStatus::Checking);
        assert!(checking.manual_check);

        let payload = machine.handle(UpdateEvent::UpdateNotAvailable {
            version: version("0.1.6"),
        });
        // El evento todavía lleva la marca de comprobación manual...
        assert!(payload.manual_check);

        // ...y el estado ya la tiene limpia.
        let state = machine.state();
        assert_eq!(state.status, UpdateStatus::NotAvailable);
        assert_eq!(state.current_version, version("0.1.6"));
        assert_eq!(state.latest_version, version("0.1.6"));
        assert!(!state.manual_check);
    }

    #[test]
    fn update_not_available_falls_back_to_the_current_version() {
        let mut machine = machine(Some("0.3.0"), false);
        machine.handle(UpdateEvent::UpdateNotAvailable { version: None });
        assert_eq!(machine.state().latest_version, version("0.3.0"));

        let mut unknown = UpdaterStateMachine::new(StateMachineConfig {
            is_packaged: true,
            updates_configured: true,
            ..StateMachineConfig::default()
        });
        unknown.handle(UpdateEvent::UpdateAvailable {
            version: version("0.4.0"),
        });
        unknown.handle(UpdateEvent::UpdateNotAvailable {
            version: Some(String::new()),
        });
        assert_eq!(unknown.state().latest_version, version("0.4.0"));
    }

    #[test]
    fn reports_no_update_channel_instead_of_querying_the_private_github_feed() {
        let mut machine = UpdaterStateMachine::new(StateMachineConfig {
            current_version: version("0.2.1"),
            is_packaged: true,
            updates_configured: false,
            ..StateMachineConfig::default()
        });

        let CheckStart::Blocked(payload) = machine.begin_check(true) else {
            panic!("sin canal no se consulta nada");
        };
        assert_eq!(payload.status, UpdateStatus::NotConfigured);
        assert!(payload.manual_check);

        let state = machine.state();
        assert_eq!(state.status, UpdateStatus::NotConfigured);
        assert!(!state.updates_configured);
        assert_eq!(state.error, None);
        assert!(!state.manual_check);
    }

    #[test]
    fn emits_the_condensed_message_to_the_renderer() {
        let mut machine = machine(Some("0.2.1"), false);

        let payload = machine.handle(UpdateEvent::Error {
            message: GITHUB_404_MESSAGE.to_string(),
        });

        let expected = "No se encontró el canal de actualizaciones (404). Revisa que las releases del repositorio sean públicas.";
        assert_eq!(payload.error.as_deref(), Some(expected));
        assert_eq!(machine.state().error.as_deref(), Some(expected));
    }

    #[test]
    fn classifies_a_failed_check_separately_from_a_failed_download() {
        let mut machine = machine(Some("0.1.9"), false);

        machine.handle(UpdateEvent::UpdateNotAvailable {
            version: version("0.1.9"),
        });
        machine.handle(UpdateEvent::Error {
            message: "feed unavailable".to_string(),
        });

        let state = machine.state();
        assert_eq!(state.status, UpdateStatus::Error);
        assert_eq!(state.latest_version, version("0.1.9"));
        assert_eq!(state.error_phase, Some(ErrorPhase::Check));

        machine.handle(UpdateEvent::UpdateAvailable {
            version: version("0.2.0"),
        });
        machine.handle(UpdateEvent::Error {
            message: "download failed".to_string(),
        });

        let state = machine.state();
        assert_eq!(state.status, UpdateStatus::Error);
        assert_eq!(state.latest_version, version("0.2.0"));
        assert_eq!(state.error_phase, Some(ErrorPhase::Download));
        assert_eq!(state.error.as_deref(), Some("download failed"));
    }

    #[test]
    fn a_failure_while_downloading_is_a_download_error() {
        let mut machine = machine(None, true);
        machine.handle(UpdateEvent::DownloadProgress { percent: 10.0 });
        let payload = machine.handle(UpdateEvent::Error {
            message: "socket hang up".to_string(),
        });
        assert_eq!(payload.error_phase, Some(ErrorPhase::Download));
    }

    #[test]
    fn leaves_restart_decisions_to_the_in_app_updater_indicator() {
        let mut machine = machine(None, false);

        assert_eq!(machine.begin_check(true), CheckStart::Ready);
        let payload = machine.handle(UpdateEvent::UpdateDownloaded {
            version: version("0.2.0"),
        });
        assert!(payload.manual_check);

        let state = machine.state();
        assert_eq!(state.status, UpdateStatus::Downloaded);
        assert_eq!(state.latest_version, version("0.2.0"));
        assert_eq!(state.percent, 100);
        assert!(!state.manual_check);
    }

    #[test]
    fn emits_status_events_and_tracks_them_in_get_state() {
        let mut machine = machine(None, true);

        machine.handle(UpdateEvent::UpdateAvailable {
            version: version("0.2.0"),
        });
        let payload = machine.handle(UpdateEvent::DownloadProgress { percent: 42.6 });

        let state = machine.state();
        assert_eq!(state.status, UpdateStatus::Downloading);
        assert_eq!(state.latest_version, version("0.2.0"));
        assert_eq!(state.percent, 43);
        assert!(state.can_auto_install);

        assert_eq!(payload.status, UpdateStatus::Downloading);
        assert_eq!(payload.percent, 43);
        assert_eq!(payload, state);
    }

    #[test]
    fn rounds_progress_like_math_round() {
        assert_eq!(round_percent(42.6), 43);
        assert_eq!(round_percent(42.5), 43);
        assert_eq!(round_percent(42.4), 42);
        assert_eq!(round_percent(0.0), 0);
        assert_eq!(round_percent(f64::NAN), 0);
        assert_eq!(round_percent(250.0), 100);
        assert_eq!(round_percent(-3.0), 0);
    }

    #[test]
    fn install_is_only_allowed_once_downloaded_on_a_supported_platform() {
        let mut supported = machine(None, true);
        assert!(!supported.can_install());
        supported.handle(UpdateEvent::UpdateDownloaded {
            version: version("0.2.0"),
        });
        assert!(supported.can_install());

        let mut notify_only = machine(None, false);
        notify_only.handle(UpdateEvent::UpdateDownloaded {
            version: version("0.2.0"),
        });
        assert!(!notify_only.can_install());
    }

    #[test]
    fn reports_development_and_unconfigured_builds_without_attempting_an_update() {
        let mut dev = UpdaterStateMachine::new(StateMachineConfig {
            current_version: version("0.1.6"),
            is_packaged: false,
            updates_configured: false,
            ..StateMachineConfig::default()
        });
        assert!(matches!(dev.begin_check(true), CheckStart::Blocked(_)));
        let state = dev.state();
        assert_eq!(state.status, UpdateStatus::Unsupported);
        assert!(!state.is_packaged);

        // Un build de desarrollo no consulta aunque tenga canal configurado.
        let mut dev_with_feed = UpdaterStateMachine::new(StateMachineConfig {
            is_packaged: false,
            updates_configured: true,
            ..StateMachineConfig::default()
        });
        assert!(matches!(
            dev_with_feed.begin_check(false),
            CheckStart::Blocked(_)
        ));
        assert_eq!(dev_with_feed.status(), UpdateStatus::Unsupported);

        let mut packaged = UpdaterStateMachine::new(StateMachineConfig {
            current_version: version("0.1.6"),
            is_packaged: true,
            updates_configured: false,
            ..StateMachineConfig::default()
        });
        assert!(matches!(packaged.begin_check(true), CheckStart::Blocked(_)));
        let state = packaged.state();
        assert_eq!(state.status, UpdateStatus::NotConfigured);
        assert!(!state.updates_configured);
    }

    #[test]
    fn a_background_check_keeps_the_manual_flag_requested_meanwhile() {
        let mut machine = machine(Some("0.1.0"), true);

        assert_eq!(machine.begin_check(false), CheckStart::Ready);
        assert!(!machine.handle(UpdateEvent::CheckingForUpdate).manual_check);

        // El usuario pulsa "Check for updates" con la comprobación ya en vuelo.
        assert_eq!(machine.begin_check(true), CheckStart::Ready);
        let available = machine.handle(UpdateEvent::UpdateAvailable {
            version: version("0.2.0"),
        });
        assert!(available.manual_check);
        // `update-available` no limpia la marca: sigue hasta que termina la descarga.
        assert!(machine.state().manual_check);

        let downloaded = machine.handle(UpdateEvent::UpdateDownloaded { version: None });
        assert!(downloaded.manual_check);
        assert_eq!(downloaded.latest_version, version("0.2.0"));
        assert!(!machine.state().manual_check);
    }

    #[test]
    fn a_new_check_clears_the_previous_error() {
        let mut machine = machine(Some("0.1.0"), true);
        machine.handle(UpdateEvent::Error {
            message: "feed unavailable".to_string(),
        });
        let checking = machine.handle(UpdateEvent::CheckingForUpdate);
        assert_eq!(checking.error, None);
        assert_eq!(checking.error_phase, None);
    }
}
