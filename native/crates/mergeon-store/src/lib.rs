//! Configuración persistente (port de `electron/store.js` y de los canales
//! `storage:*` de `electron/ipc/storage-handlers.js`).
//!
//! El estado vive en un archivo JSON propio de la app nativa (`settings.json`,
//! en el mismo directorio de datos de usuario que usaba Electron). [`Store`]
//! lo carga una vez, lo mantiene en memoria y lo reescribe de forma atómica en
//! cada cambio. La API es síncrona y bloqueante.
//!
//! # Compatibilidad con la app Electron
//!
//! La primera vez (cuando `settings.json` todavía no existe) se importan del
//! `config.json` de Electron los repos recientes, la sesión de workspace, los
//! ajustes y las ramas destacadas. Las **cuentas de GitHub no se importan**:
//! sus tokens están cifrados con `safeStorage` de Electron (clave del llavero
//! ligada a esa app) y no se pueden descifrar fuera de Electron, así que el
//! usuario tiene que volver a conectar sus cuentas en la app nativa.
//!
//! # Cifrado de tokens
//!
//! `store.js` cifra con `safeStorage` y, si no está disponible, guarda el token
//! con el prefijo `plain:`. Aquí ese papel lo cumple [`SecretCipher`], que el
//! `Store` recibe inyectado; [`PlainTextCipher`] reproduce el fallback.

mod cipher;
mod error;
mod paths;
mod schema;
mod settings;
mod store;
mod time;
mod types;

pub use cipher::{PlainTextCipher, SecretCipher, TestCipher, decode_token, encode_token};
pub use error::StoreError;
pub use paths::{
    APP_NAME, ELECTRON_CONFIG_FILE_NAME, LEGACY_APP_DIRECTORIES, LegacyMigration,
    SETTINGS_FILE_NAME, default_app_data_dir, default_user_data_dir, migrate_legacy_config,
};
pub use schema::{
    default_commit_view_settings, default_config, default_settings, default_sidebar_sections,
    default_workspace_session, validate_config, validate_github_account,
};
pub use settings::{
    ACTION_DEFAULTS_KEY, ACTION_DIALOGS_KEY, ActionDefaults, ActionDialogMode, ActionDialogs,
    CommitViewSettings, CommitViewUpdate, DEFAULT_AUTO_FETCH_INTERVAL_MINUTES,
    DEFAULT_MAX_RECENT_REPOS, DiffViewMode, EXTERNAL_EDITOR_KEY, ExternalEditorSettings,
    FetchActionDefaults, KEYBOARD_SHORTCUTS_KEY, PullActionDefaults, PushActionDefaults,
    RemoteAction, SKIP_ACTION_DIALOGS_KEY, Settings, SettingsUpdate, SidebarSections,
    SidebarSectionsUpdate, Theme,
};
pub use store::{ElectronImport, LoadReport, LoadStatus, Store, StoreOptions};
pub use types::{
    GitHubAccount, GitHubInstallation, RecentRepository, RepoWorkspace, WorkspaceSession,
};
