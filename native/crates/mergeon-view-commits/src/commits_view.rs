//! Vista de commits de un repositorio
//! (`src/components/repository/CommitView.vue`): el historial a la izquierda,
//! un divisor arrastrable y el inspector del commit seleccionado a la derecha.
//!
//! Aquí vive lo que el original tenía en su `<script setup>`: cuándo se carga
//! y recarga el historial (con las peticiones simultáneas fundidas en una),
//! qué commit queda seleccionado tras cada recarga, los detalles del commit y
//! las acciones del menú contextual. Los datos y las mutaciones son de
//! `AppModel`.

mod actions;
mod history;
mod multi;

use std::collections::HashSet;
use std::sync::Arc;

use futures::future::Shared;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyWindowHandle, App, AsyncApp, ClipboardItem, Context, Entity, EventEmitter, FocusHandle,
    Focusable, KeyDownEvent, MouseButton, Pixels, Point, Subscription, Task,
    UniformListScrollHandle, WeakEntity, Window, canvas, div, px, relative,
};
use mergeon_app::{AppEvent, AppModel};
use mergeon_backend::FileChange;
use mergeon_model::commit_view_utils::{CommitRow, graph_lane_count};
use mergeon_model::workspace_tabs::{FileTabDescriptor, HOME_TAB, WorkspaceSection};
use mergeon_ui::Lucide;
use mergeon_ui::menu::MenuItem;

pub use actions::{
    BatchCommand, BatchMenuEntry, CommitCommand, CommitCreated, CommitMenuEntry, batch_menu,
    commit_menu, is_checkout_disabled,
};
pub use multi::{MULTI_ROW_HEIGHT, MultiStats};

use crate::changes_tab::{CommitChangesTab, CommitChangesTabEvent};
use crate::checkout::{CheckoutDialogRequest, CreateBranchDialogRequest};
use crate::compare_view::CompareView;
use crate::history_list::{CommitHistoryList, CommitHistoryListEvent, WorkingChanges, row_key};
use crate::inspector::CommitInspector;
use crate::interactive_rebase::InteractiveRebaseDialog;
use crate::resizer::{RESIZER_WIDTH, drag_tracker, resizer};
use crate::selection::MultiSelection;
use mergeon_view_sidebar::dialogs::{CheckoutBranchDialog, CreateBranchDialog};

/// Ancho de diseño del panel de historial. Señal pinta el historial como una
/// tabla de una línea (grafo · mensaje · autor · fecha · hash), que necesita
/// ancho para que autor y fecha quepan.
pub const DEFAULT_HISTORY_WIDTH: f64 = 600.;
/// Por debajo, las etiquetas de rama del mensaje se montaban sobre el hash.
const MIN_HISTORY_WIDTH: f64 = 320.;
const RIGHT_PANE_MIN_WIDTH: f64 = 320.;

/// Operaciones que no cambian el contenido de ningún commit ya cargado: solo
/// mueven refs, así que basta con refrescar la lista sin releer los detalles.
const REFS_ONLY_OPERATIONS: [&str; 22] = [
    "fetch",
    "pull",
    "push",
    "checkout",
    "checkout-remote-branch",
    "checkout-tag",
    "create-branch",
    "delete-branch",
    "rename-branch",
    "delete-remote-branch",
    "fast-forward-branch",
    "setup-tracking",
    "create-tag",
    "delete-tag",
    "push-tag",
    "delete-remote-tag",
    "stash",
    "apply-stash",
    "pop-stash",
    "stash-branch",
    "drop-stash",
    "submodule-update",
];

/// Operaciones que no tocan el historial: el working tree, el índice o la
/// configuración de remotos. Todas las demás recargan la lista.
///
/// El original solo recargaba con `fetch`, `pull`, `push`, `merge`, `rebase`
/// y `external-head-change`; un commit hecho en la pestaña Local Changes lo
/// cubría el watcher del sistema de archivos. Aquí se recarga con cualquier
/// mutación que pueda mover refs, para no depender de él.
const NON_HISTORY_OPERATIONS: [&str; 19] = [
    "status-watch",
    "stage",
    "stage-all",
    "unstage",
    "unstage-all",
    "discard",
    "discard-all",
    "add-to-ignore",
    "resolve-conflict",
    "apply-patch",
    "add-remote",
    "remove-remote",
    "rename-remote",
    "set-remote-url",
    "hydrate-opened-repository",
    // Quitar, bloquear o podar un worktree no mueve ningún ref.
    "worktree-remove",
    "worktree-lock",
    "worktree-unlock",
    "worktree-prune",
];

/// El archivo tiene algo preparado en el índice (columna X de porcelain), sin
/// contar los conflictos ni los no rastreados.
fn is_staged(index: &str, working_dir: &str) -> bool {
    const CONFLICTS: [(&str, &str); 7] = [
        ("D", "D"),
        ("A", "U"),
        ("U", "D"),
        ("U", "A"),
        ("D", "U"),
        ("A", "A"),
        ("U", "U"),
    ];
    let index = index.trim();
    let working_dir = working_dir.trim();
    !CONFLICTS.contains(&(index, working_dir)) && matches!(index, "M" | "A" | "D" | "R" | "C" | "T")
}

/// La mutación puede haber cambiado el historial cargado.
pub(crate) fn is_history_mutation(operation: Option<&str>) -> bool {
    operation.is_none_or(|operation| !NON_HISTORY_OPERATIONS.contains(&operation))
}

/// `clampHistoryWidth`: el ancho válido más cercano. `container_width` es el
/// ancho que comparten historial, divisor e inspector; 0 es "aún sin medir".
pub fn clamp_history_width(next_width: f64, container_width: f64) -> f64 {
    let normalized = if next_width.is_finite() {
        next_width
    } else {
        DEFAULT_HISTORY_WIDTH
    };

    if container_width == 0. || container_width.is_nan() {
        return normalized.max(MIN_HISTORY_WIDTH);
    }

    let max_width =
        MIN_HISTORY_WIDTH.max(container_width - RIGHT_PANE_MIN_WIDTH - f64::from(RESIZER_WIDTH));
    normalized.max(MIN_HISTORY_WIDTH).min(max_width)
}

/// Qué cargar y qué dejar seleccionado al recargar el historial (las opciones
/// de `loadCommits`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct CommitLoadOptions {
    /// Commit (hash completo) que debe quedar seleccionado, si está cargado.
    pub preferred_commit_hash: Option<String>,
    /// Si no, la punta de esta rama.
    pub preferred_branch: Option<String>,
    /// Releer los archivos del commit aunque siga siendo el mismo.
    pub force_details_reload: bool,
    pub preferred_file_path: Option<String>,
    /// Ampliar la ventana del historial hasta alcanzar el commit preferido.
    pub ensure_commit_visible: bool,
}

