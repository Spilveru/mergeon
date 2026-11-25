//! `AppUpdater`: la máquina de estados de `createAppUpdater` más el motor que
//! en la app Electron ponía electron-updater (consulta del feed, descarga,
//! verificación e instalación).

use std::collections::VecDeque;
use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, ThreadId};
use std::time::{Duration, Instant};

use crate::download::{DownloadRequest, DownloadSettings, DownloadedUpdate, Downloader};
use crate::error::UpdaterError;
use crate::feed::{UpdateFeed, artifact_file_name, cache_buster_now, is_https_url};
use crate::installer::{
    InstallOptions, InstallOutcome, Installer, RelaunchCommand, default_installer,
};
use crate::machine::{CheckStart, StateMachineConfig, UpdateEvent, UpdaterStateMachine};
use crate::manifest::{
    ArtifactKind, MAX_MANIFEST_BYTES, UpdateManifest, channel_file_name, parse_update_manifest,
    resolve_artifact_kind, select_artifact,
};
use crate::platform::{
    Arch, BUILT_IN_UPDATE_FEED_URL, Env, Platform, default_cache_dir, resolve_can_auto_install,
    resolve_update_feed_url_or,
};
use crate::signature::{
    BUILT_IN_UPDATE_PUBLIC_KEY, MAX_SIGNATURE_BYTES, signature_file_name, verify_manifest_signature,
};
use crate::state::{
    DEFAULT_UPDATE_CHANNEL, InstallFailureReason, InstallResult, RELEASES_URL,
    UPDATE_CHECK_INTERVAL, UpdaterState,
};
use crate::transport::{HttpRequest, HttpTransport, UreqTransport};
use crate::version::is_update_available;

/// Configuración del actualizador.
#[derive(Debug, Clone)]
pub struct UpdaterConfig {
    /// Versión de la app (`app.getVersion()`).
    pub current_version: Option<String>,
    /// `app.isPackaged`: un build de desarrollo no se actualiza.
    pub is_packaged: bool,
    pub platform: Platform,
    pub arch: Arch,
    /// Canal de actualizaciones. `None` → estado `not-configured`.
    pub feed: Option<UpdateFeed>,
    /// Nombre del canal (`latest`): prefijo del manifiesto.
    pub channel: String,
    /// Si la app puede descargar e instalar sola en esta plataforma.
    pub can_auto_install: bool,
    /// Formato que se descarga e instala. `None` → solo se avisa.
    pub artifact_kind: Option<ArtifactKind>,
    /// AppImage en ejecución (variable `APPIMAGE`): el archivo que se reemplaza.
    pub appimage_path: Option<PathBuf>,
    pub releases_url: String,
    /// Donde se guardan las descargas (`<cache_dir>/pending`).
    pub cache_dir: PathBuf,
    /// Comprobar al llamar a `start()`, como hacía `main.js` al arrancar.
    pub check_on_start: bool,
    /// Cada cuánto se vuelve a comprobar tras `start()`. `None` → nunca.
    pub check_interval: Option<Duration>,
    /// `autoUpdater.autoInstallOnAppQuit`: habilita `install_on_quit()`.
    pub auto_install_on_app_quit: bool,
    /// Tiempo máximo para bajar el manifiesto.
    pub manifest_timeout: Duration,
    pub download: DownloadSettings,
    /// Clave pública Ed25519 (32 bytes en base64) que debe firmar `latest*.yml`.
    /// Por defecto, la fijada al compilar (`MERGEON_UPDATE_PUBLIC_KEY`).
    pub public_key: Option<String>,
    /// Exigir una firma Ed25519 válida (`<manifiesto>.sig`) antes de aceptar
    /// cualquier actualización. Activado por defecto (fail-closed).
    pub require_signature: bool,
    /// Permitir `http://` sin TLS. Siempre `false` en producción; solo para
    /// tests de integración contra `127.0.0.1`.
    pub allow_insecure_http: bool,
}

