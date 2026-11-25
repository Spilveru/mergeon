//! Dobles de prueba para quien use [`AppModel`](crate::AppModel) en tests:
//! una interfaz que registra lo pedido y responde lo programado, un backend
//! sin efectos fuera del proceso y un transporte de GitHub guionizado.
//!
//! ```no_run
//! use gpui_kit::TestAppContext;
//! use mergeon_app::testing::{RecordingUiHost, TestBackend};
//! use mergeon_app::AppModel;
//!
//! # fn example(cx: &mut TestAppContext) {
//! // Los eventos del backend llegan desde otros hilos.
//! cx.executor().allow_parking();
//!
//! let mut test_backend = TestBackend::new();
//! let (backend, events) = test_backend.build();
//! let host = RecordingUiHost::new();
//! let model = cx.update(|cx| AppModel::init(backend, events, cx));
//! model.update(cx, |model, cx| model.set_ui_host(host.clone(), cx));
//!
//! host.answer_confirm(true);
//! // ... ejercitar el modelo ...
//! assert!(host.toasts().is_empty());
//! # }
//! ```

use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use futures::channel::oneshot;
use gpui_kit::App;
use mergeon_backend::{
    Backend, BackendOptions, LaunchError, PlainTextCipher, ProcessLauncher, Shell, ShellPlatform,
    StatusWatcherOptions, Store, StoreOptions, UpdaterBridge,
};
use mergeon_github::{
    GitHubClient, GitHubConfig, HttpRequest, HttpResponse, HttpTransport, TransportError,
};
use mergeon_model::dialogs::{
    ConfirmAnswer, ConfirmOptions, ConfirmValue, PromptAnswer, PromptOptions,
};
use mergeon_model::notify::{NotifyType, ProgressToast, Toast, ToastId};
use mergeon_runtime::{ManualClock, WatchBackend, WatchError, WatchHandle, WatchSink, WatchTimers};

use crate::ui_host::{Answer, UiHost};
use crate::{EventReceiver, event_channel};

// ---------------------------------------------------------------------------
// Interfaz
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Recorded {
    toasts: Vec<Toast>,
    visible_toasts: bool,
    progress: Vec<ProgressToast>,
    dismissed: Vec<ToastId>,

    confirms: Vec<ConfirmOptions>,
    confirm_answers: VecDeque<ConfirmValue>,
    default_confirm: bool,
    held_confirms: Vec<oneshot::Sender<ConfirmValue>>,

    option_confirms: Vec<ConfirmOptions>,
    option_confirm_answers: VecDeque<ConfirmAnswer>,

    prompts: Vec<PromptOptions>,
    prompt_answers: VecDeque<PromptAnswer>,
    held_prompts: Vec<oneshot::Sender<PromptAnswer>>,

    directory_requests: usize,
    directories: VecDeque<Option<PathBuf>>,

    credential_requests: Vec<crate::CredentialsRequest>,
    credential_answers: VecDeque<Option<crate::CredentialsAnswer>>,

    hold_answers: bool,
}

/// [`UiHost`] de prueba: registra cada aviso y cada pregunta, y responde lo
/// que el test haya programado.
///
/// Sin respuesta programada, una confirmación responde `false` (o `true` tras
/// [`RecordingUiHost::confirm_by_default`]), un prompt se cancela y el
/// selector de directorio no elige nada. Con [`RecordingUiHost::hold_answers`]
/// las preguntas quedan abiertas hasta que el test las resuelve, como un
/// diálogo que el usuario aún no ha contestado.
#[derive(Default)]
pub struct RecordingUiHost {
    recorded: RefCell<Recorded>,
}

impl RecordingUiHost {
    pub fn new() -> Rc<Self> {
        Rc::new(Self::default())
    }

    // --- avisos -----------------------------------------------------------

    /// Avisos mostrados, en orden.
    pub fn toasts(&self) -> Vec<Toast> {
        self.recorded.borrow().toasts.clone()
    }

    /// Títulos de los avisos de un tipo.
    pub fn messages(&self, kind: NotifyType) -> Vec<String> {
        self.recorded
            .borrow()
            .toasts
            .iter()
            .filter(|toast| toast.kind == kind)
            .map(|toast| toast.title.clone())
            .collect()
    }

