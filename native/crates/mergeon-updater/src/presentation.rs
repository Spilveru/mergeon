//! Lógica de presentación pura del renderer: los getters de
//! `src/stores/updater.js`, las decisiones de `src/composables/useUpdater.js`
//! y los textos de `src/components/layout/UpdateSettings.vue`.

use crate::state::{ErrorPhase, InstallResult, UpdateStatus, UpdaterState};

// --- src/stores/updater.js ---------------------------------------------------

impl UpdaterState {
    /// Acción `apply(payload)` del store: el payload manda, salvo la versión
    /// actual y la URL de releases, que se conservan si no vienen.
    pub fn apply(&mut self, payload: &UpdaterState) {
        self.status = payload.status;
        if let Some(current) = payload
            .current_version
            .as_ref()
            .filter(|value| !value.is_empty())
        {
            self.current_version = Some(current.clone());
        }
        self.latest_version = payload.latest_version.clone();
        self.percent = payload.percent;
        self.error = payload.error.clone();
        self.error_phase = payload.error_phase;
        self.can_auto_install = payload.can_auto_install;
        self.updates_configured = payload.updates_configured;
        self.is_packaged = payload.is_packaged;
        self.manual_check = payload.manual_check;
        if !payload.releases_url.is_empty() {
            self.releases_url = payload.releases_url.clone();
        }
    }

    /// Whether the indicator should be visible at all.
    pub fn is_visible(&self) -> bool {
        matches!(
            self.status,
            UpdateStatus::Available
                | UpdateStatus::Downloading
                | UpdateStatus::Downloaded
                | UpdateStatus::Error
        )
    }

    pub fn is_ready(&self) -> bool {
        self.status == UpdateStatus::Downloaded
    }

    pub fn is_busy(&self) -> bool {
        matches!(
            self.status,
            UpdateStatus::Checking | UpdateStatus::Available | UpdateStatus::Downloading
        )
    }

    pub fn is_up_to_date(&self) -> bool {
        match (&self.current_version, &self.latest_version) {
            (Some(current), Some(latest)) => {
                !current.is_empty() && !latest.is_empty() && current == latest
            }
            _ => false,
        }
    }

    pub fn is_check_error(&self) -> bool {
        self.status == UpdateStatus::Error && self.error_phase == Some(ErrorPhase::Check)
    }
}

// --- src/composables/useUpdater.js -------------------------------------------

/// Qué hace el botón de actualizaciones de Ajustes (`activate`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdaterAction {
    /// `install()`: reiniciar e instalar.
    Install,
    /// Abrir la página de releases.
    OpenReleases,
    /// `check({ manual: true })`.
    Check,
}

// Action handler for the update controls in Settings.
pub fn resolve_activate_action(state: &UpdaterState) -> UpdaterAction {
    if state.is_ready() && state.can_auto_install {
        return UpdaterAction::Install;
    }
    if matches!(
        state.status,
        UpdateStatus::Available | UpdateStatus::Downloading | UpdateStatus::Downloaded
    ) {
        // Downloaded-but-not-auto-installable platforms use the releases page.
        return UpdaterAction::OpenReleases;
    }
    if state.status == UpdateStatus::NotConfigured {
        // Sin canal de actualizaciones no hay nada que comprobar: el botón lleva
        // directo a las descargas, que es la única forma de actualizar este build.
        return UpdaterAction::OpenReleases;
    }
    // Up to date, idle, checking or a failed check: retry the check.
    UpdaterAction::Check
}

// Platforms that can't self-install (mac ad-hoc, Linux .deb) fall back to
// the downloads page so the user can grab the installer manually.
pub fn should_open_releases_after_install(result: Option<&InstallResult>) -> bool {
    result.is_none_or(|result| !result.ok)
}

/// `id` con el que se muestran los avisos: uno nuevo reemplaza al anterior.
pub const UPDATE_NOTICE_ID: &str = "app-update";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeKind {
    Info,
    Success,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoticeAction {
    pub label: &'static str,
    pub action: UpdaterAction,
}

/// Aviso (toast) que la interfaz muestra ante un evento del actualizador.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateNotice {
    pub kind: NoticeKind,
    pub message: String,
    pub action: Option<NoticeAction>,
}