impl CommitLoadOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn preferred_commit_hash(mut self, hash: impl Into<String>) -> Self {
        self.preferred_commit_hash = Some(hash.into());
        self
    }

    pub fn preferred_branch(mut self, branch: impl Into<String>) -> Self {
        self.preferred_branch = Some(branch.into());
        self
    }

    pub fn force_details_reload(mut self, force: bool) -> Self {
        self.force_details_reload = force;
        self
    }

    pub fn preferred_file_path(mut self, path: impl Into<String>) -> Self {
        self.preferred_file_path = Some(path.into());
        self
    }

    pub fn ensure_commit_visible(mut self, ensure: bool) -> Self {
        self.ensure_commit_visible = ensure;
        self
    }

    /// `mergeCommitLoadOptions(base, incoming)`: lo pedido después gana, y lo
    /// que cualquiera de las dos peticiones exigía se conserva.
    pub fn merged_with(self, incoming: CommitLoadOptions) -> Self {
        Self {
            preferred_commit_hash: incoming
                .preferred_commit_hash
                .or(self.preferred_commit_hash),
            preferred_branch: incoming.preferred_branch.or(self.preferred_branch),
            force_details_reload: self.force_details_reload || incoming.force_details_reload,
            preferred_file_path: incoming.preferred_file_path.or(self.preferred_file_path),
            ensure_commit_visible: self.ensure_commit_visible || incoming.ensure_commit_visible,
        }
    }
}

/// Lo que la vista pide o cuenta a quien la aloja.
#[derive(Debug, Clone, PartialEq)]
pub enum CommitsViewEvent {
    /// `select-commit`: cambió (o se confirmó) el commit seleccionado.
    CommitSelected { hash: String },
    /// El usuario eligió un commit a mano (clic o flechas): la selección ya no
    /// corresponde al salto pedido desde el sidebar, así que su marca "estás
    /// viendo esta rama" debe retirarse.
    CommitPicked { repo_id: String, hash: String },
    /// `open-file`: abrir un archivo del commit (o su árbol) en una pestaña.
    /// En modo espacio de trabajo la vista ya lo ha abierto en `AppModel`.
    OpenFile(FileTabDescriptor),
    /// `change-view`: volver a la vista de commits.
    ChangeView(WorkspaceSection),
}

/// El repositorio de la vista, tal como estaba al empezar una operación: los
/// flujos asíncronos trabajan contra esta foto para que hash y ruta sean
/// siempre consistentes aunque el repositorio activo cambie entre esperas.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RepoRef {
    pub id: String,
    pub path: String,
    pub current_branch: Option<String>,
}

struct PendingCheckout {
    commit: CommitRow,
    request: CheckoutDialogRequest,
    /// El diálogo abierto y la escucha de su respuesta (los eventos de una
    /// entidad que nadie guarda no se entregan).
    dialog: Option<(Entity<CheckoutBranchDialog>, Subscription)>,
}

struct PendingCreateBranch {
    commit: CommitRow,
    request: CreateBranchDialogRequest,
    dialog: Option<(Entity<CreateBranchDialog>, Subscription)>,
}

struct RebaseSession {
    dialog: Entity<InteractiveRebaseDialog>,
    /// `<commit>^`, o `None` para rebasar desde la raíz.
    base: Option<String>,
    window: AnyWindowHandle,
    _subscription: Subscription,
}

/// Historial e inspector de un repositorio.
pub struct CommitsView {
    model: Entity<AppModel>,
    /// `props.repository`. `None`: el repositorio activo de la aplicación.
    repo_id: Option<String>,
    workspace_mode: bool,
    /// `activeView === 'commits'`, cuando lo fija quien aloja la vista.
    active_override: Option<bool>,

    history: Entity<CommitHistoryList>,
    changes: Entity<CommitChangesTab>,
    focus_handle: FocusHandle,
    /// La ventana de la vista: los diálogos se abren en ella.
    window: AnyWindowHandle,

    commits: Arc<Vec<CommitRow>>,
    /// Carriles del grafo de `commits`, calculado al recibirlos (no en cada
    /// `sync_children`, que corre con cualquier cambio del modelo).
    graph_lane_count: usize,
    has_more_commits: bool,
    loading_more_commits: bool,
    /// Solo la primera carga del historial de un repo puede vaciar la lista
    /// para mostrar el spinner; un refresco mantiene las filas en pantalla.
    loading_initial_history: bool,
    selected_commit: Option<CommitRow>,
    /// Una mutación del historial llegó sin estar viendo los commits: se
    /// recarga al volver.
    pending_history_reload: bool,
    current_branch: String,

    loading_details: bool,
    details_request: u64,
    commit_files: Vec<FileChange>,
    details_commit_hash: Option<String>,
    /// Ruta del repo para el que se cargaron los detalles: los diffs se piden
    /// contra ella, no contra el repositorio activo de ese momento.
    details_repo_path: Option<String>,
    selected_file: Option<FileChange>,
    file_diff: String,
    diff_too_large: bool,
    diff_whole_file_fell_back: bool,
    diff_size: u64,
    loading_diff: bool,
    diff_request: u64,
    summary_expanded: bool,

    history_width: f64,
    container_width: f64,
    container_left: Pixels,
    is_resizing_history: bool,
    /// Último ancho guardado que se vio en los ajustes.
    seen_saved_width: Option<f64>,

    history_hydrated_for_active_repo: bool,
    active_history_load_repo_id: Option<String>,
    /// Repo al que pertenecen las filas de `commits`: al cambiar de repo la
    /// lista sí debe vaciarse, al refrescar el mismo repo no.
    history_repo_id: Option<String>,
    watched_repo_id: Option<String>,
    watched_branch: Option<String>,
    /// Lo último que dijo [`Self::is_active`], puesto al día en
    /// `model_changed`: `render` lo usa sin leer el modelo.
    was_active: bool,
    /// Cuántas veces se pintó (tests).
    renders: usize,

    commit_load: Option<Shared<Task<()>>>,
    /// Pasadas de `performLoadCommits` hechas (con repositorio).
    history_load_count: usize,
    commit_load_running: bool,
    queued_commit_load_options: Option<CommitLoadOptions>,
    /// Un hash puede quedar obsoleto (historial reescrito en terminal): ante
    /// `bad object` se recarga la lista una vez por hash.
    stale_hash_reloads_attempted: HashSet<String>,

    pending_checkout: Option<PendingCheckout>,
    pending_create_branch: Option<PendingCreateBranch>,
    rebase: Option<RebaseSession>,