impl UpdaterConfig {
    /// Configuración con los mismos criterios que `createAppUpdater`: feed
    /// genérico solo si hay `APP_UPDATE_FEED_URL` (la del entorno de ejecución
    /// o, si falta, la fijada al compilar: [`BUILT_IN_UPDATE_FEED_URL`]),
    /// canal `latest` y auto-instalación según `resolve_can_auto_install`.
    pub fn from_env(
        current_version: Option<String>,
        is_packaged: bool,
        platform: Platform,
        arch: Arch,
        env: &Env,
        cache_dir: impl Into<PathBuf>,
    ) -> Self {
        Self::from_env_with_built_in_feed(
            current_version,
            is_packaged,
            platform,
            arch,
            env,
            BUILT_IN_UPDATE_FEED_URL,
            cache_dir,
        )
    }

    /// [`Self::from_env`] con el feed de respaldo explícito (`None`: solo el
    /// del entorno), para no depender de cómo se compiló.
    pub fn from_env_with_built_in_feed(
        current_version: Option<String>,
        is_packaged: bool,
        platform: Platform,
        arch: Arch,
        env: &Env,
        built_in_feed: Option<&str>,
        cache_dir: impl Into<PathBuf>,
    ) -> Self {
        // Las releases viven en un repositorio privado, así que el feed de GitHub que
        // electron-builder empaqueta en app-update.yml responde 404 a cualquiera que
        // no esté autenticado: no hay nada que consultar y preguntarlo solo produce
        // un error. Solo cuenta un feed genérico explícito (APP_UPDATE_FEED_URL).
        // Si el repositorio se hace público, basta con poner `UpdateFeed::GitHub`.
        let feed = resolve_update_feed_url_or(env, built_in_feed).map(UpdateFeed::generic);
        let can_auto_install = resolve_can_auto_install(&platform, env);
        let artifact_kind = resolve_artifact_kind(&platform, env);
        let appimage_path = env
            .get("APPIMAGE")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from);

        Self {
            current_version,
            is_packaged,
            platform,
            arch,
            feed,
            channel: DEFAULT_UPDATE_CHANNEL.to_string(),
            can_auto_install,
            artifact_kind,
            appimage_path,
            releases_url: RELEASES_URL.to_string(),
            cache_dir: cache_dir.into(),
            check_on_start: true,
            check_interval: Some(UPDATE_CHECK_INTERVAL),
            auto_install_on_app_quit: true,
            manifest_timeout: Duration::from_secs(15),
            download: DownloadSettings::default(),
            public_key: BUILT_IN_UPDATE_PUBLIC_KEY
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string),
            require_signature: true,
            allow_insecure_http: false,
        }
    }

    /// Configuración para el proceso actual: plataforma, arquitectura, entorno
    /// y directorio de caché reales.
    ///
    /// En builds release empaquetados (`is_packaged && !cfg!(debug_assertions)`),
    /// se ignora `APP_UPDATE_FEED_URL` del entorno de ejecución para que ninguna
    /// variable externa pueda redirigir el canal de actualizaciones; solo cuenta
    /// el feed fijado al compilar ([`BUILT_IN_UPDATE_FEED_URL`]).
    pub fn for_current_process(current_version: impl Into<String>, is_packaged: bool) -> Self {
        let platform = Platform::current();
        let mut env = Env::from_process();
        if is_packaged && !cfg!(debug_assertions) {
            env.remove("APP_UPDATE_FEED_URL");
        }
        let cache_dir = default_cache_dir(&platform, &env)
            .unwrap_or_else(|| std::env::temp_dir().join("mergeon-updater"));
        Self::from_env(
            Some(current_version.into()),
            is_packaged,
            platform,
            Arch::current(),
            &env,
            cache_dir,
        )
    }

    /// Solo cuenta un canal explícito: sin feed no hay nada que consultar.
    pub fn updates_configured(&self) -> bool {
        self.feed.is_some()
    }
}

/// Opciones de `checkForUpdates({ manual })`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CheckOptions {
    /// La pidió el usuario: los eventos llevan `manualCheck: true` para que
    /// la interfaz avise del resultado.
    pub manual: bool,
}

impl CheckOptions {
    pub fn manual() -> Self {
        Self { manual: true }
    }
}

/// Lo que recibe la app cuando la actualización ya está aplicada (o el
/// instalador en marcha) y toca salir.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuitRequest {
    /// Cómo reabrir la app. `None` si lo hace el propio instalador (Windows).
    pub relaunch: Option<RelaunchCommand>,
}

/// Paso "salir y relanzar": lo provee la app.
pub type QuitHandler = Arc<dyn Fn(&QuitRequest) + Send + Sync>;

