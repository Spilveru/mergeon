//! El sidebar del repositorio activo (`src/components/layout/Sidebar.vue`):
//! navegación, filtro, secciones y búsqueda de commits. Los flujos de cada
//! acción están en [`crate::flows`] y en `actions.rs`.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::input::{Escape, Input};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{ActiveTheme as _, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, Bounds, Context, Entity, EventEmitter, Focusable as _, ListAlignment,
    ListSizingBehavior, ListState, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    Pixels, Point, ScrollHandle, SharedString, Subscription, Task, TestSupportExt as _, Window,
    canvas, div, list, point, px, rems,
};
use mergeon_app::{AppEvent, AppModel, CheckoutTarget, OpenRepository, SyncRequest};
use mergeon_backend::{CommitSearchOptions, CompareMode, StashEntry, Submodule};
use mergeon_model::branch_tree::{
    BranchTreeNode, BranchTreeRow, branch_ancestor_folders, build_branch_tree, flatten_branch_tree,
};
use mergeon_model::format::now_epoch_ms;
use mergeon_model::repositories::SidebarFlags;
use mergeon_model::repository_read_model::{BranchItem, RemoteItem};
use mergeon_model::workspace_tabs::WorkspaceSection;
use mergeon_ui::theme::with_alpha;
use mergeon_ui::{ActivePaletteExt as _, IconTone, InputEvent, InputState, Lucide, Radius};

use crate::dialogs::{
    AddRemoteDialog, BranchActionDialog, CheckoutBranchDialog, CleanupBranchesDialog,
    CreateBranchDialog, TagDetailsDialog,
};
use crate::flows::{self, RepoRef};
use crate::logic::detached_head_label;
use crate::search_filters::{SEARCH_PAGE_SIZE, SearchFilterInputs, search_results};
use crate::section;
use crate::virtual_rows::RowHeights;

/// Cuánto pinta la lista de resultados por encima y por debajo de lo que se
/// ve (ocupa el alto que deje el sidebar, sin tope fijo).
const SEARCH_RESULTS_OVERDRAW: f32 = 320.;

/// Alto de un resultado de una línea de mensaje (con su hueco), para la barra
/// de scroll mientras no se mide.
const ESTIMATED_SEARCH_RESULT_HEIGHT: f32 = 60.;

/// Qué muestra el sidebar bajo la navegación (`sidebarPanelMode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PanelMode {
    /// Las secciones del repositorio.
    #[default]
    Repository,
    /// La búsqueda de commits.
    Search,
}

/// Una sección plegable del sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Section {
    Starred,
    Branches,
    Remotes,
    Tags,
    Stashes,
    Submodules,
}

impl Section {
    pub fn as_str(self) -> &'static str {
        match self {
            Section::Starred => "starred",
            Section::Branches => "branches",
            Section::Remotes => "remotes",
            Section::Tags => "tags",
            Section::Stashes => "stashes",
            Section::Submodules => "submodules",
        }
    }
}

/// Petición al historial de seleccionar (y traer a la vista) un commit
/// (`commitSelectionRequest` del repositorio en el original).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct CommitSelectionRequest {
    /// Hash completo del commit.
    pub hash: String,
    /// La ref del sidebar que originó el salto, para marcarla como "la que
    /// estás viendo" sin confundirla con la rama activa. `None` si el salto
    /// vino de un tag o de la búsqueda.
    pub ref_name: Option<String>,
    /// Momento de la petición (ms desde la época): dos peticiones al mismo
    /// commit son distintas.
    pub requested_at: i64,
}

/// Lo que el sidebar pide a otras vistas.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SidebarEvent {
    /// El historial debe seleccionar este commit y traerlo a la vista (clic en
    /// una rama, un tag o un resultado de búsqueda). La vista de commits ya
    /// está activa cuando llega.
    CommitSelectionRequested {
        repo_id: String,
        request: CommitSelectionRequest,
    },
    /// Clic en un stash: abrir el diálogo con su contenido (lo implementa el
    /// crate del visor de diffs).
    OpenStashDiff {
        repo_path: String,
        stash: StashEntry,
    },
    /// Abrir la comparación navegable de dos revisiones ("Compare with
    /// current branch", "Browse changes" de Branch Comparison). La pinta
    /// `mergeon_view_commits::open_compare_view`.
    OpenComparison(ComparisonRequest),
}

/// Qué comparar: `head` frente a `base`, en `mode`. `base` y `head` son lo
/// que eligió el usuario (ramas, tags, SHAs); la vista los fija a su SHA al
/// abrirse.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ComparisonRequest {
    pub repo_id: String,
    pub repo_path: String,
    pub base: String,
    pub head: String,
    pub mode: CompareMode,
}

impl ComparisonRequest {
    pub fn new(
        repo_id: impl Into<String>,
        repo_path: impl Into<String>,
        base: impl Into<String>,
        head: impl Into<String>,
    ) -> Self {
        Self {
            repo_id: repo_id.into(),
            repo_path: repo_path.into(),
            base: base.into(),
            head: head.into(),
            mode: CompareMode::default(),
        }
    }

    pub fn mode(mut self, mode: CompareMode) -> Self {
        self.mode = mode;
        self
    }
}

/// Un resultado de la búsqueda de commits.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct CommitSearchResult {
    pub hash: String,
    pub full_hash: String,
    pub message: String,
    pub author: String,
}

/// Lo que el diálogo de checkout abierto va a ejecutar al confirmar
/// (`pendingCheckoutBranchCall` + `checkoutBranchTargetLabel`).
pub(crate) struct PendingCheckout {
    pub repo: RepoRef,
    pub label: String,
    pub target: CheckoutTarget,
}

/// Una fila de la sección Remotes: un remoto, una carpeta de ramas o una rama.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RemoteRow {
    Remote(usize),
    Folder {
        remote: usize,
        path: String,
        label: String,
        branch_count: usize,
        depth: usize,
        expanded: bool,
    },
    Branch {
        remote: usize,
        index: usize,
        label: String,
        depth: usize,
    },
}

/// Lo que el sidebar pinta del repositorio activo: una copia que se hace una
/// vez por cambio de esos datos, no en cada frame, y que las filas comparten
/// por `Rc`. Trae ya calculado lo que cada fila consultaba (nombres en
/// minúsculas para el filtro, qué ramas están destacadas, los árboles de
/// Remotes).
pub(crate) struct SidebarData {
    pub repo_id: String,
    pub branches: Vec<BranchItem>,
    /// `branches[i].name` en minúsculas.
    pub branch_names_lower: Vec<String>,
    /// Si `branches[i]` está destacada.
    pub branch_starred: Vec<bool>,
    pub starred_branches: Vec<String>,
    /// Si la destacada `i` sigue siendo una rama local (si no, no hay menú).
    pub starred_exists: Vec<bool>,
    pub remotes: Vec<RemoteItem>,
    /// Árbol de ramas de cada remoto (en el mismo orden que `remotes`), con el
    /// índice de cada rama dentro de `remotes[remote].branches`.
    pub remote_trees: Vec<Vec<BranchTreeNode<usize>>>,
    pub tags: Vec<String>,
    pub stashes: Vec<StashEntry>,
    pub submodules: Vec<Submodule>,
    pub submodules_loading: bool,
    /// Secciones que se están leyendo por primera vez (vacías y cargando):
    /// muestran "Loading…" y no un 0 que luego salta.
    pub first_load: SidebarFlags,
    /// El tamaño de letra base con el que se calcularon los altos.
    rem: Pixels,
}

impl SidebarData {
    fn new(repository: &OpenRepository, rem: Pixels) -> Self {
        let starred: HashSet<&str> = repository
            .starred_branches
            .iter()
            .map(String::as_str)
            .collect();
        let local: HashSet<&str> = repository
            .branches
            .iter()
            .map(|branch| branch.name.as_str())
            .collect();
        let remote_trees = repository
            .remotes
            .iter()
            .map(|remote| {
                build_branch_tree(
                    (0..remote.branches.len()).collect(),
                    |&ix| remote.branches[ix].as_str(),
                    |_| false,
                )
            })
            .collect();
        Self {
            repo_id: repository.id.clone(),
            branch_names_lower: repository
                .branches
                .iter()
                .map(|branch| branch.name.to_lowercase())
                .collect(),
            branch_starred: repository
                .branches
                .iter()
                .map(|branch| starred.contains(branch.name.as_str()))
                .collect(),
            starred_exists: repository
                .starred_branches
                .iter()
                .map(|name| local.contains(name.as_str()))
                .collect(),
            branches: repository.branches.clone(),
            starred_branches: repository.starred_branches.clone(),
            remotes: repository.remotes.clone(),
            remote_trees,
            tags: repository.tags.clone(),
            stashes: repository.stashes.clone(),
            submodules: repository.submodules.clone(),
            submodules_loading: repository.sidebar_loading.submodules,
            first_load: first_load(repository),
            rem,
        }
    }

