//! La orquestación que vivía en el `<script setup>` de `App.vue`: abrir y
//! restaurar repositorios, vigilar el estado del repositorio activo, aplicar
//! los eventos de operación, fetch / pull / push / stash, auto-fetch, nueva
//! rama, continuar o abortar una operación, cancelar, atajos y la navegación
//! entre el inicio, los repositorios y Settings.
//!
//! Lo que en `App.vue` era "mostrar un diálogo" (`showFetchDialog.value =
//! true`...) aquí es un [`AppEvent::ShowDialog`]: qué diálogo está abierto es
//! estado de la cáscara.

use std::collections::{BTreeSet, HashMap};
use std::time::{Duration, Instant};

use gpui_kit::{AppContext as _, Context, Task};
use mergeon_backend::{
    DetectedEditor, EditorOpened, FetchOptions, Mutation, OperationEvent, OperationPhase,
    PullOptions, PushOptions, RepositoryKind, SequenceActionResult, StashOptions, StatusChange,
    WatchReason,
};
use mergeon_model::dialogs::{ConfirmOptions, PromptOptions};
use mergeon_model::keyboard_shortcuts::ShortcutAction;
use mergeon_model::notify::{NotifyOptions, NotifyType, ToastAction, ToastId};
use mergeon_model::operations_log::ResultContext;
use mergeon_model::repositories::{MutationQueue, normalize_repository_path};
use mergeon_model::repository_read_model::BranchItem;
use mergeon_model::workspace_tabs::WorkspaceSection;

use crate::convert::{StructuredOutcome, operation_event, queue_snapshot};
use crate::flows::{CreateBranchRequest, FlowStatus, LocalChangesMode};
use crate::repositories::AddRepository;
use crate::repository_sync::SyncRequest;
use crate::tasks::{InFlightMap, flow, settled};
use crate::{AppEvent, AppModel, OpenRepository};

/// Clave del botón "Choose editor…" de los avisos de "Open in editor": abre
/// Ajustes → Git (donde está "External editor").
pub const CHOOSE_EDITOR_ACTION: &str = "settings:choose-editor";
/// Id del aviso de "Open in editor": uno nuevo reemplaza al anterior.
const OPEN_IN_EDITOR_TOAST: &str = "open-in-editor";

/// Espera tras el último aviso del watcher antes de releer el estado.
const STATUS_REFRESH_DELAY: Duration = Duration::from_millis(250);

/// Operaciones de la barra de herramientas que se bloquean entre sí mientras
/// una está en curso (`busyToolbarOperation`).
const TOOLBAR_OPERATIONS: [&str; 5] = ["fetch", "pull", "push", "stash", "create-branch"];

/// Diálogo que un flujo pide abrir a la cáscara.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialogRequest {
    /// `showLauncher`: abrir, clonar o crear un repositorio.
    Launcher,
    /// `showAccountDialog`: añadir una cuenta de GitHub.
    GitHubAccount,
    /// `showRepoPicker`: elegir un repositorio de GitHub para clonar.
    GitHubRepositoryPicker,
    /// "Clone from URL": clonar pegando la URL de cualquier remoto.
    CloneFromUrl,
    Fetch,
    Pull,
    Push,
    /// `showCreateBranchDialog`, con lo que `handleNewBranch` preparó.
    CreateBranch {
        base_reference: String,
        base_label: String,
        has_local_changes: bool,
    },
    OperationsLog,
    Reflog,
    /// El diálogo de stash. `paths` vacío guarda todo; si no, solo esas rutas
    /// ("Stash selected files…" de Local Changes).
    Stash {
        paths: Vec<String>,
    },
}

/// Lo que solo la ventana puede hacer (`window.api.app.*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WindowRequest {
    Minimize,
    Quit,
    ToggleDevTools,
    /// `reloadApplication` terminó de reiniciar el estado: las vistas deben
    /// descartar lo que tuvieran cargado.
    Reloaded,
}

/// Por qué hay que releer el estado del repositorio activo: el `reason` de
/// `git:status-changed` o el control de una operación.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StatusSyncReason {
    FsWatchHead,
    FsWatchWorkingTree,
    PollHead,
    PollWorkingTree,
    /// Una operación terminó por cancelación o timeout.
    GitOperationControl,
}

impl StatusSyncReason {
    pub fn as_str(self) -> &'static str {
        match self {
            StatusSyncReason::FsWatchHead => "fs-watch-head",
            StatusSyncReason::FsWatchWorkingTree => "fs-watch-working-tree",
            StatusSyncReason::PollHead => "poll-head",
            StatusSyncReason::PollWorkingTree => "poll-working-tree",
            StatusSyncReason::GitOperationControl => "git-operation-control",
        }
    }

    fn is_fs_watch(self) -> bool {
        self.as_str().starts_with("fs-watch")
    }
}

impl StatusSyncReason {
    /// El motivo de releer el estado para un aviso del watcher; `None` para
    /// los de metadatos (tags, remotos, config), que van por
    /// [`AppModel::queue_metadata_refresh`].
    pub fn from_watch_reason(reason: WatchReason) -> Option<Self> {
        Some(match reason {
            WatchReason::FsWatchHead => StatusSyncReason::FsWatchHead,
            WatchReason::FsWatchWorkingTree => StatusSyncReason::FsWatchWorkingTree,
            WatchReason::PollHead => StatusSyncReason::PollHead,
            WatchReason::PollWorkingTree => StatusSyncReason::PollWorkingTree,
            WatchReason::FsWatchRefs
            | WatchReason::FsWatchConfig
            | WatchReason::PollRefs
            | WatchReason::PollConfig
            | WatchReason::FsWatchStash
            | WatchReason::PollStash => return None,
        })
    }
}

/// Metadatos del repositorio activo que cambiaron fuera de la app y hay que
/// releer: tags y ramas remotas (`refs`), remotos (`config`) y stashes
/// (`stash`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct MetadataRefresh {
    pub refs: bool,
    pub config: bool,
    pub stash: bool,
}

impl MetadataRefresh {
    pub fn from_watch_reason(reason: WatchReason) -> Self {
        Self {
            refs: reason.is_refs_change(),
            config: reason.is_config_change(),
            stash: reason.is_stash_change(),
        }
    }

    pub fn is_empty(self) -> bool {
        !self.refs && !self.config && !self.stash
    }

    pub fn merge(self, other: Self) -> Self {
        Self {
            refs: self.refs || other.refs,
            config: self.config || other.config,
            stash: self.stash || other.stash,
        }
    }

    /// Lo que hay que releer: con refs, tags, ramas (ahead/behind de sus
    /// upstream) y remotos, y se registra como mutación para que el historial
    /// pinte las etiquetas nuevas; con config, los remotos; con stash, los
    /// stashes (y también cuenta como mutación: el historial pinta el stash).
    ///
    /// Registrar la mutación invalida las lecturas en vuelo, incluida la de
    /// status que pudo lanzar a la vez un aviso de working tree (`git stash`
    /// toca ambos): por eso, con mutación, el status también se relee aquí.
    pub fn sync_request(self) -> SyncRequest {
        let request = SyncRequest::new()
            .remotes(self.refs || self.config)
            .tags(self.refs)
            .branches(self.refs)
            .stashes(self.stash);
        if self.refs || self.stash {
            request
                .status(true)
                .changes_count(true)
                .operation("external-refs-change")
        } else {
            request
        }
    }
}

/// `isHeadStatusWatchReason`: el cambio pudo mover HEAD, así que también hay
/// que releer las ramas.
pub fn is_head_status_watch_reason(reason: StatusSyncReason) -> bool {
    matches!(
        reason,
        StatusSyncReason::FsWatchHead | StatusSyncReason::PollHead
    )
}

/// `mergeStatusWatchReason`: funde dos motivos pendientes en uno. Gana el que
/// toca HEAD y, entre iguales, el que viene del sistema de archivos.
pub fn merge_status_watch_reason(
    current: Option<StatusSyncReason>,
    next: StatusSyncReason,
) -> StatusSyncReason {
    let Some(current) = current else {
        return next;
    };

    if current == next {
        return current;
    }

    let from_fs_watch = current.is_fs_watch() || next.is_fs_watch();
    if is_head_status_watch_reason(current) || is_head_status_watch_reason(next) {
        if from_fs_watch {
            StatusSyncReason::FsWatchHead
        } else {
            StatusSyncReason::PollHead
        }
    } else if from_fs_watch {
        StatusSyncReason::FsWatchWorkingTree
    } else {
        StatusSyncReason::PollWorkingTree
    }
}

/// `shouldSyncAfterSequenceAction`.
pub fn should_sync_after_sequence_action(result: &Mutation<SequenceActionResult>) -> bool {
    result.success()
        || result.outcome() == Some(mergeon_backend::Outcome::Conflicted)
        || result.code() == "CONFLICTS_REMAIN"
}

/// Lo que envía el diálogo de fetch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchRequest {
    pub remote: String,
    pub all_remotes: bool,
}

/// Lo que envía el diálogo de pull.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequest {
    pub remote: String,
    pub branch: Option<String>,
    pub rebase: bool,
    pub autostash: bool,
}

/// Lo que envía el diálogo de stash (las opciones de `git stash push`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StashRequest {
    pub message: Option<String>,
    /// Vacío: todo.
    pub paths: Vec<String>,
    pub include_untracked: bool,
    pub keep_index: bool,
    pub staged_only: bool,
}

/// Lo que envía el diálogo de push. `destination` es `<remoto>/<rama>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushRequest {
    pub branch: String,
    pub destination: String,
    pub push_tags: bool,
    pub force: bool,
}

/// Lo que envía el diálogo de nueva rama.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateBranchSubmission {
    pub branch_name: String,
    pub checkout_after_create: bool,
    pub local_changes_mode: LocalChangesMode,
}

/// Estado de `App.vue` que no es de ningún store.
#[derive(Debug)]
pub(crate) struct ShellState {
    pub single_flight: InFlightMap,

    // Navegación
    pub show_preferences: bool,
    pub settings_tab_open: bool,
    pub settings_section: String,
    /// Vista activa (commits o changes)
    pub active_view: WorkspaceSection,
    pub visited_repository_ids: BTreeSet<String>,
    pub adding_account_for_settings: bool,
    /// Huella de las refs de cada repositorio abierto pero oculto, tomada al
    /// ocultarlo (ver `tabs.rs`).
    pub refs_fingerprints: HashMap<String, String>,
    /// Rutas de los repositorios cerrados, el último al final.
    pub closed_repositories: Vec<String>,