type Listener = Arc<dyn Fn(&UpdaterState) + Send + Sync>;

/// Identificador de una suscripción a los eventos del actualizador.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SubscriptionId(u64);

struct Shared {
    machine: UpdaterStateMachine,
    in_flight_check: bool,
    /// Hilo que ejecuta la comprobación en vuelo.
    check_thread: Option<ThreadId>,
    downloading: bool,
    installing: bool,
    /// Artefacto verificado, listo para instalar.
    downloaded: Option<DownloadedUpdate>,
    /// Ya se instaló: no se vuelve a lanzar el instalador.
    installed: Option<InstallOutcome>,
    /// Payloads pendientes de entregar a los suscriptores, en orden.
    queue: VecDeque<UpdaterState>,
    dispatching: bool,
    shutdown: bool,
    started: bool,
    active_threads: usize,
}

struct Inner {
    config: UpdaterConfig,
    can_auto_install: bool,
    transport: Arc<dyn HttpTransport>,
    installer: Option<Arc<dyn Installer>>,
    quit_handler: Option<QuitHandler>,
    downloader: Downloader,
    shared: Mutex<Shared>,
    changed: Condvar,
    cancel: AtomicBool,
    listeners: Mutex<Vec<(u64, Listener)>>,
    next_listener_id: AtomicU64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InstallMode {
    /// `quitAndInstall`: lo pide el usuario; la app sale y se relanza.
    QuitAndInstall,
    /// `autoInstallOnAppQuit`: la app ya se está cerrando.
    OnAppQuit,
}

/// Cuenta un hilo propio mientras vive, para `wait_idle`.
struct ActiveThread(Arc<Inner>);

impl Drop for ActiveThread {
    fn drop(&mut self) {
        let mut shared = self.0.lock();
        shared.active_threads = shared.active_threads.saturating_sub(1);
        drop(shared);
        self.0.changed.notify_all();
    }
}

/// Libera la comprobación en vuelo aunque el motor entre en pánico.
struct InFlightCheck<'a>(&'a Inner);