    /// El repositorio sigue diciendo lo mismo que esta copia.
    fn matches(&self, repository: &OpenRepository) -> bool {
        self.repo_id == repository.id
            && self.submodules_loading == repository.sidebar_loading.submodules
            && self.first_load == first_load(repository)
            && self.branches == repository.branches
            && self.starred_branches == repository.starred_branches
            && self.remotes == repository.remotes
            && self.tags == repository.tags
            && self.stashes == repository.stashes
            && self.submodules == repository.submodules
    }

    /// Índices de las ramas locales cuyo nombre contiene `query` (ya en
    /// minúsculas): `filteredBranches`.
    fn filter_branches(&self, query: &str) -> Vec<usize> {
        if query.is_empty() {
            return (0..self.branches.len()).collect();
        }
        self.branch_names_lower
            .iter()
            .enumerate()
            .filter(|(_, name)| name.contains(query))
            .map(|(ix, _)| ix)
            .collect()
    }
}

/// Las secciones vacías que se están leyendo (la primera lectura).
fn first_load(repository: &OpenRepository) -> SidebarFlags {
    let loading = repository.sidebar_loading;
    SidebarFlags {
        branches: loading.branches && repository.branches.is_empty(),
        remotes: loading.remotes && repository.remotes.is_empty(),
        tags: loading.tags && repository.tags.is_empty(),
        stashes: loading.stashes && repository.stashes.is_empty(),
        ..SidebarFlags::default()
    }
}

/// Lo que el sidebar pinta del modelo fuera de [`SidebarData`]. Se recoge en
/// `model_changed` y `render` pinta solo desde aquí (y desde los datos), sin
/// leer el modelo: si no cambia (ni los datos), un cambio del modelo (un
/// aviso, archivos en stage, las preferencias del diff...) no repinta el
/// sidebar.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RenderKey {
    pub(crate) repo_id: Option<String>,
    pub(crate) active_view: WorkspaceSection,
    pub(crate) width: f64,
    /// Arrastrando el divisor ([`crate::SidebarResizer`]).
    pub(crate) resizing: bool,
    pub(crate) sections: mergeon_store::SidebarSections,
    pub(crate) current_branch: Option<String>,
    pub(crate) detached_head_ref: Option<String>,
    pub(crate) can_mutate: bool,
    pub(crate) loading: SidebarFlags,
    pub(crate) refreshing: bool,
}

impl RenderKey {
    fn read(model: &AppModel) -> Self {
        let repository = model.active_repository();
        let width = model.sidebar_width();
        Self {
            repo_id: repository.map(|repository| repository.id.clone()),
            active_view: model.active_view(),
            width: width.width,
            resizing: width.resizing,
            sections: model.sidebar_sections().clone(),
            current_branch: repository.and_then(|repository| repository.current_branch.clone()),
            detached_head_ref: repository
                .and_then(|repository| repository.detached_head_ref.clone()),
            can_mutate: !repository
                .and_then(|repository| repository.operation_state.as_ref())
                .is_some_and(|state| state.in_progress),
            loading: repository
                .map(|repository| repository.sidebar_loading)
                .unwrap_or_default(),
            refreshing: repository.is_some_and(|repository| {
                model.is_single_flight_running(&AppModel::get_repo_operation_key(
                    &repository.id,
                    "sidebar-refresh",
                ))
            }),
        }
    }

    fn section_expanded(&self, section: Section) -> bool {
        section_expanded_in(&self.sections, section)
    }
}

fn section_expanded_in(sections: &mergeon_store::SidebarSections, section: Section) -> bool {
    match section {
        Section::Starred => sections.starred,
        Section::Branches => sections.branches,
        Section::Remotes => sections.remotes,
        Section::Tags => sections.tags,
        Section::Stashes => sections.stashes,
        Section::Submodules => sections.submodules,
    }
}

/// Las ramas que deja pasar el filtro y su árbol, recordados por datos y texto.
struct FilterCache {
    data: Rc<SidebarData>,
    query: String,
    rows: Rc<[usize]>,
    tree: Rc<[BranchTreeNode<usize>]>,
}

/// Filas visibles de la sección Branches (carpetas y ramas), recordadas por
/// árbol, estado del filtro y versión de carpetas plegadas.
struct BranchRowsCache {
    tree: Rc<[BranchTreeNode<usize>]>,
    filter_active: bool,
    folder_version: u64,
    rows: Rc<[BranchTreeRow<usize>]>,
}

/// Filas visibles de la sección Remotes y sus altos, recordados por datos y
/// versión de carpetas remotas plegadas.
struct RemoteRowsCache {
    data: Rc<SidebarData>,
    folder_version: u64,
    rows: Rc<[RemoteRow]>,
    heights: RowHeights,
}

/// Lo que necesita un frame: los datos compartidos y lo poco que cambia sin
/// que cambien ellos (la rama actual, la ref que se está viendo).
pub(crate) struct RepoSnapshot {
    pub data: Rc<SidebarData>,
    /// Índices en `data.branches` de las ramas que deja pasar el filtro.
    pub filtered_branches: Rc<[usize]>,
    /// Filas visibles de Branches (carpetas y ramas).
    pub branch_rows: Rc<[BranchTreeRow<usize>]>,
    /// Filas visibles de Remotes (remotos, carpetas y ramas).
    pub remote_rows: Rc<[RemoteRow]>,
    /// Alto de las filas de Remotes: cada remoto con sus ramas forma un grupo
    /// con 0.075rem de aire debajo.
    pub remote_heights: RowHeights,
    pub current_branch: String,
    pub detached_head_label: String,
    pub viewed_ref: Option<String>,
    /// Sin operación a medias (merge, rebase...): se puede mutar.
    pub can_mutate: bool,
    /// El filtro de ramas tiene texto.
    pub filter_active: bool,
}

/// El sidebar del repositorio activo. Lee todo de [`AppModel`]; lo suyo es el
/// estado visual: el filtro, el panel a la vista, la búsqueda y los diálogos
/// abiertos.
pub struct Sidebar {
    pub(crate) model: Entity<AppModel>,
    pub(crate) filter: Entity<InputState>,
    pub(crate) search_query: Entity<InputState>,
    pub(crate) panel_mode: PanelMode,
    pub(crate) search_results: Vec<CommitSearchResult>,
    /// La lista virtual de los resultados.
    search_list: ListState,
    pub(crate) search_loading: bool,
    pub(crate) search_repo_id: Option<String>,
    /// Token de secuencia: una respuesta que llega tarde no pisa a la más
    /// reciente.
    pub(crate) search_token: u64,
    search_debounce: Option<Task<()>>,
    /// Campos de "More filters" y su estado.
    pub(crate) search_filters: SearchFilterInputs,
    /// La última página vino llena: puede haber más resultados.
    pub(crate) search_has_more: bool,
    pub(crate) search_loading_more: bool,
    /// El fallo de la última búsqueda (una rama que no existe...), en línea.
    pub(crate) search_error: Option<String>,
    /// El fallo de "Load more": los resultados ya cargados siguen a la vista.
    pub(crate) search_more_error: Option<String>,
    /// Las opciones de los resultados a la vista (para "Load more").
    pub(crate) search_options: Option<CommitSearchOptions>,
    pub(crate) dragged_branch: Option<String>,
    /// Dónde se hizo el último clic derecho. Mientras el puntero siga ahí, las
    /// filas no muestran su tooltip: saldría encima del menú contextual que se
    /// acaba de abrir en ese punto.
    menu_anchor: Option<Point<Pixels>>,
    pub(crate) row_tooltips: bool,
    /// Número de la última navegación a una ref: una resolución que llega
    /// tarde (otra navegación, otro repositorio) se descarta.
    navigate_token: u64,
    selection_requests: HashMap<String, CommitSelectionRequest>,
    /// Repositorio → hash del resultado de búsqueda elegido (resaltado hasta
    /// que se elige otro commit a mano).
    search_highlight: HashMap<String, String>,
    /// Lo último que se vio del modelo, para reaccionar solo a los cambios
    /// (los `watch` del original).
    last_repo_id: Option<String>,
    last_active_view: WorkspaceSection,
    pub(crate) branch_dialog: Option<Entity<BranchActionDialog>>,
    pub(crate) checkout_dialog: Option<(Entity<CheckoutBranchDialog>, PendingCheckout)>,
    pub(crate) create_branch_dialog: Option<(Entity<CreateBranchDialog>, RepoRef, String)>,
    pub(crate) add_remote_dialog: Option<Entity<AddRemoteDialog>>,
    pub(crate) tag_details_dialog: Option<Entity<TagDetailsDialog>>,
    pub(crate) cleanup_dialog: Option<Entity<CleanupBranchesDialog>>,
    /// La suscripción al diálogo abierto de cada tipo; abrir otro la reemplaza.
    pub(crate) branch_dialog_subscription: Option<Subscription>,
    pub(crate) checkout_dialog_subscription: Option<Subscription>,
    pub(crate) create_branch_dialog_subscription: Option<Subscription>,
    pub(crate) add_remote_dialog_subscription: Option<Subscription>,
    /// Dónde quedó pintado, para el divisor de [`crate::SidebarResizer`].
    pub(crate) bounds: Rc<Cell<Bounds<Pixels>>>,
    pub(crate) container_width: Option<Pixels>,
    scroll_handle: ScrollHandle,
    /// La copia de los datos del repositorio activo que se está pintando.
    /// `model_changed` la rehace si dejó de coincidir con el modelo.
    data: Option<Rc<SidebarData>>,
    /// Lo que se pinta del modelo fuera de los datos (ver [`RenderKey`]).
    render_key: RenderKey,
    /// Cuántas veces se pintó (tests).
    renders: usize,
    filter_cache: Option<FilterCache>,
    /// Carpetas plegadas en Branches (ruta completa, p. ej. `"feature/auth"`).
    /// Las carpetas arrancan desplegadas.
    pub(crate) collapsed_branch_folders: HashSet<String>,
    /// Carpetas plegadas en Remotes, por `(remoto, ruta)`.
    pub(crate) collapsed_remote_branch_folders: HashSet<(String, String)>,
    branch_folder_version: u64,
    remote_folder_version: u64,
    branch_rows_cache: Option<BranchRowsCache>,
    remote_rows_cache: Option<RemoteRowsCache>,
    /// Cuántas veces se copiaron los datos o se filtraron las ramas (tests).
    data_builds: usize,
    filter_builds: usize,
    /// La sección "Pull requests" (`pull_requests.rs`).
    pull_requests: Entity<crate::pull_requests::PullRequestsSection>,
    /// La sección Worktrees y qué ramas están en otro worktree.
    pub(crate) worktrees: crate::worktrees::WorktreeState,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<SidebarEvent> for Sidebar {}

impl Sidebar {
    /// Crea el sidebar sobre el [`AppModel`] global y carga lo que el original
    /// leía al montarse: qué secciones están desplegadas y el ancho guardado.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let model = AppModel::global(cx);
        // Filtra solo las ramas locales: el texto lo dice.
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter branches"));
        let search_query =
            cx.new(|cx| InputState::new(window, cx).placeholder("Message, author:, path:, code:…"));
        let search_filters = SearchFilterInputs::new(window, cx);