    // Nueva rama
    pub create_branch_base_reference: String,

    // Watcher de estado
    pub status_watch_sync_in_flight: bool,
    pub active_status_watch_path: Option<String>,
    pub restoring_workspace: bool,
    pub status_refresh_timer: Option<Task<()>>,
    pub queued_status_refresh_reason: Option<StatusSyncReason>,
    pub busy_sequence_action: Option<String>,

    // Foco de la ventana y auto-fetch
    pub window_focused: bool,
    pub auto_fetch_timer: Option<Task<()>>,
    pub last_auto_fetch_at: Option<Instant>,
    /// Duración mínima de una operación de red para notificarla al escritorio.
    pub desktop_notification_threshold: Duration,
    /// Motivos de metadatos (remotos, tags, config) pendientes de releer.
    pub queued_metadata_refresh: MetadataRefresh,
    pub metadata_refresh_timer: Option<Task<()>>,
}

impl Default for ShellState {
    fn default() -> Self {
        Self {
            single_flight: InFlightMap::default(),
            show_preferences: false,
            settings_tab_open: false,
            settings_section: "appearance".to_string(),
            active_view: WorkspaceSection::Commits,
            visited_repository_ids: BTreeSet::new(),
            adding_account_for_settings: false,
            refs_fingerprints: HashMap::new(),
            closed_repositories: Vec::new(),
            create_branch_base_reference: "HEAD".to_string(),
            status_watch_sync_in_flight: false,
            active_status_watch_path: None,
            restoring_workspace: false,
            status_refresh_timer: None,
            queued_status_refresh_reason: None,
            busy_sequence_action: None,
            window_focused: true,
            auto_fetch_timer: None,
            last_auto_fetch_at: None,
            desktop_notification_threshold: crate::desktop_notifications::LONG_OPERATION,
            queued_metadata_refresh: MetadataRefresh::default(),
            metadata_refresh_timer: None,
        }
    }
}

impl ShellState {
    /// Estado de una ventana recién cargada; el foco es de la ventana real y
    /// se conserva.
    pub fn reset(&mut self) {
        let window_focused = self.window_focused;
        let desktop_notification_threshold = self.desktop_notification_threshold;
        *self = Self {
            window_focused,
            desktop_notification_threshold,
            ..Self::default()
        };
    }
}

fn single_flight(model: &mut AppModel) -> &mut InFlightMap {
    &mut model.shell.single_flight
}

pub(crate) fn same_repository_path(left: &str, right: &str) -> bool {
    normalize_repository_path(left) == normalize_repository_path(right)
}

impl AppModel {
    // ------------------------------------------------------------------
    // useSingleFlight
    // ------------------------------------------------------------------

    /// `runSingleFlight`: mientras haya una tarea en curso con esa clave, las
    /// nuevas peticiones esperan a la misma en vez de arrancar otra.
    pub fn run_single_flight(
        &mut self,
        key: impl Into<String>,
        cx: &mut Context<Self>,
        task: impl FnOnce(&mut Self, &mut Context<Self>) -> Task<()>,
    ) -> Task<()> {
        let shared = self.run_shared(single_flight, key.into(), true, cx, task);
        cx.foreground_executor().spawn(async move {
            settled(shared).await;
        })
    }

    /// `isSingleFlightRunning`.
    pub fn is_single_flight_running(&self, key: &str) -> bool {
        self.shell.single_flight.contains(key)
    }

    /// `getRepoOperationKey`.
    pub fn get_repo_operation_key(repo_id: &str, operation: &str) -> String {
        format!("{repo_id}:{operation}")
    }

    /// `runRepoOperationSingleFlight`.
    pub fn run_repo_operation_single_flight(
        &mut self,
        repo_id: &str,
        operation: &str,
        cx: &mut Context<Self>,
        task: impl FnOnce(&mut Self, &mut Context<Self>) -> Task<()>,
    ) -> Task<()> {
        self.run_single_flight(Self::get_repo_operation_key(repo_id, operation), cx, task)
    }

    // ------------------------------------------------------------------
    // Lo que `App.vue` calculaba para la barra de herramientas
    // ------------------------------------------------------------------