    /// Selección múltiple (Ctrl/⌘+clic, Shift+clic): además de
    /// `selected_commit`, que sigue siendo el principal.
    multi: MultiSelection,
    multi_stats: MultiStats,
    /// El rango (`<padre del más viejo>`, `<más nuevo>`) de los totales.
    multi_stats_range: Option<(String, String)>,
    multi_stats_task: Option<Task<()>>,
    multi_scroll: UniformListScrollHandle,
    /// "Compare these two" abierto y la escucha de su doble clic.
    compare: Option<(Entity<CompareView>, Subscription)>,

    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<CommitsViewEvent> for CommitsView {}

impl Focusable for CommitsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl CommitsView {
    /// La vista de commits del repositorio `repo_id`, como la monta el espacio
    /// de trabajo: sin diff incrustado (cada archivo se abre en su pestaña).
    pub fn new(repo_id: impl Into<String>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::build(Some(repo_id.into()), true, window, cx)
    }

    /// La vista sobre el repositorio activo de la aplicación, sea cual sea, y
    /// con el diff del archivo elegido incrustado en el inspector (el
    /// `CommitView` del original sin `workspace-mode`).
    pub fn for_active_repository(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::build(None, false, window, cx)
    }

    fn build(
        repo_id: Option<String>,
        workspace_mode: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let model = AppModel::global(cx);
        let history = cx.new(|cx| CommitHistoryList::new(window, cx));
        let changes = cx.new(|cx| CommitChangesTab::new(window, cx));

        let view = cx.weak_entity();
        history.update(cx, |list, _| {
            list.set_context_menu(move |commit, _, cx| actions::menu_items(&view, commit, cx));
        });
        let view = cx.weak_entity();
        changes.update(cx, |list, _| {
            list.set_file_context_menu(move |file, _, cx| changed_file_menu(&view, &file.path, cx));
        });

        let subscriptions = vec![
            cx.observe(&model, |this, _, cx| this.model_changed(cx)),
            cx.subscribe(&model, |this, _, event: &AppEvent, cx| {
                match event {
                    AppEvent::MutationRecorded { repo_id, operation } => {
                        this.mutation_recorded(repo_id, operation.as_deref(), cx);
                    }
                    // "File history" o el blame piden ir a un commit: el
                    // modelo ya dejó la pestaña Commits a la vista.
                    AppEvent::CommitSelectionRequested { repo_id, hash }
                        if this.active_repo(cx).is_some_and(|repo| repo.id == *repo_id) =>
                    {
                        this.request_commit_selection(hash, cx);
                    }
                    _ => {}
                }
            }),
            cx.subscribe(
                &history,
                |this, _, event: &CommitHistoryListEvent, cx| match event {
                    CommitHistoryListEvent::SelectCommit(commit) => {
                        this.handle_history_select(commit.clone(), cx);
                    }
                    CommitHistoryListEvent::CheckoutCommit(commit) => {
                        this.handle_history_checkout_request(commit.clone(), cx);
                    }
                    CommitHistoryListEvent::LoadMore => this.handle_load_more_commits(cx),
                    CommitHistoryListEvent::OpenWorkingChanges => this.open_local_changes(cx),
                    CommitHistoryListEvent::ToggleCommit(commit) => {
                        this.handle_history_toggle(commit.clone(), cx);
                    }
                    CommitHistoryListEvent::ExtendSelection(commit) => {
                        this.handle_history_extend(commit.clone(), cx);
                    }
                    CommitHistoryListEvent::CheckoutRef { commit, name, kind } => {
                        this.handle_ref_checkout_request(commit.clone(), name, *kind, cx);
                    }
                },
            ),
            cx.subscribe(
                &changes,
                |this, _, event: &CommitChangesTabEvent, cx| match event {
                    // En Commits el clic y el doble clic abren pestaña.
                    CommitChangesTabEvent::SelectFile(file)
                    | CommitChangesTabEvent::OpenFile(file) => this.select_file(file.clone(), cx),
                    CommitChangesTabEvent::ToggleIgnoreWhitespace => {
                        this.handle_toggle_ignore_whitespace(cx);
                    }
                    CommitChangesTabEvent::ToggleWholeFile => this.handle_toggle_whole_file(cx),
                },
            ),
        ];

        let mut this = Self {
            model,
            repo_id,
            workspace_mode,
            active_override: None,
            history,
            changes,
            focus_handle: cx.focus_handle(),
            window: window.window_handle(),
            commits: Arc::new(Vec::new()),
            graph_lane_count: graph_lane_count(&[]),
            has_more_commits: false,
            loading_more_commits: false,
            loading_initial_history: false,
            selected_commit: None,
            pending_history_reload: false,
            current_branch: String::new(),
            loading_details: false,
            details_request: 0,
            commit_files: Vec::new(),
            details_commit_hash: None,
            details_repo_path: None,
            selected_file: None,
            file_diff: String::new(),
            diff_too_large: false,
            diff_whole_file_fell_back: false,
            diff_size: 0,
            loading_diff: false,
            diff_request: 0,
            summary_expanded: false,
            history_width: clamp_history_width(DEFAULT_HISTORY_WIDTH, 0.),
            container_width: 0.,
            container_left: Pixels::ZERO,
            is_resizing_history: false,
            seen_saved_width: None,
            history_hydrated_for_active_repo: false,
            active_history_load_repo_id: None,
            history_repo_id: None,
            watched_repo_id: None,
            watched_branch: None,
            was_active: false,
            renders: 0,
            commit_load: None,
            history_load_count: 0,
            commit_load_running: false,
            queued_commit_load_options: None,
            stale_hash_reloads_attempted: HashSet::new(),
            pending_checkout: None,
            pending_create_branch: None,
            rebase: None,
            multi: MultiSelection::default(),
            multi_stats: MultiStats::default(),
            multi_stats_range: None,
            multi_stats_task: None,
            multi_scroll: UniformListScrollHandle::new(),
            compare: None,
            _subscriptions: subscriptions,
        };

        // `onMounted`: el ancho guardado del historial y las preferencias de diff.
        this.hydrate_history_width_preference(cx);
        this.model.update(cx, |model, cx| {
            model.load_diff_preferences(false, cx).detach();
        });

        // Los `watch` del original, con `immediate` el del repositorio.
        this.was_active = this.is_active(cx);
        let repo = this.active_repo(cx);
        this.watched_branch = repo.as_ref().and_then(|repo| repo.current_branch.clone());
        this.watched_repo_id = repo.map(|repo| repo.id);
        this.repository_changed(cx);
        this.publish(cx);
        // Un aviso del modelo que ya estuviera en cola al suscribirse no llega
        // por `observe`: se mira otra vez en cuanto termine este ciclo.
        let view = cx.weak_entity();
        cx.defer(move |cx| {
            view.update(cx, |this, cx| this.model_changed(cx)).ok();
        });
        this
    }

