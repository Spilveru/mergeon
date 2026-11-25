//! [`AppModel`]: el estado global de la aplicación y el punto de entrada de
//! sus flujos. Reemplaza a los stores de Pinia y al `<script setup>` de
//! `App.vue`.

use std::rc::Rc;
use std::sync::Arc;

use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui_kit::{App, AppContext as _, Context, Entity, EventEmitter, Global, Task};
use mergeon_backend::{Backend, BackendError, BackendEvent, BackendOptions, StashEntry, Submodule};
use mergeon_model::dialogs::ConfirmOptions;
use mergeon_model::notify::{Notifier, Toast};
use mergeon_model::operations_log::OperationsLog;
use mergeon_model::repositories::{RepositoriesStore, Repository};
use mergeon_model::workspace_tabs::WorkspaceSection;

use crate::git_client::GitClientState;
use crate::github::GitHubState;
use crate::repository_sync::SyncState;
use crate::settings::SettingsState;
use crate::shell::{DialogRequest, ShellState, WindowRequest};
use crate::storage::StorageQueue;
use crate::summary::RepositorySummaries;
use crate::tasks::flow;
use crate::ui_host::UiHost;
use crate::updater::UpdaterModel;
use crate::workspace::WorkspaceState;

/// Un repositorio abierto (una pestaña), con los tipos de stash y submódulo
/// de `mergeon-git-query`.
pub type OpenRepository = Repository<StashEntry, Submodule>;

/// El store de repositorios con esos mismos tipos.
pub type Repositories = RepositoriesStore<StashEntry, Submodule>;

/// Lo que una vista necesita saber además de "algo cambió" (que llega por
/// `cx.observe`). Se recibe con `cx.subscribe(&model, ...)`.
#[derive(Debug, Clone, PartialEq)]
pub enum AppEvent {
    /// Un repositorio registró una mutación (`lastMutationAt` cambió): el
    /// historial y lo que dependa de las refs debe releerse. `operation` es el
    /// `lastMutationOperation` (`commit`, `fetch`, `status-watch`,
    /// `external-head-change`, `hydrate-opened-repository`...).
    MutationRecorded {
        repo_id: String,
        operation: Option<String>,
    },
    /// Cambió el repositorio activo (`None`: ninguno, se ve el inicio).
    ActiveRepositoryChanged { repo_id: Option<String> },
    /// Abrir Settings en una sección (`appearance`, `help`...).
    OpenSettings { section: String },
    /// Ir a la búsqueda de commits de la barra lateral y enfocarla
    /// (`sidebarSearchRequest`).
    FocusCommitSearch,
    /// `handleChangeView`: la vista fija visible del repositorio activo.
    ViewChanged { view: WorkspaceSection },
    /// Una pestaña de archivo cambió de identidad sin dejar de ser la misma
    /// (`onReplace` de `createFileOpener`): su panel debe seguir montado.
    WorkspaceTabReplaced {
        repo_path: String,
        from: String,
        to: String,
    },
    /// Llevar el historial de un repositorio a un commit y elegirlo (un clic
    /// en el hash de "File history" o del blame). La pestaña Commits ya está
    /// activa cuando llega: la vista de commits solo tiene que seleccionarlo.
    CommitSelectionRequested { repo_id: String, hash: String },
    /// Deshacer un commit devolvió su mensaje a la caja de commit (si estaba
    /// vacía): la vista de cambios locales rellena Summary y Description.
    CommitDraftRestored { repo_path: String, message: String },
    /// Un flujo pide abrir un diálogo de la cáscara.
    ShowDialog(DialogRequest),
    /// El tema cambió (`applyTheme`): la cáscara lo aplica a la ventana.
    ThemeChanged { dark: bool },
    /// Algo que solo puede hacer la ventana.
    Window(WindowRequest),
}

struct GlobalAppModel(Entity<AppModel>);

impl Global for GlobalAppModel {}

/// Extremo receptor de los eventos del backend. Lo consume
/// [`AppModel::init`], que los aplica en el hilo de UI.
pub struct EventReceiver {
    receiver: mpsc::UnboundedReceiver<BackendEvent>,
}

impl EventReceiver {
    /// Receptor al que nunca llega nada (un backend construido sin
    /// [`event_channel`]).
    pub fn disconnected() -> Self {
        let (_sender, receiver) = mpsc::unbounded();
        Self { receiver }
    }
}

