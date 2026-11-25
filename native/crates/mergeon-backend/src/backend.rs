//! [`Backend`]: el cableado que en la app Electron hace `electron/main.js` al
//! registrar los handlers IPC.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use mergeon_github::{GitHubClient, GitHubConfig};
use mergeon_runtime::StatusWatcherOptions;
use mergeon_store::{SecretCipher, Store, StoreError, StoreOptions};

use crate::cipher::KeychainCipher;
use crate::events::{BackendEvent, EventCallback, discard_events};
use crate::git::GitApi;
use crate::shell::Shell;
use crate::updater::{UpdaterApi, UpdaterBridge};

/// Fallo al construir el [`Backend`].
#[derive(Debug)]
pub enum BackendError {
    /// No se pudo abrir la configuración persistente.
    Store(StoreError),
}

impl std::fmt::Display for BackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BackendError::Store(error) => write!(f, "Failed to open the settings store: {error}"),
        }
    }
}

impl std::error::Error for BackendError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            BackendError::Store(error) => Some(error),
        }
    }
}

impl From<StoreError> for BackendError {
    fn from(error: StoreError) -> Self {
        BackendError::Store(error)
    }
}

enum StoreSource {
    /// `<userData>/settings.json`.
    Default,
    Path(PathBuf),
    Instance(Box<Store>),
}

/// Opciones de [`Backend::new`]. Por defecto todo es lo real: configuración
/// en el directorio de datos del usuario, tokens cifrados con el llavero del
/// sistema, cliente de GitHub configurado desde el entorno y programas del
/// sistema para los canales `shell:*`.
pub struct BackendOptions {
    store: StoreSource,
    cipher: Option<Arc<dyn SecretCipher>>,
    import_electron_config: bool,
    github: Option<GitHubClient>,
    shell: Option<Shell>,
    watcher_options: Option<StatusWatcherOptions>,
    updater: Option<Arc<dyn UpdaterBridge>>,
    on_event: Option<EventCallback>,
}

impl Default for BackendOptions {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for BackendOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let store = match &self.store {
            StoreSource::Default => "default".to_string(),
            StoreSource::Path(path) => path.display().to_string(),
            StoreSource::Instance(_) => "instance".to_string(),
        };
        f.debug_struct("BackendOptions")
            .field("store", &store)
            .field("custom_cipher", &self.cipher.is_some())
            .field("import_electron_config", &self.import_electron_config)
            .field("updater", &self.updater.is_some())
            .finish_non_exhaustive()
    }
}

impl BackendOptions {
    pub fn new() -> Self {
        Self {
            store: StoreSource::Default,
            cipher: None,
            import_electron_config: true,
            github: None,
            shell: None,
            watcher_options: None,
            updater: None,
            on_event: None,
        }
    }

    /// Archivo de configuración a usar en vez del de por defecto.
    pub fn store_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.store = StoreSource::Path(path.into());
        self
    }

    /// Un [`Store`] ya abierto (p. ej. `Store::in_memory` en tests, o como
    /// respaldo cuando el archivo no se puede usar). Con él se ignoran
    /// [`BackendOptions::cipher`] y [`BackendOptions::without_electron_import`].
    pub fn store(mut self, store: Store) -> Self {
        self.store = StoreSource::Instance(Box::new(store));
        self
    }

    /// Cifrador de tokens. Por defecto, [`KeychainCipher::system`].
    pub fn cipher(mut self, cipher: Arc<dyn SecretCipher>) -> Self {
        self.cipher = Some(cipher);
        self
    }

    /// No importar la configuración de la app Electron al crear el archivo.
    pub fn without_electron_import(mut self) -> Self {
        self.import_electron_config = false;
        self
    }

    /// Cliente de GitHub. Por defecto se configura desde el entorno.
    pub fn github(mut self, github: GitHubClient) -> Self {
        self.github = Some(github);
        self
    }

    /// Canales `shell:*` con un lanzador o plataforma propios.
    pub fn shell(mut self, shell: Shell) -> Self {
        self.shell = Some(shell);
        self
    }

    /// Configuración del watcher de estado (tiempos, backend de vigilancia).
    pub fn watcher_options(mut self, options: StatusWatcherOptions) -> Self {
        self.watcher_options = Some(options);
        self
    }

    /// Actualizador de la app (ver [`UpdaterBridge`]).
    pub fn updater(mut self, updater: Arc<dyn UpdaterBridge>) -> Self {
        self.updater = Some(updater);
        self
    }

    /// Receptor de los eventos del backend. Se llama desde hilos internos
    /// (watcher, cola de mutaciones, actualizador): debe reenviar el evento a
    /// su destino y volver enseguida, sin esperar a otra operación del backend.
    pub fn on_event(mut self, on_event: impl Fn(BackendEvent) + Send + Sync + 'static) -> Self {
        self.on_event = Some(Arc::new(on_event));
        self
    }
}