    // ------------------------------------------------------------------
    // Configuración (las props del original)
    // ------------------------------------------------------------------

    /// Cambia el repositorio de la vista. `None`: el repositorio activo de la
    /// aplicación.
    pub fn set_repository(&mut self, repo_id: Option<String>, cx: &mut Context<Self>) {
        if self.repo_id != repo_id {
            self.repo_id = repo_id;
            self.model_changed(cx);
        }
    }

    /// `workspace-mode`: los archivos del commit se abren en pestañas en vez de
    /// mostrar su diff en el propio inspector.
    pub fn set_workspace_mode(&mut self, workspace_mode: bool, cx: &mut Context<Self>) {
        if self.workspace_mode != workspace_mode {
            self.workspace_mode = workspace_mode;
            self.model_changed(cx);
        }
    }

    /// Fija si la vista de commits es la que se está viendo
    /// (`activeView === 'commits'`). Con `None` (por defecto) se deduce de
    /// `AppModel`: repositorio activo, pestaña "Commits" y Settings cerrado.
    pub fn set_active(&mut self, active: Option<bool>, cx: &mut Context<Self>) {
        if self.active_override != active {
            self.active_override = active;
            self.model_changed(cx);
        }
    }

    // ------------------------------------------------------------------
    // Lectura
    // ------------------------------------------------------------------

    pub fn repository_id(&self, cx: &App) -> Option<String> {
        self.active_repo(cx).map(|repo| repo.id)
    }

    pub fn is_workspace_mode(&self) -> bool {
        self.workspace_mode
    }

    /// La vista de commits es la que se está viendo.
    pub fn is_active(&self, cx: &App) -> bool {
        if let Some(active) = self.active_override {
            return active;
        }
        if !self.workspace_mode {
            return true;
        }
        let model = self.model.read(cx);
        let Some(repo) = self.active_repo(cx) else {
            return false;
        };
        !model.show_preferences()
            && model
                .active_repository()
                .is_some_and(|active| active.id == repo.id)
            && model.repository_workspace(&repo.path).active_tab == HOME_TAB
    }

    /// Cuántas veces se pintó la vista.
    #[doc(hidden)]
    pub fn render_count(&self) -> usize {
        self.renders
    }

    /// Cambia las filas del historial (y con ellas los carriles del grafo).
    pub(crate) fn set_commit_rows(&mut self, commits: Vec<CommitRow>) {
        self.graph_lane_count = graph_lane_count(&commits);
        self.commits = Arc::new(commits);
    }

    pub fn commits(&self) -> &Arc<Vec<CommitRow>> {
        &self.commits
    }

    pub fn selected_commit(&self) -> Option<&CommitRow> {
        self.selected_commit.as_ref()
    }

    /// Archivos del commit seleccionado, en el orden de git.
    pub fn commit_files(&self) -> &[FileChange] {
        &self.commit_files
    }

    pub fn selected_file(&self) -> Option<&FileChange> {
        self.selected_file.as_ref()
    }

    pub fn file_diff(&self) -> &str {
        &self.file_diff
    }

    pub fn has_more_commits(&self) -> bool {
        self.has_more_commits
    }

    pub fn is_loading_more_commits(&self) -> bool {
        self.loading_more_commits
    }

    pub fn is_loading_initial_history(&self) -> bool {
        self.loading_initial_history
    }

    pub fn is_loading_details(&self) -> bool {
        self.loading_details
    }

    pub fn is_loading_diff(&self) -> bool {
        self.loading_diff
    }

    /// Cuántas veces se ha leído el historial de git (las peticiones fundidas
    /// cuentan una vez).
    pub fn history_load_count(&self) -> usize {
        self.history_load_count
    }

    /// Hay una carga del historial en curso (o encolada).
    pub fn is_loading_history(&self) -> bool {
        self.commit_load_running
    }

    /// Rama actual tal como la conoce la vista (`HEAD` si no hay ninguna).
    pub fn current_branch(&self) -> &str {
        &self.current_branch
    }

    pub fn history_width(&self) -> f64 {
        self.history_width
    }

    pub fn is_resizing_history(&self) -> bool {
        self.is_resizing_history
    }

    pub fn is_summary_expanded(&self) -> bool {
        self.summary_expanded
    }

    /// El diálogo de checkout que está pendiente de respuesta, si lo hay.
    pub fn checkout_dialog(&self) -> Option<&CheckoutDialogRequest> {
        self.pending_checkout
            .as_ref()
            .map(|pending| &pending.request)
    }

    /// El diálogo "Create Branch" pendiente de respuesta, si lo hay.
    pub fn create_branch_dialog(&self) -> Option<&CreateBranchDialogRequest> {
        self.pending_create_branch
            .as_ref()
            .map(|pending| &pending.request)
    }

    /// El diálogo de rebase interactivo abierto, si lo hay.
    pub fn interactive_rebase_dialog(&self) -> Option<&Entity<InteractiveRebaseDialog>> {
        self.rebase.as_ref().map(|session| &session.dialog)
    }

    pub fn history_list(&self) -> &Entity<CommitHistoryList> {
        &self.history
    }

    pub fn changes_tab(&self) -> &Entity<CommitChangesTab> {
        &self.changes
    }

    // ------------------------------------------------------------------
    // Interno: repositorio, publicación y observadores
    // ------------------------------------------------------------------

    pub(crate) fn active_repo(&self, cx: &App) -> Option<RepoRef> {
        let model = self.model.read(cx);
        let repository = match &self.repo_id {
            Some(repo_id) => model.repository(repo_id.as_str()),
            None => model.active_repository(),
        }?;
        Some(RepoRef {
            id: repository.id.clone(),
            path: repository.path.clone(),
            current_branch: repository
                .current_branch
                .clone()
                .filter(|branch| !branch.is_empty()),
        })
    }

    /// `activeRepo.value?.currentBranch || currentBranch.value`.
    pub(crate) fn branch_for_reload(&self, cx: &App) -> Option<String> {
        self.active_repo(cx)
            .and_then(|repo| repo.current_branch)
            .or_else(|| (!self.current_branch.is_empty()).then(|| self.current_branch.clone()))
    }

    /// Lanza un flujo que llega hasta el final aunque nadie espere su `Task`,
    /// como una promesa del original.
    pub(crate) fn flow<R: 'static>(
        cx: &mut Context<Self>,
        work: impl AsyncFnOnce(WeakEntity<Self>, &mut AsyncApp) -> R + 'static,
    ) -> Task<R> {
        let (sender, receiver) = futures::channel::oneshot::channel();
        cx.spawn(async move |this, cx| {
            let output = work(this, cx).await;
            let _ = sender.send(output);
        })
        .detach();

        cx.foreground_executor().spawn(async move {
            match receiver.await {
                Ok(output) => output,
                // El flujo se abandonó sin terminar: la app se está cerrando.
                Err(_) => std::future::pending().await,
            }
        })
    }