impl Drop for InFlightCheck<'_> {
    fn drop(&mut self) {
        {
            let mut shared = self.0.lock();
            shared.in_flight_check = false;
            shared.check_thread = None;
        }
        self.0.changed.notify_all();
    }
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn state(&self) -> UpdaterState {
        self.lock().machine.state()
    }

    /// Aplica un evento del motor y lo reparte a los suscriptores.
    fn handle(&self, event: UpdateEvent) {
        match &event {
            UpdateEvent::CheckingForUpdate => log::info!("[updater] checking for updates"),
            UpdateEvent::UpdateAvailable { version } => log::info!(
                "[updater] update available {}",
                version.as_deref().unwrap_or("unknown")
            ),
            UpdateEvent::UpdateNotAvailable { .. } => log::info!("[updater] no updates available"),
            UpdateEvent::DownloadProgress { percent } => {
                log::info!("[updater] download progress {}%", percent.round())
            }
            UpdateEvent::UpdateDownloaded { version } => log::info!(
                "[updater] update downloaded {}",
                version.as_deref().unwrap_or("unknown")
            ),
            UpdateEvent::Error { .. } => {}
        }

        {
            let mut shared = self.lock();
            if shared.shutdown {
                return;
            }
            let payload = shared.machine.handle(event);
            shared.queue.push_back(payload);
        }
        self.flush();
    }

    fn fail(&self, error: &UpdaterError) {
        // El detalle completo (URL, hashes...) va al log; a la interfaz, el resumen.
        log::error!("[updater] update error {error:?}");
        self.handle(UpdateEvent::Error {
            message: error.to_string(),
        });
    }

    /// Entrega los payloads encolados, en orden y sin el candado tomado: un
    /// suscriptor puede volver a llamar al actualizador.
    fn flush(&self) {
        {
            let mut shared = self.lock();
            if shared.dispatching {
                // Otro hilo (o este mismo, más arriba en la pila) ya reparte.
                return;
            }
            shared.dispatching = true;
        }

        struct Dispatching<'a>(&'a Inner);
        impl Drop for Dispatching<'_> {
            fn drop(&mut self) {
                self.0.lock().dispatching = false;
                self.0.changed.notify_all();
            }
        }
        let dispatching = Dispatching(self);

        loop {
            let payload = {
                let mut shared = self.lock();
                match shared.queue.pop_front() {
                    Some(payload) => payload,
                    None => {
                        // Se suelta bajo el mismo candado que vio la cola vacía:
                        // un evento encolado después encuentra el reparto libre.
                        shared.dispatching = false;
                        drop(shared);
                        std::mem::forget(dispatching);
                        self.changed.notify_all();
                        return;
                    }
                }
            };
            let listeners: Vec<Listener> = self
                .listeners
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .iter()
                .map(|(_, listener)| Arc::clone(listener))
                .collect();
            for listener in listeners {
                listener(&payload);
            }
        }
    }

    /// `checkForUpdates({ manual })`.
    fn check(inner: &Arc<Inner>, manual: bool) -> UpdaterState {
        {
            let mut shared = inner.lock();
            if shared.shutdown {
                return shared.machine.state();
            }

            if let CheckStart::Blocked(payload) = shared.machine.begin_check(manual) {
                shared.queue.push_back(payload);
                let state = shared.machine.state();
                drop(shared);
                inner.flush();
                return state;
            }

            if shared.in_flight_check {
                // Un suscriptor que vuelve a pedir la comprobación desde su
                // callback está en el hilo que la ejecuta: esperarla sería
                // esperarse a sí mismo.
                if shared.check_thread == Some(thread::current().id()) {
                    return shared.machine.state();
                }
                // Misma promesa `inFlightCheck` del original: se espera a la
                // comprobación en curso en vez de lanzar otra.
                while shared.in_flight_check && !shared.shutdown {
                    shared = inner
                        .changed
                        .wait(shared)
                        .unwrap_or_else(PoisonError::into_inner);
                }
                return shared.machine.state();
            }

            // Con una descarga o instalación en marcha no se vuelve a consultar:
            // electron-updater reiniciaba el ciclo y el estado retrocedía a `checking`.
            if shared.downloading || shared.installing || shared.installed.is_some() {
                return shared.machine.state();
            }

            shared.in_flight_check = true;
            shared.check_thread = Some(thread::current().id());
        }

        {
            let _in_flight = InFlightCheck(inner);
            Inner::run_check(inner);
        }
        inner.state()
    }

    fn run_check(inner: &Arc<Inner>) {
        inner.handle(UpdateEvent::CheckingForUpdate);

        let manifest = match inner.fetch_manifest() {
            Ok(manifest) => manifest,
            Err(UpdaterError::Cancelled) => return,
            Err(error) => return inner.fail(&error),
        };

        let current = inner.config.current_version.as_deref().unwrap_or("");
        match is_update_available(current, &manifest.version) {
            Err(error) => inner.fail(&error),
            Ok(false) => {
                // Nada que instalar: lo que quedara descargado ya no hace falta.
                inner.lock().downloaded = None;
                inner.downloader.clear();
                inner.handle(UpdateEvent::UpdateNotAvailable {
                    version: Some(manifest.version),
                });
            }
            Ok(true) => {
                inner.handle(UpdateEvent::UpdateAvailable {
                    version: Some(manifest.version.clone()),
                });
                Inner::start_download(inner, manifest);
            }
        }
    }

    fn fetch_manifest(&self) -> Result<UpdateManifest, UpdaterError> {
        let Some(feed) = &self.config.feed else {
            return Err(UpdaterError::InvalidManifest {
                detail: "no update feed configured".to_string(),
            });
        };

        let file_name = channel_file_name(
            &self.config.channel,
            &self.config.platform,
            &self.config.arch,
        );
        let cache_buster = cache_buster_now();
        let url = feed.manifest_url(&file_name, Some(&cache_buster));
        if !self.config.allow_insecure_http && !is_https_url(&url) {
            return Err(UpdaterError::InsecureUrl { url });
        }
        let network_error = |detail: String| UpdaterError::Network {
            url: url.clone(),
            detail,
        };

        let request = HttpRequest {
            url: url.clone(),
            headers: vec![
                (
                    "Accept".to_string(),
                    "application/x-yaml, text/yaml, text/plain, */*".to_string(),
                ),
                ("Cache-Control".to_string(), "no-cache".to_string()),
            ],
            response_timeout: self.config.manifest_timeout,
            body_timeout: self.config.manifest_timeout,
        };
        let response = self
            .transport
            .get(&request)
            .map_err(|error| network_error(error.to_string()))?;
        if !response.is_ok() {
            return Err(UpdaterError::HttpStatus {
                status: response.status,
                url,
            });
        }

        let mut body = Vec::new();
        response
            .body
            .take(MAX_MANIFEST_BYTES + 1)
            .read_to_end(&mut body)
            .map_err(|error| network_error(error.to_string()))?;
        if body.len() as u64 > MAX_MANIFEST_BYTES {
            return Err(UpdaterError::InvalidManifest {
                detail: format!("the manifest at {url} is too large"),
            });
        }

        if self.config.require_signature {
            let public_key = self
                .config
                .public_key
                .as_deref()
                .map(str::trim)
                .filter(|key| !key.is_empty())
                .ok_or_else(|| UpdaterError::InvalidSignature {
                    detail: "no update signing public key configured".to_string(),
                })?;

            let sig_file = signature_file_name(&file_name);
            let sig_url = feed.manifest_url(&sig_file, Some(&cache_buster));
            if !self.config.allow_insecure_http && !is_https_url(&sig_url) {
                return Err(UpdaterError::InsecureUrl { url: sig_url });
            }
            let sig_request = HttpRequest {
                url: sig_url.clone(),
                headers: vec![
                    ("Accept".to_string(), "text/plain, */*".to_string()),
                    ("Cache-Control".to_string(), "no-cache".to_string()),
                ],
                response_timeout: self.config.manifest_timeout,
                body_timeout: self.config.manifest_timeout,
            };
            let sig_response =
                self.transport
                    .get(&sig_request)
                    .map_err(|error| UpdaterError::Network {
                        url: sig_url.clone(),
                        detail: error.to_string(),
                    })?;
            if !sig_response.is_ok() {
                return Err(UpdaterError::InvalidSignature {
                    detail: format!(
                        "missing manifest signature at {sig_url} (HTTP {})",
                        sig_response.status
                    ),
                });
            }
            let mut sig_body = Vec::new();
            sig_response
                .body
                .take(MAX_SIGNATURE_BYTES + 1)
                .read_to_end(&mut sig_body)
                .map_err(|error| UpdaterError::Network {
                    url: sig_url.clone(),
                    detail: error.to_string(),
                })?;
            if sig_body.len() as u64 > MAX_SIGNATURE_BYTES {
                return Err(UpdaterError::InvalidSignature {
                    detail: format!("manifest signature at {sig_url} is too large"),
                });
            }
            let sig_text =
                String::from_utf8(sig_body).map_err(|_| UpdaterError::InvalidSignature {
                    detail: format!("manifest signature at {sig_url} is not UTF-8"),
                })?;
            verify_manifest_signature(&body, &sig_text, public_key)?;
        }

        let text = String::from_utf8(body).map_err(|_| UpdaterError::InvalidManifest {
            detail: format!("the manifest at {url} is not UTF-8"),
        })?;
        parse_update_manifest(&text)
    }

    /// `autoDownload = true`: la descarga arranca sola al encontrar una versión nueva.
    fn start_download(inner: &Arc<Inner>, manifest: UpdateManifest) {
        if !inner.can_auto_install {
            // Plataformas que no se instalan solas (.deb, macOS sin firma):
            // electron-updater bajaba igualmente un instalador que nadie usaba.
            // Aquí se anuncia la versión sin descargar nada; la interfaz ofrece
            // la página de releases.
            inner.handle(UpdateEvent::UpdateDownloaded {
                version: Some(manifest.version),
            });
            return;
        }

        {
            let mut shared = inner.lock();
            if shared.shutdown {
                return;
            }
            shared.downloading = true;
            shared.downloaded = None;
        }

        let spawned = spawn_worker(inner, "mergeon-updater-download", move |inner| {
            let result = inner.download_update(&manifest);
            {
                let mut shared = inner.lock();
                shared.downloading = false;
                if let Ok(update) = &result {
                    shared.downloaded = Some(update.clone());
                }
            }
            inner.changed.notify_all();

            match result {
                Ok(update) => inner.handle(UpdateEvent::UpdateDownloaded {
                    version: Some(update.version),
                }),
                Err(UpdaterError::Cancelled) => {}
                Err(error) => inner.fail(&error),
            }
        });

        if let Err(error) = spawned {
            inner.lock().downloading = false;
            inner.fail(&UpdaterError::io(
                "No se pudo iniciar la descarga de la actualización",
                &error,
            ));
        }
    }

    fn download_update(&self, manifest: &UpdateManifest) -> Result<DownloadedUpdate, UpdaterError> {
        let (Some(feed), Some(kind)) = (&self.config.feed, self.config.artifact_kind) else {
            return Err(UpdaterError::NoSuitableArtifact {
                wanted: "installer".to_string(),
            });
        };

        let files = manifest.resolved_files()?;
        let file = select_artifact(&files, kind, &self.config.platform, &self.config.arch)?;
        let sha512 = file
            .sha512
            .clone()
            .ok_or_else(|| UpdaterError::MissingChecksum {
                file: file.url.clone(),
            })?;

        let url = feed.file_url(&file.url);
        if !self.config.allow_insecure_http && !is_https_url(&url) {
            return Err(UpdaterError::InsecureUrl { url });
        }
        let request = DownloadRequest {
            url,
            file_name: artifact_file_name(&file.url),
            sha512,
            size: file.size,
            version: manifest.version.clone(),
        };

        self.downloader.download(
            &request,
            &mut |percent| self.handle(UpdateEvent::DownloadProgress { percent }),
            &self.cancel,
        )
    }

    fn install(&self, mode: InstallMode) -> InstallResult {
        let quit = mode == InstallMode::QuitAndInstall;
        let options = InstallOptions {
            silent: true,
            relaunch: quit,
        };

        let (installer, update) = {
            let mut shared = self.lock();

            if let Some(outcome) = shared.installed.clone() {
                // Ya instalado (p. ej. `install()` y luego el cierre de la app):
                // el instalador no se lanza dos veces.
                drop(shared);
                if quit {
                    self.request_quit(outcome);
                }
                return InstallResult::ok();
            }

            let allowed = match mode {
                // `installUpdate()`: solo con el estado en `downloaded`.
                InstallMode::QuitAndInstall => shared.machine.can_install(),
                // `autoInstallOnAppQuit` no mira el estado visible: si hay una
                // descarga verificada se aplica, aunque una comprobación
                // posterior haya fallado y la interfaz muestre un error.
                InstallMode::OnAppQuit => self.can_auto_install,
            };
            if !allowed || shared.installing || shared.downloading {
                return InstallResult::unsupported();
            }
            let (Some(installer), Some(update)) =
                (self.installer.clone(), shared.downloaded.clone())
            else {
                return InstallResult::unsupported();
            };
            shared.installing = true;
            (installer, update)
        };

        let result = installer.install(&update, &options);

        {
            let mut shared = self.lock();
            shared.installing = false;
            if let Ok(outcome) = &result {
                shared.installed = Some(outcome.clone());
            }
        }

        match result {
            Ok(outcome) => {
                log::info!("[updater] update {} installed", update.version);
                if quit {
                    self.request_quit(outcome);
                }
                InstallResult::ok()
            }
            Err(error) => {
                self.fail(&error);
                InstallResult::failure(InstallFailureReason::Failed)
            }
        }
    }

    fn request_quit(&self, outcome: InstallOutcome) {
        match &self.quit_handler {
            Some(handler) => handler(&QuitRequest {
                relaunch: outcome.relaunch,
            }),
            None => log::warn!("[updater] update installed but no quit handler is registered"),
        }
    }

    fn shutdown(&self) {
        self.cancel.store(true, Ordering::SeqCst);
        {
            let mut shared = self.lock();
            shared.shutdown = true;
            shared.queue.clear();
        }
        self.changed.notify_all();
    }

    /// Espera `duration` o hasta el cierre. Devuelve `false` si se cerró.
    fn wait_for_next_check(&self, duration: Duration) -> bool {
        let deadline = Instant::now() + duration;
        let mut shared = self.lock();
        loop {
            if shared.shutdown {
                return false;
            }
            let now = Instant::now();
            if now >= deadline {
                return true;
            }
            shared = self
                .changed
                .wait_timeout(shared, deadline - now)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

fn spawn_worker(
    inner: &Arc<Inner>,
    name: &str,
    work: impl FnOnce(&Arc<Inner>) + Send + 'static,
) -> std::io::Result<()> {
    inner.lock().active_threads += 1;
    let guard = ActiveThread(Arc::clone(inner));

    thread::Builder::new()
        .name(name.to_string())
        .spawn(move || {
            let guard = guard;
            work(&guard.0);
        })
        .map(|_| ())
}

/// Construye un `AppUpdater` con transporte, instalador y salida inyectables.
pub struct AppUpdaterBuilder {
    config: UpdaterConfig,
    transport: Option<Arc<dyn HttpTransport>>,
    installer: Option<Arc<dyn Installer>>,
    quit_handler: Option<QuitHandler>,
}

impl AppUpdaterBuilder {
    /// Transporte HTTP. Por defecto, `UreqTransport`.
    pub fn transport(mut self, transport: Arc<dyn HttpTransport>) -> Self {
        self.transport = Some(transport);
        self
    }

    /// Instalador. Por defecto, el real de la plataforma (`default_installer`).
    pub fn installer(mut self, installer: Arc<dyn Installer>) -> Self {
        self.installer = Some(installer);
        self
    }

    /// Paso "salir y relanzar" de la app, llamado tras instalar con `install()`.
    pub fn on_quit(mut self, handler: impl Fn(&QuitRequest) + Send + Sync + 'static) -> Self {
        self.quit_handler = Some(Arc::new(handler));
        self
    }

    pub fn build(self) -> AppUpdater {
        let config = self.config;
        let transport = self
            .transport
            .unwrap_or_else(|| Arc::new(UreqTransport::new()));
        let installer = self.installer.or_else(|| {
            // El instalador real sale de la misma configuración que eligió el
            // formato, no del entorno del proceso.
            let mut env = Env::new();
            if let Some(path) = &config.appimage_path {
                env.set("APPIMAGE", path.to_string_lossy());
            }
            config
                .artifact_kind
                .filter(|_| config.can_auto_install)
                .and_then(|kind| default_installer(kind, &env))
        });
        // Sin formato instalable, sin instalador o sin clave pública cuando se
        // exige firma, la app no instala actualizaciones automáticamente.
        let has_signing_key = !config.require_signature
            || config
                .public_key
                .as_deref()
                .map(str::trim)
                .is_some_and(|key| !key.is_empty());
        let can_auto_install = config.can_auto_install
            && config.artifact_kind.is_some()
            && installer.is_some()
            && has_signing_key;

        let machine = UpdaterStateMachine::new(StateMachineConfig {
            current_version: config.current_version.clone(),
            is_packaged: config.is_packaged,
            updates_configured: config.updates_configured(),
            can_auto_install,
            releases_url: config.releases_url.clone(),
        });
        let downloader = Downloader::new(
            Arc::clone(&transport),
            config.cache_dir.clone(),
            config.download.clone(),
        );

        AppUpdater {
            inner: Arc::new(Inner {
                config,
                can_auto_install,
                transport,
                installer,
                quit_handler: self.quit_handler,
                downloader,
                shared: Mutex::new(Shared {
                    machine,
                    in_flight_check: false,
                    check_thread: None,
                    downloading: false,
                    installing: false,
                    downloaded: None,
                    installed: None,
                    queue: VecDeque::new(),
                    dispatching: false,
                    shutdown: false,
                    started: false,
                    active_threads: 0,
                }),
                changed: Condvar::new(),
                cancel: AtomicBool::new(false),
                listeners: Mutex::new(Vec::new()),
                next_listener_id: AtomicU64::new(1),
            }),
        }
    }
}

/// Actualizador de la app. Seguro entre hilos (`Send + Sync`); al soltarlo se
/// detienen el temporizador y la descarga en curso.
pub struct AppUpdater {
    inner: Arc<Inner>,
}

impl AppUpdater {
    /// Con el transporte real (`ureq`) y el instalador real de la plataforma.
    pub fn new(config: UpdaterConfig) -> Self {
        Self::builder(config).build()
    }

    pub fn builder(config: UpdaterConfig) -> AppUpdaterBuilder {
        AppUpdaterBuilder {
            config,
            transport: None,
            installer: None,
            quit_handler: None,
        }
    }

    pub fn config(&self) -> &UpdaterConfig {
        &self.inner.config
    }

    /// `getState()`: copia del estado actual.
    pub fn state(&self) -> UpdaterState {
        self.inner.state()
    }

    /// `checkForUpdates({ manual })`. Bloquea hasta que termina la consulta
    /// del feed (no la descarga, que sigue en su propio hilo) y devuelve el
    /// estado resultante, como el handler IPC `updater:check`.
    pub fn check(&self, options: CheckOptions) -> UpdaterState {
        Inner::check(&self.inner, options.manual)
    }

    /// Igual que `check`, sin bloquear: el resultado llega por los eventos.
    pub fn check_in_background(&self, options: CheckOptions) {
        let spawned = spawn_worker(&self.inner, "mergeon-updater-check", move |inner| {
            Inner::check(inner, options.manual);
        });
        if let Err(error) = spawned {
            log::error!("[updater] could not start the update check: {error}");
        }
    }

    /// Comprobación al arrancar y, después, cada `check_interval`, como hacía
    /// `main.js`. Solo tiene efecto la primera vez.
    pub fn start(&self) {
        {
            let mut shared = self.inner.lock();
            if shared.started || shared.shutdown {
                return;
            }
            shared.started = true;
        }

        let spawned = spawn_worker(&self.inner, "mergeon-updater-timer", |inner| {
            if inner.config.check_on_start {
                Inner::check(inner, false);
            }
            // Re-check periodically so long-running sessions still pick up new releases.
            while let Some(interval) = inner.config.check_interval {
                if !inner.wait_for_next_check(interval) {
                    break;
                }
                Inner::check(inner, false);
            }
        });
        if let Err(error) = spawned {
            log::error!("[updater] could not start the update timer: {error}");
        }
    }

    /// `installUpdate()`: instala la actualización descargada y pide a la app
    /// que salga (y se relance) con el callback de `on_quit`.
    ///
    /// Devuelve `{ ok: false, reason: "unsupported" }` si la plataforma no se
    /// instala sola o no hay nada descargado, y `reason: "failed"` si el
    /// instalador falló (además del evento de error).
    pub fn install(&self) -> InstallResult {
        self.inner.install(InstallMode::QuitAndInstall)
    }

    /// `autoInstallOnAppQuit`: aplica la actualización descargada mientras la
    /// app se cierra, sin relanzarla ni llamar al callback de salida. La app
    /// lo llama en su camino de salida normal. Basta con que haya una descarga
    /// verificada: no depende de que el estado visible siga en `downloaded`.
    pub fn install_on_quit(&self) -> InstallResult {
        if !self.inner.config.auto_install_on_app_quit {
            return InstallResult::unsupported();
        }
        self.inner.install(InstallMode::OnAppQuit)
    }

    /// Se suscribe a los eventos (`updater:event`): `listener` recibe el
    /// payload completo en cada cambio de estado, en orden, desde hilos del
    /// actualizador.
    pub fn subscribe(
        &self,
        listener: impl Fn(&UpdaterState) + Send + Sync + 'static,
    ) -> SubscriptionId {
        let id = self.inner.next_listener_id.fetch_add(1, Ordering::SeqCst);
        self.inner
            .listeners
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((id, Arc::new(listener)));
        SubscriptionId(id)
    }

    /// Cancela una suscripción. `false` si ya no existía.
    pub fn unsubscribe(&self, id: SubscriptionId) -> bool {
        let mut listeners = self
            .inner
            .listeners
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let before = listeners.len();
        listeners.retain(|(listener_id, _)| *listener_id != id.0);
        listeners.len() != before
    }

    /// Detiene el temporizador y cancela la descarga en curso. No bloquea; a
    /// partir de aquí no se emiten más eventos ni se aceptan comprobaciones.
    pub fn shutdown(&self) {
        self.inner.shutdown();
    }

    /// Espera a que no quede trabajo en curso (hilos propios, comprobación,
    /// eventos por entregar). Devuelve `false` si se agotó el tiempo.
    pub fn wait_idle(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut shared = self.inner.lock();
        loop {
            let busy = shared.active_threads > 0
                || shared.in_flight_check
                || shared.dispatching
                || !shared.queue.is_empty();
            if !busy {
                return true;
            }
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            shared = self
                .inner
                .changed
                .wait_timeout(shared, deadline - now)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

impl Drop for AppUpdater {
    fn drop(&mut self) {
        self.inner.shutdown();
    }
}