        let subscriptions = vec![
            cx.observe_in(&model, window, |this, _, window, cx| {
                this.model_changed(window, cx);
            }),
            cx.subscribe_in(&model, window, |this, _, event: &AppEvent, window, cx| {
                // Al abrir o restaurar un repositorio, su hidratación invalida
                // las lecturas en vuelo: tags, stashes o submódulos desplegados
                // quedaban vacíos hasta un refresco manual. Con cada mutación
                // del repositorio activo se reintenta; las secciones ya
                // cargadas se saltan solas. (La mutación no avisa por
                // `observe`: una relectura que no cambia nada no repinta.)
                if let AppEvent::MutationRecorded { repo_id, .. } = event
                    && this.last_repo_id.as_deref() == Some(repo_id.as_str())
                {
                    this.load_expanded_sections(cx);
                }
                // Apertura externa de la búsqueda (atajo Cmd/Ctrl+F).
                if matches!(event, AppEvent::FocusCommitSearch) {
                    this.show_search_panel(window, cx);
                    let query = this.search_query.clone();
                    query.update(cx, |state, cx| state.focus(window, cx));
                }
            }),
            cx.subscribe(&filter, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }),
            cx.subscribe(
                &search_query,
                |this, _, event: &InputEvent, cx| match event {
                    InputEvent::Change => this.search_query_changed(cx),
                    // Enter busca ya, sin esperar a la pausa de tecleo.
                    InputEvent::PressEnter { .. } => this.run_commit_search(cx).detach(),
                    _ => {}
                },
            ),
        ];
        let mut subscriptions = subscriptions;
        // Si la ventana pierde el foco a mitad de un arrastre, la rama no se
        // queda atenuada.
        subscriptions.push(cx.observe_window_activation(window, |this, window, cx| {
            if !window.is_window_active() && this.dragged_branch.take().is_some() {
                cx.notify();
            }
        }));
        for input in search_filters.inputs() {
            subscriptions.push(cx.subscribe(&input, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.search_query_changed(cx);
                }
            }));
        }

        let (last_repo_id, last_active_view, render_key) = {
            let model = model.read(cx);
            (
                model.active_repository().map(|repo| repo.id.clone()),
                model.active_view(),
                RenderKey::read(model),
            )
        };

        let mut this = Self {
            model: model.clone(),
            filter,
            search_query,
            panel_mode: PanelMode::Repository,
            search_results: Vec::new(),
            search_list: ListState::new(0, ListAlignment::Top, px(SEARCH_RESULTS_OVERDRAW)),
            search_loading: false,
            search_repo_id: None,
            search_token: 0,
            search_debounce: None,
            search_filters,
            search_has_more: false,
            search_loading_more: false,
            search_error: None,
            search_more_error: None,
            search_options: None,
            dragged_branch: None,
            menu_anchor: None,
            row_tooltips: true,
            navigate_token: 0,
            selection_requests: HashMap::new(),
            search_highlight: HashMap::new(),
            last_repo_id,
            last_active_view,
            branch_dialog: None,
            checkout_dialog: None,
            create_branch_dialog: None,
            add_remote_dialog: None,
            tag_details_dialog: None,
            cleanup_dialog: None,
            branch_dialog_subscription: None,
            checkout_dialog_subscription: None,
            create_branch_dialog_subscription: None,
            add_remote_dialog_subscription: None,
            bounds: Rc::new(Cell::new(Bounds::default())),
            container_width: None,
            scroll_handle: ScrollHandle::new(),
            data: None,
            render_key,
            renders: 0,
            filter_cache: None,
            collapsed_branch_folders: HashSet::new(),
            collapsed_remote_branch_folders: HashSet::new(),
            branch_folder_version: 0,
            remote_folder_version: 0,
            branch_rows_cache: None,
            remote_rows_cache: None,
            data_builds: 0,
            filter_builds: 0,
            pull_requests: {
                let sidebar = cx.weak_entity();
                cx.new(|cx| crate::pull_requests::PullRequestsSection::new(sidebar, cx))
            },
            worktrees: Default::default(),
            _subscriptions: subscriptions,
        };

        this.sync_data(window, cx);
        // Un aviso del modelo que ya estuviera en cola al suscribirse no llega
        // por `observe`: se mira otra vez en cuanto termine este ciclo.
        cx.defer_in(window, |this, window, cx| this.model_changed(window, cx));

        // `useSidebarWidth()` y `onMounted`.
        model
            .update(cx, |model, cx| model.load_sidebar_width(cx))
            .detach();
        let sections = model.update(cx, |model, cx| model.load_sidebar_sections(cx));
        cx.spawn(async move |this, cx| {
            sections.await;
            this.update(cx, |this, cx| this.load_expanded_sections(cx))
                .ok();
        })
        .detach();

        this
    }

    // ------------------------------------------------------------------
    // Lectura
    // ------------------------------------------------------------------

    pub fn panel_mode(&self) -> PanelMode {
        self.panel_mode
    }

    /// El campo del filtro de ramas.
    pub fn filter_input(&self) -> &Entity<InputState> {
        &self.filter
    }

    /// El campo de la búsqueda de commits.
    pub fn search_input(&self) -> &Entity<InputState> {
        &self.search_query
    }

    pub fn filter_text(&self, cx: &App) -> String {
        self.filter.read(cx).value().to_string()
    }

    /// Escribe en el filtro de ramas, como si lo tecleara el usuario.
    pub fn set_filter_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.filter.update(cx, |state, cx| {
            state.set_value(text.to_string(), window, cx)
        });
        cx.notify();
    }

    /// Escribe la búsqueda de commits y la lanza sin esperar a la pausa de
    /// tecleo.
    pub fn set_search_query(&mut self, query: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.search_query.update(cx, |state, cx| {
            state.set_value(query.to_string(), window, cx)
        });
        self.run_commit_search(cx).detach();
    }

    pub fn search_results(&self) -> &[CommitSearchResult] {
        &self.search_results
    }

    /// Cambia los resultados de la búsqueda y se lo cuenta a su lista.
    pub(crate) fn set_search_results(&mut self, results: Vec<CommitSearchResult>) {
        if results.is_empty() && self.search_results.is_empty() {
            return;
        }
        self.search_results = results;
        self.search_list.reset(self.search_results.len());
        // Hasta medirlos, cada resultado cuenta como uno de una línea.
        self.search_list
            .clone()
            .with_uniform_item_height(px(ESTIMATED_SEARCH_RESULT_HEIGHT));
    }

    /// Añade la página siguiente a los resultados sin mover la lista.
    pub(crate) fn append_search_results(&mut self, results: Vec<CommitSearchResult>) {
        let start = self.search_results.len();
        let added = results.len();
        self.search_results.extend(results);
        self.search_list.splice(start..start, added);
    }

    pub fn is_search_loading(&self) -> bool {
        self.search_loading
    }

    /// El diálogo Branch Actions abierto, si lo hay.
    pub fn branch_action_dialog(&self) -> Option<&Entity<BranchActionDialog>> {
        self.branch_dialog.as_ref()
    }

    /// El diálogo de checkout abierto, si lo hay.
    pub fn checkout_dialog(&self) -> Option<&Entity<CheckoutBranchDialog>> {
        self.checkout_dialog.as_ref().map(|(dialog, _)| dialog)
    }

    /// El diálogo «Create Branch» abierto, si lo hay.
    pub fn create_branch_dialog(&self) -> Option<&Entity<CreateBranchDialog>> {
        self.create_branch_dialog
            .as_ref()
            .map(|(dialog, _, _)| dialog)
    }

    /// El diálogo «Remote» abierto, si lo hay.
    pub fn add_remote_dialog(&self) -> Option<&Entity<AddRemoteDialog>> {
        self.add_remote_dialog.as_ref()
    }

    /// El último diálogo de detalles de tag que se abrió.
    pub fn tag_details_dialog(&self) -> Option<&Entity<TagDetailsDialog>> {
        self.tag_details_dialog.as_ref()
    }

    /// El diálogo "Clean up merged branches", si se abrió.
    pub fn cleanup_branches_dialog(&self) -> Option<&Entity<CleanupBranchesDialog>> {
        self.cleanup_dialog.as_ref()
    }

    /// La última petición de selección de commit de un repositorio.
    pub fn commit_selection_request(&self, repo_id: &str) -> Option<&CommitSelectionRequest> {
        self.selection_requests.get(repo_id)
    }

    /// `viewedRefName`: la ref del sidebar cuyo tip se está viendo en el
    /// historial del repositorio.
    pub fn viewed_ref(&self, repo_id: &str) -> Option<&str> {
        self.selection_requests
            .get(repo_id)
            .and_then(|request| request.ref_name.as_deref())
    }

    /// La vista de commits lo llama cuando el usuario elige otro commit a mano
    /// (`handleHistorySelect`): la selección ya no corresponde al salto que la
    /// originó y la marca "estás viendo esta rama" se retira.
    /// También apaga el resultado de búsqueda resaltado: la selección ya no
    /// es la de ese salto.
    pub fn clear_viewed_ref(&mut self, repo_id: &str, cx: &mut Context<Self>) {
        let mut changed = self.search_highlight.remove(repo_id).is_some();
        if let Some(request) = self.selection_requests.get_mut(repo_id) {
            changed |= request.ref_name.take().is_some();
        }
        if changed {
            cx.notify();
        }
    }

    /// El resultado de búsqueda resaltado (el último elegido) del repositorio.
    pub fn search_highlight(&self, repo_id: &str) -> Option<&str> {
        self.search_highlight.get(repo_id).map(String::as_str)
    }

    /// Ancho del contenedor que comparten el sidebar y la vista de commits,
    /// para que el divisor no la aplaste. Sin él se usa lo que queda de la
    /// ventana a la derecha del borde izquierdo del sidebar.
    pub fn set_container_width(&mut self, width: Option<Pixels>, cx: &mut Context<Self>) {
        self.container_width = width;
        cx.notify();
    }

    pub(crate) fn active_repo(&self, cx: &App) -> Option<RepoRef> {
        self.model
            .read(cx)
            .active_repository()
            .map(|repository| RepoRef {
                id: repository.id.clone(),
                path: repository.path.clone(),
            })
    }

    /// `showCommitSearchPanel`.
    pub fn is_search_panel_visible(&self, cx: &App) -> bool {
        self.panel_mode == PanelMode::Search
            && self.model.read(cx).active_view() == WorkspaceSection::Commits
    }

    /// Como [`Self::is_search_panel_visible`], con lo ya recogido del modelo
    /// (para pintar).
    fn search_panel_shown(&self) -> bool {
        self.panel_mode == PanelMode::Search
            && self.render_key.active_view == WorkspaceSection::Commits
    }

    /// `isBranchOperationRunning`.
    pub(crate) fn is_operation_running(&self, repo_id: &str, operation: &str, cx: &App) -> bool {
        self.model
            .read(cx)
            .is_single_flight_running(&AppModel::get_repo_operation_key(repo_id, operation))
    }

    /// `sidebarRefreshRunning`.
    pub fn is_refreshing(&self, cx: &App) -> bool {
        self.active_repo(cx)
            .is_some_and(|repo| self.is_operation_running(&repo.id, "sidebar-refresh", cx))
    }

    /// Lo que pinta este frame, sin leer el modelo: los datos y lo demás se
    /// recogieron en `model_changed`. Las ramas se filtran solo si cambiaron
    /// ellas o el texto del filtro: un frame de hover o de scroll no copia ni
    /// recorre nada.
    pub(crate) fn snapshot(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Rc<RepoSnapshot>> {
        let data = self.data.clone()?;
        // Cambió el tamaño de letra (zoom): los altos de fila dependen de él.
        // Los datos se rehacen fuera del pintado; este frame usa los de antes.
        if data.rem != window.rem_size() {
            cx.defer_in(window, |this, window, cx| {
                if this.sync_data(window, cx) {
                    cx.notify();
                }
            });
        }

        let query = self.filter.read(cx).value().to_lowercase();
        let filter_active = !query.is_empty();
        let (filtered_branches, branch_tree) = match &self.filter_cache {
            Some(cache) if Rc::ptr_eq(&cache.data, &data) && cache.query == query => {
                (cache.rows.clone(), cache.tree.clone())
            }
            _ => {
                let filtered = data.filter_branches(&query);
                let tree: Rc<[BranchTreeNode<usize>]> = build_branch_tree(
                    filtered.clone(),
                    |&ix| data.branches[ix].name.as_str(),
                    |&ix| data.branches[ix].active,
                )
                .into();
                let rows: Rc<[usize]> = filtered.into();
                self.filter_builds += 1;
                self.filter_cache = Some(FilterCache {
                    data: data.clone(),
                    query,
                    rows: rows.clone(),
                    tree: tree.clone(),
                });
                (rows, tree)
            }
        };

        let branch_rows = match &self.branch_rows_cache {
            Some(cache)
                if Rc::ptr_eq(&cache.tree, &branch_tree)
                    && cache.filter_active == filter_active
                    && cache.folder_version == self.branch_folder_version =>
            {
                cache.rows.clone()
            }
            _ => {
                let rows: Rc<[BranchTreeRow<usize>]> = flatten_branch_tree(&branch_tree, |path| {
                    filter_active || !self.collapsed_branch_folders.contains(path)
                })
                .into();
                self.branch_rows_cache = Some(BranchRowsCache {
                    tree: branch_tree,
                    filter_active,
                    folder_version: self.branch_folder_version,
                    rows: rows.clone(),
                });
                rows
            }
        };

        let (remote_rows, remote_heights) = match &self.remote_rows_cache {
            Some(cache)
                if Rc::ptr_eq(&cache.data, &data)
                    && cache.folder_version == self.remote_folder_version =>
            {
                (cache.rows.clone(), cache.heights.clone())
            }
            _ => {
                let mut rows = Vec::new();
                for (remote_ix, (remote, tree)) in data
                    .remotes
                    .iter()
                    .zip(data.remote_trees.iter())
                    .enumerate()
                {
                    rows.push(RemoteRow::Remote(remote_ix));
                    let remote_name = remote.name.as_str();
                    let collapsed = &self.collapsed_remote_branch_folders;
                    let flat = flatten_branch_tree(tree, |path| {
                        collapsed.is_empty()
                            || !collapsed.contains(&(remote_name.to_string(), path.to_string()))
                    });
                    for item in flat {
                        match item {
                            BranchTreeRow::Folder {
                                path,
                                label,
                                branch_count,
                                depth,
                                expanded,
                                ..
                            } => rows.push(RemoteRow::Folder {
                                remote: remote_ix,
                                path,
                                label,
                                branch_count,
                                depth,
                                expanded,
                            }),
                            BranchTreeRow::Branch {
                                label,
                                depth,
                                item: index,
                            } => rows.push(RemoteRow::Branch {
                                remote: remote_ix,
                                index,
                                label,
                                depth,
                            }),
                        }
                    }
                }
                let row = f32::from(section::row_height(data.rem));
                let gap = f32::from(data.rem) * 0.075;
                let heights = RowHeights::from_extents(rows.iter().enumerate().map(|(ix, _)| {
                    let ends_group = matches!(rows.get(ix + 1), None | Some(RemoteRow::Remote(_)));
                    (row, if ends_group { gap } else { 0. })
                }));
                let rows: Rc<[RemoteRow]> = rows.into();
                self.remote_rows_cache = Some(RemoteRowsCache {
                    data: data.clone(),
                    folder_version: self.remote_folder_version,
                    rows: rows.clone(),
                    heights: heights.clone(),
                });
                (rows, heights)
            }
        };

        let key = &self.render_key;
        let request = self.selection_requests.get(&data.repo_id);
        Some(Rc::new(RepoSnapshot {
            filtered_branches,
            branch_rows,
            remote_rows,
            remote_heights,
            current_branch: key.current_branch.clone().unwrap_or_default(),
            detached_head_label: detached_head_label(
                key.current_branch.as_deref(),
                key.detached_head_ref.as_deref(),
            ),
            viewed_ref: request.and_then(|request| request.ref_name.clone()),
            can_mutate: key.can_mutate,
            filter_active,
            data,
        }))
    }

    /// Rehace la copia de los datos del repositorio activo si dejó de
    /// coincidir con el modelo (o con el tamaño de letra). Devuelve si cambió.
    fn sync_data(&mut self, window: &Window, cx: &App) -> bool {
        let rem = window.rem_size();
        let model = self.model.read(cx);
        let repository = model.active_repository();
        let stale = match (repository, &self.data) {
            (Some(repository), Some(data)) => {
                data.repo_id != repository.id || data.rem != rem || !data.matches(repository)
            }
            (None, None) => false,
            _ => true,
        };
        if !stale {
            return false;
        }
        self.data = repository.map(|repository| Rc::new(SidebarData::new(repository, rem)));
        if self.data.is_some() {
            self.data_builds += 1;
        } else {
            self.filter_cache = None;
            self.branch_rows_cache = None;
            self.remote_rows_cache = None;
        }
        true
    }

    /// Lo que se pinta del modelo fuera de los datos, ya recogido.
    pub(crate) fn render_key(&self) -> &RenderKey {
        &self.render_key
    }

    /// Vuelve a recoger del modelo lo que se pinta fuera de los datos. Para
    /// cambios que el modelo no avisa (el ancho a cada píxel de un arrastre
    /// del divisor). Devuelve si cambió.
    pub(crate) fn sync_render_key(&mut self, cx: &App) -> bool {
        let key = RenderKey::read(self.model.read(cx));
        if self.render_key == key {
            return false;
        }
        let branch_changed = self.render_key.current_branch != key.current_branch;
        self.render_key = key;
        if branch_changed && let Some(current) = self.render_key.current_branch.clone() {
            self.expand_branch_ancestors(&current);
        }
        true
    }

    /// Cuántas veces se pintó el sidebar.
    #[doc(hidden)]
    pub fn render_count(&self) -> usize {
        self.renders
    }

    /// La sección "Pull requests".
    pub fn pull_requests_section(&self) -> &Entity<crate::pull_requests::PullRequestsSection> {
        &self.pull_requests
    }

    /// Cuántas veces se copiaron los datos del repositorio para pintarlos.
    #[doc(hidden)]
    pub fn data_builds(&self) -> usize {
        self.data_builds
    }

    /// Cuántas veces se filtró la lista de ramas.
    #[doc(hidden)]
    pub fn filter_builds(&self) -> usize {
        self.filter_builds
    }

    // ------------------------------------------------------------------
    // Cambios del modelo (los `watch` del original)
    // ------------------------------------------------------------------

    fn model_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (repo_id, active_view, open_ids) = {
            let model = self.model.read(cx);
            (
                model.active_repository().map(|repo| repo.id.clone()),
                model.active_view(),
                model
                    .open_repositories()
                    .iter()
                    .map(|repo| repo.id.clone())
                    .collect::<Vec<_>>(),
            )
        };

        // Lo pedido para un repositorio que ya se cerró no se guarda.
        self.selection_requests
            .retain(|repo_id, _| open_ids.contains(repo_id));
        self.search_highlight
            .retain(|repo_id, _| open_ids.contains(repo_id));

        let mut changed = false;
        if repo_id != self.last_repo_id {
            let previous = std::mem::replace(&mut self.last_repo_id, repo_id.clone());
            self.active_repository_changed(previous, repo_id.is_some(), window, cx);
            changed = true;
        }

        // Al volver a Commits el panel siempre es el del repositorio (al salir
        // se cerró la búsqueda): no hay búsqueda que repetir.
        if active_view != self.last_active_view {
            self.last_active_view = active_view;
            if active_view != WorkspaceSection::Commits {
                self.panel_mode = PanelMode::Repository;
            }
            changed = true;
        }

        // Los datos se comparan (y, si cambiaron, se copian) una vez aquí:
        // `render` pinta desde la copia sin volver a leer el modelo.
        let data_changed = self.sync_data(window, cx);
        changed |= self.sync_render_key(cx);

        if changed || data_changed {
            cx.notify();
        }
    }

    /// `watch(() => activeRepo.value?.id)`.
    fn active_repository_changed(
        &mut self,
        previous: Option<String>,
        has_repository: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !has_repository && let Some(previous) = previous {
            self.model.update(cx, |model, cx| {
                model.invalidate_repository_sidebar_data(previous.as_str(), cx);
            });
        }

        // En el original cada repositorio monta su propio sidebar: al cambiar de
        // pestaña el filtro, la búsqueda y los diálogos empiezan de cero. Todos
        // los diálogos: uno que siguiera abierto actuaría sobre el otro
        // repositorio.
        self.dismiss_dialogs(window, cx);
        self.reset_commit_search(window, cx);
        self.search_filters.open = false;
        self.filter
            .update(cx, |state, cx| state.set_value("", window, cx));
        self.panel_mode = PanelMode::Repository;
        self.dragged_branch = None;
        self.collapsed_branch_folders.clear();
        self.collapsed_remote_branch_folders.clear();
        self.branch_folder_version = self.branch_folder_version.wrapping_add(1);
        self.remote_folder_version = self.remote_folder_version.wrapping_add(1);
        self.branch_rows_cache = None;
        self.remote_rows_cache = None;
        // Una navegación en vuelo era del repositorio anterior.
        self.navigate_token += 1;
        self.scroll_handle.set_offset(point(px(0.), px(0.)));

        if has_repository {
            self.load_expanded_sections(cx);
        } else {
            self.worktrees = Default::default();
        }
    }

    /// Cierra todos los diálogos del sidebar abiertos.
    pub(crate) fn dismiss_dialogs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(dialog) = self.branch_dialog.take() {
            dialog.update(cx, |dialog, cx| dialog.dismiss(window, cx));
        }
        self.dismiss_checkout_dialog(window, cx);
        self.dismiss_create_branch_dialog(window, cx);
        if let Some(dialog) = self.add_remote_dialog.take() {
            dialog.update(cx, |dialog, cx| dialog.dismiss(window, cx));
        }
        self.dismiss_tag_details_dialog(window, cx);
        self.dismiss_cleanup_dialog(window, cx);
        self.dismiss_new_worktree_dialog(window, cx);
    }

    pub(crate) fn dismiss_checkout_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((dialog, _)) = self.checkout_dialog.take() {
            dialog.update(cx, |dialog, cx| dialog.dismiss(window, cx));
        }
    }

    pub(crate) fn dismiss_create_branch_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some((dialog, _, _)) = self.create_branch_dialog.take() {
            dialog.update(cx, |dialog, cx| dialog.dismiss(window, cx));
        }
    }

    pub(crate) fn dismiss_tag_details_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(dialog) = self.tag_details_dialog.take() {
            dialog.update(cx, |dialog, cx| dialog.dismiss(window, cx));
        }
    }

    pub(crate) fn dismiss_cleanup_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(dialog) = self.cleanup_dialog.take() {
            dialog.update(cx, |dialog, cx| dialog.dismiss(window, cx));
        }
    }

    pub(crate) fn dismiss_new_worktree_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some((dialog, _)) = self.worktrees.dialog.take() {
            dialog.update(cx, |dialog, cx| dialog.dismiss(window, cx));
        }
    }

    /// Las secciones recuerdan si estaban desplegadas. Una que se restaura ya
    /// desplegada (o tras cambiar de repositorio) todavía necesita sus datos: si
    /// no, se ve abierta pero vacía. Ramas y destacadas llegan con la apertura
    /// del repositorio; el resto se carga aquí. Tags, stashes y submódulos se
    /// leen aunque estén plegados (lecturas baratas): su cabecera muestra el
    /// número y con la sección sin leer decía 0.
    pub(crate) fn load_expanded_sections(&mut self, cx: &mut Context<Self>) {
        // La lista de worktrees es barata y marca las ramas ocupadas en otro
        // worktree: se lee aunque la sección esté plegada.
        self.load_worktrees(cx);
        let Some(repo) = self.active_repo(cx) else {
            return;
        };
        let sections = self.model.read(cx).sidebar_sections().clone();
        let request = SyncRequest::new()
            .remotes(sections.remotes)
            .tags(true)
            .stashes(true)
            .submodules(true);
        flows::ensure_loaded(&self.model, &repo.id, request, cx);
    }

    // ------------------------------------------------------------------
    // Secciones
    // ------------------------------------------------------------------

    pub fn is_section_expanded(&self, section: Section, cx: &App) -> bool {
        section_expanded_in(self.model.read(cx).sidebar_sections(), section)
    }

    /// Como [`Self::is_section_expanded`], con lo ya recogido del modelo
    /// (para pintar).
    pub(crate) fn section_shown_expanded(&self, section: Section) -> bool {
        self.render_key.section_expanded(section)
    }

    /// `toggleSection`: despliega o pliega, guarda el resultado y carga los
    /// datos de la sección que se acaba de abrir.
    pub fn toggle_section(&mut self, section: Section, cx: &mut Context<Self>) {
        let expanded = !self.is_section_expanded(section, cx);
        self.model
            .update(cx, |model, cx| {
                model.update_sidebar_sections(cx, |sections| match section {
                    Section::Starred => sections.starred = expanded,
                    Section::Branches => sections.branches = expanded,
                    Section::Remotes => sections.remotes = expanded,
                    Section::Tags => sections.tags = expanded,
                    Section::Stashes => sections.stashes = expanded,
                    Section::Submodules => sections.submodules = expanded,
                })
            })
            .detach();

        if !expanded {
            return;
        }
        let Some(repo) = self.active_repo(cx) else {
            return;
        };
        let request = match section {
            Section::Starred => SyncRequest::new().starred_branches(true),
            Section::Branches => SyncRequest::new().branches(true),
            Section::Remotes => SyncRequest::new().remotes(true),
            Section::Tags => SyncRequest::new().tags(true),
            Section::Stashes => SyncRequest::new().stashes(true),
            Section::Submodules => SyncRequest::new().submodules(true),
        };
        flows::ensure_loaded(&self.model, &repo.id, request, cx);
    }

    /// Pliega o despliega una carpeta de ramas locales (por su ruta completa).
    pub(crate) fn toggle_branch_folder(&mut self, path: &str, cx: &mut Context<Self>) {
        if !self.collapsed_branch_folders.remove(path) {
            self.collapsed_branch_folders.insert(path.to_string());
        }
        self.branch_folder_version = self.branch_folder_version.wrapping_add(1);
        cx.notify();
    }

    /// Pliega o despliega una carpeta de ramas dentro de un remoto.
    pub(crate) fn toggle_remote_branch_folder(
        &mut self,
        remote: &str,
        path: &str,
        cx: &mut Context<Self>,
    ) {
        let key = (remote.to_string(), path.to_string());
        if !self.collapsed_remote_branch_folders.remove(&key) {
            self.collapsed_remote_branch_folders.insert(key);
        }
        self.remote_folder_version = self.remote_folder_version.wrapping_add(1);
        cx.notify();
    }

    /// Asegura que las carpetas ancestro de `branch_name` estén desplegadas
    /// (p. ej. tras cambiar HEAD a una rama dentro de una carpeta plegada).
    fn expand_branch_ancestors(&mut self, branch_name: &str) -> bool {
        let mut changed = false;
        for ancestor in branch_ancestor_folders(branch_name) {
            if self.collapsed_branch_folders.remove(&ancestor) {
                changed = true;
            }
        }
        if changed {
            self.branch_folder_version = self.branch_folder_version.wrapping_add(1);
        }
        changed
    }

    // ------------------------------------------------------------------
    // Navegación y búsqueda
    // ------------------------------------------------------------------

    /// `handleShowRepositoryPanel`.
    pub fn show_repository_panel(&mut self, cx: &mut Context<Self>) {
        self.panel_mode = PanelMode::Repository;
        cx.notify();
    }

    /// `handleShowSearchPanel`: pasa a la vista de commits si hace falta y
    /// muestra la búsqueda.
    pub fn show_search_panel(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if self.model.read(cx).active_view() != WorkspaceSection::Commits {
            // El cambio de vista lo ve `model_changed`; se adelanta para que no
            // lo tome por un cambio ajeno.
            self.last_active_view = WorkspaceSection::Commits;
            self.model.update(cx, |model, cx| {
                model.handle_change_view(WorkspaceSection::Commits, cx);
            });
        }

        self.panel_mode = PanelMode::Search;
        cx.notify();
        self.run_commit_search(cx).detach();
    }

    /// `resetCommitSearchState`.
    fn reset_commit_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search_token += 1;
        self.search_debounce = None;
        self.search_query
            .update(cx, |state, cx| state.set_value("", window, cx));
        self.set_search_results(Vec::new());
        self.search_loading = false;
        self.search_repo_id = None;
        self.search_has_more = false;
        self.search_loading_more = false;
        self.search_error = None;
        self.search_more_error = None;
        self.search_options = None;
        self.reset_search_filters(window, cx);
        cx.notify();
    }

    /// `watch(commitSearchQuery)`: busca 300 ms después de la última tecla.
    fn search_query_changed(&mut self, cx: &mut Context<Self>) {
        cx.notify();
        if !self.is_search_panel_visible(cx) {
            return;
        }

        self.search_debounce = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(300))
                .await;
            // `run_commit_search` suelta esta tarea (la espera pendiente): la
            // búsqueda va aparte para no cancelarse con ella.
            this.update(cx, |this, cx| this.run_commit_search(cx).detach())
                .ok();
        }));
    }

    /// `runCommitSearch`: busca en el historial completo con la query actual.
    pub(crate) fn run_commit_search(&mut self, cx: &mut Context<Self>) -> Task<()> {
        let Some(repo) = self.active_repo(cx) else {
            return Task::ready(());
        };
        if self.model.read(cx).active_view() != WorkspaceSection::Commits {
            return Task::ready(());
        }

        // Una búsqueda inmediata (Enter, un filtro, un chip) deja sin efecto la
        // que esperaba a la pausa de tecleo.
        self.search_debounce = None;
        let options = self.current_search_options(cx);
        self.search_token += 1;
        let token = self.search_token;
        self.search_loading_more = false;
        self.search_more_error = None;

        let Some(options) = options else {
            self.set_search_results(Vec::new());
            self.search_loading = false;
            self.search_has_more = false;
            self.search_error = None;
            self.search_options = None;
            cx.notify();
            return Task::ready(());
        };

        self.search_loading = true;
        cx.notify();
        let search = self.model.update(cx, |model, cx| {
            model.search_commits(&repo.path, options.clone(), cx)
        });

        cx.spawn(async move |this, cx| {
            let result = search.await;
            this.update(cx, |this, cx| {
                if token != this.search_token {
                    return;
                }
                this.search_repo_id = Some(repo.id.clone());
                match result {
                    Ok(commits) => {
                        this.search_has_more = commits.len() >= SEARCH_PAGE_SIZE as usize;
                        this.set_search_results(search_results(commits));
                        this.search_error = None;
                    }
                    Err(error) => {
                        // En línea y no como aviso: suele ser un filtro a medio
                        // escribir (una rama que no existe), y sin el mensaje
                        // un fallo se confunde con "no hay resultados".
                        this.set_search_results(Vec::new());
                        this.search_has_more = false;
                        this.search_error = Some(if error.message().is_empty() {
                            "Failed to search commits".to_string()
                        } else {
                            error.message().to_string()
                        });
                    }
                }
                this.search_options = Some(options);
                this.search_loading = false;
                cx.notify();
            })
            .ok();
        })
    }

    /// `handleSidebarRefresh`: relee todas las secciones o, con la búsqueda a
    /// la vista, repite la búsqueda.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.active_repo(cx) else {
            return;
        };
        self.refresh_worktrees(cx);
        let search_visible = self.is_search_panel_visible(cx);
        let sidebar = cx.entity().downgrade();

        flows::single_flight(
            &self.model,
            &repo.clone(),
            "sidebar-refresh",
            cx,
            async move |model, cx| {
                if search_visible {
                    let search = sidebar.update(cx, |this, cx| this.run_commit_search(cx));
                    if let Ok(search) = search {
                        search.await;
                    }
                    return;
                }

                flows::refresh(
                    &model,
                    &repo,
                    SyncRequest::new()
                        .branches(true)
                        .remotes(true)
                        .tags(true)
                        .stashes(true)
                        .submodules(true)
                        .changes_count(true)
                        .starred_branches(true)
                        .operation("manual-refresh"),
                    cx,
                )
                .await;
            },
        )
        .detach();
    }

    /// `requestCommitFocus`: pide al historial seleccionar un commit concreto.
    /// `ref_name` guarda qué ref del sidebar originó el salto.
    pub(crate) fn request_commit_focus(
        &mut self,
        repo_id: &str,
        full_hash: &str,
        ref_name: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if full_hash.is_empty() {
            return;
        }

        self.last_active_view = WorkspaceSection::Commits;
        self.model.update(cx, |model, cx| {
            model.handle_change_view(WorkspaceSection::Commits, cx);
        });
        let request = CommitSelectionRequest {
            hash: full_hash.to_string(),
            ref_name,
            requested_at: now_epoch_ms(),
        };
        self.selection_requests
            .insert(repo_id.to_string(), request.clone());
        cx.emit(SidebarEvent::CommitSelectionRequested {
            repo_id: repo_id.to_string(),
            request,
        });
        cx.notify();
    }

    /// `handleCommitSearchSelect`.
    pub fn select_search_result(&mut self, full_hash: &str, cx: &mut Context<Self>) {
        if let Some(repo) = self.active_repo(cx) {
            self.search_highlight
                .insert(repo.id.clone(), full_hash.to_string());
            self.request_commit_focus(&repo.id, full_hash, None, cx);
        }
    }

    /// `navigateToRef`: resuelve la ref y pide al historial ir a su commit.
    pub(crate) fn navigate_to_ref(
        &mut self,
        reference: String,
        ref_name: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(repo) = self.active_repo(cx) else {
            return;
        };
        self.navigate_token += 1;
        let token = self.navigate_token;
        let resolve = self.model.update(cx, |model, cx| {
            model.resolve_ref(&repo.path, &reference, cx)
        });

        cx.spawn(async move |this, cx| {
            let resolved = resolve.await;
            this.update(cx, |this, cx| {
                // Otra navegación, otro repositorio o la búsqueda a la vista:
                // esta resolución ya no vale.
                let still_active = this
                    .active_repo(cx)
                    .is_some_and(|active| active.id == repo.id);
                if token != this.navigate_token
                    || !still_active
                    || this.panel_mode != PanelMode::Repository
                {
                    return;
                }
                this.navigated_to_ref(&repo, resolved, &reference, ref_name, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Lo que se hace con la ref resuelta: ir a su commit o avisar.
    fn navigated_to_ref(
        &mut self,
        repo: &RepoRef,
        resolved: mergeon_backend::QueryResult<String>,
        reference: &str,
        ref_name: Option<String>,
        cx: &mut Context<Self>,
    ) {
        match resolved {
            Ok(hash) => self.request_commit_focus(&repo.id, &hash, ref_name, cx),
            Err(error) => {
                let fallback = format!(
                    "Could not find the commit for '{}'",
                    ref_name.as_deref().unwrap_or(reference)
                );
                self.model.update(cx, |model, cx| {
                    let message = if error.message().is_empty() {
                        fallback.as_str()
                    } else {
                        error.message()
                    };
                    model.notify_error_with(message, &error, cx);
                });
            }
        }
    }

    // ------------------------------------------------------------------
    // Render
    // ------------------------------------------------------------------

    fn nav_button(
        &self,
        id: &'static str,
        icon: Lucide,
        tooltip: &'static str,
        active: bool,
        spin: bool,
        cx: &App,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        let palette = cx.palette();
        div()
            .id(id)
            .flex()
            .flex_1()
            .items_center()
            .justify_center()
            .h(px(26.))
            .rounded(Radius::Lg.px(cx))
            // Señal: el modo activo se invierte a tinta, como un segmentado.
            .map(|this| {
                if active {
                    this.bg(palette.sel_bg).text_color(palette.sel_fg)
                } else {
                    this.hover(|style| style.bg(palette.sidebar_row_hover))
                }
            })
            .tooltip(mergeon_ui::tooltip(tooltip))
            .child(
                mergeon_ui::icon(icon)
                    .size(px(14.))
                    .tone(if active {
                        IconTone::Current
                    } else {
                        IconTone::Muted
                    })
                    .spin(spin),
            )
    }

    /// `SidebarNavigation.vue`.
    fn render_navigation(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let refreshing = self.render_key.refreshing;
        let repository = self.panel_mode == PanelMode::Repository;
        let search = self.panel_mode == PanelMode::Search;

        div()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .child(
                div()
                    .flex()
                    .gap(rems(0.2))
                    .px(rems(0.45))
                    .py(rems(0.2))
                    .child(
                        self.nav_button(
                            "sidebar-show-repository",
                            Lucide::GitBranch,
                            "Repository sidebar",
                            repository,
                            false,
                            cx,
                        )
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, _, cx| this.show_repository_panel(cx)))
                        .test_support(),
                    )
                    .child(
                        self.nav_button(
                            "sidebar-show-search",
                            Lucide::Search,
                            "Search commits",
                            search,
                            false,
                            cx,
                        )
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.show_search_panel(window, cx);
                        }))
                        .test_support(),
                    )
                    .child(
                        // Refrescar es una acción, no un modo: mientras corre gira,
                        // pero no se rellena como una pestaña activa.
                        self.nav_button(
                            "sidebar-refresh",
                            if refreshing {
                                Lucide::LoaderCircle
                            } else {
                                Lucide::RefreshCcw
                            },
                            "Refresh",
                            false,
                            refreshing,
                            cx,
                        )
                        .map(|this| {
                            if refreshing {
                                this.cursor_default()
                            } else {
                                this.cursor_pointer()
                                    .on_click(cx.listener(|this, _, _, cx| this.refresh(cx)))
                            }
                        })
                        .test_support(),
                    ),
            )
            .when(repository, |this| {
                let focused = self.filter.focus_handle(cx).is_focused(window);
                this.child(
                    div().px(rems(0.45)).py(rems(0.3)).child(
                        div()
                            .relative()
                            .flex()
                            .items_center()
                            .gap(rems(0.45))
                            .h(px(28.))
                            .border_1()
                            .border_color(palette.border)
                            .rounded(Radius::Xl.px(cx))
                            .bg(palette.background)
                            .px(rems(0.65))
                            // Escape vacía el filtro.
                            .capture_action(cx.listener(|this, _: &Escape, window, cx| {
                                if this.filter.read(cx).value().is_empty() {
                                    cx.propagate();
                                } else {
                                    this.set_filter_text("", window, cx);
                                }
                            }))
                            .child(
                                mergeon_ui::icon(Lucide::Search)
                                    .size(px(13.))
                                    .tone(IconTone::Muted),
                            )
                            .child(
                                Input::new(&self.filter)
                                    .id("sidebar-filter")
                                    .small()
                                    .appearance(false)
                                    .cleanable(true)
                                    .px_0()
                                    .text_size(rems(0.8125)),
                            )
                            .when(focused, |this| {
                                this.child(mergeon_ui::style::input_focus_overlay(
                                    Radius::Xl.px(cx),
                                    cx,
                                ))
                            }),
                    ),
                )
            })
    }

    /// `SidebarCommitSearchPanel.vue`. Ocupa el alto que queda: los
    /// resultados llenan el sidebar con su propia barra de scroll.
    fn render_search_panel(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        // La query se separa una sola vez por frame (chips y estado).
        let fields = self.search_filters.fields(cx);
        let query = self.current_search_query_with(&fields, cx);
        let query_is_blank = query.is_empty();
        let focused = self.search_query.focus_handle(cx).is_focused(window);
        // `.commit-search-panel__state`: una línea centrada, o varias si el
        // texto no cabe.
        let state = |muted: bool| {
            div()
                .flex()
                .items_center()
                .gap(rems(0.45))
                .min_h(rems(1.8))
                .text_size(rems(0.8125))
                .line_height(gpui_kit::relative(1.5))
                .text_color(if muted {
                    palette.muted_foreground
                } else {
                    palette.foreground
                })
        };
        let state_text = |text: &'static str| div().flex_1().min_w_0().child(text);

        div()
            .id("commit-search-panel")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .gap(rems(0.6))
            .pt(rems(0.55))
            .px(rems(0.5))
            .pb(rems(0.6))
            .border_b_1()
            .border_color(palette.border)
            .child(self.render_search_header(cx))
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap(rems(0.45))
                    .h(px(30.))
                    .border_1()
                    .border_color(palette.border)
                    .rounded(Radius::Lg.px(cx))
                    .bg(palette.background)
                    .px(rems(0.62))
                    // Escape vacía la búsqueda; ya vacía, vuelve a las secciones.
                    .capture_action(cx.listener(|this, _: &Escape, window, cx| {
                        if this.search_query.read(cx).value().is_empty() {
                            this.show_repository_panel(cx);
                        } else {
                            this.set_search_query("", window, cx);
                        }
                    }))
                    .child(
                        mergeon_ui::icon(Lucide::Search)
                            .size(px(13.))
                            .tone(IconTone::Muted),
                    )
                    .child(
                        Input::new(&self.search_query)
                            .id("commit-search-input")
                            .small()
                            .appearance(false)
                            .cleanable(true)
                            .px_0()
                            .text_size(rems(0.79)),
                    )
                    .when(focused, |this| {
                        this.child(mergeon_ui::style::input_focus_overlay(
                            Radius::Lg.px(cx),
                            cx,
                        ))
                    }),
            )
            .children(self.render_search_chips(&query, &fields, cx))
            .when(self.search_filters.open, |this| {
                this.child(self.render_search_fields(cx))
            })
            .map(|this| {
                if let Some(error) = self.search_error.clone().filter(|_| !self.search_loading) {
                    this.child(
                        div()
                            .id("commit-search-error")
                            .test_support()
                            .text_size(rems(0.8125))
                            .line_height(gpui_kit::relative(1.5))
                            .text_color(palette.destructive_foreground)
                            .child(error),
                    )
                } else if self.search_loading {
                    this.child(
                        state(false)
                            .child(
                                mergeon_ui::icon(Lucide::LoaderCircle)
                                    .size(px(15.))
                                    .tone(IconTone::Muted)
                                    .spin(true),
                            )
                            .child("Loading commits..."),
                    )
                } else if query_is_blank {
                    this.child(state(true).child(state_text(
                        "Search the full history by message, author, hash, path, branch, date or code.",
                    )))
                } else if self.search_results.is_empty() {
                    this.child(state(true).child(state_text("No commits found.")))
                } else {
                    this.child(self.render_search_results(cx))
                        .children(self.render_load_more(cx))
                }
            })
            .test_support()
    }

    /// Los resultados de la búsqueda, en una lista virtual con su propio
    /// scroll: solo se construyen los que están a la vista.
    fn render_search_results(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // Llena el alto que deja el panel (sin tope fijo) y recorta: la lista
        // virtual solo construye lo que se ve, con su barra de scroll.
        div()
            .id("commit-search-results")
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .child(
                list(
                    self.search_list.clone(),
                    cx.processor(|this, ix, _, cx| this.render_search_result(ix, cx)),
                )
                .with_sizing_behavior(ListSizingBehavior::Auto)
                .size_full()
                .pr(rems(0.45)),
            )
            .vertical_scrollbar(&self.search_list)
    }

    fn render_search_result(&mut self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(commit) = self.search_results.get(ix) else {
            return div().into_any_element();
        };
        let palette = *cx.palette();
        let mono = cx.theme().mono_font_family.clone();
        let active = self
            .search_repo_id
            .as_deref()
            .and_then(|repo_id| self.search_highlight.get(repo_id))
            .is_some_and(|hash| *hash == commit.full_hash);
        let full_hash = commit.full_hash.clone();
        let last = ix + 1 == self.search_results.len();
        let card = div()
            .id(SharedString::from(format!(
                "commit-result:{}",
                commit.full_hash
            )))
            .flex()
            .flex_col()
            .flex_shrink_0()
            .gap(px(3.))
            .w_full()
            .border_1()
            .rounded(Radius::Lg.px(cx))
            .px(rems(0.55))
            .py(rems(0.45))
            .cursor_pointer()
            .map(|this| {
                if active {
                    this.border_color(with_alpha(palette.primary, 0.35))
                        .bg(with_alpha(palette.primary, 0.1))
                } else {
                    this.border_color(gpui_kit::transparent_black())
                        .hover(|style| style.bg(palette.sidebar_row_hover))
                }
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                this.select_search_result(&full_hash, cx);
            }))
            .child(
                // Como mucho dos líneas: un mensaje largo no se come la lista.
                div()
                    .w_full()
                    .min_w_0()
                    .text_size(rems(0.8125))
                    .line_height(gpui_kit::relative(1.5))
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .text_color(palette.foreground)
                    .line_clamp(2)
                    .text_ellipsis()
                    .child(commit.message.clone()),
            )
            .child(
                div()
                    .flex()
                    .min_w_0()
                    .gap(rems(0.4))
                    .text_size(rems(0.75))
                    .line_height(gpui_kit::relative(1.5))
                    .text_color(palette.muted_foreground)
                    .child(
                        div()
                            .flex_shrink_0()
                            .font_family(mono.clone())
                            .child(commit.hash.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(commit.author.clone()),
                    ),
            )
            .test_support();
        // `gap: 0.22rem` entre resultados.
        div()
            .when(!last, |this| this.pb(rems(0.22)))
            .child(card)
            .into_any_element()
    }
}