    /// Pone al día la lista y el inspector y vuelve a pintar la vista.
    pub(crate) fn publish(&mut self, cx: &mut Context<Self>) {
        self.sync_children(cx);
        cx.notify();
    }

    /// Pone al día la lista y el inspector con el estado de la vista. Cada
    /// uno se repinta solo si algo suyo cambió.
    fn sync_children(&mut self, cx: &mut Context<Self>) {
        let commits = self.commits.clone();
        let lane_count = self.graph_lane_count;
        let selected = self.selected_commit.as_ref().map(row_key);
        let (loading, has_more, loading_more) = (
            self.loading_initial_history,
            self.has_more_commits,
            self.loading_more_commits,
        );
        let working_changes = self.working_changes(cx);
        let multi = self.multi.set().clone();
        self.history.update(cx, |list, cx| {
            list.set_multi_selection(multi, cx);
            list.set_working_changes(working_changes, cx);
            list.set_graph_lane_count(lane_count, cx);
            list.set_has_more(has_more, cx);
            list.set_commits(commits, cx);
            list.set_loading_more(loading_more, cx);
            list.set_loading(loading, cx);
            list.set_selected_commit_hash(selected, cx);
        });

        let preferences = *self.model.read(cx).diff_preferences();
        let selected_path = self.selected_file.as_ref().map(|file| file.path.as_str());
        let (files, diff) = (&self.commit_files, self.file_diff.clone());
        let (external, loading_details, loading_diff, too_large, whole_file_fell_back, size) = (
            self.workspace_mode,
            self.loading_details,
            self.loading_diff,
            self.diff_too_large,
            self.diff_whole_file_fell_back,
            self.diff_size,
        );
        self.changes.update(cx, |tab, cx| {
            tab.set_external_files(external, cx);
            // En el espacio de trabajo el bloque de cifras ya cuenta los archivos.
            tab.set_show_header(!external, cx);
            tab.set_loading_details(loading_details, cx);
            tab.set_commit_files(files, cx);
            tab.set_diff_preferences(
                preferences.diff_view_mode,
                preferences.ignore_whitespace,
                cx,
            );
            tab.set_whole_file(preferences.whole_file, cx);
            tab.set_whole_file_fell_back(whole_file_fell_back, cx);
            tab.set_selected_path(selected_path, cx);
            tab.set_diff(diff, too_large, size, cx);
            tab.set_loading_diff(loading_diff, cx);
        });
    }

    /// Los `watch` del original sobre el repositorio, su rama y la vista
    /// activa, más lo que la vista pinta de `AppModel`.
    fn model_changed(&mut self, cx: &mut Context<Self>) {
        let repo = self.active_repo(cx);

        let repo_id = repo.as_ref().map(|repo| repo.id.clone());
        if repo_id != self.watched_repo_id {
            self.watched_repo_id = repo_id;
            self.repository_changed(cx);
        }

        let branch = repo.and_then(|repo| repo.current_branch);
        if branch != self.watched_branch {
            let previous = std::mem::replace(&mut self.watched_branch, branch.clone());
            self.current_branch_changed(branch, previous, cx);
        }

        let active = self.is_active(cx);
        if active != self.was_active {
            self.was_active = active;
            if active {
                self.became_active(cx);
            }
            cx.notify();
        }

        // El ancho del historial es un ajuste compartido: si otra vista lo
        // cambió, esta lo adopta.
        let saved = self.model.read(cx).history_width();
        if saved != self.seen_saved_width {
            self.seen_saved_width = saved;
            if !self.is_resizing_history
                && let Some(width) = saved
            {
                self.history_width = clamp_history_width(width, self.container_width);
                cx.notify();
            }
        }

        // Las preferencias de diff son de `AppModel`.
        self.sync_children(cx);
    }

    /// `watch(() => activeRepo.value?.id, …, { immediate: true })`.
    fn repository_changed(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.active_repo(cx) else {
            self.active_history_load_repo_id = None;
            self.history_hydrated_for_active_repo = false;
            self.close_create_branch_dialog(cx);
            self.clear_history(cx);
            return;
        };

        if self.active_history_load_repo_id.as_deref() == Some(repo.id.as_str()) {
            return;
        }

        self.active_history_load_repo_id = Some(repo.id.clone());
        self.history_hydrated_for_active_repo = false;

        // `initial-commit-hash`: el commit que estaba seleccionado la última vez.
        let initial_commit_hash = self
            .workspace_mode
            .then(|| self.model.read(cx).repository_workspace(&repo.path))
            .and_then(|workspace| workspace.selected_commit);
        // Sin commit guardado se elige el de la rama actual o, si aún no se
        // conoce (llega con el primer status), el de HEAD: nunca la primera
        // fila por defecto, que puede ser un stash. La rama vacía no coincide
        // con ninguna y evita caer en el `current_branch` del repo anterior.
        let preferred_branch = Some(repo.current_branch.clone().unwrap_or_default());
        let options = CommitLoadOptions {
            preferred_branch,
            ensure_commit_visible: initial_commit_hash.is_some(),
            preferred_commit_hash: initial_commit_hash,
            force_details_reload: true,
            preferred_file_path: None,
        };
        let load = self.load_commits(options, cx);
        let repo_id = repo.id;
        cx.spawn(async move |this, cx| {
            load.await;
            let _ = this.update(cx, |this, cx| {
                this.history_hydrated_for_active_repo = true;
                if this.active_repo(cx).is_some_and(|repo| repo.id == repo_id) {
                    this.active_history_load_repo_id = None;
                }
            });
        })
        .detach();
    }

    /// `watch(() => activeRepo.value?.currentBranch, …)`.
    fn current_branch_changed(
        &mut self,
        new_branch: Option<String>,
        old_branch: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(new_branch) = new_branch else {
            return;
        };
        if self.active_repo(cx).is_none() || Some(&new_branch) == old_branch.as_ref() {
            return;
        }

        if !self.history_hydrated_for_active_repo || !self.is_active(cx) {
            self.current_branch = new_branch;
            return;
        }

        self.reload_history(
            CommitLoadOptions::new()
                .preferred_branch(new_branch)
                .force_details_reload(true),
            cx,
        );
    }