    /// Se mostró un aviso de ese tipo con ese título.
    pub fn has_message(&self, kind: NotifyType, message: &str) -> bool {
        self.recorded
            .borrow()
            .toasts
            .iter()
            .any(|toast| toast.kind == kind && toast.title == message)
    }

    /// Olvida los avisos registrados.
    pub fn clear_toasts(&self) {
        let mut recorded = self.recorded.borrow_mut();
        recorded.toasts.clear();
        recorded.progress.clear();
        recorded.dismissed.clear();
    }

    /// Avisos de progreso mostrados, en orden.
    pub fn progress_toasts(&self) -> Vec<ProgressToast> {
        self.recorded.borrow().progress.clone()
    }

    /// Avisos retirados con [`UiHost::dismiss_toast`], en orden.
    pub fn dismissed_toasts(&self) -> Vec<ToastId> {
        self.recorded.borrow().dismissed.clone()
    }

    /// Hace que los avisos registrados cuenten como "aún en pantalla": un
    /// aviso idéntico a uno visible no se repite.
    pub fn keep_toasts_visible(&self, visible: bool) {
        self.recorded.borrow_mut().visible_toasts = visible;
    }

    // --- confirmaciones ---------------------------------------------------

    /// Programa la respuesta de la siguiente confirmación.
    pub fn answer_confirm(&self, confirmed: bool) {
        self.recorded
            .borrow_mut()
            .confirm_answers
            .push_back(Some(confirmed));
    }

    /// Respuesta de las confirmaciones sin respuesta programada.
    pub fn confirm_by_default(&self, confirmed: bool) {
        self.recorded.borrow_mut().default_confirm = confirmed;
    }

    /// Confirmaciones pedidas, en orden.
    pub fn confirms(&self) -> Vec<ConfirmOptions> {
        self.recorded.borrow().confirms.clone()
    }

    /// Programa la respuesta del siguiente `confirm_with_option`.
    pub fn answer_confirm_with_option(&self, confirmed: bool, checked: bool) {
        self.recorded
            .borrow_mut()
            .option_confirm_answers
            .push_back(ConfirmAnswer::WithOption { confirmed, checked });
    }

    /// Confirmaciones con opción pedidas, en orden.
    pub fn option_confirms(&self) -> Vec<ConfirmOptions> {
        self.recorded.borrow().option_confirms.clone()
    }

    // --- prompts ----------------------------------------------------------

    /// Programa que el siguiente prompt se confirme con `value`.
    pub fn answer_prompt(&self, value: &str) {
        self.recorded
            .borrow_mut()
            .prompt_answers
            .push_back(PromptAnswer {
                confirmed: true,
                value: Some(value.to_string()),
            });
    }

    /// Programa que el siguiente prompt se cancele.
    pub fn cancel_prompt(&self) {
        self.recorded
            .borrow_mut()
            .prompt_answers
            .push_back(PromptAnswer {
                confirmed: false,
                value: None,
            });
    }

    /// Prompts pedidos, en orden.
    pub fn prompts(&self) -> Vec<PromptOptions> {
        self.recorded.borrow().prompts.clone()
    }

    // --- selector de directorio --------------------------------------------

    /// Programa lo que elige el siguiente selector de directorio.
    pub fn answer_directory(&self, directory: Option<&Path>) {
        self.recorded
            .borrow_mut()
            .directories
            .push_back(directory.map(Path::to_path_buf));
    }

    /// Veces que se abrió el selector de directorio.
    pub fn directory_requests(&self) -> usize {
        self.recorded.borrow().directory_requests
    }

    /// Programa la respuesta del siguiente diálogo de credenciales (`None`:
    /// cancelarlo). Sin respuesta programada se cancela.
    pub fn answer_credentials(&self, answer: Option<crate::CredentialsAnswer>) {
        self.recorded
            .borrow_mut()
            .credential_answers
            .push_back(answer);
    }

    /// Diálogos de credenciales pedidos, en orden.
    pub fn credential_requests(&self) -> Vec<crate::CredentialsRequest> {
        self.recorded.borrow().credential_requests.clone()
    }

    // --- preguntas abiertas -------------------------------------------------