impl std::fmt::Debug for EventReceiver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventReceiver").finish_non_exhaustive()
    }
}

/// El callback para [`BackendOptions::on_event`] y el receptor que hay que
/// pasar a [`AppModel::init`]. El callback se puede llamar desde cualquier
/// hilo y vuelve enseguida: solo deja el evento en el canal.
pub fn event_channel() -> (impl Fn(BackendEvent) + Send + Sync + 'static, EventReceiver) {
    let (sender, receiver) = mpsc::unbounded();
    let callback = move |event: BackendEvent| {
        // Si la interfaz ya no escucha (la app se cierra) el evento se pierde.
        let _ = sender.unbounded_send(event);
    };
    (callback, EventReceiver { receiver })
}

/// Construye el [`Backend`] con sus eventos ya encaminados hacia la interfaz.
///
/// ```no_run
/// use mergeon_app::{AppModel, backend_with_events};
/// use mergeon_backend::BackendOptions;
///
/// # fn run(cx: &mut gpui_kit::App) -> Result<(), mergeon_backend::BackendError> {
/// let (backend, events) = backend_with_events(BackendOptions::new())?;
/// let model = AppModel::init(backend, events, cx);
/// model.update(cx, |model, cx| model.start(cx)).detach();
/// # Ok(())
/// # }
/// ```
pub fn backend_with_events(
    options: BackendOptions,
) -> Result<(Arc<Backend>, EventReceiver), BackendError> {
    let (callback, receiver) = event_channel();
    let backend = Backend::new(options.on_event(callback))?;
    Ok((Arc::new(backend), receiver))
}

/// Estado global de la aplicación: repositorios abiertos, recientes, sesión,
/// ajustes, cuentas de GitHub, log de operaciones y actualizador, más los
/// flujos que los cambian.
///
/// Es una entidad de GPUI: las vistas la obtienen con [`AppModel::global`],
/// la leen con `model.read(cx)`, la observan con `cx.observe(&model, ...)` y
/// llaman a sus métodos con `model.update(cx, |model, cx| ...)`. Los métodos
/// con trabajo de fondo devuelven un `Task`; las acciones (mutaciones y
/// flujos) siguen hasta el final aunque el `Task` se suelte.
pub struct AppModel {
    pub(crate) backend: Arc<Backend>,
    pub(crate) storage_queue: StorageQueue,
    pub(crate) ui_host: Option<Rc<dyn UiHost>>,
    pub(crate) notifier: Notifier,
    /// Avisos pedidos antes de que la cáscara instalara su [`UiHost`].
    pub(crate) pending_toasts: Vec<Toast>,
    pub(crate) repositories: Repositories,
    pub(crate) operations_log: OperationsLog,
    pub(crate) git: GitClientState,
    pub(crate) sync: SyncState,
    pub(crate) shell: ShellState,
    pub(crate) workspaces: WorkspaceState,
    pub(crate) github: GitHubState,
    pub(crate) settings: SettingsState,
    pub(crate) updater: UpdaterModel,
    pub(crate) summaries: RepositorySummaries,
    pub(crate) progress_toasts: crate::progress::ProgressToasts,
    pub(crate) undo: crate::undo::UndoState,
    /// Credenciales HTTP tecleadas en la sesión (ver `credentials`).
    pub(crate) credentials: crate::credentials::CredentialState,
    /// Hay un aviso a las vistas pendiente ([`AppModel::notify_soon`]).
    notify_pending: bool,
    _event_pump: Task<()>,
}

impl EventEmitter<AppEvent> for AppModel {}

impl AppModel {
    /// Crea el modelo, lo registra como global y empieza a aplicar los eventos
    /// del backend. No lee nada todavía: la cáscara llama a
    /// [`AppModel::start`] cuando tenga su ventana (y su [`UiHost`]).
    pub fn init(backend: Arc<Backend>, events: EventReceiver, cx: &mut App) -> Entity<AppModel> {
        let model = cx.new(|cx| AppModel::new(backend, events, cx));
        cx.set_global(GlobalAppModel(model.clone()));
        model
    }

    /// El modelo registrado por [`AppModel::init`].
    ///
    /// # Panics
    ///
    /// Si `init` no se ha llamado.
    pub fn global(cx: &App) -> Entity<AppModel> {
        cx.global::<GlobalAppModel>().0.clone()
    }