    /// `watch(() => [lastMutationAt, lastMutationOperation], …)`.
    fn mutation_recorded(
        &mut self,
        repo_id: &str,
        operation: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        if !self.active_repo(cx).is_some_and(|repo| repo.id == repo_id) {
            return;
        }
        if !is_history_mutation(operation) {
            return;
        }

        // Si el usuario no está viendo los commits, se recarga al volver.
        if !self.is_active(cx) {
            self.pending_history_reload = true;
            return;
        }

        /*
         * Fetch y pull solo mueven refs. El contenido de un commit es inmutable, así
         * que si el seleccionado sigue en la lista no hay nada que releer: forzarlo
         * vaciaba el panel de detalle y lo repintaba en cada fetch.
         */
        let refs_only_mutation = operation.is_some_and(|name| REFS_ONLY_OPERATIONS.contains(&name));
        let options = CommitLoadOptions {
            preferred_branch: self.branch_for_reload(cx),
            force_details_reload: !refs_only_mutation,
            ..Default::default()
        };
        self.reload_history(options, cx);
    }

    /// `watch(() => props.activeView, …)`: ponerse al día con las mutaciones
    /// que llegaron mientras no se veían los commits.
    fn became_active(&mut self, cx: &mut Context<Self>) {
        if self.active_repo(cx).is_none() || !self.pending_history_reload {
            return;
        }

        self.pending_history_reload = false;
        let options = CommitLoadOptions {
            preferred_branch: self.branch_for_reload(cx),
            force_details_reload: true,
            ..Default::default()
        };
        self.reload_history(options, cx);
    }

    // ------------------------------------------------------------------
    // Peticiones de fuera
    // ------------------------------------------------------------------

    /// `commitSelectionRequest` del original: el sidebar (clic en una rama o un
    /// tag) o la búsqueda piden seleccionar un commit y traerlo a la vista. Si
    /// queda fuera de la ventana cargada se reintenta con una mayor.
    pub fn request_commit_selection(&mut self, commit_hash: &str, cx: &mut Context<Self>) {
        if self.active_repo(cx).is_none() || !self.is_active(cx) || commit_hash.is_empty() {
            return;
        }

        let options = CommitLoadOptions {
            preferred_commit_hash: Some(commit_hash.to_string()),
            preferred_branch: self.branch_for_reload(cx),
            force_details_reload: true,
            ensure_commit_visible: true,
            preferred_file_path: None,
        };
        self.reload_history(options, cx);
    }

    // ------------------------------------------------------------------
    // Inspector
    // ------------------------------------------------------------------

    /// `selectFile`: en el espacio de trabajo abre el archivo en una pestaña;
    /// fuera de él, muestra su diff en el inspector.
    pub fn select_file(&mut self, file: FileChange, cx: &mut Context<Self>) {
        if self.workspace_mode
            && let Some(commit_hash) = self
                .selected_commit
                .as_ref()
                .map(|commit| commit.full_hash.clone())
                .filter(|hash| !hash.is_empty())
        {
            let descriptor = FileTabDescriptor::commit(file.path.clone(), &commit_hash);
            self.selected_file = Some(file);
            self.open_file(descriptor, cx);
            self.publish(cx);
            return;
        }

        self.selected_file = Some(file.clone());
        self.load_diff(Some(file), cx).detach();
    }

    /// "File history" de un archivo del commit elegido: su historial
    /// siguiendo renombres, desde ese commit, en una pestaña.
    pub fn open_file_history(&mut self, path: &str, cx: &mut Context<Self>) {
        let commit_hash = self
            .selected_commit
            .as_ref()
            .map(|commit| commit.full_hash.clone())
            .filter(|hash| !hash.is_empty());
        self.open_file(
            FileTabDescriptor::history(path.to_string(), commit_hash.as_deref()),
            cx,
        );
    }

    /// `openRepoTree`: el árbol del repositorio en ese commit, como pestaña
    /// propia y sin archivo elegido.
    pub fn open_repo_tree(&mut self, cx: &mut Context<Self>) {
        let Some(commit_hash) = self
            .selected_commit
            .as_ref()
            .map(|commit| commit.full_hash.clone())
            .filter(|hash| !hash.is_empty())
        else {
            return;
        };
        self.open_file(FileTabDescriptor::tree("", &commit_hash), cx);
    }

    fn open_file(&mut self, descriptor: FileTabDescriptor, cx: &mut Context<Self>) {
        // `@open-file="openFile"` de `RepositoryWorkspace.vue`.
        if self.workspace_mode
            && let Some(repo) = self.active_repo(cx)
        {
            self.model.update(cx, |model, cx| {
                model.open_workspace_file(&repo.path, &descriptor, false, cx);
            });
        }
        cx.emit(CommitsViewEvent::OpenFile(descriptor));
    }

    /// `handleToggleIgnoreWhitespace`: el diff se vuelve a pedir a git.
    pub fn handle_toggle_ignore_whitespace(&mut self, cx: &mut Context<Self>) {
        self.model.update(cx, |model, cx| {
            model.toggle_ignore_whitespace(cx).detach();
        });
        if let Some(file) = self.selected_file.clone() {
            self.load_diff(Some(file), cx).detach();
        }
    }

    /// "Whole file": el diff (con el archivo entero o solo los cambios) se
    /// vuelve a pedir a git.
    pub fn handle_toggle_whole_file(&mut self, cx: &mut Context<Self>) {
        self.model.update(cx, |model, cx| {
            model.toggle_whole_file(cx).detach();
        });
        if let Some(file) = self.selected_file.clone() {
            self.load_diff(Some(file), cx).detach();
        }
    }

    /// Los cambios sin commitear del repositorio según su último status, para
    /// la fila que va encima del historial.
    fn working_changes(&self, cx: &App) -> Option<WorkingChanges> {
        let repo = self.active_repo(cx)?;
        let model = self.model.read(cx);
        let status = model.repository(repo.id.as_str())?.status.as_ref()?;
        let staged = status
            .files
            .iter()
            .filter(|file| is_staged(&file.index, &file.working_dir))
            .count();
        WorkingChanges::new(status.files.len(), staged)
    }

    /// Clic en la fila de cambios sin commitear: la pestaña Local Changes.
    pub fn open_local_changes(&mut self, cx: &mut Context<Self>) {
        if self.workspace_mode
            && let Some(repo) = self.active_repo(cx)
        {
            self.model.update(cx, |model, cx| {
                model.show_workspace_home(&repo.path, Some(WorkspaceSection::Changes), cx);
            });
        }
        cx.emit(CommitsViewEvent::ChangeView(WorkspaceSection::Changes));
    }

    /// `copyToClipboard`.
    pub fn copy_to_clipboard(&mut self, text: &str, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
        self.model.update(cx, |model, cx| {
            model.notify_success("Copied to clipboard", cx);
        });
    }