    /// Con `true`, las confirmaciones y prompts sin respuesta programada
    /// quedan abiertos hasta [`Self::release_confirm`] /
    /// [`Self::release_prompt`].
    pub fn hold_answers(&self, hold: bool) {
        self.recorded.borrow_mut().hold_answers = hold;
    }

    /// Responde la confirmación abierta más antigua. `false` si no había.
    pub fn release_confirm(&self, confirmed: bool) -> bool {
        let sender = {
            let mut recorded = self.recorded.borrow_mut();
            if recorded.held_confirms.is_empty() {
                return false;
            }
            recorded.held_confirms.remove(0)
        };
        let _ = sender.send(Some(confirmed));
        true
    }

    /// Responde el prompt abierto más antiguo. `false` si no había.
    pub fn release_prompt(&self, answer: PromptAnswer) -> bool {
        let sender = {
            let mut recorded = self.recorded.borrow_mut();
            if recorded.held_prompts.is_empty() {
                return false;
            }
            recorded.held_prompts.remove(0)
        };
        let _ = sender.send(answer);
        true
    }
}

impl UiHost for RecordingUiHost {
    fn notify(&self, toast: Toast, _cx: &mut App) {
        let mut recorded = self.recorded.borrow_mut();
        // Un aviso con el mismo id reescribe al anterior en pantalla, pero el
        // test quiere ver los dos.
        recorded.toasts.push(toast);
    }

    fn show_progress(&self, toast: ProgressToast, _cx: &mut App) {
        self.recorded.borrow_mut().progress.push(toast);
    }

    fn dismiss_toast(&self, id: &ToastId, _cx: &mut App) {
        self.recorded.borrow_mut().dismissed.push(id.clone());
    }

    fn visible_toast_ids(&self, _cx: &App) -> Vec<ToastId> {
        let recorded = self.recorded.borrow();
        if recorded.visible_toasts {
            recorded
                .toasts
                .iter()
                .map(|toast| toast.id.clone())
                .collect()
        } else {
            Vec::new()
        }
    }

    fn confirm(&self, options: ConfirmOptions, _cx: &mut App) -> Answer<ConfirmValue> {
        let (sender, receiver) = oneshot::channel();
        let mut recorded = self.recorded.borrow_mut();
        recorded.confirms.push(options);

        match recorded.confirm_answers.pop_front() {
            Some(answer) => {
                let _ = sender.send(answer);
            }
            None if recorded.hold_answers => recorded.held_confirms.push(sender),
            None => {
                let _ = sender.send(Some(recorded.default_confirm));
            }
        }
        receiver
    }

    fn confirm_with_option(&self, options: ConfirmOptions, _cx: &mut App) -> Answer<ConfirmAnswer> {
        let (sender, receiver) = oneshot::channel();
        let mut recorded = self.recorded.borrow_mut();
        let fallback = ConfirmAnswer::WithOption {
            confirmed: recorded.default_confirm,
            checked: options.option_checked,
        };
        recorded.option_confirms.push(options);
        let answer = recorded.option_confirm_answers.pop_front();
        let _ = sender.send(answer.unwrap_or(fallback));
        receiver
    }

    fn prompt(&self, options: PromptOptions, _cx: &mut App) -> Answer<PromptAnswer> {
        let (sender, receiver) = oneshot::channel();
        let mut recorded = self.recorded.borrow_mut();
        recorded.prompts.push(options);

        match recorded.prompt_answers.pop_front() {
            Some(answer) => {
                let _ = sender.send(answer);
            }
            None if recorded.hold_answers => recorded.held_prompts.push(sender),
            None => {
                let _ = sender.send(PromptAnswer {
                    confirmed: false,
                    value: None,
                });
            }
        }
        receiver
    }

    fn ask_credentials(
        &self,
        request: crate::CredentialsRequest,
        _cx: &mut App,
    ) -> Answer<Option<crate::CredentialsAnswer>> {
        let (sender, receiver) = oneshot::channel();
        let mut recorded = self.recorded.borrow_mut();
        recorded.credential_requests.push(request);
        let answer = recorded.credential_answers.pop_front().flatten();
        let _ = sender.send(answer);
        receiver
    }

    fn pick_directory(&self, _cx: &mut App) -> Answer<Option<PathBuf>> {
        let (sender, receiver) = oneshot::channel();
        let mut recorded = self.recorded.borrow_mut();
        recorded.directory_requests += 1;
        let directory = recorded.directories.pop_front().flatten();
        let _ = sender.send(directory);
        receiver
    }
}

