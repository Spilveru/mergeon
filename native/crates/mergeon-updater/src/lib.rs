//! Actualizador de la app (port de `electron/services/app-updater.js` sin
//! electron-updater).
//!
//! - `machine`: la máquina de estados de `createAppUpdater`, pura.
//! - `updater`: `AppUpdater`, que le añade el motor que antes ponía
//!   electron-updater: manifiesto del canal, comparación de versiones,
//!   descarga verificada e instalación.
//! - `presentation`: la lógica pura del renderer (store, `useUpdater`,
//!   `UpdateSettings.vue`).
//!
//! La red va detrás de `HttpTransport` y la instalación detrás de `Installer`;
//! `testing` trae dobles de ambos.

mod download;
mod error;
mod error_summary;
mod feed;
mod installer;
mod machine;
mod manifest;
mod platform;
mod presentation;
mod signature;
mod state;
pub mod testing;
mod transport;
mod updater;
mod version;
mod yaml;

pub use download::{
    DownloadRequest, DownloadSettings, DownloadedUpdate, Downloader, sha512_base64,
    sha512_base64_of_file, sha512_matches, verify_sha512,
};
pub use error::UpdaterError;
pub use error_summary::{MAX_ERROR_MESSAGE_LENGTH, summarize_updater_error};
pub use feed::{GITHUB_RELEASES_OWNER, GITHUB_RELEASES_REPO, UpdateFeed, is_https_url};
pub use installer::{
    AppImageInstaller, InstallOptions, InstallOutcome, Installer, MacAppInstaller, NsisInstaller,
    RelaunchCommand, appimage_destination, bundle_path_for_executable, default_installer,
    nsis_install_args,
};
pub use machine::{CheckStart, StateMachineConfig, UpdateEvent, UpdaterStateMachine};
pub use manifest::{
    ArtifactKind, MAX_MANIFEST_BYTES, UpdateFile, UpdateManifest, channel_file_name,
    parse_update_manifest, resolve_artifact_kind, select_artifact,
};
pub use platform::{
    Arch, BUILT_IN_UPDATE_FEED_URL, Env, Platform, default_cache_dir, resolve_can_auto_install,
    resolve_update_feed_url, resolve_update_feed_url_or,
};
pub use presentation::{
    NoticeAction, NoticeKind, UPDATE_NOTICE_ID, UpdateNotice, UpdateNotifier, UpdaterAction,
    resolve_activate_action, settings_action_disabled, settings_action_label, settings_is_busy,
    settings_shows_progress, settings_status_text, should_open_releases_after_install,
};
pub use signature::{
    BUILT_IN_UPDATE_PUBLIC_KEY, MAX_SIGNATURE_BYTES, SIGNATURE_SUFFIX, generate_signing_keypair,
    sign_manifest_bytes, signature_file_name, verify_manifest_signature,
};
pub use state::{
    DEFAULT_UPDATE_CHANNEL, ErrorPhase, InstallFailureReason, InstallResult, RELEASES_URL,
    UPDATE_CHECK_INTERVAL, UPDATER_EVENT_CHANNEL, UpdateStatus, UpdaterState,
};
pub use transport::{HttpRequest, HttpResponse, HttpTransport, TransportError, UreqTransport};
pub use updater::{
    AppUpdater, AppUpdaterBuilder, CheckOptions, QuitHandler, QuitRequest, SubscriptionId,
    UpdaterConfig,
};
pub use version::{Identifier, Version, compare_versions, is_update_available};