    /// La URL web de GitHub del repositorio (`https://github.com/<owner>/<repo>`),
    /// si tiene un remoto de GitHub: el del upstream de la rama o `origin`.
    pub fn github_web_url(&self, cx: &App) -> Option<String> {
        let model = self.model.read(cx);
        let repository = match &self.repo_id {
            Some(repo_id) => model.repository(repo_id.as_str()),
            None => model.active_repository(),
        }?;
        crate::github_link::open_repository_github_url(repository)
    }

    /// "Copy GitHub link" del menú de un commit.
    pub fn copy_github_commit_link(&mut self, commit: &CommitRow, cx: &mut Context<Self>) {
        if let Some(url) = self.github_web_url(cx) {
            let link = crate::github_link::github_commit_url(&url, &commit.full_hash);
            self.copy_github_link(&link, cx);
        }
    }

    /// "Copy GitHub link" del menú de un archivo de "Changed Files": el
    /// archivo tal como quedó en el commit elegido.
    pub fn copy_github_file_link(&mut self, path: &str, cx: &mut Context<Self>) {
        let Some(commit) = self.selected_commit.as_ref() else {
            return;
        };
        if let Some(url) = self.github_web_url(cx) {
            let link = crate::github_link::github_blob_url(&url, &commit.full_hash, path, None);
            self.copy_github_link(&link, cx);
        }
    }