// ---------------------------------------------------------------------------
// Backend
// ---------------------------------------------------------------------------

/// Vigilancia que nunca avisa de nada: los tests entregan los cambios de
/// estado a mano con `AppModel::handle_backend_event`.
#[derive(Debug, Default)]
struct SilentWatchBackend;

impl WatchBackend for SilentWatchBackend {
    fn watch(
        &self,
        _repo_path: &Path,
        _sink: WatchSink,
    ) -> Result<Box<dyn WatchHandle>, WatchError> {
        Ok(Box::new(()))
    }
}

/// Opciones de un watcher de estado que no vigila el disco ni sondea.
pub fn silent_watcher_options() -> StatusWatcherOptions {
    StatusWatcherOptions {
        backend: Arc::new(SilentWatchBackend),
        timers: WatchTimers::Manual(ManualClock::new()),
        ..StatusWatcherOptions::default()
    }
}

/// Un proceso que el backend habría lanzado.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launched {
    pub command: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
}

/// Lanzador que no lanza nada: registra lo pedido y responde que arrancó.
#[derive(Debug, Default)]
pub struct RecordingLauncher {
    launched: Mutex<Vec<Launched>>,
    failure: Mutex<Option<LaunchError>>,
}

impl RecordingLauncher {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Lo que se habría lanzado, en orden (sin las consultas a `which`).
    pub fn launched(&self) -> Vec<Launched> {
        self.launched
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .filter(|launched| launched.command != "which")
            .cloned()
            .collect()
    }

    /// Argumentos de todo lo lanzado, para buscar una URL o una ruta.
    pub fn launched_args(&self) -> Vec<String> {
        self.launched()
            .into_iter()
            .flat_map(|launched| launched.args)
            .collect()
    }

    /// A partir de ahora todo lanzamiento falla con `error`.
    pub fn fail_with(&self, error: LaunchError) {
        *self
            .failure
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(error);
    }

    fn record(
        &self,
        command: &str,
        args: &[String],
        cwd: Option<&Path>,
    ) -> Result<(), LaunchError> {
        self.launched
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(Launched {
                command: command.to_string(),
                args: args.to_vec(),
                cwd: cwd.map(Path::to_path_buf),
            });
        match self
            .failure
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
        {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

impl ProcessLauncher for RecordingLauncher {
    fn spawn_detached(
        &self,
        command: &str,
        args: &[String],
        cwd: Option<&Path>,
    ) -> Result<(), LaunchError> {
        self.record(command, args, cwd)
    }

    fn run(
        &self,
        command: &str,
        args: &[String],
        cwd: Option<&Path>,
    ) -> Result<Option<i32>, LaunchError> {
        self.record(command, args, cwd)?;
        Ok(Some(0))
    }
}

type Responder = dyn FnMut(&HttpRequest) -> Result<HttpResponse, TransportError> + Send;

/// Transporte de GitHub guionizado: cada petición se registra y la responde la
/// función que el test instaló.
pub struct FakeGitHubTransport {
    responder: Mutex<Box<Responder>>,
    requests: Mutex<Vec<HttpRequest>>,
}

impl FakeGitHubTransport {
    /// Transporte que responde a todo con un fallo de red.
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            responder: Mutex::new(Box::new(|_| {
                Err(TransportError::Network("no response scripted".to_string()))
            })),
            requests: Mutex::new(Vec::new()),
        })
    }

    /// Instala quién responde a las peticiones a partir de ahora.
    pub fn respond_with(
        &self,
        responder: impl FnMut(&HttpRequest) -> Result<HttpResponse, TransportError> + Send + 'static,
    ) {
        *self
            .responder
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Box::new(responder);
    }