    /// Como [`AppModel::global`], sin pánico si aún no existe.
    pub fn try_global(cx: &App) -> Option<Entity<AppModel>> {
        cx.try_global::<GlobalAppModel>()
            .map(|global| global.0.clone())
    }

    fn new(backend: Arc<Backend>, events: EventReceiver, cx: &mut Context<Self>) -> Self {
        let mut events = events.receiver;
        let event_pump = cx.spawn(async move |this, cx| {
            while let Some(event) = events.next().await {
                let applied = this.update(cx, |this, cx| this.handle_backend_event(event, cx));
                if applied.is_err() {
                    break;
                }
            }
        });

        Self {
            storage_queue: StorageQueue::new(cx),
            backend,
            ui_host: None,
            notifier: Notifier::new(),
            pending_toasts: Vec::new(),
            repositories: Repositories::new(),
            operations_log: OperationsLog::new(),
            git: GitClientState::default(),
            sync: SyncState::default(),
            shell: ShellState::default(),
            workspaces: WorkspaceState::default(),
            github: GitHubState::default(),
            settings: SettingsState::default(),
            updater: UpdaterModel::default(),
            summaries: RepositorySummaries::default(),
            progress_toasts: Default::default(),
            undo: Default::default(),
            credentials: Default::default(),
            notify_pending: false,
            _event_pump: event_pump,
        }
    }