    fn copy_github_link(&mut self, link: &str, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(link.to_string()));
        self.model.update(cx, |model, cx| {
            model.notify_success("GitHub link copied to clipboard", cx);
        });
    }

    /// Despliega o pliega los detalles de la cabecera del commit.
    pub fn toggle_summary(&mut self, cx: &mut Context<Self>) {
        self.summary_expanded = !self.summary_expanded;
        cx.notify();
    }

    // ------------------------------------------------------------------
    // Ancho del historial
    // ------------------------------------------------------------------

    /// `hydrateHistoryWidthPreference`.
    fn hydrate_history_width_preference(&mut self, cx: &mut Context<Self>) {
        let load = self
            .model
            .update(cx, |model, cx| model.load_history_width(cx));
        cx.spawn(async move |this, cx| {
            let saved = load.await;
            let _ = this.update(cx, |this, cx| {
                this.seen_saved_width = saved;
                if !this.is_resizing_history {
                    this.history_width = clamp_history_width(
                        saved.unwrap_or(DEFAULT_HISTORY_WIDTH),
                        this.container_width,
                    );
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// El contenedor se midió (o cambió de tamaño): `handleWindowResize`.
    fn container_measured(&mut self, left: Pixels, width: f64, cx: &mut Context<Self>) {
        self.container_left = left;
        if self.container_width == width {
            return;
        }
        self.container_width = width;
        let clamped = clamp_history_width(self.history_width, width);
        if clamped != self.history_width {
            self.history_width = clamped;
        }
        cx.notify();
    }

    fn start_history_resize(&mut self, cx: &mut Context<Self>) {
        self.is_resizing_history = true;
        cx.notify();
    }

    /// `handleHistoryResize`.
    fn handle_history_resize(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        if !self.is_resizing_history || self.container_width == 0. {
            return;
        }
        let width = clamp_history_width(
            f64::from(f32::from(position.x - self.container_left)),
            self.container_width,
        );
        if width != self.history_width {
            self.history_width = width;
            cx.notify();
        }
    }

    /// `stopHistoryResize`: al soltar el divisor el ancho se guarda.
    fn stop_history_resize(&mut self, cx: &mut Context<Self>) {
        if !self.is_resizing_history {
            return;
        }
        self.is_resizing_history = false;
        let width = self.history_width;
        self.seen_saved_width = Some(width);
        self.model.update(cx, |model, cx| {
            model.persist_history_width(width, cx).detach();
        });
        cx.notify();
    }

    // ------------------------------------------------------------------
    // Teclado
    // ------------------------------------------------------------------

    /// `handleHistoryKeydown`: las flechas mueven la selección al commit
    /// vecino mientras se está en la vista de commits.
    fn handle_history_keydown(
        &mut self,
        event: &KeyDownEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.is_active(cx) {
            return;
        }
        let key = event.keystroke.key.as_str();
        let modifiers = event.keystroke.modifiers;
        // Esc vuelve a la selección simple.
        if key == "escape" && !modifiers.modified() && self.multi.is_multi() {
            cx.stop_propagation();
            self.collapse_multi_selection(cx);
            return;
        }
        if key != "down" && key != "up" {
            return;
        }
        // Shift+flechas extienden la selección desde el ancla.
        if modifiers.shift && !modifiers.control && !modifiers.alt && !modifiers.platform {
            if self.extend_selection_by(if key == "down" { 1 } else { -1 }, cx) {
                cx.stop_propagation();
            }
            return;
        }
        if modifiers.modified() || self.commits.is_empty() {
            return;
        }
        // Una flecha sola vuelve a la selección simple.
        if self.multi.is_multi() {
            let anchor = self
                .selected_commit
                .as_ref()
                .map(|commit| row_key(commit).to_string());
            self.multi.collapse(anchor.as_deref());
        }

        let current = self.selected_commit.as_ref().and_then(|selected| {
            self.commits
                .iter()
                .position(|commit| commit.full_hash == selected.full_hash)
        });
        let next = match (key, current) {
            ("down", Some(index)) => (index + 1).min(self.commits.len() - 1),
            ("down", None) => 0,
            (_, Some(index)) => index.saturating_sub(1),
            (_, None) => 0,
        };
        if Some(next) == current {
            return;
        }
        let Some(commit) = self.commits.get(next).cloned() else {
            return;
        };

        cx.stop_propagation();
        self.handle_history_select(commit, cx);
    }
}

/// El menú de un archivo de "Changed Files": abrirlo en el editor externo (su
/// versión del working tree), su historial ("File history"), copiar su ruta
/// y, si el repositorio está en GitHub, el enlace al archivo en el commit
/// elegido.
fn changed_file_menu(view: &WeakEntity<CommitsView>, path: &str, cx: &App) -> Vec<MenuItem> {
    let on_github = view
        .upgrade()
        .is_some_and(|view| view.read(cx).github_web_url(cx).is_some());
    let editor = (view.clone(), path.to_string());
    let copy_path = (view.clone(), path.to_string());
    let history = (view.clone(), path.to_string());
    let mut items = vec![
        MenuItem::new("Open in editor")
            .icon(Lucide::CodeXml)
            .on_click(move |_, cx| {
                let (view, path) = editor.clone();
                let _ = view.update(cx, |view, cx| view.open_file_in_editor(&path, cx));
            }),
        MenuItem::separator(),
        MenuItem::new("File history")
            .icon(Lucide::FileClock)
            .on_click(move |_, cx| {
                let (view, path) = history.clone();
                let _ = view.update(cx, |view, cx| view.open_file_history(&path, cx));
            }),
        MenuItem::separator(),
        MenuItem::new("Copy Path")
            .icon(Lucide::Copy)
            .on_click(move |_, cx| {
                let (view, path) = copy_path.clone();
                let _ = view.update(cx, |view, cx| view.copy_to_clipboard(&path, cx));
            }),
    ];
    if on_github {
        let copy_link = (view.clone(), path.to_string());
        items.push(
            MenuItem::new("Copy GitHub link")
                .icon(Lucide::Link2)
                .on_click(move |_, cx| {
                    let (view, path) = copy_link.clone();
                    let _ = view.update(cx, |view, cx| view.copy_github_file_link(&path, cx));
                }),
        );
    }
    items
}

impl Render for CommitsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Sin nada enfocado, las flechas son del historial (el original las
        // escuchaba en `window`).
        // `was_active` lo pone al día `model_changed`: pintar no lee el modelo.
        self.renders += 1;
        if self.was_active && window.focused(cx).is_none() {
            window.focus(&self.focus_handle, cx);
        }

        let entity = cx.weak_entity();
        let measure = {
            let entity = entity.clone();
            canvas(
                move |bounds, _, cx| {
                    let _ = entity.update(cx, |view, cx| {
                        view.container_measured(
                            bounds.left(),
                            f64::from(f32::from(bounds.size.width)),
                            cx,
                        );
                    });
                },
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0()
        };

        let multi_summary = self.multi.is_multi().then(|| self.render_multi_summary(cx));
        let inspector = self
            .selected_commit
            .as_ref()
            .filter(|_| multi_summary.is_none())
            .map(|commit| {
                // Checkout se desactiva igual que su entrada del menú contextual.
                let checkout_disabled = is_checkout_disabled(commit, &self.current_branch);
                let menu_commit = commit.clone();
                let command_commit = commit.clone();
                CommitInspector::new(commit, &self.changes)
                    .expanded(self.summary_expanded)
                    .checkout_disabled(checkout_disabled)
                    .on_toggle_details(cx.listener(|view, _, _, cx| view.toggle_summary(cx)))
                    .on_open_tree(cx.listener(|view, _, _, cx| view.open_repo_tree(cx)))
                    .on_copy_sha({
                        let entity = entity.clone();
                        move |hash, _, cx| {
                            let _ = entity.update(cx, |view, cx| view.copy_to_clipboard(hash, cx));
                        }
                    })
                    .on_select_parent({
                        let entity = entity.clone();
                        move |hash, _, cx| {
                            let _ =
                                entity.update(cx, |view, cx| view.select_parent_commit(hash, cx));
                        }
                    })
                    .on_command({
                        let entity = entity.clone();
                        move |command, window, cx| {
                            let _ = entity.update(cx, |view, cx| {
                                view.run_command(command, &command_commit, window, cx);
                            });
                        }
                    })
                    .menu({
                        let entity = entity.clone();
                        move |_, cx| actions::menu_items(&entity, &menu_commit, cx)
                    })
            });

        div()
            .id("commit-view")
            .test_support()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::handle_history_keydown))
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .w_full()
            .h_full()
            .overflow_hidden()
            .line_height(relative(1.5))
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(measure)
                    .child(drag_tracker(
                        self.is_resizing_history,
                        entity,
                        |view, position, _, cx| view.handle_history_resize(position, cx),
                        |view, _, cx| view.stop_history_resize(cx),
                    ))
                    .child(
                        div()
                            .flex_shrink_0()
                            .h_full()
                            .w(px(self.history_width as f32))
                            .child(self.history.clone()),
                    )
                    .child(
                        resizer("commit-resizer", self.is_resizing_history, cx)
                            .test_support()
                            .aria_label("Resize commit history")
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|view, _, window, cx| {
                                    window.prevent_default();
                                    view.start_history_resize(cx);
                                }),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .children(multi_summary)
                            .children(inspector),
                    ),
            )
    }
}

impl CommitsView {
    /// "Open in editor" de un archivo del commit: el del working tree (si ya
    /// no existe, se avisa).
    pub fn open_file_in_editor(&mut self, path: &str, cx: &mut Context<Self>) {
        let Some(repo) = self.active_repo(cx) else {
            return;
        };
        self.model
            .update(cx, |model, cx| {
                model.open_working_tree_file_in_editor(&repo.path, path, None, cx)
            })
            .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamps_the_history_width_to_its_container() {
        // Sin medir el contenedor solo se impone el mínimo.
        assert_eq!(clamp_history_width(560., 0.), 560.);
        assert_eq!(clamp_history_width(20., 0.), 320.);
        assert_eq!(clamp_history_width(f64::NAN, 0.), DEFAULT_HISTORY_WIDTH);

        assert_eq!(clamp_history_width(560., 1000.), 560.);
        // 700 - 320 del inspector - 8 del divisor.
        assert_eq!(clamp_history_width(900., 700.), 372.);
        // Un contenedor diminuto no baja del mínimo.
        assert_eq!(clamp_history_width(900., 300.), 320.);
        assert_eq!(clamp_history_width(20., 1000.), 320.);
    }

    #[test]
    fn reloads_the_history_after_any_mutation_that_can_move_refs() {
        for operation in [
            "commit",
            "fetch",
            "pull",
            "push",
            "merge",
            "rebase",
            "reset",
            "external-head-change",
            "stash",
        ] {
            assert!(is_history_mutation(Some(operation)), "{operation}");
        }
        assert!(is_history_mutation(None));
        for operation in ["status-watch", "stage", "discard", "add-remote"] {
            assert!(!is_history_mutation(Some(operation)), "{operation}");
        }
    }

    #[test]
    fn counts_as_staged_only_what_is_in_the_index() {
        assert!(is_staged("M", " "));
        assert!(is_staged("A", "M"));
        assert!(is_staged("R", ""));
        assert!(!is_staged(" ", "M"));
        assert!(!is_staged("?", "?"));
        assert!(!is_staged("U", "U"));
        assert!(!is_staged("A", "A"));
    }

    #[test]
    fn merges_reload_requests_made_while_one_is_running() {
        let base = CommitLoadOptions::new()
            .preferred_commit_hash("aaa")
            .preferred_branch("main")
            .force_details_reload(true);
        let incoming = CommitLoadOptions::new()
            .preferred_branch("feature")
            .ensure_commit_visible(true);

        let merged = base.merged_with(incoming);

        assert_eq!(merged.preferred_commit_hash.as_deref(), Some("aaa"));
        assert_eq!(merged.preferred_branch.as_deref(), Some("feature"));
        assert!(merged.force_details_reload);
        assert!(merged.ensure_commit_visible);
        assert_eq!(merged.preferred_file_path, None);
    }
}