impl Sidebar {
    /// Las filas muestran tooltip salvo justo donde se abrió un menú
    /// contextual. La sección de PRs es otra vista: se le avisa del cambio.
    fn sync_row_tooltips(&mut self, cx: &mut Context<Self>) {
        let enabled = self.menu_anchor.is_none();
        if self.row_tooltips != enabled {
            self.row_tooltips = enabled;
            self.pull_requests.update(cx, |_, cx| cx.notify());
        }
    }

    /// Si las filas pueden mostrar tooltip (no hay un menú recién abierto
    /// bajo el puntero).
    pub fn row_tooltips_enabled(&self) -> bool {
        self.row_tooltips
    }
}

impl Render for Sidebar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        let palette = *cx.palette();
        if self
            .menu_anchor
            .is_some_and(|anchor| anchor != window.mouse_position())
        {
            self.menu_anchor = None;
        }
        self.sync_row_tooltips(cx);
        // Un arrastre que GPUI ya dio por terminado (cancelado, soltado fuera
        // de la ventana) no deja la rama atenuada.
        if self.dragged_branch.is_some() && !cx.has_active_drag() {
            self.dragged_branch = None;
        }
        // Nada de esto lee el modelo: lo recogió `model_changed`.
        let width = self.render_key.width;
        let snapshot = self.snapshot(window, cx);
        let search_visible = self.search_panel_shown();

        let bounds = self.bounds.clone();
        let dragging = self.dragged_branch.is_some();
        let sidebar = cx.entity().downgrade();

        div()
            .id("sidebar")
            .relative()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .h_full()
            .w(px(width as f32))
            .bg(palette.sidebar)
            .border_r_1()
            .border_color(palette.sidebar_border)
            .capture_any_mouse_down(cx.listener(|this, event: &MouseDownEvent, _, cx| {
                if event.button == MouseButton::Right {
                    this.menu_anchor = Some(event.position);
                    this.sync_row_tooltips(cx);
                    cx.notify();
                }
            }))
            // Al mover el puntero (o al volver tras cerrar el menú) las filas
            // recuperan su tooltip: sin repintar, seguirían sin él.
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                if this
                    .menu_anchor
                    .is_some_and(|anchor| anchor != event.position)
                {
                    this.menu_anchor = None;
                    this.sync_row_tooltips(cx);
                    cx.notify();
                }
            }))
            .child(
                canvas(
                    move |measured, _, _| bounds.set(measured),
                    move |_, _, window, _| {
                        if !dragging {
                            return;
                        }
                        // `dragend`: el arrastre de una rama termina al soltar,
                        // caiga donde caiga. En cualquier fase: un elemento
                        // que detenga la propagación no lo deja colgado.
                        window.on_mouse_event(move |_: &MouseUpEvent, _, _, cx| {
                            sidebar
                                .update(cx, |this, cx| {
                                    if this.dragged_branch.take().is_some() {
                                        cx.notify();
                                    }
                                })
                                .ok();
                        });
                    },
                )
                .absolute()
                .size_full(),
            )
            .map(|this| {
                if search_visible {
                    // La búsqueda no hace scroll entera: sus resultados llenan
                    // el alto y llevan su propia barra.
                    this.child(
                        div()
                            .id("sidebar-scroll")
                            .flex()
                            .flex_col()
                            .size_full()
                            .min_h_0()
                            .pt(rems(0.25))
                            .px(rems(0.5))
                            .pb(rems(0.5))
                            .child(self.render_navigation(window, cx))
                            .when(snapshot.is_some(), |this| {
                                this.child(self.render_search_panel(window, cx))
                            }),
                    )
                } else {
                    this.child(
                        div()
                            .id("sidebar-scroll")
                            .flex()
                            .flex_col()
                            .size_full()
                            .overflow_y_scroll()
                            .track_scroll(&self.scroll_handle)
                            .pt(rems(0.25))
                            .px(rems(0.5))
                            .pb(rems(0.5))
                            .child(self.render_navigation(window, cx))
                            .when_some(snapshot, |this, snapshot| {
                                this.child(self.render_branch_sections(&snapshot, window, cx))
                                    .child(self.pull_requests.clone())
                                    .child(self.render_remote_section(&snapshot, cx))
                                    .child(self.render_worktree_section(window, cx))
                                    .child(self.render_tag_section(&snapshot, window, cx))
                                    .child(self.render_stash_section(&snapshot, window, cx))
                                    .child(self.render_submodule_section(&snapshot, window, cx))
                            }),
                    )
                    .vertical_scrollbar(&self.scroll_handle)
                }
            })
            .test_support()
    }
}