    /// Peticiones recibidas, en orden.
    pub fn requests(&self) -> Vec<HttpRequest> {
        self.requests
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// URLs de las peticiones recibidas, en orden.
    pub fn requested_urls(&self) -> Vec<String> {
        self.requests()
            .into_iter()
            .map(|request| request.url)
            .collect()
    }

    /// Respuesta JSON con ese estado.
    pub fn json(status: u16, body: serde_json::Value) -> Result<HttpResponse, TransportError> {
        Ok(HttpResponse {
            status,
            headers: vec![("content-type".to_string(), "application/json".to_string())],
            body: body.to_string().into_bytes(),
        })
    }
}

impl HttpTransport for FakeGitHubTransport {
    fn send(&self, request: &HttpRequest) -> Result<HttpResponse, TransportError> {
        self.requests
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(request.clone());
        let mut responder = self
            .responder
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        responder(request)
    }
}

/// Un backend para tests: configuración en memoria, sin llavero, sin vigilar
/// el disco, sin lanzar procesos del sistema y con GitHub guionizado.
pub struct TestBackend {
    /// Lo que el backend habría abierto (URLs, terminal, explorador).
    pub launcher: Arc<RecordingLauncher>,
    /// El "GitHub" del test.
    pub github: Arc<FakeGitHubTransport>,
    store: Option<Store>,
    updater: Option<Arc<dyn UpdaterBridge>>,
    github_config: GitHubConfig,
    /// Programas "instalados" en el `PATH` de mentira ([`TEST_PATH`]).
    installed: Vec<String>,
}

/// El `PATH` del sistema de mentira de [`TestBackend::with_installed`].
pub const TEST_PATH: &str = "/test-bin";

impl Default for TestBackend {
    fn default() -> Self {
        Self {
            launcher: RecordingLauncher::new(),
            github: FakeGitHubTransport::new(),
            store: None,
            updater: None,
            github_config: GitHubConfig::default(),
            installed: Vec::new(),
        }
    }
}

impl TestBackend {
    pub fn new() -> Self {
        Self::default()
    }

    /// Store en memoria sin cifrado, como el que usa por defecto.
    pub fn memory_store() -> Store {
        Store::in_memory(StoreOptions::new(Arc::new(PlainTextCipher)))
    }

    /// Usa este store (ya sembrado) en vez de uno vacío.
    pub fn with_store(mut self, store: Store) -> Self {
        self.store = Some(store);
        self
    }

    /// Simula que estos programas están instalados en el `PATH` (que pasa a
    /// ser solo [`TEST_PATH`]): así se prueba la detección de editores.
    pub fn with_installed(mut self, programs: &[&str]) -> Self {
        self.installed = programs
            .iter()
            .map(|program| format!("{TEST_PATH}/{program}"))
            .collect();
        self
    }

    /// Enchufa un actualizador.
    pub fn with_updater(mut self, updater: Arc<dyn UpdaterBridge>) -> Self {
        self.updater = Some(updater);
        self
    }

    /// Configuración del cliente de GitHub (modo App u OAuth, URLs...).
    pub fn with_github_config(mut self, config: GitHubConfig) -> Self {
        self.github_config = config;
        self
    }

    /// Las opciones del backend, por si el test quiere ajustarlas antes de
    /// construirlo con [`crate::backend_with_events`].
    pub fn options(&mut self) -> BackendOptions {
        let mut options = BackendOptions::new()
            .store(self.store.take().unwrap_or_else(Self::memory_store))
            .github(GitHubClient::with_transport(
                self.github_config.clone(),
                self.github.clone(),
            ))
            // Ningún test abre programas del sistema, ni encuentra los
            // editores que haya instalados en la máquina que los corre.
            .shell(self.shell())
            .watcher_options(silent_watcher_options());
        if let Some(updater) = self.updater.take() {
            options = options.updater(updater);
        }
        options
    }

    fn shell(&self) -> Shell {
        let shell = Shell::with_launcher(self.launcher.clone(), ShellPlatform::Linux);
        if self.installed.is_empty() {
            return shell.with_file_probe(|_| false);
        }
        let installed = self.installed.clone();
        shell
            .with_env(|name| (name == "PATH").then(|| TEST_PATH.to_string()))
            .with_file_probe(move |path| installed.iter().any(|program| program == path))
    }

    /// Construye el backend con sus eventos encaminados.
    ///
    /// # Panics
    ///
    /// Si el backend no se puede construir, que con un store en memoria no
    /// ocurre.
    pub fn build(&mut self) -> (Arc<Backend>, EventReceiver) {
        let (callback, events) = event_channel();
        let backend = Backend::new(self.options().on_event(callback)).expect("test backend");
        (Arc::new(backend), events)
    }
}