impl UpdateNotice {
    fn new(kind: NoticeKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            action: None,
        }
    }

    fn with_action(mut self, label: &'static str, action: UpdaterAction) -> Self {
        self.action = Some(NoticeAction { label, action });
        self
    }
}

fn non_empty(value: &Option<String>) -> Option<&str> {
    value.as_deref().filter(|text| !text.is_empty())
}

/// `handleEvent` de `useUpdater`: decide qué avisos mostrar para cada evento.
/// Recuerda la versión ya anunciada para no repetir el aviso de descarga.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UpdateNotifier {
    toasted_version: Option<String>,
}

impl UpdateNotifier {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn notices_for(&mut self, payload: &UpdaterState) -> Vec<UpdateNotice> {
        let mut notices = Vec::new();

        if payload.manual_check {
            match payload.status {
                UpdateStatus::NotAvailable => {
                    let latest = non_empty(&payload.latest_version)
                        .or(non_empty(&payload.current_version))
                        .unwrap_or("the current version");
                    notices.push(UpdateNotice::new(
                        NoticeKind::Info,
                        format!("You are up to date: the latest version is {latest}."),
                    ));
                }
                UpdateStatus::Available => {
                    let latest = non_empty(&payload.latest_version).unwrap_or("new version");
                    notices.push(UpdateNotice::new(
                        NoticeKind::Info,
                        format!("A newer version is available: {latest}."),
                    ));
                }
                UpdateStatus::NotConfigured => notices.push(
                    UpdateNotice::new(
                        NoticeKind::Info,
                        "This build does not update itself: download the latest version from the releases page.",
                    )
                    .with_action("View releases", UpdaterAction::OpenReleases),
                ),
                UpdateStatus::Unsupported => notices.push(UpdateNotice::new(
                    NoticeKind::Info,
                    "Update checks are available in the packaged version of the app.",
                )),
                UpdateStatus::Error => {
                    let action = if payload.error_phase == Some(ErrorPhase::Download) {
                        "download the update"
                    } else {
                        "check for updates"
                    };
                    let detail = non_empty(&payload.error).unwrap_or("unknown error");
                    // El detalle ya viene resumido del proceso principal y puede traer su
                    // propio punto final; no se le añade otro.
                    let detail = if detail.ends_with(['.', '!', '?']) {
                        detail.to_string()
                    } else {
                        format!("{detail}.")
                    };
                    notices.push(UpdateNotice::new(
                        NoticeKind::Warning,
                        format!("Could not {action}: {detail}"),
                    ));
                }
                _ => {}
            }
        }

        if payload.status == UpdateStatus::Downloaded
            && let Some(latest) = non_empty(&payload.latest_version)
            && self.toasted_version.as_deref() != Some(latest)
        {
            self.toasted_version = Some(latest.to_string());
            if payload.can_auto_install {
                notices.push(
                    UpdateNotice::new(
                        NoticeKind::Success,
                        format!("Version {latest} is ready to install."),
                    )
                    .with_action("Restart", UpdaterAction::Install),
                );
            } else {
                notices.push(
                    UpdateNotice::new(NoticeKind::Info, format!("New version {latest} available."))
                        .with_action("Download", UpdaterAction::OpenReleases),
                );
            }
        }

        notices
    }
}

// --- src/components/layout/UpdateSettings.vue --------------------------------

/// `busy`: mientras se comprueba o se descarga no se admite otra acción.
pub fn settings_is_busy(state: &UpdaterState) -> bool {
    matches!(
        state.status,
        UpdateStatus::Checking | UpdateStatus::Downloading
    )
}

/// `:disabled` del botón.
pub fn settings_action_disabled(state: &UpdaterState) -> bool {
    settings_is_busy(state) || state.status == UpdateStatus::Unsupported
}

/// La barra de progreso solo se ve durante la descarga.
pub fn settings_shows_progress(state: &UpdaterState) -> bool {
    state.status == UpdateStatus::Downloading
}