/// Equivalente síncrono de `window.api`: un método por canal IPC de
/// `electron/preload.js`. Es `Send + Sync` y se comparte con `Arc`; todas las
/// llamadas bloquean, así que la interfaz las hace desde hilos de fondo.
pub struct Backend {
    git: GitApi,
    store: Store,
    github: GitHubClient,
    shell: Shell,
    updater: UpdaterApi,
}

impl std::fmt::Debug for Backend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Backend")
            .field("git", &self.git)
            .field("store", &self.store)
            .field("shell", &self.shell)
            .field("updater", &self.updater)
            .finish_non_exhaustive()
    }
}

/// `import 'dotenv/config'` de `main.js`: además del entorno del proceso se
/// lee el `.env` del directorio de trabajo, si existe.
fn github_from_env() -> GitHubClient {
    GitHubClient::new(GitHubConfig::from_env_with_dotenv(Path::new(".env")))
}

impl Backend {
    /// Abre la configuración, crea el watcher de estado y la cola de
    /// mutaciones enlazados, el cliente de GitHub y los canales de shell.
    ///
    /// Solo falla si la configuración persistente no se puede abrir; quien
    /// llama puede reintentar con un `Store::in_memory` para arrancar igual.
    pub fn new(options: BackendOptions) -> Result<Self, BackendError> {
        let BackendOptions {
            store,
            cipher,
            import_electron_config,
            github,
            shell,
            watcher_options,
            updater,
            on_event,
        } = options;

        let store = match store {
            StoreSource::Instance(store) => *store,
            source => {
                let cipher = cipher.unwrap_or_else(|| Arc::new(KeychainCipher::system()));
                let mut store_options = StoreOptions::new(cipher);
                if !import_electron_config {
                    store_options = store_options.without_electron_import();
                }
                match source {
                    StoreSource::Path(path) => Store::open(path, store_options)?,
                    _ => Store::open_default(store_options)?,
                }
            }
        };

        let events = on_event.unwrap_or_else(discard_events);
        let git = GitApi::new(events.clone(), watcher_options.unwrap_or_default());

        if let Some(updater) = &updater {
            let updater_events = events.clone();
            updater.subscribe(Box::new(move |state| {
                updater_events(BackendEvent::Updater(state));
            }));
        }

        let shell = shell.unwrap_or_default();
        // El editor externo guardado; desde aquí lo mantiene al día quien
        // cambie el ajuste (`Shell::set_external_editor`).
        shell.set_external_editor(store.get_settings().external_editor);

        Ok(Self {
            git,
            store,
            github: github.unwrap_or_else(github_from_env),
            shell,
            updater: UpdaterApi::new(updater),
        })
    }

    /// `window.api.git`: los canales `git:*`.
    pub fn git(&self) -> &GitApi {
        &self.git
    }

    /// `window.api.storage`: los canales `storage:*` son los métodos de [`Store`].
    pub fn storage(&self) -> &Store {
        &self.store
    }

    /// `window.api.auth` y `window.api.github`: los canales `github:*` son
    /// los métodos de [`GitHubClient`] (`auth_start`, `auth_poll`,
    /// `auth_refresh`, `auth_token`, `get_user_repositories`,
    /// `search_repositories`).
    pub fn github(&self) -> &GitHubClient {
        &self.github
    }

    /// `window.api.shell`: los canales `shell:*`.
    pub fn shell(&self) -> &Shell {
        &self.shell
    }

    /// `window.api.updater`: los canales `updater:*`.
    pub fn updater(&self) -> &UpdaterApi {
        &self.updater
    }

    /// Lo que `main.js` hace al cerrarse la última ventana y antes de salir:
    /// deja de vigilar el repositorio. También avisa al actualizador. La cola
    /// de mutaciones sigue viva hasta que se suelta el `Backend`.
    pub fn shutdown(&self) {
        self.git.stop_status_watcher();
        self.updater.shutdown();
    }
}

impl Drop for Backend {
    /// Detiene el watcher y el actualizador; al soltarse después los campos,
    /// la cola cancela lo que tenga pendiente y espera a sus hilos.
    fn drop(&mut self) {
        self.shutdown();
    }
}