    /// Avisa a las vistas de un cambio, una sola vez por vuelta del executor
    /// de la interfaz: varios cambios seguidos (los pasos de una relectura que
    /// corren sin esperar nada entre medias, o varias tareas que terminan a
    /// la vez) se pintan en un solo aviso. Cada aviso del modelo hace que las
    /// vistas que lo leen vuelvan a pintar la ventana entera.
    pub(crate) fn notify_soon(&mut self, cx: &mut Context<Self>) {
        if self.notify_pending {
            return;
        }
        self.notify_pending = true;
        cx.spawn(async move |this, cx| {
            this.update(cx, |this, cx| {
                if std::mem::take(&mut this.notify_pending) {
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// El backend, para lo que no tenga método propio aquí. Sus llamadas
    /// bloquean: hay que hacerlas desde `cx.background_spawn`.
    pub fn backend(&self) -> &Arc<Backend> {
        &self.backend
    }

    /// Lo que `App.vue` hacía en `onMounted`: carga el tema, los recientes y
    /// la sesión, inicializa las cuentas de GitHub, restaura los repositorios
    /// abiertos, lee los ajustes y programa el auto-fetch.
    ///
    /// El original lo hacía todo en serie y la sesión esperaba a GitHub, que
    /// puede refrescar tokens por red. Aquí, tras el tema y los recientes
    /// (lo que pinta el primer frame), la sesión se restaura mientras GitHub,
    /// los ajustes y el actualizador se leen en paralelo. El `Task` termina
    /// cuando ha terminado todo.
    pub fn start(&mut self, cx: &mut Context<Self>) -> Task<()> {
        flow(cx, async move |this, cx| {
            // Cargar tema
            this.update(cx, |this, cx| this.load_theme(cx)).await;
            crate::startup::trace("theme loaded");

            // Cargar datos desde storage
            this.update(cx, |this, cx| this.load_repositories_from_storage(cx))
                .await;

            // Lo que no hace falta para ver y usar los repositorios, de fondo.
            let github = this.update(cx, |this, cx| {
                let initialized = this.initialize_github(cx);
                flow(cx, async move |this, cx| {
                    initialized.await;
                    crate::startup::trace("github initialized");
                    this.update(cx, |this, cx| {
                        if !this.github.auth_service_available {
                            let message = this.github.auth_service_message.clone();
                            this.notify_warning(&message, cx);
                        }
                    });
                })
            });
            let settings = this.update(cx, |this, cx| {
                let loaded = this.load_app_settings(false, cx);
                flow(cx, async move |this, cx| {
                    loaded.await;
                    this.update(cx, |this, cx| this.schedule_auto_fetch(cx));
                })
            });
            // `useUpdater`: el estado inicial del actualizador.
            let updater = this.update(cx, |this, cx| this.load_updater_state(cx));

            this.update(cx, |this, cx| this.restore_workspace_session(cx))
                .await;
            crate::startup::trace("session restored");

            futures::join!(github, settings, updater);
            crate::startup::trace("start finished");
        })
    }

    /// Botón y atajo "Reload application": pide confirmación y, si se acepta,
    /// recarga ([`AppModel::reload_application`]). Devuelve si se recargó.
    pub fn request_reload_application(&mut self, cx: &mut Context<Self>) -> Task<bool> {
        let answer = self.confirm(
            ConfirmOptions {
                title: Some("Reload Mergeon?".to_string()),
                description: Some(
                    "Repositories are read again from disk. Open tabs and commit drafts are kept."
                        .to_string(),
                ),
                confirm_text: Some("Reload".to_string()),
                cancel_text: Some("Cancel".to_string()),
                ..Default::default()
            },
            cx,
        );
        flow(cx, async move |this, cx| {
            let confirmed = answer.await;
            if confirmed {
                this.update(cx, |this, cx| this.reload_application(cx))
                    .await;
            }
            confirmed
        })
    }

    /// `reloadApplication` de `App.vue`: guarda el estado de las pestañas de
    /// cada repositorio y recarga los datos como al arrancar. En la app
    /// Electron esto recargaba la ventana; aquí se reinicia el estado que
    /// vivía en el renderer y se vuelve a ejecutar [`AppModel::start`].
    pub fn reload_application(&mut self, cx: &mut Context<Self>) -> Task<()> {
        flow(cx, async move |this, cx| {
            this.update(cx, |this, cx| this.flush_workspaces(cx)).await;
            this.update(cx, |this, cx| this.stop_watching_repository_status(cx))
                .await;
            this.update(cx, |this, cx| {
                this.reset_session_state(cx);
                cx.emit(AppEvent::Window(WindowRequest::Reloaded));
            });
            this.update(cx, |this, cx| this.start(cx)).await;
        })
    }

    /// Deja el estado como recién creado, conservando el backend, la interfaz
    /// instalada y lo que en Electron vivía en el proceso principal.
    fn reset_session_state(&mut self, cx: &mut Context<Self>) {
        let previous_id = self
            .repositories
            .active_repository()
            .map(|repo| repo.id.clone());

        self.repositories = Repositories::new();
        self.operations_log = OperationsLog::new();
        self.git = GitClientState::default();
        self.sync = SyncState::default();
        self.shell.reset();
        self.workspaces = WorkspaceState::default();
        self.github = GitHubState::default();
        self.settings.reset();
        self.updater = UpdaterModel::default();
        self.summaries = RepositorySummaries::default();

        if previous_id.is_some() {
            cx.emit(AppEvent::ActiveRepositoryChanged { repo_id: None });
        }
        cx.notify();
    }

    /// Reparte un evento del backend: lo que `App.vue` escuchaba con
    /// `onStatusChanged`, `onOperationEvent` y `updater.onEvent`. La cáscara
    /// no necesita llamarlo: [`AppModel::init`] ya consume el canal.
    pub fn handle_backend_event(&mut self, event: BackendEvent, cx: &mut Context<Self>) {
        match event {
            BackendEvent::StatusChanged(change) => self.handle_status_changed(&change, cx),
            BackendEvent::Operation(event) => self.handle_operation_event(&event, cx),
            BackendEvent::OperationProgress(update) => self.handle_operation_progress(&update, cx),
            BackendEvent::Updater(payload) => self.handle_updater_payload(&payload, cx),
        }
    }

    /// La app se cierra: se guarda el estado de las pestañas y se detiene el
    /// backend (watcher y actualizador).
    pub fn shutdown(&mut self, cx: &mut Context<Self>) -> Task<()> {
        let flushed = self.flush_workspaces(cx);
        let backend = self.backend.clone();
        // Las copias de los descartes de esta sesión ya no sirven.
        let backups = self.discard_undo_backups();
        cx.spawn(async move |_, cx| {
            flushed.await;
            cx.background_spawn(async move {
                backend.shutdown();
                if let Some(backups) = backups {
                    let _ = std::fs::remove_dir_all(backups);
                }
            })
            .await;
        })
    }

    /// Log de operaciones de la sesión.
    pub fn operations_log(&self) -> &OperationsLog {
        &self.operations_log
    }

    /// `operationsLogStore.markSeen()`.
    pub fn mark_operations_seen(&mut self, cx: &mut Context<Self>) {
        self.operations_log.mark_seen();
        cx.notify();
    }

    /// `operationsLogStore.clear()`.
    pub fn clear_operations_log(&mut self, cx: &mut Context<Self>) {
        self.operations_log.clear();
        cx.notify();
    }
}