    /// `busyToolbarOperation`: la operación de la barra que está en curso en el
    /// repositorio activo.
    pub fn busy_toolbar_operation(&self) -> Option<&'static str> {
        let repo_id = &self.active_repository()?.id;
        TOOLBAR_OPERATIONS.into_iter().find(|operation| {
            self.is_single_flight_running(&Self::get_repo_operation_key(repo_id, operation))
        })
    }

    /// `activeRepoMutationQueue`.
    pub fn active_repo_mutation_queue(&self) -> MutationQueue {
        self.active_repository()
            .map(|repository| repository.mutation_queue.clone())
            .unwrap_or_default()
    }

    /// `busySequenceAction`: la acción (`continue`, `abort`, `skip`) en curso.
    pub fn busy_sequence_action(&self) -> Option<&str> {
        self.shell.busy_sequence_action.as_deref()
    }

    /// `activeRepoCurrentBranch`.
    pub fn active_repo_current_branch(&self) -> &str {
        self.active_repository()
            .and_then(|repository| repository.current_branch.as_deref())
            .filter(|branch| !branch.is_empty())
            .unwrap_or("HEAD")
    }

    /// `activeBranchEntry`: la rama actual tal como la lista la barra lateral.
    pub fn active_branch_entry(&self) -> Option<&BranchItem> {
        let current = self.active_repo_current_branch();
        self.repositories
            .active_repository_branches()
            .iter()
            .find(|branch| branch.name == current)
    }

    /// `activeRepoHasLocalChanges`.
    pub fn active_repo_has_local_changes(&self) -> bool {
        self.active_repository()
            .and_then(|repository| repository.status.as_ref())
            .is_some_and(|status| !status.files.is_empty())
    }

    /// `defaultRemoteName`: el remoto del upstream de la rama actual o, si no
    /// tiene, el primero configurado (`origin` si no hay ninguno).
    pub fn default_remote_name(&self) -> String {
        let upstream = self
            .active_branch_entry()
            .and_then(|branch| branch.upstream.as_deref())
            .unwrap_or_default();
        if let Some((remote, _)) = upstream.split_once('/') {
            return remote.to_string();
        }

        self.repositories
            .active_repository_remotes()
            .first()
            .map(|remote| remote.name.clone())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "origin".to_string())
    }

    /// `defaultRemoteBranchName`: la rama del upstream o, si no tiene, la
    /// actual.
    pub fn default_remote_branch_name(&self) -> String {
        let upstream = self
            .active_branch_entry()
            .and_then(|branch| branch.upstream.as_deref())
            .unwrap_or_default();
        match upstream.split_once('/') {
            Some((_, branch)) => branch.to_string(),
            None => self.active_repo_current_branch().to_string(),
        }
    }

    // ------------------------------------------------------------------
    // Navegación
    // ------------------------------------------------------------------

    /// Vista fija visible del repositorio activo (`activeView`).
    pub fn active_view(&self) -> WorkspaceSection {
        self.shell.active_view
    }

    /// Settings está a la vista (`showPreferences`).
    pub fn show_preferences(&self) -> bool {
        self.shell.show_preferences
    }

    /// La pestaña de Settings está abierta, aunque no sea la visible
    /// (`settingsTabOpen`).
    pub fn settings_tab_open(&self) -> bool {
        self.shell.settings_tab_open
    }

    /// Sección de Settings (`settingsSection`).
    pub fn settings_section(&self) -> &str {
        &self.shell.settings_section
    }

    /// `visitedRepositories`: los repositorios que ya se mostraron alguna vez
    /// y cuyo espacio de trabajo sigue montado.
    pub fn visited_repositories(&self) -> impl Iterator<Item = &OpenRepository> {
        self.repositories
            .repositories
            .iter()
            .filter(|repository| self.shell.visited_repository_ids.contains(&repository.id))
    }

    /// `showHomeTab`.
    pub fn show_home_tab(&mut self, cx: &mut Context<Self>) {
        self.shell.show_preferences = false;
        self.set_active_repository(None, cx);
        cx.notify();
    }

    /// `selectRepositoryTab`.
    pub fn select_repository_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        self.shell.show_preferences = false;
        self.set_active_repository(Some(index), cx);
        cx.notify();
    }

    /// `closeSettingsTab`.
    pub fn close_settings_tab(&mut self, cx: &mut Context<Self>) {
        self.shell.settings_tab_open = false;
        self.shell.show_preferences = false;
        cx.notify();
    }

    /// `showPreferences = false`: el "Back" de Settings. Oculta los ajustes sin
    /// cerrar su pestaña.
    pub fn hide_preferences(&mut self, cx: &mut Context<Self>) {
        if self.shell.show_preferences {
            self.shell.show_preferences = false;
            cx.notify();
        }
    }

    /// `openSettings(section)`.
    pub fn open_settings(&mut self, section: &str, cx: &mut Context<Self>) {
        self.shell.settings_tab_open = true;
        self.shell.settings_section = section.to_string();
        self.shell.show_preferences = true;
        cx.emit(AppEvent::OpenSettings {
            section: section.to_string(),
        });
        cx.notify();
    }

    /// `handleChangeView`: muestra una de las vistas fijas del repositorio
    /// activo.
    pub fn handle_change_view(&mut self, view: WorkspaceSection, cx: &mut Context<Self>) {
        self.shell.show_preferences = false;
        self.shell.active_view = view;
        if let Some(path) = self.active_repository().map(|repo| repo.path.clone()) {
            self.show_workspace_home(&path, Some(view), cx);
        }
        cx.emit(AppEvent::ViewChanged { view });
        cx.notify();
    }

    /// `addAccountFromSettings`: el diálogo de cuenta se abre desde Settings y
    /// al terminar no debe saltar al selector de repositorios.
    pub fn add_account_from_settings(&mut self, cx: &mut Context<Self>) {
        self.shell.adding_account_for_settings = true;
        cx.emit(AppEvent::ShowDialog(DialogRequest::GitHubAccount));
    }

    /// `closeAccountDialog`.
    pub fn close_account_dialog(&mut self) {
        self.shell.adding_account_for_settings = false;
    }

    /// `handleAccountAdded`: tras añadir una cuenta desde el flujo de clonar,
    /// se pasa al selector de repositorios.
    pub fn handle_account_added(&mut self, cx: &mut Context<Self>) {
        if self.shell.adding_account_for_settings {
            self.shell.adding_account_for_settings = false;
            return;
        }
        // Mostrar el selector de repositorios después de agregar la cuenta
        cx.emit(AppEvent::ShowDialog(DialogRequest::GitHubRepositoryPicker));
    }

    /// Los atajos globales de `App.vue` y el de `RepositoryWorkspace.vue`. La
    /// cáscara resuelve la pulsación con
    /// `mergeon_model::keyboard_shortcuts::resolve_shortcut` y pasa aquí la
    /// acción.
    pub fn handle_shortcut(&mut self, action: ShortcutAction, cx: &mut Context<Self>) {
        match action {
            ShortcutAction::OpenRepository => self.show_repository_options(cx),
            ShortcutAction::CloneFromGitHub => self.open_github_picker(cx).detach(),
            ShortcutAction::CloneFromUrl => self.show_clone_from_url(cx),
            ShortcutAction::Fetch => self.handle_fetch(cx),
            ShortcutAction::Pull => self.handle_pull(cx),
            ShortcutAction::Push => self.handle_push(cx),
            ShortcutAction::NewBranch => self.handle_new_branch(cx).detach(),
            ShortcutAction::ReloadApplication => self.request_reload_application(cx).detach(),
            ShortcutAction::Quit => self.request_quit(cx),
            ShortcutAction::MinimizeWindow => self.request_minimize(cx),
            ShortcutAction::ToggleDevTools => {
                cx.emit(AppEvent::Window(WindowRequest::ToggleDevTools));
            }
            ShortcutAction::ShowCommitsView => {
                self.handle_change_view(WorkspaceSection::Commits, cx);
            }
            ShortcutAction::ShowChangesView => {
                self.handle_change_view(WorkspaceSection::Changes, cx);
            }
            ShortcutAction::SearchCommits => {
                self.handle_change_view(WorkspaceSection::Commits, cx);
                cx.emit(AppEvent::FocusCommitSearch);
            }
            ShortcutAction::ShowOperationsLog => {
                cx.emit(AppEvent::ShowDialog(DialogRequest::OperationsLog));
            }
            ShortcutAction::ShowReflog => cx.emit(AppEvent::ShowDialog(DialogRequest::Reflog)),
            ShortcutAction::OpenInEditor => self.open_active_in_editor(cx).detach(),
            ShortcutAction::OpenPreferences => self.open_settings("appearance", cx),
            ShortcutAction::OpenShortcutsHelp => self.open_settings("shortcuts", cx),
            ShortcutAction::CloseTab => self.close_current_tab(cx),
            ShortcutAction::CloseRepository => self.close_active_repository(cx),
            ShortcutAction::ReopenClosedRepository => {
                self.reopen_closed_repository(cx).detach();
            }
            ShortcutAction::NextRepositoryTab => self.cycle_repository_tab(true, cx),
            ShortcutAction::PreviousRepositoryTab => self.cycle_repository_tab(false, cx),
            ShortcutAction::NextDocumentTab => {
                self.cycle_document_tab(true, cx);
            }
            ShortcutAction::PreviousDocumentTab => {
                self.cycle_document_tab(false, cx);
            }
            ShortcutAction::SelectRepositoryTab(number) => {
                self.select_repository_tab_number(number, cx);
            }
            // La paleta es un diálogo de la cáscara (`crates/mergeon`), que
            // atiende el atajo antes de llegar aquí.
            ShortcutAction::OpenCommandPalette => {}
            ShortcutAction::Undo => self.undo(cx).detach(),
            ShortcutAction::Redo => self.redo(cx).detach(),
        }
    }

    /// `window.api.app.minimize()`: lo ejecuta la cáscara al recibir el evento.
    pub fn request_minimize(&mut self, cx: &mut Context<Self>) {
        cx.emit(AppEvent::Window(WindowRequest::Minimize));
    }

    /// `window.api.app.quit()`: lo ejecuta la cáscara al recibir el evento.
    pub fn request_quit(&mut self, cx: &mut Context<Self>) {
        cx.emit(AppEvent::Window(WindowRequest::Quit));
    }

    // ------------------------------------------------------------------
    // Repositorio activo
    // ------------------------------------------------------------------

    /// Los dos `watch(() => activeRepository?.id)` de `App.vue`: recuerda el
    /// repositorio como visitado, adopta su vista activa y, salvo mientras se
    /// restaura la sesión, lo hidrata y pasa a vigilarlo.
    pub(crate) fn after_active_repository_change(
        &mut self,
        previous_id: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let active = self
            .active_repository()
            .map(|repository| (repository.id.clone(), repository.path.clone()));
        let new_id = active.as_ref().map(|(id, _)| id.clone());
        if new_id == previous_id {
            return;
        }

        // El que deja de verse se queda sin watcher: se anota cómo estaban sus
        // refs para saber al volver si alguien lo cambió.
        if !self.shell.restoring_workspace
            && let Some(previous_path) = previous_id
                .as_deref()
                .and_then(|id| self.repository(id))
                .map(|repository| repository.path.clone())
        {
            self.remember_hidden_repository_refs(previous_path, cx);
        }

        if let Some((id, path)) = &active {
            self.shell.visited_repository_ids.insert(id.clone());
            self.shell.active_view = self.repository_workspace(path).section;
        }
        cx.emit(AppEvent::ActiveRepositoryChanged {
            repo_id: new_id.clone(),
        });
        cx.notify();

        if self.shell.restoring_workspace {
            return;
        }

        let Some((_, path)) = active else {
            self.stop_watching_repository_status(cx).detach();
            return;
        };

        flow(cx, async move |this, cx| {
            let refs_changed = this.update(cx, |this, cx| {
                this.hidden_repository_refs_changed(&path, cx)
            });
            this.update(cx, |this, cx| this.hydrate_opened_repository(cx))
                .await;
            // `hydrate-opened-repository` no recarga el historial (no es una
            // mutación); si las refs se movieron mientras estaba oculto, sí
            // hay que hacerlo, como haría el watcher con la pestaña a la vista.
            if refs_changed.await {
                this.update(cx, |this, cx| {
                    let repo_id = this
                        .open_repositories()
                        .iter()
                        .find(|repository| repository.path == path)
                        .map(|repository| repository.id.clone());
                    if let Some(repo_id) = repo_id {
                        this.record_mutation(repo_id.as_str(), Some("external-head-change"), cx);
                        this.reconcile_undo_after_external_head_change(&path, cx);
                    }
                });
            }
            // Mientras se hidrataba pudo cambiar otra vez la pestaña activa.
            let still_active = this.read_with(cx, |this, _| {
                this.active_repository()
                    .is_some_and(|repository| repository.path == path)
            });
            if still_active {
                this.update(cx, |this, cx| {
                    this.watch_active_repository_status(Some(path), cx)
                })
                .await;
            }
        })
        .detach();
    }

    /// `hydrateOpenedRepository`: primera lectura del repositorio activo.
    pub fn hydrate_opened_repository(&mut self, cx: &mut Context<Self>) -> Task<()> {
        let Some(repo_id) = self.active_repository().map(|repo| repo.id.clone()) else {
            return Task::ready(());
        };

        let sync = self.sync_repository_read_model(
            repo_id.as_str(),
            SyncRequest::new()
                .status(true)
                .branches(true)
                .remotes(true)
                .changes_count(true)
                .starred_branches(true)
                .operation("hydrate-opened-repository"),
            cx,
        );
        cx.foreground_executor().spawn(async move {
            sync.await;
        })
    }

    /// `restoreWorkspaceSession`: reabre los repositorios que estaban abiertos
    /// al cerrar, sin reescribir los recientes, y deja activo el que lo estaba.
    pub fn restore_workspace_session(&mut self, cx: &mut Context<Self>) -> Task<()> {
        let session = &self.repositories.workspace_session;
        let mut open_repository_paths = session.open_repositories.clone();
        let preferred_active_path = session.active_repository_path.clone();
        let active_workspace_id = session.active_workspace_id.clone();

        if let Some(ws_id) = active_workspace_id.as_deref()
            && let Some(workspace) = self.repositories.workspace(ws_id)
        {
            for path in &workspace.repositories {
                let normalized = normalize_repository_path(path);
                if !open_repository_paths
                    .iter()
                    .any(|existing| normalize_repository_path(existing) == normalized)
                {
                    open_repository_paths.push(normalized);
                }
            }
        }

        if open_repository_paths.is_empty() {
            return Task::ready(());
        }

        self.shell.restoring_workspace = true;

        flow(cx, async move |this, cx| {
            // Cada inspección lanza git: todas a la vez, no una tras otra. Se
            // abren después en el orden de la sesión.
            let inspections = this.update(cx, |this, cx| {
                open_repository_paths
                    .iter()
                    .map(|repo_path| this.inspect_repository(repo_path, cx))
                    .collect::<Vec<_>>()
            });
            let inspections = futures::future::join_all(inspections).await;

            // Se abren todos en la misma vuelta: la cáscara pinta las pestañas
            // de una vez.
            let added = this.update(cx, |this, cx| {
                open_repository_paths
                    .iter()
                    .zip(inspections)
                    .filter(|(_, inspection)| inspection.valid)
                    .map(|(repo_path, inspection)| {
                        let canonical_path = inspection
                            .canonical_path
                            .or(inspection.path)
                            .filter(|path| !path.is_empty())
                            .unwrap_or_else(|| repo_path.clone());
                        if let Some(ws_id) = active_workspace_id.as_deref() {
                            this.repositories.replace_workspace_repository_path(
                                ws_id,
                                repo_path,
                                &canonical_path,
                            );
                        }
                        let added = this.add_repository(
                            &canonical_path,
                            AddRepository {
                                skip_recent_repository_update: true,
                                skip_active_workspace_assignment: true,
                                ..Default::default()
                            },
                            cx,
                        );
                        (repo_path.clone(), canonical_path, added)
                    })
                    .collect::<Vec<_>>()
            });
            let mut restored_paths: Vec<String> = Vec::new();
            for (repo_path, canonical_path, added) in added {
                match added.await {
                    Ok(()) => restored_paths.push(normalize_repository_path(&canonical_path)),
                    Err(error) => {
                        log::warn!("Failed to restore repository {repo_path}: {error}");
                    }
                }
            }

            let active = this.update(cx, |this, cx| {
                let next_active_path = if let Some(ws_id) = active_workspace_id.as_deref()
                    && let Some(workspace) = this.repositories.workspace(ws_id)
                {
                    preferred_active_path
                        .as_deref()
                        .map(normalize_repository_path)
                        .filter(|path| {
                            workspace.repositories.contains(path) && restored_paths.contains(path)
                        })
                        .or_else(|| {
                            workspace
                                .active_repository_path
                                .as_deref()
                                .map(normalize_repository_path)
                                .filter(|path| restored_paths.contains(path))
                        })
                        .or_else(|| {
                            workspace
                                .repositories
                                .iter()
                                .find(|path| restored_paths.contains(*path))
                                .cloned()
                        })
                } else {
                    preferred_active_path
                        .as_deref()
                        .map(normalize_repository_path)
                        .filter(|path| restored_paths.contains(path))
                        .or_else(|| restored_paths.first().cloned())
                };

                let active_index = next_active_path.as_deref().and_then(|next_active_path| {
                    this.repositories
                        .repositories
                        .iter()
                        .position(|repository| repository.path == next_active_path)
                });
                this.set_active_repository(active_index, cx);
                this.shell.restoring_workspace = false;
                this.active_repository().map(|repo| repo.path.clone())
            });
            crate::startup::trace("tabs restored");

            if let Some(path) = active {
                this.update(cx, |this, cx| this.hydrate_opened_repository(cx))
                    .await;
                crate::startup::trace("active repository hydrated");
                this.update(cx, |this, cx| {
                    this.watch_active_repository_status(Some(path), cx)
                })
                .await;
            }
        })
    }

    // ------------------------------------------------------------------
    // Watcher de estado
    // ------------------------------------------------------------------

    /// `clearQueuedStatusRefresh`.
    fn clear_queued_status_refresh(&mut self) {
        self.shell.status_refresh_timer = None;
    }

    /// `syncActiveRepositoryStatus`: relee el estado del repositorio activo (y
    /// sus ramas si pudo moverse HEAD). Si ya hay una relectura en curso, el
    /// motivo se funde con los pendientes y se atiende al terminar.
    pub fn sync_active_repository_status(
        &mut self,
        reason: StatusSyncReason,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        let Some((repo_id, repo_path)) = self
            .active_repository()
            .map(|repo| (repo.id.clone(), repo.path.clone()))
        else {
            return Task::ready(());
        };

        if self.shell.status_watch_sync_in_flight {
            self.shell.queued_status_refresh_reason = Some(merge_status_watch_reason(
                self.shell.queued_status_refresh_reason,
                reason,
            ));
            return Task::ready(());
        }

        self.shell.status_watch_sync_in_flight = true;
        let should_refresh_branches = is_head_status_watch_reason(reason);

        let sync = self.sync_repository_read_model(
            repo_id.as_str(),
            SyncRequest::new()
                .status(true)
                .changes_count(true)
                .branches(should_refresh_branches)
                .operation(if should_refresh_branches {
                    "external-head-change"
                } else {
                    "status-watch"
                }),
            cx,
        );

        flow(cx, async move |this, cx| {
            sync.await;
            this.update(cx, |this, cx| {
                this.shell.status_watch_sync_in_flight = false;
                if should_refresh_branches {
                    this.reconcile_undo_after_external_head_change(&repo_path, cx);
                }

                if let Some(next_reason) = this.shell.queued_status_refresh_reason.take() {
                    this.queue_active_repository_status_sync(next_reason, cx);
                }
            });
        })
    }

    /// `queueActiveRepositoryStatusSync`: agrupa una ráfaga de avisos en una
    /// sola relectura, 250 ms después del último.
    pub fn queue_active_repository_status_sync(
        &mut self,
        reason: StatusSyncReason,
        cx: &mut Context<Self>,
    ) {
        self.shell.queued_status_refresh_reason = Some(merge_status_watch_reason(
            self.shell.queued_status_refresh_reason,
            reason,
        ));
        self.clear_queued_status_refresh();

        let timer = cx.background_executor().timer(STATUS_REFRESH_DELAY);
        self.shell.status_refresh_timer = Some(cx.spawn(async move |this, cx| {
            timer.await;
            this.update(cx, |this, cx| {
                // Este temporizador ya venció: se suelta sin cancelarlo, que
                // es él mismo quien está corriendo.
                if let Some(timer) = this.shell.status_refresh_timer.take() {
                    timer.detach();
                }
                let next_reason = this
                    .shell
                    .queued_status_refresh_reason
                    .take()
                    .unwrap_or(reason);
                this.sync_active_repository_status(next_reason, cx).detach();
            })
            .ok();
        }));
    }

    /// `watchActiveRepositoryStatus`: pasa a vigilar `repo_path` (o deja de
    /// vigilar, con `None`) y trae la cola de operaciones de ese repositorio.
    pub fn watch_active_repository_status(
        &mut self,
        repo_path: Option<String>,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        let Some(normalized_path) = repo_path.filter(|path| !path.is_empty()) else {
            if self.shell.active_status_watch_path.is_none() {
                return Task::ready(());
            }
            let unwatch = self.blocking(cx, |backend| {
                backend.git().unwatch_status();
            });
            return flow(cx, async move |this, cx| {
                unwatch.await;
                this.update(cx, |this, cx| {
                    this.shell.active_status_watch_path = None;
                    cx.notify();
                });
            });
        };

        if self.shell.active_status_watch_path.as_deref() == Some(normalized_path.as_str()) {
            return Task::ready(());
        }

        let watch = self.blocking(cx, {
            let path = normalized_path.clone();
            move |backend| {
                backend.git().unwatch_status();
                backend.git().watch_status(&path);
                backend.git().get_operation_queue(&path)
            }
        });
        let window_focused = self.shell.window_focused;

        flow(cx, async move |this, cx| {
            let queue = watch.await;
            let focus = this.update(cx, |this, cx| {
                let active = this
                    .active_repository()
                    .filter(|repository| repository.path == normalized_path)
                    .map(|repository| repository.id.clone());
                if let Some(repo_id) = active {
                    this.set_repository_mutation_queue(
                        repo_id.as_str(),
                        &queue_snapshot(&queue),
                        cx,
                    );
                }
                this.blocking(cx, move |backend| {
                    backend.git().set_status_watch_window_focus(window_focused);
                })
            });
            focus.await;
            this.update(cx, |this, cx| {
                this.shell.active_status_watch_path = Some(normalized_path);
                cx.notify();
            });
        })
    }

    /// `stopWatchingRepositoryStatus`.
    pub fn stop_watching_repository_status(&mut self, cx: &mut Context<Self>) -> Task<()> {
        self.clear_queued_status_refresh();
        self.shell.queued_status_refresh_reason = None;
        self.shell.status_watch_sync_in_flight = false;
        self.shell.active_status_watch_path = None;
        cx.notify();
        self.blocking(cx, |backend| {
            backend.git().unwatch_status();
        })
    }

    /// Se están reabriendo los repositorios de la sesión anterior: los que
    /// pasan por activos mientras tanto no deben montar sus vistas.
    pub fn is_restoring_workspace(&self) -> bool {
        self.shell.restoring_workspace
    }

    /// Ruta que el watcher vigila ahora mismo (`activeStatusWatchPath`).
    pub fn active_status_watch_path(&self) -> Option<&str> {
        self.shell.active_status_watch_path.as_deref()
    }

    /// `handleStatusChanged`: el watcher vio cambios en el repositorio activo.
    pub fn handle_status_changed(&mut self, payload: &StatusChange, cx: &mut Context<Self>) {
        let is_active = self
            .active_repository()
            .is_some_and(|repository| same_repository_path(&repository.path, &payload.repo_path));
        if !is_active {
            return;
        }

        match StatusSyncReason::from_watch_reason(payload.reason) {
            Some(reason) => self.queue_active_repository_status_sync(reason, cx),
            None => {
                self.queue_metadata_refresh(MetadataRefresh::from_watch_reason(payload.reason), cx)
            }
        }
    }

    /// Agrupa los avisos de metadatos en una sola relectura, 250 ms después
    /// del último (como los de estado).
    pub fn queue_metadata_refresh(&mut self, refresh: MetadataRefresh, cx: &mut Context<Self>) {
        if refresh.is_empty() {
            return;
        }
        self.shell.queued_metadata_refresh = self.shell.queued_metadata_refresh.merge(refresh);
        let timer = cx.background_executor().timer(STATUS_REFRESH_DELAY);
        self.shell.metadata_refresh_timer = Some(cx.spawn(async move |this, cx| {
            timer.await;
            this.update(cx, |this, cx| {
                if let Some(timer) = this.shell.metadata_refresh_timer.take() {
                    timer.detach();
                }
                let refresh = std::mem::take(&mut this.shell.queued_metadata_refresh);
                this.refresh_active_repository_metadata(refresh, cx)
                    .detach();
            })
            .ok();
        }));
    }

    /// Relee remotos y tags del repositorio activo tras un cambio hecho
    /// fuera de la app (`git remote add`, `git tag`, `git fetch` en la
    /// terminal). Si cambió la lista de remotos, la sección de pull requests
    /// vuelve a mirar si hay uno de GitHub.
    pub fn refresh_active_repository_metadata(
        &mut self,
        refresh: MetadataRefresh,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        let Some(repository) = self.active_repository() else {
            return Task::ready(());
        };
        if refresh.is_empty() {
            return Task::ready(());
        }
        let repo_id = repository.id.clone();
        let remotes_before: Vec<(String, String)> = repository
            .remotes
            .iter()
            .map(|remote| (remote.name.clone(), remote.refs.fetch.clone()))
            .collect();
        let sync = self.sync_repository_read_model(repo_id.as_str(), refresh.sync_request(), cx);
        flow(cx, async move |this, cx| {
            let Some(outcome) = sync.await else {
                return;
            };
            let Some(remotes) = outcome.remotes else {
                return;
            };
            let mut remotes_after: Vec<(String, String)> = remotes
                .iter()
                .map(|remote| (remote.name.clone(), remote.refs.fetch.clone()))
                .collect();
            remotes_after.sort();
            let mut remotes_before = remotes_before;
            remotes_before.sort();
            if remotes_after != remotes_before {
                this.update(cx, |this, cx| {
                    this.refresh_pull_requests(&repo_id, true, cx).detach();
                });
            }
        })
    }

    /// `handleOperationEvent`: una operación de la cola cambió de fase.
    pub fn handle_operation_event(&mut self, payload: &OperationEvent, cx: &mut Context<Self>) {
        self.attach_progress_operation(payload, cx);
        let event = operation_event(payload);
        // El log ve todas las fases (queued/running/terminales) con sus timestamps.
        self.operations_log.record_event(&event);
        cx.notify();

        let Some(repo_id) = self
            .repositories
            .repositories
            .iter()
            .find(|repository| same_repository_path(&repository.path, &payload.repo_path))
            .map(|repository| repository.id.clone())
        else {
            return;
        };

        self.apply_repository_operation_event(repo_id.as_str(), &event.operations, cx);

        if matches!(
            payload.phase,
            OperationPhase::Cancelled | OperationPhase::Timeout
        ) && self
            .active_repository()
            .is_some_and(|repository| repository.id == repo_id)
        {
            self.queue_active_repository_status_sync(StatusSyncReason::GitOperationControl, cx);
        }
    }

    // ------------------------------------------------------------------
    // Foco de la ventana y auto-fetch
    // ------------------------------------------------------------------

    /// La ventana ganó o perdió el foco (los `focus` / `blur` de `window`).
    /// Sin foco el watcher deja de sondear; al volver, si el auto-fetch está
    /// vencido, se pone al día.
    pub fn set_window_focused(&mut self, focused: bool, cx: &mut Context<Self>) {
        self.shell.window_focused = focused;
        self.blocking(cx, move |backend| {
            backend.git().set_status_watch_window_focus(focused);
        })
        .detach();

        if !focused {
            return;
        }

        // Al volver a la app: si el auto-fetch está vencido, ponerse al día ya.
        let interval =
            Duration::from_secs(u64::from(self.settings.auto_fetch_interval_minutes) * 60);
        let now = cx.background_executor().now();
        let overdue = self
            .shell
            .last_auto_fetch_at
            .is_none_or(|last| now.duration_since(last) > interval);
        if self.settings.auto_fetch_enabled && overdue {
            self.run_auto_fetch(cx).detach();
        }
    }

    /// La ventana tiene el foco (`document.hasFocus()`).
    pub fn window_focused(&self) -> bool {
        self.shell.window_focused
    }

    /// `runAutoFetch`: fetch silencioso del remoto por defecto. Solo con la
    /// ventana enfocada y repositorio activo; el single-flight de fetch
    /// absorbe cualquier solape con un fetch manual.
    pub fn run_auto_fetch(&mut self, cx: &mut Context<Self>) -> Task<()> {
        if !self.settings.auto_fetch_enabled || !self.shell.window_focused {
            return Task::ready(());
        }
        if self.active_repository().is_none() {
            return Task::ready(());
        }

        self.shell.last_auto_fetch_at = Some(cx.background_executor().now());
        let remote = self.default_remote_name();
        self.run_fetch(
            FetchRequest {
                remote,
                all_remotes: false,
            },
            true,
            cx,
        )
    }

    /// `scheduleAutoFetch`: (re)programa el auto-fetch periódico según los
    /// ajustes.
    pub fn schedule_auto_fetch(&mut self, cx: &mut Context<Self>) {
        self.shell.auto_fetch_timer = None;
        if !self.settings.auto_fetch_enabled {
            return;
        }

        let interval =
            Duration::from_secs(u64::from(self.settings.auto_fetch_interval_minutes.max(1)) * 60);
        self.shell.auto_fetch_timer = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(interval).await;
                let alive = this
                    .update(cx, |this, cx| this.run_auto_fetch(cx).detach())
                    .is_ok();
                if !alive {
                    break;
                }
            }
        }));
    }

    // ------------------------------------------------------------------
    // Abrir repositorios
    // ------------------------------------------------------------------

    /// `handleNonGitRepositoryOpen`: ofrece inicializar git en un directorio
    /// que no es un repositorio y, si se acepta, lo abre.
    pub fn handle_non_git_repository_open(
        &mut self,
        repo_path: &str,
        cx: &mut Context<Self>,
    ) -> Task<anyhow::Result<bool>> {
        let repo_path = repo_path.to_string();
        flow(cx, async move |this, cx| {
            let confirmed = this
                .update(cx, |this, cx| {
                    this.confirm(
                        ConfirmOptions {
                            title: Some("Directory is not under Git source control".to_string()),
                            description: Some(format!(
                                "The '{repo_path}' directory is not a Git repository."
                            )),
                            confirm_text: Some("Initialize git repository here".to_string()),
                            cancel_text: Some("Close".to_string()),
                            ..Default::default()
                        },
                        cx,
                    )
                })
                .await;

            if !confirmed {
                return Ok(true);
            }

            let result = this
                .update(cx, |this, cx| this.init_repository(&repo_path, cx))
                .await;
            let is_noop = result.outcome() == Some(mergeon_backend::Outcome::Noop);
            if !result.success() && !is_noop {
                this.update(cx, |this, cx| {
                    let message = result.message();
                    this.notify_error(
                        if message.is_empty() {
                            "Failed to initialize Git repository"
                        } else {
                            message
                        },
                        cx,
                    );
                });
                return Ok(true);
            }

            this.update(cx, |this, cx| {
                this.add_repository(&repo_path, AddRepository::default(), cx)
            })
            .await?;
            this.update(cx, |this, cx| {
                this.notify_success(
                    if is_noop {
                        "Repository opened successfully"
                    } else {
                        "Git repository initialized and opened successfully"
                    },
                    cx,
                );
            });
            Ok(true)
        })
    }

    /// `openRepositoryPath`: inspecciona la ruta y, si es un repositorio, lo
    /// abre. Devuelve `true` si quedó abierto por esta vía.
    pub fn open_repository_path(
        &mut self,
        repo_path: &str,
        invalid_fallback_message: &str,
        cx: &mut Context<Self>,
    ) -> Task<anyhow::Result<bool>> {
        let repo_path = repo_path.to_string();
        let invalid_fallback_message = invalid_fallback_message.to_string();
        flow(cx, async move |this, cx| {
            let inspection = this
                .update(cx, |this, cx| this.inspect_repository(&repo_path, cx))
                .await;

            if !inspection.valid {
                if inspection.kind == RepositoryKind::NotGitRepository {
                    this.update(cx, |this, cx| {
                        this.handle_non_git_repository_open(&repo_path, cx)
                    })
                    .await?;
                    return Ok(false);
                }

                this.update(cx, |this, cx| {
                    this.notify_error(
                        if inspection.message.is_empty() {
                            &invalid_fallback_message
                        } else {
                            &inspection.message
                        },
                        cx,
                    );
                });
                return Ok(false);
            }

            let path = inspection
                .canonical_path
                .or(inspection.path)
                .filter(|path| !path.is_empty())
                .unwrap_or(repo_path);
            this.update(cx, |this, cx| {
                this.add_repository(&path, AddRepository::default(), cx)
            })
            .await?;
            this.update(cx, |this, cx| {
                this.notify_success("Repository opened successfully", cx);
            });
            Ok(true)
        })
    }

    /// `showRepositoryOptions`: mostrar opciones para abrir repositorio.
    pub fn show_repository_options(&mut self, cx: &mut Context<Self>) {
        cx.emit(AppEvent::ShowDialog(DialogRequest::Launcher));
    }

    /// `openLocalRepository`: elige un directorio y lo abre.
    pub fn open_local_repository(&mut self, cx: &mut Context<Self>) -> Task<()> {
        self.run_single_flight("open-local-repository", cx, |_, cx| {
            flow(cx, async move |this, cx| {
                let Some(repo_path) = this.update(cx, |this, cx| this.pick_directory(cx)).await
                else {
                    return;
                };
                let repo_path = repo_path.to_string_lossy().into_owned();

                let opened = this
                    .update(cx, |this, cx| {
                        this.open_repository_path(
                            &repo_path,
                            "The selected directory is not a valid Git repository",
                            cx,
                        )
                    })
                    .await;
                if let Err(error) = opened {
                    this.update(cx, |this, cx| {
                        this.notify_error_with(
                            &format!("Failed to open repository: {error}"),
                            &error,
                            cx,
                        );
                    });
                }
            })
        })
    }

    /// `openGitHubPicker`: abre el selector de repositorios de GitHub o, si no
    /// hay cuentas, ofrece añadir una primero.
    pub fn open_github_picker(&mut self, cx: &mut Context<Self>) -> Task<()> {
        if self.has_github_accounts() {
            cx.emit(AppEvent::ShowDialog(DialogRequest::GitHubRepositoryPicker));
            return Task::ready(());
        }

        // Si no hay cuentas, mostrar diálogo de agregar cuenta primero
        let confirm = self.confirm(
            ConfirmOptions {
                title: Some("No GitHub accounts".to_string()),
                description: Some(
                    "You need to add a GitHub account first to clone repositories.".to_string(),
                ),
                confirm_text: Some("Add Account".to_string()),
                cancel_text: Some("Cancel".to_string()),
                ..Default::default()
            },
            cx,
        );
        flow(cx, async move |this, cx| {
            if confirm.await {
                this.update(cx, |_, cx| {
                    cx.emit(AppEvent::ShowDialog(DialogRequest::GitHubAccount));
                });
            }
        })
    }

    /// `handleRepositoryCloned`: abre el repositorio recién clonado desde
    /// GitHub.
    pub fn handle_repository_cloned(
        &mut self,
        repo_path: &str,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        self.handle_repository_cloned_from(repo_path, true, cx)
    }

    /// Abre un repositorio recién clonado; `is_github` dice si su remoto es
    /// de github.com (lo que guarda la lista de recientes).
    pub fn handle_repository_cloned_from(
        &mut self,
        repo_path: &str,
        is_github: bool,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        let repo_path = repo_path.to_string();
        self.run_single_flight(
            format!("open-cloned-repository:{repo_path}"),
            cx,
            |_, cx| {
                flow(cx, async move |this, cx| {
                    let is_valid = this
                        .update(cx, |this, cx| this.is_valid_repository(&repo_path, cx))
                        .await;

                    if !is_valid {
                        this.update(cx, |this, cx| {
                            this.notify_error(
                                "The cloned directory is not a valid Git repository",
                                cx,
                            );
                        });
                        return;
                    }

                    let added = this
                        .update(cx, |this, cx| {
                            this.add_repository(
                                &repo_path,
                                AddRepository {
                                    is_github,
                                    ..Default::default()
                                },
                                cx,
                            )
                        })
                        .await;
                    if let Err(error) = added {
                        this.update(cx, |this, cx| {
                            this.notify_error_with(
                                &format!("Failed to open cloned repository: {error}"),
                                &error,
                                cx,
                            );
                        });
                    }
                })
            },
        )
    }

    /// `handleOpenRepository`: abre un repositorio desde el inicio (recientes).
    pub fn handle_open_repository(&mut self, repo_path: &str, cx: &mut Context<Self>) -> Task<()> {
        let repo_path = repo_path.to_string();
        self.run_single_flight(
            format!("open-dashboard-repository:{repo_path}"),
            cx,
            |_, cx| {
                flow(cx, async move |this, cx| {
                    let opened = this
                        .update(cx, |this, cx| {
                            this.open_repository_path(
                                &repo_path,
                                "The selected directory is not a valid Git repository",
                                cx,
                            )
                        })
                        .await;
                    if let Err(error) = opened {
                        this.update(cx, |this, cx| {
                            this.notify_error_with(
                                &format!("Failed to open repository: {error}"),
                                &error,
                                cx,
                            );
                        });
                    }
                })
            },
        )
    }

    // ------------------------------------------------------------------
    // Operación en curso
    // ------------------------------------------------------------------

    /// `handleSequenceAction`: continúa, aborta o salta la operación en curso
    /// del repositorio activo. Abortar pide confirmación.
    pub fn handle_sequence_action(&mut self, action: &str, cx: &mut Context<Self>) -> Task<()> {
        let Some(repository) = self.active_repository() else {
            return Task::ready(());
        };
        let repo_id = repository.id.clone();
        let repo_path = repository.path.clone();
        let operation_type = repository
            .operation_state
            .as_ref()
            .and_then(|state| state.kind)
            .map_or("Git", |kind| kind.as_str());
        let action = action.to_string();

        let confirmation = (action == "abort").then(|| {
            self.confirm(
                ConfirmOptions {
                    title: Some("Abort operation".to_string()),
                    description: Some(format!("Abort the current {operation_type} operation?")),
                    confirm_text: Some("Abort".to_string()),
                    cancel_text: Some("Cancel".to_string()),
                    variant: Some("destructive".to_string()),
                    ..Default::default()
                },
                cx,
            )
        });

        flow(cx, async move |this, cx| {
            if let Some(confirmation) = confirmation
                && !confirmation.await
            {
                return;
            }

            this.update(cx, |this, cx| {
                let operation = format!("sequence-action:{action}");
                let single_flight_repo_id = repo_id.clone();
                this.run_repo_operation_single_flight(
                    &single_flight_repo_id,
                    &operation,
                    cx,
                    move |this, cx| {
                        this.shell.busy_sequence_action = Some(action.clone());
                        cx.notify();
                        flow(cx, async move |this, cx| {
                            let result = this
                                .update(cx, |this, cx| {
                                    this.sequence_action(&repo_path, &action, cx)
                                })
                                .await;
                            this.update(cx, |this, cx| {
                                this.apply_repository_operation_result(
                                    repo_id.as_str(),
                                    &result,
                                    cx,
                                );
                            });

                            if should_sync_after_sequence_action(&result) {
                                let summary = result.to_operation_result();
                                let request = SyncRequest::new()
                                    .status(summary.status.is_none())
                                    .branches(true)
                                    .changes_count(true)
                                    .operation_or(summary.operation.as_deref(), "sequence-action")
                                    .result(summary);
                                this.update(cx, |this, cx| {
                                    this.sync_repository_read_model(repo_id.as_str(), request, cx)
                                })
                                .await;
                            }

                            this.update(cx, |this, cx| {
                                this.shell.busy_sequence_action = None;
                                cx.notify();
                            });
                        })
                    },
                )
            })
            .await;
        })
    }

    /// `handleCancelOperation`: pide cancelar una operación de la cola.
    pub fn handle_cancel_operation(
        &mut self,
        operation_id: &str,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        let operation_id = operation_id.to_string();
        let cancel = self.blocking(cx, move |backend| {
            backend.git().cancel_operation(&operation_id)
        });
        flow(cx, async move |this, cx| {
            let result = cancel.await;
            if !result.ok {
                this.update(cx, |this, cx| {
                    this.notify_warning("This Git operation is no longer cancellable.", cx);
                });
            }
        })
    }

    // ------------------------------------------------------------------
    // Fetch / pull / push
    // ------------------------------------------------------------------

    /// Abre siempre el diálogo de fetch (p. ej. Shift+clic o Command Palette).
    pub fn open_fetch_dialog(&mut self, cx: &mut Context<Self>) {
        if self.active_repository().is_some() {
            cx.emit(AppEvent::ShowDialog(DialogRequest::Fetch));
        }
    }

    /// Abre siempre el diálogo de pull (p. ej. Shift+clic o Command Palette).
    pub fn open_pull_dialog(&mut self, cx: &mut Context<Self>) {
        if self.active_repository().is_some() {
            cx.emit(AppEvent::ShowDialog(DialogRequest::Pull));
        }
    }

    /// Abre siempre el diálogo de push (p. ej. Shift+clic o Command Palette).
    pub fn open_push_dialog(&mut self, cx: &mut Context<Self>) {
        if self.active_repository().is_some() {
            cx.emit(AppEvent::ShowDialog(DialogRequest::Push));
        }
    }

    /// Petición segura para ejecutar Fetch directamente (`all_remotes: true`
    /// siempre que el repositorio activo tenga al menos un remoto).
    pub(crate) fn direct_fetch_request(&self) -> Option<FetchRequest> {
        self.active_repository()?;
        if self.repositories.active_repository_remotes().is_empty() {
            return None;
        }
        Some(FetchRequest {
            remote: self.default_remote_name(),
            all_remotes: true,
        })
    }

    /// Petición segura para ejecutar Pull directamente: rama local (no HEAD
    /// desanclado), remoto por defecto válido y rama remota destino (upstream
    /// configurado o rama homónima en ese remoto).
    pub(crate) fn direct_pull_request(&self) -> Option<PullRequest> {
        self.active_repository()?;
        let current = self.active_repo_current_branch();
        if current.is_empty() || current == "HEAD" {
            return None;
        }
        let remote = self.default_remote_name();
        let remote_entry = self
            .repositories
            .active_repository_remotes()
            .iter()
            .find(|entry| entry.name == remote)?;
        let upstream_branch = self
            .active_branch_entry()
            .and_then(|entry| entry.upstream.as_deref())
            .and_then(|upstream| upstream.split_once('/'))
            .filter(|(up_remote, up_branch)| *up_remote == remote && !up_branch.is_empty())
            .map(|(_, up_branch)| up_branch.to_string());
        let branch = upstream_branch.or_else(|| {
            remote_entry
                .branches
                .iter()
                .find(|branch| branch.as_str() == current)
                .cloned()
        })?;
        Some(PullRequest {
            remote,
            branch: Some(branch),
            rebase: false,
            autostash: false,
        })
    }

    /// Petición segura para ejecutar Push directamente: rama local (no HEAD
    /// desanclado) con upstream configurado. Nunca hace force push ni sube tags.
    pub(crate) fn direct_push_request(&self) -> Option<PushRequest> {
        self.active_repository()?;
        let branch = self.active_repo_current_branch();
        if branch.is_empty() || branch == "HEAD" {
            return None;
        }
        let upstream = self
            .active_branch_entry()
            .filter(|entry| !entry.is_remote)
            .and_then(|entry| entry.upstream.as_deref())
            .filter(|upstream| !upstream.is_empty())?
            .to_string();
        Some(PushRequest {
            branch: branch.to_string(),
            destination: upstream,
            push_tags: false,
            force: false,
        })
    }

    /// `handleFetch`: ejecuta fetch directamente si `skip_action_dialogs` está
    /// activo y existe al menos un remoto; si no, abre el diálogo de fetch.
    pub fn handle_fetch(&mut self, cx: &mut Context<Self>) {
        if self.active_repository().is_none() {
            return;
        }
        if self.settings.skip_action_dialogs
            && let Some(request) = self.direct_fetch_request()
        {
            self.run_fetch(request, false, cx).detach();
            return;
        }
        self.open_fetch_dialog(cx);
    }

    /// `handlePull`: ejecuta pull directamente si `skip_action_dialogs` está
    /// activo y hay un destino seguro; si no, abre el diálogo de pull.
    pub fn handle_pull(&mut self, cx: &mut Context<Self>) {
        if self.active_repository().is_none() {
            return;
        }
        if self.settings.skip_action_dialogs
            && let Some(request) = self.direct_pull_request()
        {
            self.handle_pull_submit(request, cx).detach();
            return;
        }
        self.open_pull_dialog(cx);
    }

    /// `handlePush`: ejecuta push directamente si `skip_action_dialogs` está
    /// activo y la rama actual tiene upstream; si no, abre el diálogo de push.
    pub fn handle_push(&mut self, cx: &mut Context<Self>) {
        if self.active_repository().is_none() {
            return;
        }
        if self.settings.skip_action_dialogs
            && let Some(request) = self.direct_push_request()
        {
            self.handle_push_submit(request, cx).detach();
            return;
        }
        self.open_push_dialog(cx);
    }

    /// `promptForOriginRemote`: pide la URL del remoto y lo añade como
    /// `origin`. Devuelve si quedó añadido.
    pub fn prompt_for_origin_remote(
        &mut self,
        repo_path: &str,
        cx: &mut Context<Self>,
    ) -> Task<bool> {
        let repo_path = repo_path.to_string();
        let prompt = self.prompt(
            PromptOptions {
                title: Some("Configure remote".to_string()),
                description: Some(
                    "No remote repository configured. Please enter the remote URL.".to_string(),
                ),
                confirm_text: Some("Add Remote".to_string()),
                cancel_text: Some("Cancel".to_string()),
                pattern: Some("^https?://.+".to_string()),
                input_error_message: Some("Please enter a valid URL (https://...)".to_string()),
                placeholder: Some("https://github.com/username/repository.git".to_string()),
                ..Default::default()
            },
            cx,
        );

        flow(cx, async move |this, cx| {
            let answer = prompt.await;
            let Some(remote_url) = answer
                .value
                .filter(|value| answer.confirmed && !value.is_empty())
            else {
                return false;
            };

            let clean_url = remote_url.trim();
            let clean_url = clean_url.strip_suffix('/').unwrap_or(clean_url).to_string();

            let result = this
                .update(cx, |this, cx| {
                    this.add_remote(&repo_path, "origin", &clean_url, false, cx)
                })
                .await;
            result.success()
        })
    }

    /// `runFetch`: fetch del repositorio activo y relectura de lo que pudo
    /// traer. Con `silent` (auto-fetch) no hay avisos, pero el resultado queda
    /// en el log de operaciones.
    pub fn run_fetch(
        &mut self,
        request: FetchRequest,
        silent: bool,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        let Some(repository) = self.active_repository() else {
            return Task::ready(());
        };
        let repo_id = repository.id.clone();
        let repo_name = repository.name.clone();
        let repo_path = repository.path.clone();

        self.run_repo_operation_single_flight(&repo_id.clone(), "fetch", cx, move |_, cx| {
            flow(cx, async move |this, cx| {
                let result = this
                    .update(cx, |this, cx| {
                        let fetch = {
                            let repo_path = repo_path.clone();
                            move |this: &mut AppModel, cx: &mut Context<AppModel>| {
                                let options = FetchOptions {
                                    remote: request.remote.clone(),
                                    all_remotes: request.all_remotes,
                                    auth_token: this.git_auth_token(),
                                };
                                this.fetch(&repo_path, options, silent, cx)
                            }
                        };
                        // El auto-fetch nunca abre el diálogo de credenciales
                        // (sí usa las que ya se tecleáramos).
                        if silent {
                            fetch(this, cx)
                        } else {
                            this.with_http_credentials("fetch", &repo_path, &repo_path, fetch, cx)
                        }
                    })
                    .await;
                let summary = result.to_operation_result();

                if silent {
                    // Sin toasts, pero el log de operaciones sí lo registra.
                    this.update(cx, |this, cx| {
                        this.operations_log.record_result(
                            &summary,
                            ResultContext {
                                repo_name: Some(&repo_name),
                                repo_path: Some(&repo_path),
                            },
                        );
                        cx.notify();
                    });
                }
                if !result.succeeded_or_noop() {
                    return;
                }
                /*
                 * Si ningún ref se movió no hay nada que releer ni que anunciar: marcar
                 * mutación aquí haría que la vista de commits reconstruyera el historial
                 * entero en cada fetch, incluido el auto-fetch periódico.
                 */
                if result.result().and_then(|result| result.refs_changed) == Some(false) {
                    return;
                }
                // El fetch puede traer ramas y tags nuevos del remoto: sin estas
                // secciones el sidebar no los muestra hasta un refresh manual.
                this.update(cx, |this, cx| {
                    this.sync_repository_read_model(
                        repo_id.as_str(),
                        SyncRequest::new()
                            .status(true)
                            .branches(true)
                            .remotes(true)
                            .tags(true)
                            .operation_or(summary.operation.as_deref(), "fetch"),
                        cx,
                    )
                })
                .await;
            })
        })
    }

    /// `handleFetchSubmit`.
    pub fn handle_fetch_submit(
        &mut self,
        request: FetchRequest,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        self.run_fetch(request, false, cx)
    }

    /// `handlePullSubmit`.
    pub fn handle_pull_submit(&mut self, request: PullRequest, cx: &mut Context<Self>) -> Task<()> {
        let Some(repository) = self.active_repository() else {
            return Task::ready(());
        };
        let repo_id = repository.id.clone();
        let repo_path = repository.path.clone();

        self.run_repo_operation_single_flight(&repo_id.clone(), "pull", cx, move |_, cx| {
            flow(cx, async move |this, cx| {
                let result = this
                    .update(cx, |this, cx| {
                        let pull = {
                            let repo_path = repo_path.clone();
                            let request = request.clone();
                            move |this: &mut AppModel, cx: &mut Context<AppModel>| {
                                let options = PullOptions {
                                    remote: Some(request.remote.clone()),
                                    branch: request.branch.clone(),
                                    auth_token: this.git_auth_token(),
                                    rebase: request.rebase,
                                    autostash: request.autostash,
                                };
                                this.pull(&repo_path, options, cx)
                            }
                        };
                        this.with_http_credentials("pull", &repo_path, &repo_path, pull, cx)
                    })
                    .await;

                /*
                 * Pull que no movió ningún ref: el árbol de trabajo sí pudo tocarse (p.
                 * ej. autostash), así que se relee el estado, pero sin marcar mutación
                 * para no forzar la reconstrucción del historial.
                 */
                let request =
                    if result.result().and_then(|result| result.refs_changed) == Some(false) {
                        SyncRequest::new().status(true)
                    } else {
                        // Pull hace fetch por debajo: mismas secciones que runFetch.
                        SyncRequest::new()
                            .status(true)
                            .branches(true)
                            .remotes(true)
                            .tags(true)
                            .operation("pull")
                    };
                this.update(cx, |this, cx| {
                    this.sync_repository_read_model(repo_id.as_str(), request, cx)
                })
                .await;
            })
        })
    }

    /// `handlePushSubmit`. Si el remoto no existe, pide su URL, lo añade como
    /// `origin` y reintenta.
    pub fn handle_push_submit(&mut self, request: PushRequest, cx: &mut Context<Self>) -> Task<()> {
        let Some(repository) = self.active_repository() else {
            return Task::ready(());
        };
        if request.destination.is_empty() {
            return Task::ready(());
        }
        let repo_id = repository.id.clone();
        let repo_path = repository.path.clone();

        let (remote, target_branch) = match request.destination.split_once('/') {
            Some((remote, branch)) if !branch.is_empty() => {
                (remote.to_string(), branch.to_string())
            }
            Some((remote, _)) => (remote.to_string(), request.branch.clone()),
            None => (request.destination.clone(), request.branch.clone()),
        };

        self.run_repo_operation_single_flight(&repo_id.clone(), "push", cx, move |_, cx| {
            flow(cx, async move |this, cx| {
                let mut retried = false;
                let result = loop {
                    let result = this
                        .update(cx, |this, cx| {
                            let push = {
                                let repo_path = repo_path.clone();
                                let remote = remote.clone();
                                let target_branch = target_branch.clone();
                                let request = request.clone();
                                move |this: &mut AppModel, cx: &mut Context<AppModel>| {
                                    let options = PushOptions {
                                        remote: remote.clone(),
                                        branch: Some(request.branch.clone()),
                                        auth_token: this.git_auth_token(),
                                        target_branch: Some(target_branch.clone()),
                                        force: request.force,
                                        push_tags: request.push_tags,
                                    };
                                    this.push(&repo_path, options, false, cx)
                                }
                            };
                            this.with_http_credentials("push", &repo_path, &repo_path, push, cx)
                        })
                        .await;

                    if result.code() != "REMOTE_NOT_FOUND" || retried {
                        break result;
                    }

                    let added = this
                        .update(cx, |this, cx| this.prompt_for_origin_remote(&repo_path, cx))
                        .await;
                    if !added {
                        return;
                    }
                    retried = true;
                };

                if result.succeeded_or_noop() {
                    // Un push de rama nueva cambia las refs bajo el remoto en el sidebar.
                    this.update(cx, |this, cx| {
                        this.sync_repository_read_model(
                            repo_id.as_str(),
                            SyncRequest::new()
                                .status(true)
                                .branches(true)
                                .remotes(true)
                                .changes_count(true)
                                .operation_or(result.operation(), "push"),
                            cx,
                        )
                    })
                    .await;
                }
            })
        })
    }

    // ------------------------------------------------------------------
    // Stash y nueva rama
    // ------------------------------------------------------------------

    /// `handleStash`: abre el diálogo de stash (mensaje y opciones) sobre
    /// todos los cambios locales. Lo que hace al enviarse está en
    /// [`AppModel::handle_stash_submit`].
    pub fn handle_stash(&mut self, cx: &mut Context<Self>) -> Task<()> {
        self.handle_stash_paths(Vec::new(), cx);
        Task::ready(())
    }

    /// "Stash selected files…": el diálogo de stash solo para esas rutas.
    pub fn handle_stash_paths(&mut self, paths: Vec<String>, cx: &mut Context<Self>) {
        if self.active_repository().is_some() {
            cx.emit(AppEvent::ShowDialog(DialogRequest::Stash { paths }));
        }
    }

    /// Lo que envía el diálogo de stash: `git stash push` con sus opciones
    /// por la cola de mutaciones; si sale bien, se releen los stashes (el
    /// nuevo aparece en la barra lateral).
    pub fn handle_stash_submit(
        &mut self,
        request: StashRequest,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        let Some(repository) = self.active_repository() else {
            return Task::ready(());
        };
        let repo_id = repository.id.clone();
        let repo_path = repository.path.clone();

        self.run_repo_operation_single_flight(&repo_id.clone(), "stash", cx, move |_, cx| {
            flow(cx, async move |this, cx| {
                let options = StashOptions {
                    message: request
                        .message
                        .as_deref()
                        .map(str::trim)
                        .filter(|message| !message.is_empty())
                        .map(str::to_string),
                    paths: request.paths,
                    include_untracked: request.include_untracked,
                    keep_index: request.keep_index,
                    staged_only: request.staged_only,
                };
                let result = this
                    .update(cx, |this, cx| {
                        this.stash_with_options(&repo_path, options, cx)
                    })
                    .await;
                this.update(cx, |this, cx| {
                    this.apply_repository_operation_result(repo_id.as_str(), &result, cx);
                });
                if result.success() {
                    let summary = result.to_operation_result();
                    let request = SyncRequest::new()
                        .stashes(true)
                        .operation_or(summary.operation.as_deref(), "stash")
                        .result(summary);
                    this.update(cx, |this, cx| {
                        this.sync_repository_read_model(repo_id.as_str(), request, cx)
                    })
                    .await;
                }
            })
        })
    }

    /// `handleNewBranch`: prepara el diálogo de nueva rama (desde la rama
    /// actual) y pide a la cáscara que lo abra.
    pub fn handle_new_branch(&mut self, cx: &mut Context<Self>) -> Task<()> {
        let Some(repository) = self.active_repository() else {
            return Task::ready(());
        };
        let repo_path = repository.path.clone();
        let base = repository
            .current_branch
            .clone()
            .filter(|branch| !branch.is_empty())
            .unwrap_or_else(|| "HEAD".to_string());

        let inspection = self.has_uncommitted_changes(&repo_path, cx);
        flow(cx, async move |this, cx| match inspection.await {
            Ok(local_changes) => this.update(cx, |this, cx| {
                this.shell.create_branch_base_reference = base.clone();
                cx.emit(AppEvent::ShowDialog(DialogRequest::CreateBranch {
                    base_reference: base.clone(),
                    base_label: base,
                    has_local_changes: local_changes.has_changes,
                }));
            }),
            Err(error) => this.update(cx, |this, cx| {
                let message = error.message();
                this.notify_error_with(
                    if message.is_empty() {
                        "Failed to prepare branch creation"
                    } else {
                        message
                    },
                    &error,
                    cx,
                );
            }),
        })
    }

    /// `handleCreateBranchSubmit`: crea la rama desde la referencia que
    /// preparó [`Self::handle_new_branch`] y relee lo que cambió.
    pub fn handle_create_branch_submit(
        &mut self,
        submission: CreateBranchSubmission,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        let Some(repo_id) = self.active_repository().map(|repo| repo.id.clone()) else {
            return Task::ready(());
        };
        let base_reference = self.shell.create_branch_base_reference.clone();

        self.run_repo_operation_single_flight(
            &repo_id.clone(),
            "create-branch",
            cx,
            move |_, cx| {
                flow(cx, async move |this, cx| {
                    let checkout_after_create = submission.checkout_after_create;
                    let local_changes_mode = submission.local_changes_mode;
                    let outcome = this
                        .update(cx, |this, cx| {
                            this.submit_create_branch(
                                repo_id.as_str(),
                                CreateBranchRequest {
                                    branch_name: submission.branch_name,
                                    checkout_after_create,
                                    local_changes_mode,
                                    base_reference: Some(base_reference),
                                },
                                cx,
                            )
                        })
                        .await;

                    if outcome.status != FlowStatus::Completed {
                        return;
                    }
                    let Some(branch_result) = outcome.branch_result else {
                        return;
                    };

                    let touched_working_tree =
                        checkout_after_create || local_changes_mode != LocalChangesMode::Keep;
                    let summary = branch_result.to_operation_result();
                    let request = SyncRequest::new()
                        .branches(true)
                        .status(touched_working_tree)
                        .changes_count(touched_working_tree)
                        .stashes(outcome.stashed_changes)
                        .operation_or(summary.operation.as_deref(), "create-branch")
                        .result(summary);
                    this.update(cx, |this, cx| {
                        this.sync_repository_read_model(repo_id.as_str(), request, cx)
                    })
                    .await;
                })
            },
        )
    }

    // ------------------------------------------------------------------
    // Sistema
    // ------------------------------------------------------------------

    /// `handleOpenTerminal`: abre un terminal en el repositorio activo.
    pub fn handle_open_terminal(&mut self, cx: &mut Context<Self>) -> Task<()> {
        let Some(repo_path) = self.active_repository().map(|repo| repo.path.clone()) else {
            return Task::ready(());
        };

        let open = self.blocking(cx, move |backend| {
            backend
                .shell()
                .open_terminal(&repo_path)
                .map_err(|error| error.to_string())
        });
        flow(cx, async move |this, cx| match open.await {
            Ok(result) if result.success => {}
            Ok(_) => this.update(cx, |this, cx| {
                this.notify_warning("Could not open terminal: Unknown error", cx);
            }),
            Err(error) => this.update(cx, |this, cx| {
                log::error!("Open terminal error: {error}");
                this.notify_error_with("Failed to open terminal", &error, cx);
            }),
        })
    }

    /// `handleOpenExplorer`: abre el explorador de archivos en el repositorio
    /// activo.
    pub fn handle_open_explorer(&mut self, cx: &mut Context<Self>) -> Task<()> {
        let Some(repo_path) = self.active_repository().map(|repo| repo.path.clone()) else {
            return Task::ready(());
        };

        let open = self.blocking(cx, move |backend| {
            backend
                .shell()
                .open_path(&repo_path)
                .map(|_| ())
                .map_err(|error| error.to_string())
        });
        flow(cx, async move |this, cx| {
            if let Err(error) = open.await {
                this.update(cx, |this, cx| {
                    log::error!("Open explorer error: {error}");
                    this.notify_error_with("Failed to open folder", &error, cx);
                });
            }
        })
    }

    /// "Open in editor" del repositorio activo entero (la carpeta).
    pub fn handle_open_in_editor(&mut self, cx: &mut Context<Self>) -> Task<()> {
        let Some(repo_path) = self.active_repository().map(|repo| repo.path.clone()) else {
            return Task::ready(());
        };
        self.open_in_editor(&repo_path, None, cx)
    }

    /// El atajo "Open in editor": el archivo de la pestaña de archivo activa
    /// (su versión del working tree) o, si no hay ninguna, el repositorio.
    pub fn open_active_in_editor(&mut self, cx: &mut Context<Self>) -> Task<()> {
        let Some(repo_path) = self.active_repository().map(|repo| repo.path.clone()) else {
            return Task::ready(());
        };
        let workspace = self.repository_workspace(&repo_path);
        let file = workspace
            .tabs
            .iter()
            .find(|tab| tab.id == workspace.active_tab && !tab.path.is_empty())
            .map(|tab| tab.path.clone());
        match file {
            Some(file) => self.open_working_tree_file_in_editor(&repo_path, &file, None, cx),
            None => self.open_in_editor(&repo_path, None, cx),
        }
    }

    /// Abre `path` (ruta absoluta de un archivo o carpeta) en el editor
    /// externo elegido, en `line` si la hay. Un fallo (el editor no está
    /// instalado, por ejemplo) se avisa con su explicación y el botón
    /// "Choose editor…" (Ajustes → Git → External editor). Sin editor elegido
    /// ni detectado se abre con el programa del sistema, y se dice, con el
    /// mismo botón: si el sistema tampoco tiene con qué abrirlo, al menos se
    /// sabe qué pasó.
    pub fn open_in_editor(
        &mut self,
        path: &str,
        line: Option<u32>,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        let path = path.to_string();
        let open = self.blocking(cx, move |backend| {
            backend
                .shell()
                .open_in_editor(&path, line)
                .map_err(|error| error.to_string())
        });
        flow(cx, async move |this, cx| {
            let (kind, message) = match open.await {
                Ok(EditorOpened::SystemDefault { fallback: true }) => (
                    NotifyType::Info,
                    "No external editor was found, so it opened with the system default app."
                        .to_string(),
                ),
                Ok(_) => return,
                Err(error) => {
                    log::warn!("Open in editor error: {error}");
                    (NotifyType::Warning, error)
                }
            };
            this.update(cx, |this, cx| {
                this.notify(
                    kind,
                    &message,
                    NotifyOptions {
                        id: Some(ToastId::from(OPEN_IN_EDITOR_TOAST)),
                        action: Some(ToastAction::custom("Choose editor…", CHOOSE_EDITOR_ACTION)),
                        ..Default::default()
                    },
                    cx,
                );
            });
        })
    }

    /// Abre en el editor el archivo `relative_path` del working tree de
    /// `repo_path`. Si ya no existe ahí (borrado, o solo está en un commit
    /// antiguo) se avisa en vez de abrir nada.
    pub fn open_working_tree_file_in_editor(
        &mut self,
        repo_path: &str,
        relative_path: &str,
        line: Option<u32>,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        let full_path = format!(
            "{}/{}",
            repo_path.trim_end_matches(['/', '\\']),
            relative_path.trim_start_matches('/')
        );
        let display = relative_path.to_string();
        let check = {
            let full_path = full_path.clone();
            cx.background_spawn(async move { std::path::Path::new(&full_path).exists() })
        };
        flow(cx, async move |this, cx| {
            if !check.await {
                this.update(cx, |this, cx| {
                    this.notify_warning(
                        &format!("{display} no longer exists in the working tree."),
                        cx,
                    );
                });
                return;
            }
            let open = this.update(cx, |this, cx| this.open_in_editor(&full_path, line, cx));
            open.await;
        })
    }

    /// Los editores del catálogo instalados en este sistema (para el selector
    /// de Settings).
    pub fn detect_editors(&mut self, cx: &mut Context<Self>) -> Task<Vec<DetectedEditor>> {
        self.blocking(cx, |backend| backend.shell().available_editors())
    }

    /// `window.api.shell.openExternal(url)`: abre una URL http(s) en el
    /// navegador.
    pub fn open_external(&mut self, url: &str, cx: &mut Context<Self>) -> Task<Result<(), String>> {
        let url = url.to_string();
        self.blocking(cx, move |backend| {
            backend
                .shell()
                .open_external(&url)
                .map_err(|error| error.to_string())
        })
    }

    /// `window.api.shell.showItemInFolder(path)`.
    pub fn show_item_in_folder(&mut self, path: &str, cx: &mut Context<Self>) -> Task<bool> {
        let path = path.to_string();
        self.blocking(cx, move |backend| {
            backend.shell().show_item_in_folder(&path).success
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use StatusSyncReason::*;

    #[test]
    fn head_reasons_are_the_ones_that_can_move_head() {
        assert!(is_head_status_watch_reason(FsWatchHead));
        assert!(is_head_status_watch_reason(PollHead));
        assert!(!is_head_status_watch_reason(FsWatchWorkingTree));
        assert!(!is_head_status_watch_reason(PollWorkingTree));
        assert!(!is_head_status_watch_reason(GitOperationControl));
    }

    #[test]
    fn merging_reasons_keeps_the_strongest() {
        assert_eq!(merge_status_watch_reason(None, PollHead), PollHead);
        assert_eq!(
            merge_status_watch_reason(Some(FsWatchWorkingTree), FsWatchWorkingTree),
            FsWatchWorkingTree
        );
        // HEAD gana al working tree; el origen es fs-watch si alguno lo era.
        assert_eq!(
            merge_status_watch_reason(Some(FsWatchWorkingTree), FsWatchHead),
            FsWatchHead
        );
        assert_eq!(
            merge_status_watch_reason(Some(PollHead), FsWatchWorkingTree),
            FsWatchHead
        );
        assert_eq!(
            merge_status_watch_reason(Some(PollWorkingTree), PollHead),
            PollHead
        );
        assert_eq!(
            merge_status_watch_reason(Some(PollWorkingTree), FsWatchWorkingTree),
            FsWatchWorkingTree
        );
        // Un control de operación no viene de fs-watch ni toca HEAD.
        assert_eq!(
            merge_status_watch_reason(Some(GitOperationControl), PollWorkingTree),
            PollWorkingTree
        );
        assert_eq!(
            merge_status_watch_reason(Some(GitOperationControl), FsWatchHead),
            FsWatchHead
        );
        assert_eq!(
            merge_status_watch_reason(Some(GitOperationControl), GitOperationControl),
            GitOperationControl
        );
    }
}