pub fn settings_status_text(state: &UpdaterState) -> String {
    let latest = match non_empty(&state.latest_version) {
        Some(version) => format!("v{version}"),
        None => "A new version".to_string(),
    };

    match state.status {
        UpdateStatus::Checking => "Checking for updates…".to_string(),
        UpdateStatus::Available => format!("{latest} is available."),
        UpdateStatus::Downloading => format!("Downloading {latest}… {}%", state.percent),
        UpdateStatus::Downloaded => {
            if state.can_auto_install {
                format!("{latest} is ready to install.")
            } else {
                format!("{latest} is available to download.")
            }
        }
        UpdateStatus::NotAvailable => "You are up to date.".to_string(),
        UpdateStatus::NotConfigured => "Download updates from the releases page.".to_string(),
        UpdateStatus::Unsupported => {
            "Update checks are available in the packaged application.".to_string()
        }
        UpdateStatus::Error => {
            if state.is_up_to_date() {
                format!("Latest is {latest}. Could not check again. Please retry.")
            } else if state.error_phase == Some(ErrorPhase::Download) {
                "Could not download the update. Please retry.".to_string()
            } else {
                "Could not check for updates. Please retry.".to_string()
            }
        }
        UpdateStatus::Idle => "Check whether a newer version of Mergeon is available.".to_string(),
    }
}

pub fn settings_action_label(state: &UpdaterState) -> &'static str {
    match state.status {
        UpdateStatus::Checking => "Checking…",
        UpdateStatus::Downloading => "Downloading…",
        UpdateStatus::Downloaded if state.can_auto_install => "Restart and install",
        UpdateStatus::Available | UpdateStatus::Downloaded => "Download update",
        UpdateStatus::NotConfigured => "Open downloads",
        UpdateStatus::Error => "Retry",
        _ => "Check for updates",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(text: &str) -> Option<String> {
        Some(text.to_string())
    }

    fn applied(payload: UpdaterState) -> UpdaterState {
        let mut store = UpdaterState::default();
        store.apply(&payload);
        store
    }

    // --- UpdateSettings.test.js ---

    #[test]
    fn keeps_the_latest_version_message_neutral_after_a_failed_follow_up_check_and_allows_retry() {
        let state = applied(UpdaterState {
            status: UpdateStatus::Error,
            current_version: version("0.7.0"),
            latest_version: version("0.7.0"),
            error_phase: Some(ErrorPhase::Check),
            ..UpdaterState::default()
        });

        assert!(settings_status_text(&state).contains("Latest is v0.7.0. Could not check again."));
        assert_eq!(settings_action_label(&state), "Retry");
        assert!(!settings_action_disabled(&state));
        assert_eq!(resolve_activate_action(&state), UpdaterAction::Check);
    }

    #[test]
    fn shows_progress_without_allowing_a_second_action_then_offers_installation_when_ready() {
        let mut store = UpdaterState::default();
        store.apply(&UpdaterState {
            status: UpdateStatus::Downloading,
            percent: 42,
            latest_version: version("0.8.0"),
            ..UpdaterState::default()
        });
        assert!(settings_shows_progress(&store));
        assert_eq!(store.percent, 42);
        assert!(settings_action_disabled(&store));
        assert_eq!(settings_action_label(&store), "Downloading…");
        assert_eq!(settings_status_text(&store), "Downloading v0.8.0… 42%");

        store.apply(&UpdaterState {
            status: UpdateStatus::Downloaded,
            latest_version: version("0.8.0"),
            can_auto_install: true,
            ..UpdaterState::default()
        });
        assert!(!settings_shows_progress(&store));
        assert_eq!(settings_action_label(&store), "Restart and install");
        assert!(!settings_action_disabled(&store));
        assert_eq!(settings_status_text(&store), "v0.8.0 is ready to install.");
        assert_eq!(resolve_activate_action(&store), UpdaterAction::Install);
    }

    #[test]
    fn offers_a_manual_download_when_the_platform_cannot_install_automatically() {
        let state = applied(UpdaterState {
            status: UpdateStatus::Downloaded,
            latest_version: version("0.8.0"),
            can_auto_install: false,
            ..UpdaterState::default()
        });

        assert_eq!(settings_action_label(&state), "Download update");
        assert!(!settings_action_disabled(&state));
        assert_eq!(
            settings_status_text(&state),
            "v0.8.0 is available to download."
        );
        assert_eq!(resolve_activate_action(&state), UpdaterAction::OpenReleases);
    }

    // --- UpdateSettings.vue: el resto de estados ---

    #[test]
    fn describes_every_status() {
        let with = |status: UpdateStatus| UpdaterState {
            status,
            ..UpdaterState::default()
        };

        assert_eq!(
            settings_status_text(&with(UpdateStatus::Idle)),
            "Check whether a newer version of Mergeon is available."
        );
        assert_eq!(
            settings_status_text(&with(UpdateStatus::Checking)),
            "Checking for updates…"
        );
        assert_eq!(
            settings_status_text(&with(UpdateStatus::Available)),
            "A new version is available."
        );
        assert_eq!(
            settings_status_text(&with(UpdateStatus::NotAvailable)),
            "You are up to date."
        );
        assert_eq!(
            settings_status_text(&with(UpdateStatus::NotConfigured)),
            "Download updates from the releases page."
        );
        assert_eq!(
            settings_status_text(&with(UpdateStatus::Unsupported)),
            "Update checks are available in the packaged application."
        );
        assert_eq!(
            settings_status_text(&with(UpdateStatus::Error)),
            "Could not check for updates. Please retry."
        );
        assert_eq!(
            settings_status_text(&UpdaterState {
                status: UpdateStatus::Error,
                error_phase: Some(ErrorPhase::Download),
                ..UpdaterState::default()
            }),
            "Could not download the update. Please retry."
        );

        assert_eq!(
            settings_action_label(&with(UpdateStatus::Idle)),
            "Check for updates"
        );
        assert_eq!(
            settings_action_label(&with(UpdateStatus::Checking)),
            "Checking…"
        );
        assert_eq!(
            settings_action_label(&with(UpdateStatus::Available)),
            "Download update"
        );
        assert_eq!(
            settings_action_label(&with(UpdateStatus::NotConfigured)),
            "Open downloads"
        );
        assert_eq!(
            settings_action_label(&with(UpdateStatus::NotAvailable)),
            "Check for updates"
        );
        assert_eq!(
            settings_action_label(&with(UpdateStatus::Unsupported)),
            "Check for updates"
        );

        assert!(settings_action_disabled(&with(UpdateStatus::Checking)));
        assert!(settings_action_disabled(&with(UpdateStatus::Unsupported)));
        assert!(!settings_action_disabled(&with(UpdateStatus::Available)));
    }

    // --- stores/updater.js ---

    #[test]
    fn store_starts_with_the_documented_defaults() {
        let store = UpdaterState::default();
        assert_eq!(store.status, UpdateStatus::Idle);
        assert_eq!(
            store.releases_url,
            "https://github.com/Spilveru/mergeon/releases/latest"
        );
        assert!(!store.is_visible());
        assert!(!store.is_ready());
        assert!(!store.is_busy());
        assert!(!store.is_up_to_date());
        assert!(!store.is_check_error());
    }

    #[test]
    fn apply_keeps_the_current_version_and_releases_url_when_the_payload_lacks_them() {
        let mut store = UpdaterState::default();
        store.apply(&UpdaterState {
            status: UpdateStatus::Available,
            current_version: version("0.7.0"),
            latest_version: version("0.8.0"),
            releases_url: "https://example.com/releases".to_string(),
            manual_check: true,
            ..UpdaterState::default()
        });
        assert_eq!(store.current_version, version("0.7.0"));
        assert!(store.manual_check);

        store.apply(&UpdaterState {
            status: UpdateStatus::Error,
            error: version("feed unavailable"),
            error_phase: Some(ErrorPhase::Check),
            releases_url: String::new(),
            ..UpdaterState::default()
        });
        assert_eq!(store.status, UpdateStatus::Error);
        assert_eq!(store.current_version, version("0.7.0"));
        assert_eq!(store.releases_url, "https://example.com/releases");
        // Lo demás lo pisa el payload, aunque venga vacío.
        assert_eq!(store.latest_version, None);
        assert!(!store.manual_check);
        assert_eq!(store.error.as_deref(), Some("feed unavailable"));
    }

    #[test]
    fn getters_follow_the_status() {
        let with = |status: UpdateStatus| UpdaterState {
            status,
            ..UpdaterState::default()
        };

        for status in [
            UpdateStatus::Available,
            UpdateStatus::Downloading,
            UpdateStatus::Downloaded,
            UpdateStatus::Error,
        ] {
            assert!(with(status).is_visible(), "{status:?}");
        }
        for status in [
            UpdateStatus::Idle,
            UpdateStatus::Checking,
            UpdateStatus::NotAvailable,
            UpdateStatus::NotConfigured,
            UpdateStatus::Unsupported,
        ] {
            assert!(!with(status).is_visible(), "{status:?}");
        }

        assert!(with(UpdateStatus::Downloaded).is_ready());
        assert!(with(UpdateStatus::Checking).is_busy());
        assert!(with(UpdateStatus::Available).is_busy());
        assert!(with(UpdateStatus::Downloading).is_busy());
        assert!(!with(UpdateStatus::Downloaded).is_busy());

        let same = UpdaterState {
            current_version: version("1.0.0"),
            latest_version: version("1.0.0"),
            ..UpdaterState::default()
        };
        assert!(same.is_up_to_date());
        let newer = UpdaterState {
            latest_version: version("1.1.0"),
            ..same.clone()
        };
        assert!(!newer.is_up_to_date());

        let check_error = UpdaterState {
            status: UpdateStatus::Error,
            error_phase: Some(ErrorPhase::Check),
            ..UpdaterState::default()
        };
        assert!(check_error.is_check_error());
        let download_error = UpdaterState {
            error_phase: Some(ErrorPhase::Download),
            ..check_error
        };
        assert!(!download_error.is_check_error());
    }

    // --- useUpdater.js ---

    #[test]
    fn activate_picks_the_action_for_each_state() {
        let with = |status: UpdateStatus, can_auto_install: bool| UpdaterState {
            status,
            can_auto_install,
            ..UpdaterState::default()
        };

        assert_eq!(
            resolve_activate_action(&with(UpdateStatus::Downloaded, true)),
            UpdaterAction::Install
        );
        for status in [
            UpdateStatus::Available,
            UpdateStatus::Downloading,
            UpdateStatus::Downloaded,
            UpdateStatus::NotConfigured,
        ] {
            assert_eq!(
                resolve_activate_action(&with(status, false)),
                UpdaterAction::OpenReleases,
                "{status:?}"
            );
        }
        // Aunque la plataforma se instale sola, hasta que no esté descargada no hay nada que instalar.
        assert_eq!(
            resolve_activate_action(&with(UpdateStatus::Downloading, true)),
            UpdaterAction::OpenReleases
        );
        for status in [
            UpdateStatus::Idle,
            UpdateStatus::Checking,
            UpdateStatus::NotAvailable,
            UpdateStatus::Error,
            UpdateStatus::Unsupported,
        ] {
            assert_eq!(
                resolve_activate_action(&with(status, true)),
                UpdaterAction::Check,
                "{status:?}"
            );
        }
    }

    #[test]
    fn a_failed_or_missing_install_result_opens_the_releases_page() {
        assert!(should_open_releases_after_install(None));
        assert!(should_open_releases_after_install(Some(
            &InstallResult::unsupported()
        )));
        assert!(should_open_releases_after_install(Some(
            &InstallResult::unavailable()
        )));
        assert!(!should_open_releases_after_install(Some(
            &InstallResult::ok()
        )));
    }

    #[test]
    fn manual_checks_announce_their_result() {
        let mut notifier = UpdateNotifier::new();
        let manual = |status: UpdateStatus| UpdaterState {
            status,
            manual_check: true,
            ..UpdaterState::default()
        };

        assert_eq!(
            notifier.notices_for(&UpdaterState {
                latest_version: version("0.9.0"),
                ..manual(UpdateStatus::NotAvailable)
            }),
            [UpdateNotice::new(
                NoticeKind::Info,
                "You are up to date: the latest version is 0.9.0."
            )]
        );
        assert_eq!(
            notifier.notices_for(&UpdaterState {
                current_version: version("0.8.0"),
                ..manual(UpdateStatus::NotAvailable)
            })[0]
                .message,
            "You are up to date: the latest version is 0.8.0."
        );
        assert_eq!(
            notifier.notices_for(&manual(UpdateStatus::NotAvailable))[0].message,
            "You are up to date: the latest version is the current version."
        );

        assert_eq!(
            notifier.notices_for(&UpdaterState {
                latest_version: version("0.9.1"),
                ..manual(UpdateStatus::Available)
            })[0]
                .message,
            "A newer version is available: 0.9.1."
        );
        assert_eq!(
            notifier.notices_for(&manual(UpdateStatus::Available))[0].message,
            "A newer version is available: new version."
        );

        assert_eq!(
            notifier.notices_for(&manual(UpdateStatus::NotConfigured)),
            [UpdateNotice::new(
                NoticeKind::Info,
                "This build does not update itself: download the latest version from the releases page."
            )
            .with_action("View releases", UpdaterAction::OpenReleases)]
        );
        assert_eq!(
            notifier.notices_for(&manual(UpdateStatus::Unsupported))[0].message,
            "Update checks are available in the packaged version of the app."
        );

        // Estados intermedios y comprobaciones en segundo plano no avisan.
        assert!(
            notifier
                .notices_for(&manual(UpdateStatus::Checking))
                .is_empty()
        );
        assert!(
            notifier
                .notices_for(&UpdaterState {
                    status: UpdateStatus::NotAvailable,
                    ..UpdaterState::default()
                })
                .is_empty()
        );
    }

    #[test]
    fn manual_check_errors_name_the_failed_phase_without_doubling_the_period() {
        let mut notifier = UpdateNotifier::new();
        let error = |phase: Option<ErrorPhase>, detail: Option<&str>| UpdaterState {
            status: UpdateStatus::Error,
            manual_check: true,
            error_phase: phase,
            error: detail.map(str::to_string),
            ..UpdaterState::default()
        };

        assert_eq!(
            notifier.notices_for(&error(
                Some(ErrorPhase::Check),
                Some("No se pudo contactar con el servidor de actualizaciones.")
            )),
            [UpdateNotice::new(
                NoticeKind::Warning,
                "Could not check for updates: No se pudo contactar con el servidor de actualizaciones."
            )]
        );
        assert_eq!(
            notifier.notices_for(&error(Some(ErrorPhase::Download), Some("download failed")))[0]
                .message,
            "Could not download the update: download failed."
        );
        assert_eq!(
            notifier.notices_for(&error(None, None))[0].message,
            "Could not check for updates: unknown error."
        );
    }

    #[test]
    fn announces_a_downloaded_version_once() {
        let mut notifier = UpdateNotifier::new();
        let downloaded = |latest: &str, can_auto_install: bool| UpdaterState {
            status: UpdateStatus::Downloaded,
            latest_version: version(latest),
            can_auto_install,
            ..UpdaterState::default()
        };

        assert_eq!(
            notifier.notices_for(&downloaded("0.9.1", true)),
            [
                UpdateNotice::new(NoticeKind::Success, "Version 0.9.1 is ready to install.")
                    .with_action("Restart", UpdaterAction::Install)
            ]
        );
        // El mismo evento otra vez (comprobación periódica) no repite el aviso.
        assert!(notifier.notices_for(&downloaded("0.9.1", true)).is_empty());

        assert_eq!(
            notifier.notices_for(&downloaded("0.9.2", false)),
            [
                UpdateNotice::new(NoticeKind::Info, "New version 0.9.2 available.")
                    .with_action("Download", UpdaterAction::OpenReleases)
            ]
        );

        // Sin versión no hay nada que anunciar.
        assert!(
            notifier
                .notices_for(&UpdaterState {
                    status: UpdateStatus::Downloaded,
                    ..UpdaterState::default()
                })
                .is_empty()
        );
    }
}
