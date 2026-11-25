//! La vista de cambios locales (`src/components/repository/ChangesView.vue`):
//! la lista de archivos, el diff del elegido —o el resolvedor, si está en
//! conflicto— y la caja de commit, con todo lo que se puede hacer sobre el
//! working tree: preparar y sacar del índice (por archivo, en lote y por
//! bloque), descartar, ignorar, hacer commit y resolver conflictos.
//!
//! Tiene los tres modos del original:
//!
//! - [`ChangesMode::List`] (`list-only`): la pestaña "Local Changes". A la
//!   izquierda la lista y, debajo, la caja de commit; a la derecha el diff del
//!   archivo elegido. El doble clic abre el archivo en su pestaña.
//! - [`ChangesMode::FileTab`] (`file-tab`): la pestaña de un archivo del
//!   working tree o del índice. Lista (si está abierta) y diff o resolvedor.
//! - [`ChangesMode::Full`]: lista, diff y caja de commit juntos.

use std::cell::Cell;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AnyWindowHandle, App, AppContext as _, AsyncApp, Bounds, ClipboardItem, Context,
    Entity, EventEmitter, FocusHandle, InteractiveElement as _, IntoElement, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Task, TestSupportExt as _,
    WeakEntity, Window, canvas, div, px,
};
use mergeon_app::settings::LocalChangesPrefs;
use mergeon_app::{
    AppEvent, AppModel, GitMutation, OpenRepository, StructuredOutcome, SyncRequest,
};
use mergeon_backend::{
    Commit, CommitOptions, CommitSearchOptions, ConflictResolution, ConflictVersions, DiffOptions,
    PushOptions,
};
use mergeon_git_query::{
    CommitTemplateRead, CommitTemplateStamp, FixupCandidate, MAX_FIXUP_CANDIDATES,
};
use mergeon_gitcmd::{ConflictState, StructuredStatus};
use mergeon_model::dialogs::ConfirmOptions;
use mergeon_model::diff_preferences::DiffPreferences;
use mergeon_model::notify::{NotifyOptions, NotifyType};
use mergeon_model::operation_types::OperationResult as OperationSummary;
use mergeon_model::repositories::RepoLifecycleState;
use mergeon_model::workspace_tabs::{FileTab, FileTabDescriptor, Scroll, TabSource};
use mergeon_ui::menu::MenuItem;
use mergeon_ui::theme::with_alpha;
use mergeon_ui::{ActivePaletteExt as _, InputEvent, InputState, TextareaState};
use mergeon_view_diff::{
    DiffPanel, DiffSide, DiffViewer, DiffViewerEvent, HunkAction, ImageDiff, ImageDiffViewer,
    ImageVersion, LineAnchor, ScrollPosition, is_image_path,
};

use crate::commit_panel::CommitPanel;
use crate::conflict_resolver::{ConflictResolver, ConflictResolverEvent};
use crate::dialogs::open_blame_dialog;
use crate::logic::{
    self, BusyAction, ChangedFile, DEFAULT_LIST_WIDTH, DEFAULT_LOCAL_CHANGES_WIDTH, FileMenuAction,
    FileMenuEntry, RESIZER_WIDTH, STACKED_LAYOUT_MAX_WIDTH, SidebarSource, clamp_list_width,
    clamp_local_changes_width, file_descriptor, file_menu_entries, is_conflicted, is_untracked,
    lifecycle_allows_changes,
};
use crate::sidebar::{
    ChangesSection, ChangesSidebar, FileAction, FileActionKind, MultiSelection, StatsByPath,
};

mod patch_actions;

/// Cómo se monta la vista.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangesMode {
    /// `list-only`: la pestaña "Local Changes" del espacio de trabajo.
    List,
    /// `file-tab`: la pestaña de un archivo del working tree o del índice.
    /// `tab_id` es el id de esa pestaña en `AppModel::repository_workspace`.
    FileTab { tab_id: String },
    /// Lista, diff y caja de commit en una sola vista (el modo por defecto del
    /// original, fuera del espacio de trabajo por pestañas).
    Full,
}

/// Lo que la vista avisa a quien la aloja. Nada de esto queda por hacer: la
/// vista ya lo aplicó en `AppModel`.
#[derive(Debug, Clone, PartialEq)]
pub enum ChangesViewEvent {
    /// `@open-file`: se pidió abrir un archivo en una pestaña (con `replace`,
    /// reutilizando la actual). Ya está hecho con
    /// `AppModel::open_workspace_file`.
    OpenFile {
        descriptor: FileTabDescriptor,
        replace: bool,
    },
    /// `@commit-created`: se creó un commit. `sync_already_applied` dice si,
    /// tras empujarlo, ya se releyeron estado y ramas.
    CommitCreated {
        result: Box<GitMutation>,
        sync_already_applied: bool,
    },
}

/// Vista de cambios locales de un repositorio.
///
/// ```ignore
/// // "Local Changes"
/// let changes = cx.new(|cx| ChangesView::new(repo_id, ChangesMode::List, window, cx));
/// // Pestaña de archivo
/// let tab = cx.new(|cx| ChangesView::new(repo_id, ChangesMode::FileTab { tab_id }, window, cx));
/// tab.update(cx, |view, cx| view.set_files_open(files_open, cx));
/// ```
pub struct ChangesView {
    model: Entity<AppModel>,
    repo_id: String,
    mode: ChangesMode,
    /// `active`: lo que dice quien la aloja (por defecto, sí).
    active: bool,
    /// La actividad efectiva del frame anterior, para detectar el paso a activa.
    was_active: bool,
    files_open: bool,
    /// La pestaña que muestra (solo en [`ChangesMode::FileTab`]).
    tab: Option<FileTab>,

    /// Cambiar solo con [`ChangesView::set_files`]: invalida `file_sections`.
    files: Vec<ChangedFile>,
    /// Las secciones de la lista (conflictos, sin preparar, sin seguimiento,
    /// preparados) de `files`, calculadas una vez por cambio de `files` y no
    /// en cada render: con miles de archivos filtrarlos y clonarlos por frame
    /// hacía que hasta pasar el ratón por la lista costara CPU.
    file_sections: Option<FileSections>,
    selected_file: Option<ChangedFile>,
    /// Dos o más archivos elegidos en la lista (de una misma sección): el
    /// panel derecho enseña su resumen y las acciones en lote.
    multi_selection: Option<Rc<MultiSelection>>,
    /// El asunto del mensaje de commit (una línea).
    commit_title: Entity<InputState>,
    /// El cuerpo del mensaje de commit.
    commit_body: Entity<TextareaState>,
    /// "Sign (SSH/GPG)", recordado por repositorio.
    sign: bool,
    /// El foco de la lista de archivos: con él, Espacio, D y las flechas son
    /// de la lista.
    list_focus: FocusHandle,
    file_diff: SharedString,
    file_error: SharedString,
    /// Carrera entre cargas de diff: solo vale la última.
    diff_request: u64,
    diff_too_large: bool,
    diff_whole_file_fell_back: bool,
    diff_size: u64,
    file_content: SharedString,
    /// El archivo del que es `file_content`.
    content_path: Option<String>,
    /// Las tres versiones del archivo en conflicto elegido (las del índice),
    /// para el resolvedor. Son de `content_path`.
    conflict_versions: Option<Arc<ConflictVersions>>,
    loading_changes: bool,
    loading_diff: bool,
    push_after_commit: bool,
    is_resolving_conflict: bool,
    busy_action: Option<BusyAction>,
    amend: bool,
    last_commit: Option<Commit>,
    /// Rama y hash corto de HEAD con los que se leyó `last_commit`: si cambian
    /// (commit, amend, deshacer/rehacer, reset, checkout…) se vuelve a leer.
    last_commit_key: Option<(Option<String>, Option<String>)>,
    last_commit_request: u64,

    /// Ancho de la lista fuera de una pestaña de archivo, propio de la vista.
    local_sidebar_width: f64,
    resizing: bool,
    bounds: Rc<Cell<Bounds<Pixels>>>,

    /// El último estado del repositorio que la vista ya recogió (el
    /// `previousStatus` del `watch` del original).
    seen_status: Option<StructuredStatus>,
    /// El estado cambió desde la última mutación registrada.
    status_changed_since_mutation: bool,

    /// Las cifras `+N −N` de la lista, por ruta: sin preparar (con lo que no
    /// se sigue) y preparados. Se leen aparte del estado y llegan después.
    unstaged_stats: StatsByPath,
    staged_stats: StatsByPath,
    /// La lectura de cifras en curso: solo una a la vez.
    stats_task: Option<Task<()>>,
    /// Algo cambió mientras se leían: al acabar se vuelven a leer.
    stats_dirty: bool,
    /// Cambiaron mientras la lista no se veía: se leen al volver.
    stats_stale: bool,
    /// El mensaje de `commit.template` sin sus comentarios, si hay plantilla.
    commit_template: Option<String>,
    /// El archivo de la plantilla tal como se leyó: al releer, si no cambió
    /// ni él ni la configuración, no se vuelve a leer.
    template_stamp: Option<CommitTemplateStamp>,
    /// La última relectura pedida: las anteriores que lleguen tarde no valen.
    template_generation: u64,
    /// La caja estaba vacía en el último cambio (releer al vaciarse, no en
    /// cada tecla).
    message_was_empty: bool,
    /// La ventana de la vista: rellenar la caja con la plantilla necesita una
    /// `Window`, y la relectura acaba en un hilo de fondo.
    window_handle: AnyWindowHandle,

    /// Los commits que ofrece "Fixup into…", leídos en segundo plano.
    fixup_targets: Rc<Vec<FixupCandidate>>,
    /// Rama, HEAD, upstream y `ahead` con los que se leyeron: si cambian, se
    /// vuelven a leer (un commit, un push, un reset…).
    fixup_key: Option<FixupKey>,
    fixup_task: Option<Task<()>>,
    /// Cambió la clave mientras se leía: al acabar se lee otra vez.
    fixup_dirty: bool,

    diff_viewer: Entity<DiffViewer>,
    /// La línea que se leía al pulsar "Whole file": el diff nuevo vuelve a
    /// ella (el desplazamiento en píxeles de un diff no vale en el otro).
    line_anchor: Option<LineAnchor>,
    /// Antes y después de una imagen cambiada (en lugar del diff).
    image_viewer: Entity<ImageDiffViewer>,
    resolver: Entity<ConflictResolver>,
    /// Lo que la vista enseña del modelo, leído al avisar el modelo. El
    /// pintado sale de aquí y no lee `AppModel`: la ventana repinta cuando
    /// avisa una entidad leída al pintar, así que leer el modelo en `render`
    /// repintaba con cada aviso suyo (un toast, otro repositorio...).
    shown_model: ShownModelState,
    renders: usize,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ChangesViewEvent> for ChangesView {}

/// Lo que la vista enseña de `AppModel` (aparte del estado de git, que recoge
/// `absorb_status`). El modelo avisa por cualquier cosa —un toast, el
/// progreso de una operación, otro repositorio, otra pestaña— y la vista solo
/// se repinta si cambia esto.
#[derive(Debug, Clone, Default, PartialEq)]
struct ShownModelState {
    /// En una pestaña de archivo, si es la pestaña activa de su repositorio
    /// (fuera de ellas, siempre).
    tab_is_active: bool,
    /// El archivo y el origen de su pestaña (sin el desplazamiento, que la
    /// vista guarda en el modelo al hacer scroll y no se pinta desde ahí).
    tab: Option<(String, TabSource)>,
    /// El ancho guardado de la lista de las pestañas de archivo.
    file_list_width: f64,
    diff_preferences: DiffPreferences,
    lifecycle: RepoLifecycleState,
    conflict_state: ConflictState,
    operation_in_progress: Option<&'static str>,
    head_short_hash: Option<String>,
    branch: Option<String>,
    upstream: Option<String>,
    amend_is_pushed: bool,
    /// Ancho, lista plana y atajos de "Local Changes".
    local_changes: LocalChangesPrefs,
}

/// De qué depende la lista de "Fixup into…".
#[derive(Debug, Clone, PartialEq, Eq)]
struct FixupKey {
    branch: Option<String>,
    head: Option<String>,
    upstream: Option<String>,
    ahead: u32,
}

/// Las secciones de la lista de cambios, compartidas con la barra lateral sin
/// copiarlas: la barra guarda el árbol de cada una mientras sea el mismo `Rc`.
#[derive(Clone)]
struct FileSections {
    conflicted: Rc<Vec<ChangedFile>>,
    unstaged: Rc<Vec<ChangedFile>>,
    untracked: Rc<Vec<ChangedFile>>,
    staged: Rc<Vec<ChangedFile>>,
}

impl ChangesView {
    /// Reemplaza los archivos con cambios e invalida sus secciones. De la
    /// selección múltiple quedan los que siguen en su sección.
    fn set_files(&mut self, files: Vec<ChangedFile>) {
        self.files = files;
        self.file_sections = None;
        if let Some(multi) = self.multi_selection.take() {
            let section = self.section_files(multi.section);
            let paths: Vec<String> = multi
                .paths
                .iter()
                .filter(|path| section.iter().any(|file| file.path == **path))
                .cloned()
                .collect();
            self.multi_selection = (paths.len() > 1).then(|| {
                Rc::new(MultiSelection {
                    section: multi.section,
                    paths,
                })
            });
        }
    }

    /// Los archivos de una sección de la lista.
    fn section_files(&mut self, section: ChangesSection) -> Vec<ChangedFile> {
        let sections = self.file_sections();
        match section {
            ChangesSection::Conflict => sections.conflicted.to_vec(),
            ChangesSection::Unstaged => sections
                .unstaged
                .iter()
                .chain(sections.untracked.iter())
                .cloned()
                .collect(),
            ChangesSection::Staged => sections.staged.to_vec(),
        }
    }

    /// Las secciones de `files`, calculadas solo si cambiaron desde la última vez.
    fn file_sections(&mut self) -> &FileSections {
        let files = &self.files;
        self.file_sections.get_or_insert_with(|| FileSections {
            conflicted: Rc::new(logic::conflicted_files(files)),
            unstaged: Rc::new(logic::unstaged_files(files)),
            untracked: Rc::new(logic::untracked_files(files)),
            staged: Rc::new(logic::staged_files(files)),
        })
    }

    pub fn new(
        repo_id: impl Into<String>,
        mode: ChangesMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let model = AppModel::global(cx);
        let repo_id: String = repo_id.into();
        let diff_viewer = cx.new(|cx| DiffViewer::new(window, cx));
        let resolver = cx.new(|cx| ConflictResolver::new(window, cx));
        // El borrador del mensaje vive en la sesión: vuelve tras cerrar el
        // repositorio o la aplicación.
        let draft = if matches!(mode, ChangesMode::FileTab { .. }) {
            String::new()
        } else {
            let model = model.read(cx);
            model
                .repository(repo_id.as_str())
                .map(|repository| model.commit_draft(&repository.path))
                .unwrap_or_default()
        };
        // Un borrador de cuando el mensaje era una sola caja se parte igual:
        // asunto hasta el primer salto, el resto al cuerpo.
        let (draft_title, draft_body) = logic::split_commit_message(&draft);
        let commit_title = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Summary")
                .default_value(draft_title)
        });
        let commit_body = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Description")
                .default_value(draft_body)
        });
        let sign = !matches!(mode, ChangesMode::FileTab { .. }) && {
            let model = model.read(cx);
            model
                .repository(repo_id.as_str())
                .is_some_and(|repository| model.commit_sign(&repository.path))
        };

        let subscriptions = vec![
            cx.observe(&model, |this, _, cx| this.model_changed(cx)),
            cx.subscribe(&model, |this, _, event: &AppEvent, cx| {
                this.handle_app_event(event, cx);
            }),
            cx.subscribe(
                &diff_viewer,
                |this, _, event: &DiffViewerEvent, cx| match event {
                    DiffViewerEvent::ApplyHunk(request) => this.handle_hunk_request(request, cx),
                    DiffViewerEvent::ApplyLines(request) => this.handle_lines_request(request, cx),
                    DiffViewerEvent::Scrolled(position) => this.remember_scroll(*position, cx),
                },
            ),
            cx.subscribe(
                &resolver,
                |this, _, event: &ConflictResolverEvent, cx| match event {
                    ConflictResolverEvent::Save {
                        resolution,
                        warning,
                    } => this.confirm_resolve(resolution.clone(), warning.clone(), cx),
                    ConflictResolverEvent::SelectFile(path) => {
                        this.handle_conflict_file_select(path, cx);
                    }
                },
            ),
            // El mensaje decide si se puede hacer commit, y se guarda como
            // borrador del repositorio.
            cx.subscribe(&commit_title, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.commit_message_changed(cx);
                }
            }),
            cx.subscribe(&commit_body, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.commit_message_changed(cx);
                }
            }),
        ];

        let list_only = mode == ChangesMode::List;
        let mut this = Self {
            model: model.clone(),
            repo_id,
            mode,
            active: true,
            was_active: false,
            files_open: true,
            tab: None,
            files: Vec::new(),
            file_sections: None,
            selected_file: None,
            multi_selection: None,
            commit_title,
            commit_body,
            sign,
            list_focus: cx.focus_handle(),
            file_diff: SharedString::default(),
            file_error: SharedString::default(),
            diff_request: 0,
            diff_too_large: false,
            diff_whole_file_fell_back: false,
            diff_size: 0,
            file_content: SharedString::default(),
            content_path: None,
            conflict_versions: None,
            loading_changes: false,
            loading_diff: false,
            push_after_commit: false,
            is_resolving_conflict: false,
            busy_action: None,
            amend: false,
            last_commit: None,
            last_commit_key: None,
            last_commit_request: 0,
            local_sidebar_width: if list_only {
                DEFAULT_LOCAL_CHANGES_WIDTH
            } else {
                DEFAULT_LIST_WIDTH
            },
            resizing: false,
            bounds: Rc::default(),
            seen_status: None,
            status_changed_since_mutation: false,
            unstaged_stats: Rc::default(),
            staged_stats: Rc::default(),
            stats_task: None,
            stats_dirty: false,
            stats_stale: false,
            commit_template: None,
            template_stamp: None,
            template_generation: 0,
            message_was_empty: true,
            window_handle: window.window_handle(),
            fixup_targets: Rc::default(),
            fixup_key: None,
            fixup_task: None,
            fixup_dirty: false,
            diff_viewer,
            line_anchor: None,
            image_viewer: cx.new(ImageDiffViewer::new),
            resolver,
            shown_model: ShownModelState::default(),
            renders: 0,
            _subscriptions: subscriptions,
        };
        this.tab = this.find_tab(cx);
        this.was_active = this.is_active(cx);
        this.shown_model = this.shown_model_state(cx);
        // El observador empieza a escuchar al final de este ciclo: un aviso
        // del modelo que ya estuviera en cola no le llegaría.
        let view = cx.weak_entity();
        cx.defer(move |cx| {
            view.update(cx, |this, cx| this.refresh_shown_model(cx))
                .ok();
        });

        // `loadDiffPreferences()` y, en una pestaña de archivo, el ancho
        // guardado de la lista.
        model.update(cx, |model, cx| {
            model.load_diff_preferences(false, cx).detach();
            if this.is_file_tab() {
                model.load_file_list_width(cx).detach();
            } else {
                model.load_local_changes_prefs(cx).detach();
            }
        });
        this.adopt_local_changes_width();
        // El `watch` inmediato del repositorio.
        this.load_changes(cx).detach();
        if !this.is_file_tab() {
            this.last_commit_key = this.current_last_commit_key(cx);
            this.load_last_commit(cx).detach();
            this.refresh_commit_template(cx);
            this.refresh_fixup_targets(cx);
        }
        this
    }

    // ------------------------------------------------------------------
    // Lo que pasa quien aloja la vista (las props del original)
    // ------------------------------------------------------------------

    /// `active`: la vista está a la vista. Una pestaña de archivo inactiva no
    /// carga su diff hasta que se activa. Por defecto, sí.
    ///
    /// En una pestaña de archivo cuenta además que sea la pestaña activa de su
    /// repositorio: eso lo mira la vista en `AppModel`, sin que nadie se lo diga.
    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.active != active {
            self.active = active;
            self.activity_changed(cx);
            cx.notify();
        }
    }

    /// `files-open`: en una pestaña de archivo, si la lista de archivos se ve
    /// (el botón "Toggle file list" de la cabecera de la pestaña). Por defecto, sí.
    pub fn set_files_open(&mut self, files_open: bool, cx: &mut Context<Self>) {
        if self.files_open != files_open {
            self.files_open = files_open;
            if files_open && self.stats_stale {
                self.request_stats(cx);
            }
            cx.notify();
        }
    }

    /// Cambia la pestaña que muestra la vista. No suele hacer falta: cuando un
    /// clic reutiliza la pestaña con otro archivo (`WorkspaceTabReplaced`), la
    /// vista sigue a su pestaña sola.
    pub fn set_tab(&mut self, tab_id: impl Into<String>, cx: &mut Context<Self>) {
        let tab_id = tab_id.into();
        if matches!(&self.mode, ChangesMode::FileTab { tab_id: current } if *current != tab_id) {
            self.mode = ChangesMode::FileTab { tab_id };
            self.tab_changed(cx);
            self.shown_model = self.shown_model_state(cx);
            cx.notify();
        }
    }

    // ------------------------------------------------------------------
    // Lectura
    // ------------------------------------------------------------------

    pub fn repo_id(&self) -> &str {
        &self.repo_id
    }

    pub fn mode(&self) -> &ChangesMode {
        &self.mode
    }

    /// El id de la pestaña que muestra, si es una pestaña de archivo.
    pub fn tab_id(&self) -> Option<&str> {
        match &self.mode {
            ChangesMode::FileTab { tab_id } => Some(tab_id),
            _ => None,
        }
    }

    pub fn is_files_open(&self) -> bool {
        self.files_open
    }

    /// Los archivos con cambios, como los trae el estado del repositorio.
    pub fn files(&self) -> &[ChangedFile] {
        &self.files
    }

    pub fn selected_file(&self) -> Option<&ChangedFile> {
        self.selected_file.as_ref()
    }

    /// El diff del archivo elegido.
    pub fn file_diff(&self) -> &SharedString {
        &self.file_diff
    }

    /// El contenido (con marcadores) del archivo en conflicto elegido.
    pub fn file_content(&self) -> &SharedString {
        &self.file_content
    }

    /// El error de la última carga de diff; vacío si fue bien.
    pub fn file_error(&self) -> &SharedString {
        &self.file_error
    }

    pub fn is_loading_changes(&self) -> bool {
        self.loading_changes
    }

    pub fn is_loading_diff(&self) -> bool {
        self.loading_diff
    }

    /// La mutación en curso, si hay alguna.
    pub fn busy_action(&self) -> Option<BusyAction> {
        self.busy_action
    }

    /// `isInteractionLocked`: hay una mutación o una resolución en curso.
    pub fn is_interaction_locked(&self) -> bool {
        self.busy_action.is_some() || self.is_resolving_conflict
    }

    /// Se está guardando la resolución de un conflicto.
    pub fn is_resolving_conflict(&self) -> bool {
        self.is_resolving_conflict
    }

    /// El mensaje de commit tal como se va a usar: `asunto\n\ncuerpo`.
    pub fn commit_message(&self, cx: &App) -> SharedString {
        logic::compose_commit_message(
            &self.commit_title.read(cx).value(),
            &self.commit_body.read(cx).value(),
        )
        .into()
    }

    /// El asunto escrito.
    pub fn commit_title(&self, cx: &App) -> SharedString {
        self.commit_title.read(cx).value()
    }

    /// El cuerpo escrito.
    pub fn commit_body(&self, cx: &App) -> SharedString {
        self.commit_body.read(cx).value()
    }

    /// El mensaje de HEAD que rellenará "Amend last commit", si hay commit.
    pub fn amendable_message(&self) -> Option<String> {
        self.last_commit
            .as_ref()
            .map(|commit| logic::amend_message(commit).trim_end().to_string())
    }

    /// "Sign (SSH/GPG)".
    pub fn is_sign(&self) -> bool {
        self.sign
    }

    /// El foco de la lista de archivos (Espacio, D, flechas).
    pub fn list_focus_handle(&self) -> &FocusHandle {
        &self.list_focus
    }

    pub fn is_push_after_commit(&self) -> bool {
        self.push_after_commit
    }

    pub fn is_amend(&self) -> bool {
        self.amend
    }

    /// `hunkAction`: la acción de bloque que se ofrece sobre el diff a la vista.
    pub fn hunk_action(&self, cx: &App) -> Option<HunkAction> {
        logic::hunk_action(
            self.selected_file.as_ref(),
            self.model.read(cx).diff_preferences().ignore_whitespace,
        )
    }

    /// `canCommitAction`.
    pub fn is_committable(&self, cx: &App) -> bool {
        let lifecycle = self
            .repository(cx)
            .map(|repository| repository.repo_lifecycle_state)
            .unwrap_or_default();
        logic::can_commit(
            lifecycle,
            &self.commit_message(cx),
            logic::staged_count(&self.files),
            self.amend,
        ) && !self.is_interaction_locked()
    }

    /// El visor de diffs que aloja, para consultar lo que pinta.
    pub fn diff_viewer(&self) -> &Entity<DiffViewer> {
        &self.diff_viewer
    }

    /// El resolvedor de conflictos que aloja.
    pub fn conflict_resolver(&self) -> &Entity<ConflictResolver> {
        &self.resolver
    }

    /// Ancho actual de la lista de archivos.
    pub fn sidebar_width(&self, cx: &App) -> f64 {
        self.sidebar_width_with(self.model.read(cx).file_list_width().width)
    }

    /// [`Self::sidebar_width`] con el ancho guardado de las pestañas de archivo.
    fn sidebar_width_with(&self, file_list_width: f64) -> f64 {
        if self.is_file_tab() {
            file_list_width
        } else if self.is_list_only() {
            // El ancho elegido, sin dejar al diff sin sitio si la ventana encoge.
            clamp_local_changes_width(self.local_sidebar_width, self.container_geometry().1)
        } else {
            self.local_sidebar_width
        }
    }

    // ------------------------------------------------------------------
    // Estado derivado
    // ------------------------------------------------------------------

    fn repository<'a>(&self, cx: &'a App) -> Option<&'a OpenRepository> {
        self.model.read(cx).repository(self.repo_id.as_str())
    }

    fn repo_path(&self, cx: &App) -> Option<String> {
        self.repository(cx)
            .map(|repository| repository.path.clone())
    }

    /// Guarda el mensaje de commit como borrador del repositorio (sin
    /// repintar nada y con el guardado en disco diferido). Se guarda unido,
    /// como el mensaje: el formato del borrador no cambia.
    fn remember_commit_draft(&mut self, cx: &mut Context<Self>) {
        if self.is_file_tab() {
            return;
        }
        let draft = self.commit_message(cx).to_string();
        let Some(path) = self.repo_path(cx) else {
            return;
        };
        self.model
            .update(cx, |model, cx| model.set_commit_draft(&path, &draft, cx));
    }

    fn is_file_tab(&self) -> bool {
        matches!(self.mode, ChangesMode::FileTab { .. })
    }

    fn is_list_only(&self) -> bool {
        self.mode == ChangesMode::List
    }

    fn find_tab(&self, cx: &App) -> Option<FileTab> {
        let tab_id = self.tab_id()?;
        let path = self.repo_path(cx)?;
        self.model
            .read(cx)
            .repository_workspace(&path)
            .tabs
            .into_iter()
            .find(|tab| tab.id == tab_id)
    }

    /// `props.active`, tal como lo calculaba el espacio de trabajo: la vista
    /// se ve y, si es una pestaña de archivo, es la pestaña activa.
    fn is_active(&self, cx: &App) -> bool {
        if !self.active {
            return false;
        }
        let Some(tab_id) = self.tab_id() else {
            return true;
        };
        self.repo_path(cx).is_some_and(|path| {
            self.model.read(cx).repository_workspace(&path).active_tab == tab_id
        })
    }

    fn source(&self) -> SidebarSource {
        self.tab.as_ref().map_or(SidebarSource::All, |tab| {
            SidebarSource::from_tab_source(tab.source)
        })
    }

    fn is_conflict(&self) -> bool {
        self.selected_file.as_ref().is_some_and(is_conflicted)
    }

    fn selected_path(&self) -> Option<String> {
        self.selected_file.as_ref().map(|file| file.path.clone())
    }

    // ------------------------------------------------------------------
    // Lo que cambia fuera de la vista
    // ------------------------------------------------------------------

    /// Lee lo que la vista enseña del modelo (ver [`ShownModelState`]).
    fn shown_model_state(&self, cx: &App) -> ShownModelState {
        let model = self.model.read(cx);
        let repository = model.repository(self.repo_id.as_str());
        let tracking = repository.and_then(|repository| repository.tracking_status.as_ref());
        let tab_is_active = self.tab_id().is_none_or(|tab_id| {
            repository.is_some_and(|repository| {
                model.repository_workspace(&repository.path).active_tab == tab_id
            })
        });
        ShownModelState {
            tab_is_active,
            tab: self.tab.as_ref().map(|tab| (tab.path.clone(), tab.source)),
            file_list_width: model.file_list_width().width,
            diff_preferences: *model.diff_preferences(),
            lifecycle: repository
                .map(|repository| repository.repo_lifecycle_state)
                .unwrap_or_default(),
            conflict_state: repository
                .map(|repository| repository.conflict_state.clone())
                .unwrap_or_default(),
            operation_in_progress: self.operation_in_progress(cx),
            head_short_hash: repository.and_then(|repository| repository.head_short_hash.clone()),
            branch: repository.and_then(|repository| repository.current_branch.clone()),
            upstream: tracking.and_then(|tracking| tracking.tracking.clone()),
            amend_is_pushed: logic::amend_is_pushed(tracking),
            local_changes: model.local_changes_prefs(),
        }
    }

    /// En "Local Changes", el ancho guardado (si lo hay) es el de la columna,
    /// salvo en mitad de un arrastre.
    fn adopt_local_changes_width(&mut self) {
        if !self.is_list_only() || self.resizing {
            return;
        }
        self.local_sidebar_width = self
            .shown_model
            .local_changes
            .width
            .unwrap_or(DEFAULT_LOCAL_CHANGES_WIDTH);
    }

    /// Veces que se pintó (tests).
    #[doc(hidden)]
    pub fn render_count(&self) -> usize {
        self.renders
    }

    fn model_changed(&mut self, cx: &mut Context<Self>) {
        // Lo que sigue solo hace trabajo si cambió lo suyo: cada paso compara
        // con lo que ya recogió (la pestaña, la actividad, HEAD, el estado...).
        self.tab_changed(cx);
        self.activity_changed(cx);
        self.refresh_last_commit(cx);
        self.refresh_fixup_targets(cx);
        // `watch(() => activeRepo.value?.status)`. Si cambió, `absorb_status`
        // ya repinta.
        if self.repository(cx).is_some() && self.absorb_status(cx) {
            self.status_changed_since_mutation = true;
            let selected = self.selected_path();
            self.reconcile_selected_file(selected.as_deref(), cx)
                .detach();
        }
        // Y solo se repinta si cambió algo de lo que se ve.
        self.refresh_shown_model(cx);
    }

    /// Relee lo que la vista enseña del modelo y repinta si cambió.
    fn refresh_shown_model(&mut self, cx: &mut Context<Self>) {
        let shown = self.shown_model_state(cx);
        if shown != self.shown_model {
            let width_changed = shown.local_changes.width != self.shown_model.local_changes.width;
            self.shown_model = shown;
            if width_changed {
                self.adopt_local_changes_width();
            }
            cx.notify();
        }
    }

    /// [`Self::is_active`] según lo último leído del modelo (para pintar).
    fn shown_active(&self) -> bool {
        self.active && self.shown_model.tab_is_active
    }

    /// [`Self::lifecycle_allows_changes`] según lo último leído del modelo.
    fn shown_lifecycle_allows_changes(&self) -> bool {
        lifecycle_allows_changes(self.shown_model.lifecycle)
    }

    fn handle_app_event(&mut self, event: &AppEvent, cx: &mut Context<Self>) {
        match event {
            // El panel de `from` pasa a ser el de `to`: la pestaña sigue
            // montada cuando un clic la reutiliza con otro archivo.
            AppEvent::WorkspaceTabReplaced {
                repo_path,
                from,
                to,
            } if self.tab_id() == Some(from.as_str())
                && self.repo_path(cx).as_deref() == Some(repo_path.as_str()) =>
            {
                self.mode = ChangesMode::FileTab { tab_id: to.clone() };
                self.tab_changed(cx);
                self.activity_changed(cx);
                self.shown_model = self.shown_model_state(cx);
                cx.notify();
            }
            AppEvent::CommitDraftRestored { repo_path, message }
                if !self.is_file_tab()
                    && self.repo_path(cx).as_deref() == Some(repo_path.as_str()) =>
            {
                let window = self.window_handle;
                let message = message.clone();
                cx.spawn(async move |this, cx| {
                    cx.update_window(window, |_, window, cx| {
                        this.update(cx, |this, cx| {
                            if this.message_is_untouched(cx) {
                                let (title, body) = logic::split_commit_message(&message);
                                this.set_commit_message_parts(title, body, window, cx);
                                this.message_was_empty = false;
                                cx.notify();
                            }
                        })
                        .ok();
                    })
                    .ok();
                })
                .detach();
            }
            // Una relectura que deja el mismo estado (el archivo ya estaba
            // modificado y volvió a cambiar) no pasa por el `watch` de arriba,
            // pero el diff que se ve sí pudo cambiar.
            AppEvent::MutationRecorded { repo_id, operation } if *repo_id == self.repo_id => {
                // Las cifras cambian aunque el estado quede igual (un archivo
                // ya modificado que se vuelve a editar).
                self.request_stats(cx);
                let changed = std::mem::take(&mut self.status_changed_since_mutation);
                let external = matches!(
                    operation.as_deref(),
                    Some("status-watch" | "external-head-change")
                );
                if external && !changed && self.seen_status.is_some() {
                    let selected = self.selected_path();
                    self.reconcile_selected_file(selected.as_deref(), cx)
                        .detach();
                }
            }
            _ => {}
        }
    }

    /// `watch(() => [fileTab.path, fileTab.source])`: la pestaña pasó a
    /// mostrar otro archivo.
    fn tab_changed(&mut self, cx: &mut Context<Self>) {
        let Some(tab) = self.find_tab(cx) else {
            return;
        };
        let changed = self
            .tab
            .as_ref()
            .is_some_and(|previous| previous.path != tab.path || previous.source != tab.source);
        let first = self.tab.is_none();
        self.tab = Some(tab);
        if changed || first {
            self.select_tab_file(cx).detach();
        }
    }

    /// `watch(() => props.active)`: una pestaña de archivo que pasa a activa
    /// carga lo suyo.
    fn activity_changed(&mut self, cx: &mut Context<Self>) {
        let active = self.is_active(cx);
        if active == self.was_active {
            return;
        }
        self.was_active = active;
        if !active {
            return;
        }
        if self.stats_stale {
            self.request_stats(cx);
        }
        if self.is_file_tab() {
            self.load_changes(cx).detach();
            return;
        }
        // Volver a "Local Changes": la plantilla pudo cambiar mientras tanto.
        self.refresh_commit_template(cx);
        if self.is_list_only() {
            // Mientras no se veía, el diff del archivo elegido no se releía.
            let selected = self.selected_path();
            self.reconcile_selected_file(selected.as_deref(), cx)
                .detach();
        }
    }

    /// Recoge el estado del repositorio si cambió desde la última vez.
    /// Devuelve si cambió.
    fn absorb_status(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(status) = self.repository(cx).map(|repository| &repository.status) else {
            return false;
        };
        if *status == self.seen_status {
            return false;
        }
        self.seen_status = status.clone();
        match &self.seen_status {
            Some(status) => self.set_files(status.files.clone()),
            None => {
                self.set_files(Vec::new());
                self.clear_selection();
            }
        }
        self.request_stats(cx);
        cx.notify();
        true
    }

    // ------------------------------------------------------------------
    // Cifras de la lista
    // ------------------------------------------------------------------

    /// La lista se ve: la vista está activa y, en una pestaña de archivo, con
    /// la lista abierta.
    fn list_visible(&self, cx: &App) -> bool {
        self.is_active(cx) && (!self.is_file_tab() || self.files_open)
    }

    /// Pide releer las cifras `+N −N` de la lista. Nunca bloquea: las filas
    /// salen sin cifras (o con las de antes) hasta que llegan. Una sola lectura
    /// a la vez; lo que se pida mientras tanto se junta en una más al acabar.
    fn request_stats(&mut self, cx: &mut Context<Self>) {
        if !self.list_visible(cx) {
            self.stats_stale = true;
            return;
        }
        self.stats_stale = false;
        if self.stats_task.is_some() {
            self.stats_dirty = true;
            return;
        }
        let Some(repo_path) = self.repo_path(cx) else {
            return;
        };
        if self.files.is_empty() {
            self.unstaged_stats = Rc::default();
            self.staged_stats = Rc::default();
            return;
        }
        let untracked: Vec<String> = self
            .files
            .iter()
            .filter(|file| is_untracked(file))
            .map(|file| file.path.clone())
            .collect();
        let read = cx.background_spawn(async move {
            mergeon_git_query::get_working_tree_stats(Path::new(&repo_path), &untracked)
        });
        self.stats_task = Some(cx.spawn(async move |this, cx| {
            let result = read.await;
            this.update(cx, |this, cx| {
                this.stats_task = None;
                match result {
                    Ok(stats) => {
                        this.unstaged_stats = Rc::new(stats.unstaged);
                        this.staged_stats = Rc::new(stats.staged);
                        cx.notify();
                    }
                    // Sin cifras no pasa nada: la lista sigue sin ellas.
                    Err(error) => log::debug!("{error}"),
                }
                if std::mem::take(&mut this.stats_dirty) {
                    this.request_stats(cx);
                }
            })
            .ok();
        }));
    }

    /// Las cifras `+N −N` de un archivo de la lista (sin preparar o
    /// preparado), si ya se leyeron.
    pub fn file_line_stats(&self, path: &str, staged: bool) -> Option<(u64, u64)> {
        let stats: &HashMap<_, _> = if staged {
            &self.staged_stats
        } else {
            &self.unstaged_stats
        };
        stats
            .get(path)
            .map(|stats| (stats.additions, stats.deletions))
    }

    // ------------------------------------------------------------------
    // "Fixup into…"
    // ------------------------------------------------------------------

    fn fixup_key(&self, cx: &App) -> Option<FixupKey> {
        let repository = self.repository(cx)?;
        let status = repository.status.as_ref();
        Some(FixupKey {
            branch: repository.current_branch.clone(),
            head: status.and_then(|status| status.head_short_hash.clone()),
            upstream: status.and_then(|status| status.tracking.clone()),
            ahead: status.map_or(0, |status| status.ahead),
        })
    }

    /// Relee los commits de "Fixup into…" si cambió la rama, HEAD, el
    /// upstream o lo que va por delante de él. Una lectura a la vez, fuera
    /// del hilo de la interfaz; no se hace en las pestañas de archivo.
    fn refresh_fixup_targets(&mut self, cx: &mut Context<Self>) {
        if self.is_file_tab() {
            return;
        }
        let key = self.fixup_key(cx);
        if key.is_none() || key == self.fixup_key {
            return;
        }
        if self.fixup_task.is_some() {
            self.fixup_dirty = true;
            return;
        }
        let Some(repo_path) = self.repo_path(cx) else {
            return;
        };
        self.fixup_key = key;
        let read = cx.background_spawn(async move {
            mergeon_git_query::get_fixup_candidates(Path::new(&repo_path), MAX_FIXUP_CANDIDATES)
        });
        self.fixup_task = Some(cx.spawn(async move |this, cx| {
            let result = read.await;
            this.update(cx, |this, cx| {
                this.fixup_task = None;
                match result {
                    Ok(targets) => {
                        this.fixup_targets = Rc::new(targets);
                        cx.notify();
                    }
                    Err(error) => {
                        log::debug!("{error}");
                        // Se reintenta con el siguiente cambio.
                        this.fixup_key = None;
                    }
                }
                if std::mem::take(&mut this.fixup_dirty) {
                    this.refresh_fixup_targets(cx);
                }
            })
            .ok();
        }));
    }

    /// El merge, rebase, cherry-pick o revert a medias del repositorio, si lo hay.
    fn operation_in_progress(&self, cx: &App) -> Option<&'static str> {
        self.repository(cx)
            .and_then(|repository| repository.operation_state.as_ref())
            .filter(|state| state.in_progress)
            .map(|state| state.kind.map_or("operation", |kind| kind.as_str()))
    }

    /// Los commits que ofrece ahora "Fixup into…".
    pub fn fixup_targets(&self) -> &[FixupCandidate] {
        &self.fixup_targets
    }

    /// "Fixup into…": un commit `fixup! <asunto>` de `target` con lo
    /// preparado. Si `target` ya está publicado, pregunta antes. La caja de
    /// mensaje no se toca (el mensaje lo pone git).
    pub fn commit_fixup(&mut self, target: &FixupCandidate, cx: &mut Context<Self>) {
        if self.amend || self.is_interaction_locked() || self.operation_in_progress(cx).is_some() {
            return;
        }
        let target = target.clone();
        if !target.published {
            self.run_fixup(target, cx);
            return;
        }
        let upstream = self
            .repository(cx)
            .and_then(|repository| repository.status.as_ref())
            .and_then(|status| status.tracking.clone())
            .unwrap_or_else(|| "the remote".to_string());
        let confirm = self.model.update(cx, |model, cx| {
            model.confirm(
                ConfirmOptions {
                    title: Some("Fix up a pushed commit?".to_string()),
                    description: Some(format!(
                        "{} is already on {upstream}. Squashing the fixup into it later rewrites published history and needs a force push.",
                        target.short_hash
                    )),
                    confirm_text: Some("Create fixup".to_string()),
                    cancel_text: Some("Cancel".to_string()),
                    ..Default::default()
                },
                cx,
            )
        });
        cx.spawn(async move |this, cx| {
            if !confirm.await {
                return;
            }
            this.update(cx, |this, cx| this.run_fixup(target, cx)).ok();
        })
        .detach();
    }

    fn run_fixup(&mut self, target: FixupCandidate, cx: &mut Context<Self>) {
        let Some(repo_path) = self.repo_path(cx) else {
            return;
        };
        let model = self.model.clone();
        let repo_id = self.repo_id.clone();
        let options = CommitOptions {
            sign: self.sign,
            fixup: Some(target.hash.clone()),
            ..Default::default()
        };
        let preferred_path = self.selected_path();

        self.run_action(BusyAction::Commit, cx, async move |this, cx| {
            let result = model
                .update(cx, |model, cx| {
                    model.commit(&repo_path, "", options, false, cx)
                })
                .await;
            Self::apply_operation_result(&this, &result, preferred_path, true, cx).await;
            if !result.success() {
                return;
            }
            let request = SyncRequest::new()
                .status(result.status().is_none())
                .branches(true)
                .operation_or(result.operation(), "commit")
                .result(result.clone());
            model
                .update(cx, |model, cx| {
                    model.sync_repository_read_model(repo_id.as_str(), request, cx)
                })
                .await;
            this.update(cx, |_, cx| {
                cx.emit(ChangesViewEvent::CommitCreated {
                    result: Box::new(result),
                    sync_already_applied: true,
                });
            })
            .ok();
        });
    }

    /// "Stash selected files…": el diálogo de stash con los archivos marcados
    /// o, si `file` no lo está, solo con él ([`logic::stash_selection`]).
    pub fn stash_selected_files(&mut self, file: &ChangedFile, cx: &mut Context<Self>) {
        let paths = logic::stash_selection(file, &self.files);
        self.model
            .update(cx, |model, cx| model.handle_stash_paths(paths, cx));
    }

    // ------------------------------------------------------------------
    // Plantilla de commit
    // ------------------------------------------------------------------

    /// `commit.template`: si no hay borrador, el mensaje empieza con la
    /// plantilla (sin sus comentarios, como la deja git).
    ///
    /// Se relee al abrir la vista, al volver a "Local Changes", tras un commit
    /// y al quedar la caja vacía, en un hilo de fondo: un `git config` y la
    /// fecha del archivo (el archivo solo si cambió). Si la plantilla cambió o
    /// se quitó y la caja está vacía o con la plantilla anterior sin tocar, se
    /// pone la vigente; un borrador nunca se pisa.
    fn refresh_commit_template(&mut self, cx: &mut Context<Self>) {
        if self.is_file_tab() {
            return;
        }
        let Some(repo_path) = self.repo_path(cx) else {
            return;
        };
        self.template_generation += 1;
        let generation = self.template_generation;
        let last = self.template_stamp.clone();
        let read = cx.background_spawn(async move {
            mergeon_git_query::read_commit_template_if_changed(Path::new(&repo_path), last.as_ref())
        });
        let window = self.window_handle;
        cx.spawn(async move |this, cx| {
            let read = match read.await {
                Ok(read) => read,
                Err(error) => {
                    log::debug!("{error}");
                    return;
                }
            };
            cx.update_window(window, |_, window, cx| {
                this.update(cx, |this, cx| {
                    if generation == this.template_generation {
                        this.apply_commit_template(read, window, cx);
                    }
                })
                .ok();
            })
            .ok();
        })
        .detach();
    }

    fn apply_commit_template(
        &mut self,
        read: CommitTemplateRead,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !read.changed_since(self.template_stamp.as_ref()) {
            return;
        }
        // Lo que hay escrito es "nada" si está vacío o es la plantilla de
        // antes tal cual: eso se puede reemplazar.
        let untouched = !self.amend && self.message_is_untouched(cx);
        self.template_stamp = read.stamp;
        self.commit_template = read
            .content
            .as_deref()
            .map(logic::commit_template_message)
            .filter(|message| !message.is_empty());
        if untouched {
            self.fill_with_template(window, cx);
        }
    }

    /// El asunto o el cuerpo cambiaron: se guarda el borrador y, si la caja
    /// acaba de quedarse vacía, se mira si la plantilla sigue siendo la misma.
    fn commit_message_changed(&mut self, cx: &mut Context<Self>) {
        self.remember_commit_draft(cx);
        let empty = self.commit_message(cx).trim().is_empty();
        if empty && !self.message_was_empty {
            self.refresh_commit_template(cx);
        }
        self.message_was_empty = empty;
        cx.notify();
    }

    /// Pone la plantilla (o nada, si no hay) como asunto y cuerpo.
    fn fill_with_template(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (title, body) = self
            .commit_template
            .as_deref()
            .map(logic::split_commit_message)
            .unwrap_or_default();
        self.set_commit_message_parts(title, body, window, cx);
        cx.notify();
    }

    /// El mensaje es el de la plantilla tal cual (o no hay nada escrito).
    fn message_is_untouched(&self, cx: &App) -> bool {
        let message = self.commit_message(cx);
        message.trim().is_empty()
            || self
                .commit_template
                .as_deref()
                .is_some_and(|template| message.trim() == template.trim())
    }

    // ------------------------------------------------------------------
    // Carga
    // ------------------------------------------------------------------

    fn clear_selection(&mut self) {
        self.selected_file = None;
        self.file_diff = SharedString::default();
        self.file_content = SharedString::default();
        self.content_path = None;
        self.conflict_versions = None;
    }

    /// `loadChanges`: el estado del repositorio (el que ya tenga el modelo, o
    /// una lectura si no tiene ninguno) y la selección que le corresponde.
    fn load_changes(&mut self, cx: &mut Context<Self>) -> Task<()> {
        let Some(has_status) = self
            .repository(cx)
            .map(|repository| repository.status.is_some())
        else {
            self.set_files(Vec::new());
            self.selected_file = None;
            cx.notify();
            return Task::ready(());
        };

        self.loading_changes = true;
        cx.notify();
        // El estado que ya tenga el modelo vale; si no tiene ninguno, se lee.
        let read = (!has_status).then(|| {
            self.model.update(cx, |model, cx| {
                model.sync_repository_read_model(
                    self.repo_id.as_str(),
                    SyncRequest::new().status(true).changes_count(true),
                    cx,
                )
            })
        });

        cx.spawn(async move |this, cx| {
            if let Some(read) = read {
                read.await;
            }
            let selection = this
                .update(cx, |this, cx| this.select_after_load(cx))
                .unwrap_or_else(|_| Task::ready(()));
            selection.await;
            this.update(cx, |this, cx| {
                this.loading_changes = false;
                cx.notify();
            })
            .ok();
        })
    }

    /// Lo que `loadChanges` hace una vez tiene el estado.
    fn select_after_load(&mut self, cx: &mut Context<Self>) -> Task<()> {
        self.absorb_status(cx);
        if self.seen_status.is_none() {
            self.set_files(Vec::new());
            self.clear_selection();
            cx.notify();
            return Task::ready(());
        }

        if self.is_file_tab() {
            return self.select_tab_file(cx);
        }

        let matching = self
            .selected_file
            .as_ref()
            .and_then(|selected| self.same_side(selected));
        let in_conflict = self
            .repository(cx)
            .is_some_and(|repository| repository.conflict_state.in_conflict);
        // Si hay conflictos, el primero; si no, el primer archivo. En "Local
        // Changes" no se elige nada por defecto: el diff espera al clic.
        let pick_default = !self.is_list_only();
        let next = matching
            .or_else(|| {
                (pick_default && in_conflict)
                    .then(|| self.files.iter().find(|file| is_conflicted(file)).cloned())
                    .flatten()
            })
            .or_else(|| pick_default.then(|| self.files.first().cloned()).flatten());

        match next {
            Some(file) => {
                self.selected_file = Some(file.clone());
                cx.notify();
                self.load_diff(Some(file), cx)
            }
            None => {
                self.clear_selection();
                cx.notify();
                Task::ready(())
            }
        }
    }

    fn current_last_commit_key(&self, cx: &App) -> Option<(Option<String>, Option<String>)> {
        let repository = self.repository(cx)?;
        let head = repository
            .status
            .as_ref()
            .and_then(|status| status.head_short_hash.clone());
        Some((repository.current_branch.clone(), head))
    }

    fn refresh_last_commit(&mut self, cx: &mut Context<Self>) {
        if self.is_file_tab() {
            return;
        }
        let key = self.current_last_commit_key(cx);
        if key == self.last_commit_key {
            return;
        }
        self.last_commit_key = key;
        self.load_last_commit(cx).detach();
    }

    /// `loadLastCommit`: el commit que "Amend last commit" enmendaría. En un
    /// repositorio sin commits, enmendar simplemente no se ofrece.
    fn load_last_commit(&mut self, cx: &mut Context<Self>) -> Task<()> {
        let Some(path) = self.repo_path(cx) else {
            self.last_commit = None;
            return Task::ready(());
        };
        self.last_commit_request += 1;
        let version = self.last_commit_request;
        // El commit de HEAD, no el más reciente de todas las refs: con
        // `getCommits({ maxCount: 1 })` (que lee `--all`) el original tomaba
        // el del último stash y lo ofrecía para enmendar.
        let read = self.model.update(cx, |model, cx| {
            model.search_commits(
                &path,
                CommitSearchOptions {
                    max_count: Some(1),
                    hash: Some("HEAD".to_string()),
                    ..Default::default()
                },
                cx,
            )
        });
        cx.spawn(async move |this, cx| {
            let last_commit = read.await.ok().and_then(logic::head_commit);
            this.update(cx, |this, cx| {
                if version != this.last_commit_request {
                    return;
                }
                this.last_commit = last_commit;
                cx.notify();
            })
            .ok();
        })
    }

    /// `selectTabFile`: en una pestaña de archivo, el archivo es el de la
    /// pestaña, en el lado de la pestaña. Si ya no tiene cambios ahí, la
    /// pestaña se queda vacía en su contexto en vez de elegir otro archivo.
    fn select_tab_file(&mut self, cx: &mut Context<Self>) -> Task<()> {
        let Some(tab) = self.tab.clone() else {
            return Task::ready(());
        };
        if !self.is_active(cx) {
            self.diff_request += 1;
            return Task::ready(());
        }

        let file = if tab.source == TabSource::Index {
            logic::staged_files(&self.files)
                .into_iter()
                .find(|file| file.path == tab.path)
        } else {
            logic::conflicted_files(&self.files)
                .into_iter()
                .chain(logic::unstaged_files(&self.files))
                .chain(logic::untracked_files(&self.files))
                .find(|file| file.path == tab.path)
        };

        match file {
            Some(file) => {
                self.selected_file = Some(file.clone());
                cx.notify();
                self.load_diff(Some(file), cx)
            }
            None => {
                self.diff_request += 1;
                self.clear_selection();
                self.loading_diff = false;
                cx.notify();
                Task::ready(())
            }
        }
    }

    /// `reconcileSelectedFile`: tras un cambio de estado, qué archivo queda
    /// elegido: el que se prefiere, el que ya lo estaba, el primer conflicto o
    /// el primero de la lista.
    fn reconcile_selected_file(
        &mut self,
        preferred_path: Option<&str>,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        if self.is_file_tab() {
            return self.select_tab_file(cx);
        }
        if self.repository(cx).is_none() || self.files.is_empty() {
            self.clear_selection();
            cx.notify();
            return Task::ready(());
        }

        let find = |path: &str| self.files.iter().find(|file| file.path == path).cloned();
        // Sigue en el mismo lado (sin preparar / preparado) si todavía lo hay.
        let same_side = self
            .selected_file
            .as_ref()
            .filter(|selected| preferred_path.is_none_or(|path| path == selected.path))
            .and_then(|selected| self.same_side(selected));
        // En "Local Changes" no se elige otro archivo por su cuenta.
        let pick_default = !self.is_list_only();
        let next = same_side
            .or_else(|| preferred_path.and_then(find))
            .or_else(|| {
                self.selected_file
                    .as_ref()
                    .and_then(|selected| find(&selected.path))
            })
            .or_else(|| {
                pick_default
                    .then(|| self.files.iter().find(|file| is_conflicted(file)).cloned())
                    .flatten()
            })
            .or_else(|| pick_default.then(|| self.files.first().cloned()).flatten());

        match next {
            Some(file) => {
                self.selected_file = Some(file.clone());
                cx.notify();
                self.load_diff(Some(file), cx)
            }
            None => {
                self.clear_selection();
                cx.notify();
                Task::ready(())
            }
        }
    }

    /// El archivo de la lista que es `selected` en su mismo lado: el lado sin
    /// preparar de un archivo a medio preparar sigue siéndolo tras releer el
    /// estado (en la lista, ese lado lleva el índice en blanco).
    fn same_side(&self, selected: &ChangedFile) -> Option<ChangedFile> {
        let file = self.files.iter().find(|file| file.path == selected.path)?;
        let unstaged_side = selected.index == " "
            && !is_conflicted(file)
            && !is_untracked(file)
            && !file.working_dir.trim().is_empty();
        Some(if unstaged_side {
            ChangedFile {
                index: " ".to_string(),
                ..file.clone()
            }
        } else {
            file.clone()
        })
    }

    /// `loadDiff`: el diff del archivo, o su contenido si está en conflicto.
    fn load_diff(&mut self, file: Option<ChangedFile>, cx: &mut Context<Self>) -> Task<()> {
        if !self.is_active(cx) {
            return Task::ready(());
        }
        self.diff_request += 1;
        let version = self.diff_request;
        // En una pestaña el desplazamiento es el que la pestaña recuerda.
        let scroll = self
            .tab
            .as_ref()
            .map(|tab| ScrollPosition::new(tab.scroll.top as f32, tab.scroll.left as f32));
        self.file_error = SharedString::default();

        let Some((file, repo_path)) = file.zip(self.repo_path(cx)) else {
            self.file_diff = SharedString::default();
            cx.notify();
            return Task::ready(());
        };

        self.loading_diff = true;
        cx.notify();

        if is_conflicted(&file) {
            self.file_diff = SharedString::default();
            self.diff_too_large = false;
            self.diff_whole_file_fell_back = false;
            self.diff_size = 0;
            // El contenido de otro archivo no es el de este: el resolvedor no
            // debe tomarlo por suyo mientras llega el bueno.
            if self.content_path.as_deref() != Some(file.path.as_str()) {
                self.file_content = SharedString::default();
                self.content_path = None;
                self.conflict_versions = None;
            }
            let read = self.model.update(cx, |model, cx| {
                model.read_conflict_versions(&repo_path, &file.path, cx)
            });
            return cx.spawn(async move |this, cx| {
                let result = read.await;
                this.update(cx, |this, cx| {
                    if version != this.diff_request {
                        return;
                    }
                    match result {
                        Ok(versions) => {
                            this.file_content = versions.working.clone().unwrap_or_default().into();
                            this.conflict_versions = Some(Arc::new(versions));
                            this.content_path = Some(file.path);
                        }
                        Err(error) => this.diff_failed(error.message()),
                    }
                    this.diff_loaded(scroll, cx);
                })
                .ok();
            });
        }

        let is_staged = match &self.tab {
            Some(tab) => tab.source == TabSource::Index,
            None => !file.index.is_empty() && file.index != " ",
        };
        let preferences = *self.model.read(cx).diff_preferences();
        let options = DiffOptions {
            ignore_whitespace: preferences.ignore_whitespace,
            whole_file: preferences.whole_file,
        };
        self.load_image(&file, is_staged, &repo_path, version, cx);
        let read = self.model.update(cx, |model, cx| {
            model.get_diff(
                &repo_path,
                &file.path,
                is_staged,
                is_untracked(&file),
                options,
                cx,
            )
        });
        cx.spawn(async move |this, cx| {
            let result = read.await;
            this.update(cx, |this, cx| {
                if version != this.diff_request {
                    return;
                }
                match result {
                    Ok(payload) => {
                        this.file_diff = payload.diff.into();
                        this.diff_too_large = payload.too_large;
                        this.diff_whole_file_fell_back = payload.whole_file_fell_back;
                        this.diff_size = payload.size;
                        this.file_content = SharedString::default();
                        this.content_path = None;
                        this.conflict_versions = None;
                    }
                    Err(error) => this.diff_failed(error.message()),
                }
                this.diff_loaded(scroll, cx);
            })
            .ok();
        })
    }

    /// Si el archivo es una imagen, sus dos versiones (lo preparado: HEAD →
    /// índice; lo no preparado: índice → disco) para verlas lado a lado.
    fn load_image(
        &mut self,
        file: &ChangedFile,
        staged: bool,
        repo_path: &str,
        version: u64,
        cx: &mut Context<Self>,
    ) {
        if !is_image_path(&file.path) {
            self.image_viewer.update(cx, |viewer, cx| viewer.clear(cx));
            return;
        }
        self.image_viewer
            .update(cx, |viewer, cx| viewer.set_loading(cx));
        let request = mergeon_backend::ImagePairRequest::WorkingTree {
            path: file.path.clone(),
            staged,
            untracked: is_untracked(file),
        };
        let read = self.model.update(cx, |model, cx| {
            model.read_image_pair(repo_path, request, cx)
        });
        let path = file.path.clone();
        cx.spawn(async move |this, cx| {
            let pair = read.await;
            // Medir y preparar los bytes tampoco va en el hilo de la interfaz.
            let images = cx
                .background_spawn(async move {
                    pair.map(|pair| {
                        let version = |blob: mergeon_backend::ImageBlob| {
                            ImageVersion::new(&path, blob.bytes, blob.size, blob.too_large)
                        };
                        ImageDiff {
                            before: pair.before.map(version),
                            after: pair.after.map(version),
                        }
                    })
                })
                .await;
            this.update(cx, |this, cx| {
                if version != this.diff_request {
                    return;
                }
                this.image_viewer.update(cx, |viewer, cx| match images {
                    Ok(images) => viewer.set_images(images, cx),
                    Err(error) => viewer.set_error(error.message().to_string(), cx),
                });
            })
            .ok();
        })
        .detach();
    }

    fn diff_failed(&mut self, message: &str) {
        self.file_error = if message.is_empty() {
            "Unable to load this file.".into()
        } else {
            message.to_string().into()
        };
        self.file_diff = SharedString::default();
        self.diff_too_large = false;
        self.diff_whole_file_fell_back = false;
        self.diff_size = 0;
    }

    fn diff_loaded(&mut self, scroll: Option<ScrollPosition>, cx: &mut Context<Self>) {
        self.loading_diff = false;
        // En una pestaña se vuelve a donde estaba; tras "Whole file", a la
        // línea que se leía. El visor recibe ya el diff para que el
        // desplazamiento se le pueda poner encima.
        let anchor = self.line_anchor.take();
        if scroll.is_some() || anchor.is_some() {
            let diff = self.file_diff.clone();
            let path = self.selected_path().unwrap_or_default();
            self.diff_viewer.update(cx, |viewer, cx| {
                viewer.set_diff(diff, path, cx);
                match (&anchor, scroll) {
                    (Some(anchor), _) => viewer.scroll_to_line(anchor, cx),
                    (None, Some(scroll)) => viewer.set_scroll_position(scroll, cx),
                    (None, None) => {}
                }
            });
        }
        cx.notify();
    }

    /// `rememberScroll`: el desplazamiento del visor es el de la pestaña.
    fn remember_scroll(&mut self, position: ScrollPosition, cx: &mut Context<Self>) {
        if self.loading_diff || !self.is_active(cx) {
            return;
        }
        let (Some(tab_id), Some(path)) = (self.tab_id().map(str::to_string), self.repo_path(cx))
        else {
            return;
        };
        let scroll = Scroll {
            top: f64::from(position.top),
            left: f64::from(position.left),
        };
        if let Some(tab) = &mut self.tab {
            tab.scroll = scroll;
        }
        self.model.update(cx, |model, cx| {
            model.remember_workspace_scroll(&path, &tab_id, scroll, cx);
        });
    }

    // ------------------------------------------------------------------
    // Selección y pestañas
    // ------------------------------------------------------------------

    /// `emit('open-file', descriptor)`.
    fn open_file(&mut self, descriptor: FileTabDescriptor, replace: bool, cx: &mut Context<Self>) {
        let Some(path) = self.repo_path(cx) else {
            return;
        };
        self.model.update(cx, |model, cx| {
            model.open_workspace_file(&path, &descriptor, replace, cx);
        });
        cx.emit(ChangesViewEvent::OpenFile {
            descriptor,
            replace,
        });
    }

    /// `openFileInNewTab`: el doble clic.
    pub fn open_file_in_new_tab(&mut self, file: &ChangedFile, cx: &mut Context<Self>) {
        self.open_file(file_descriptor(file), false, cx);
    }

    /// `selectFile`: el clic en un archivo de la lista.
    pub fn select_file(&mut self, file: &ChangedFile, cx: &mut Context<Self>) {
        self.multi_selection = None;
        match self.mode {
            // En una pestaña de archivo el clic cambia el archivo de esta
            // pestaña; el doble clic y el menú son los que abren otra.
            ChangesMode::FileTab { .. } => self.open_file(file_descriptor(file), true, cx),
            // En "Local Changes" y en la vista completa el diff está al lado:
            // el clic lo enseña y el doble clic abre la pestaña del archivo.
            ChangesMode::List | ChangesMode::Full => {
                self.selected_file = Some(file.clone());
                cx.notify();
                self.load_diff(Some(file.clone()), cx).detach();
            }
        }
    }

    /// Las flechas de la lista: lo que el clic (enseñar el diff de ese
    /// archivo, o en una pestaña de archivo, cambiarla a ese archivo).
    pub fn move_selection(&mut self, file: &ChangedFile, cx: &mut Context<Self>) {
        self.select_file(file, cx);
    }

    /// `Shift+clic`, `Ctrl/Cmd+clic` o `Ctrl/Cmd+A` en la lista: varios
    /// archivos de una sección. El panel derecho enseña su resumen.
    pub fn select_many(&mut self, selection: &MultiSelection, cx: &mut Context<Self>) {
        if self.is_file_tab() || selection.paths.len() < 2 {
            return;
        }
        self.multi_selection = Some(Rc::new(selection.clone()));
        cx.notify();
    }

    /// La selección múltiple, si hay una.
    pub fn multi_selection(&self) -> Option<&MultiSelection> {
        self.multi_selection.as_deref()
    }

    /// Los archivos de la selección múltiple, en el orden de su sección.
    pub fn multi_selected_files(&mut self) -> Vec<ChangedFile> {
        let Some(multi) = self.multi_selection.clone() else {
            return Vec::new();
        };
        self.section_files(multi.section)
            .into_iter()
            .filter(|file| multi.paths.contains(&file.path))
            .collect()
    }

    /// La sección de la lista del archivo elegido.
    fn selected_section(&self) -> Option<ChangesSection> {
        let file = self.selected_file.as_ref()?;
        Some(if is_conflicted(file) {
            ChangesSection::Conflict
        } else if file.index.trim().is_empty() || is_untracked(file) {
            ChangesSection::Unstaged
        } else {
            ChangesSection::Staged
        })
    }

    /// El lado del archivo elegido que enseña el diff (nada si está en
    /// conflicto).
    pub fn selected_side(&self) -> Option<DiffSide> {
        if self.is_conflict() {
            return None;
        }
        if let Some(tab) = &self.tab {
            return Some(match tab.source {
                TabSource::Index => DiffSide::Staged,
                _ => DiffSide::Unstaged,
            });
        }
        match self.selected_section()? {
            ChangesSection::Staged => Some(DiffSide::Staged),
            ChangesSection::Unstaged => Some(DiffSide::Unstaged),
            ChangesSection::Conflict => None,
        }
    }

    /// El archivo elegido tiene cambios preparados y sin preparar.
    fn selected_has_both_sides(&self) -> bool {
        self.selected_file.as_ref().is_some_and(|selected| {
            self.files
                .iter()
                .any(|file| file.path == selected.path && logic::is_partially_staged(file))
        })
    }

    /// El segmentado `Unstaged | Staged`: enseña el otro lado del archivo.
    pub fn switch_side(&mut self, side: DiffSide, cx: &mut Context<Self>) {
        if self.is_file_tab() || self.selected_side() == Some(side) {
            return;
        }
        let Some(path) = self.selected_path() else {
            return;
        };
        let section = match side {
            DiffSide::Unstaged => ChangesSection::Unstaged,
            DiffSide::Staged => ChangesSection::Staged,
        };
        if let Some(file) = self
            .section_files(section)
            .into_iter()
            .find(|file| file.path == path)
        {
            self.select_file(&file, cx);
        }
    }

    /// Lo que pide la lista sobre uno o varios archivos (Espacio, D, una
    /// casilla). Al preparar o descartar el elegido, la selección pasa al
    /// siguiente de la misma sección antes de que el archivo se vaya.
    pub fn handle_file_action(&mut self, action: &FileAction, cx: &mut Context<Self>) {
        if self.is_interaction_locked() || action.files.is_empty() {
            return;
        }
        match action.kind {
            FileActionKind::Stage | FileActionKind::Unstage => {
                if let Some(next) = &action.next {
                    self.select_file(next, cx);
                }
                self.multi_selection = None;
                let busy = if action.kind == FileActionKind::Stage {
                    BusyAction::StageFile
                } else {
                    BusyAction::UnstageFile
                };
                self.change_files(action.files.clone(), busy, cx);
            }
            FileActionKind::Discard => {
                self.confirm_discard(action.files.clone(), action.next.clone(), cx);
            }
        }
    }

    /// Prepara los archivos de la selección múltiple.
    pub fn stage_selection(&mut self, cx: &mut Context<Self>) {
        self.act_on_selection(FileActionKind::Stage, cx);
    }

    /// Quita del stage los archivos de la selección múltiple.
    pub fn unstage_selection(&mut self, cx: &mut Context<Self>) {
        self.act_on_selection(FileActionKind::Unstage, cx);
    }

    /// Descarta (tras confirmarlo) los archivos de la selección múltiple.
    pub fn discard_selection(&mut self, cx: &mut Context<Self>) {
        self.act_on_selection(FileActionKind::Discard, cx);
    }

    fn act_on_selection(&mut self, kind: FileActionKind, cx: &mut Context<Self>) {
        let Some(section) = self.multi_selection.as_ref().map(|multi| multi.section) else {
            return;
        };
        let allowed = matches!(
            (kind, section),
            (
                FileActionKind::Stage | FileActionKind::Discard,
                ChangesSection::Unstaged
            ) | (FileActionKind::Unstage, ChangesSection::Staged)
        );
        if !allowed {
            return;
        }
        let files = self.multi_selected_files();
        self.handle_file_action(
            &FileAction {
                kind,
                section,
                files,
                next: None,
            },
            cx,
        );
    }

    /// `handleConflictFileSelect`: clic en un archivo de la lista del resolvedor.
    fn handle_conflict_file_select(&mut self, path: &str, cx: &mut Context<Self>) {
        if let Some(file) = self.files.iter().find(|file| file.path == path).cloned() {
            self.select_file(&file, cx);
        }
    }

    // ------------------------------------------------------------------
    // Mutaciones
    // ------------------------------------------------------------------

    /// `runLocalChangesAction`: una mutación a la vez. Mientras dura, la vista
    /// queda ocupada con `action`.
    fn run_action(
        &mut self,
        action: BusyAction,
        cx: &mut Context<Self>,
        flow: impl AsyncFnOnce(WeakEntity<Self>, &mut AsyncApp) + 'static,
    ) {
        if self.is_interaction_locked() {
            return;
        }
        self.busy_action = Some(action);
        cx.notify();
        cx.spawn(async move |this, cx| {
            flow(this.clone(), cx).await;
            this.update(cx, |this, cx| {
                this.busy_action = None;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// `applyOperationResult`: aplica el resultado al repositorio, relee lo
    /// que haga falta y deja elegido el archivo que corresponde.
    async fn apply_operation_result(
        this: &WeakEntity<Self>,
        result: &impl StructuredOutcome,
        preferred_path: Option<String>,
        refresh_status: bool,
        cx: &mut AsyncApp,
    ) {
        let summary: OperationSummary = result.to_operation_result();
        let Ok((model, repo_id, preferred_path)) = this.update(cx, |this, cx| {
            // Lo que estaba elegido antes de que el resultado toque nada.
            let preferred_path = preferred_path.or_else(|| this.selected_path());
            this.model.update(cx, |model, cx| {
                model.apply_repository_operation_result(this.repo_id.as_str(), &summary, cx);
            });
            this.absorb_status(cx);
            (this.model.clone(), this.repo_id.clone(), preferred_path)
        }) else {
            return;
        };

        let operation = summary.operation.clone().filter(|name| !name.is_empty());
        let mut resolved_status = summary.status.clone();
        if resolved_status.is_none() && refresh_status {
            let mut request = SyncRequest::new().status(true).changes_count(true);
            if let Some(operation) = operation {
                request = request.operation(operation);
            }
            resolved_status = model
                .update(cx, |model, cx| {
                    model.sync_repository_read_model(repo_id.as_str(), request, cx)
                })
                .await
                .and_then(|outcome| outcome.status);
        } else if let Some(operation) = operation {
            model
                .update(cx, |model, cx| {
                    model.sync_repository_read_model(
                        repo_id.as_str(),
                        SyncRequest::new().operation(operation),
                        cx,
                    )
                })
                .await;
        }

        let reconciled = this.update(cx, |this, cx| {
            if let Some(status) = resolved_status {
                this.set_files(status.files);
            }
            this.absorb_status(cx);
            this.reconcile_selected_file(preferred_path.as_deref(), cx)
        });
        if let Ok(reconciled) = reconciled {
            reconciled.await;
        }
    }

    /// `stageFile`.
    pub fn stage_file(&mut self, file: &ChangedFile, cx: &mut Context<Self>) {
        self.change_files(vec![file.clone()], BusyAction::StageFile, cx);
    }

    /// `unstageFile`.
    pub fn unstage_file(&mut self, file: &ChangedFile, cx: &mut Context<Self>) {
        self.change_files(vec![file.clone()], BusyAction::UnstageFile, cx);
    }

    /// Prepara o quita del stage `files`, uno tras otro en una sola mutación.
    /// La selección se queda donde esté: si es uno de ellos (y su sección
    /// quedó vacía), lo sigue al otro lado.
    fn change_files(
        &mut self,
        files: Vec<ChangedFile>,
        action: BusyAction,
        cx: &mut Context<Self>,
    ) {
        let paths: Vec<String> = files
            .iter()
            .filter(|file| !is_conflicted(file))
            .map(|file| file.path.clone())
            .collect();
        if paths.is_empty() {
            return;
        }
        let Some(repo_path) = self.repo_path(cx) else {
            return;
        };
        let model = self.model.clone();
        let preferred_path = self
            .selected_path()
            .or_else(|| (paths.len() == 1).then(|| paths[0].clone()));
        self.run_action(action, cx, async move |this, cx| {
            let mut last = None;
            for path in &paths {
                let result = model
                    .update(cx, |model, cx| match action {
                        BusyAction::UnstageFile => model.unstage(&repo_path, path, cx),
                        _ => model.stage(&repo_path, path, cx),
                    })
                    .await;
                let failed = !result.success();
                last = Some(result);
                if failed {
                    break;
                }
            }
            if let Some(result) = last {
                Self::apply_operation_result(&this, &result, preferred_path, true, cx).await;
            }
        });
    }

    /// "Stage file" de la cabecera del diff, sobre el lado sin preparar.
    pub fn stage_selected(&mut self, cx: &mut Context<Self>) {
        if self.selected_side() != Some(DiffSide::Unstaged) {
            return;
        }
        if let Some(file) = self.selected_file.clone() {
            self.stage_file(&file, cx);
        }
    }

    /// "Unstage file" de la cabecera del diff, sobre el lado preparado.
    pub fn unstage_selected(&mut self, cx: &mut Context<Self>) {
        if self.selected_side() != Some(DiffSide::Staged) {
            return;
        }
        if let Some(file) = self.selected_file.clone() {
            self.unstage_file(&file, cx);
        }
    }

    /// `discardChanges`: el botón "Discard" del panel, sobre el archivo
    /// elegido. Pregunta antes, como D y el menú, y solo existe con el lado
    /// sin preparar a la vista: con el preparado a la vista descartaría
    /// cambios que no se ven.
    pub fn discard_selected(&mut self, cx: &mut Context<Self>) {
        if self.selected_side() != Some(DiffSide::Unstaged) {
            return;
        }
        let Some(file) = self.selected_file.clone() else {
            return;
        };
        self.confirm_discard(vec![file], None, cx);
    }

    /// `handleDiscard`: "Discard Changes" del menú, que pregunta antes.
    pub fn discard_file(&mut self, file: &ChangedFile, cx: &mut Context<Self>) {
        self.confirm_discard(vec![file.clone()], None, cx);
    }

    /// Descarta lo no preparado de `files` tras confirmarlo; si se confirma,
    /// la selección pasa antes a `next`.
    fn confirm_discard(
        &mut self,
        files: Vec<ChangedFile>,
        next: Option<ChangedFile>,
        cx: &mut Context<Self>,
    ) {
        // Un conflicto se resuelve, no se descarta.
        let paths: Vec<String> = files
            .iter()
            .filter(|file| !is_conflicted(file))
            .map(|file| file.path.clone())
            .collect();
        if paths.is_empty() {
            return;
        }
        let confirmed = self.model.update(cx, |model, cx| {
            model.confirm(
                ConfirmOptions {
                    title: Some("Discard changes".to_string()),
                    description: Some(logic::discard_description(&paths)),
                    confirm_text: Some("Discard".to_string()),
                    cancel_text: Some("Cancel".to_string()),
                    variant: Some("destructive".to_string()),
                    ..Default::default()
                },
                cx,
            )
        });
        cx.spawn(async move |this, cx| {
            if !confirmed.await {
                return;
            }
            this.update(cx, |this, cx| {
                if this.is_interaction_locked() {
                    return;
                }
                let Some(repo_path) = this.repo_path(cx) else {
                    return;
                };
                if let Some(next) = &next {
                    this.select_file(next, cx);
                }
                this.multi_selection = None;
                let preferred_path = this.selected_path();
                let model = this.model.clone();
                this.run_action(BusyAction::DiscardFile, cx, async move |this, cx| {
                    let result = model
                        .update(cx, |model, cx| match paths.as_slice() {
                            [one] => model.discard_changes(&repo_path, one, cx),
                            _ => model.discard_all(&repo_path, Some(paths.clone()), cx),
                        })
                        .await;
                    Self::apply_operation_result(&this, &result, preferred_path, true, cx).await;
                });
            })
            .ok();
        })
        .detach();
    }

    /// `stageAll`.
    pub fn stage_all(&mut self, cx: &mut Context<Self>) {
        self.change_all(BusyAction::StageAll, cx);
    }

    /// `unstageAll`.
    pub fn unstage_all(&mut self, cx: &mut Context<Self>) {
        self.change_all(BusyAction::UnstageAll, cx);
    }

    fn change_all(&mut self, action: BusyAction, cx: &mut Context<Self>) {
        let Some(repo_path) = self.repo_path(cx) else {
            return;
        };
        let model = self.model.clone();
        let preferred_path = self.selected_path();
        self.run_action(action, cx, async move |this, cx| {
            let result = model
                .update(cx, |model, cx| match action {
                    BusyAction::StageAll => model.stage_all(&repo_path, cx),
                    _ => model.unstage_all(&repo_path, cx),
                })
                .await;
            Self::apply_operation_result(&this, &result, preferred_path, true, cx).await;
        });
    }

    /// `discardAll`: descarta todo lo que no está preparado, tras confirmarlo.
    pub fn discard_all(&mut self, cx: &mut Context<Self>) {
        let unstaged_count = logic::unstaged_files(&self.files).len();
        let confirmed = self.model.update(cx, |model, cx| {
            model.confirm(
                ConfirmOptions {
                    title: Some("Discard all changes".to_string()),
                    description: Some(format!(
                        "This will discard all unstaged changes ({unstaged_count} files)."
                    )),
                    confirm_text: Some("Discard all".to_string()),
                    cancel_text: Some("Cancel".to_string()),
                    variant: Some("destructive".to_string()),
                    ..Default::default()
                },
                cx,
            )
        });
        cx.spawn(async move |this, cx| {
            if !confirmed.await {
                return;
            }
            this.update(cx, |this, cx| {
                let Some(repo_path) = this.repo_path(cx) else {
                    return;
                };
                let model = this.model.clone();
                this.run_action(BusyAction::DiscardAll, cx, async move |this, cx| {
                    // Las rutas se leen al descartar, no al preguntar.
                    let Ok(paths) = this.read_with(cx, |this, _| {
                        logic::unstaged_files(&this.files)
                            .into_iter()
                            .map(|file| file.path)
                            .collect::<Vec<_>>()
                    }) else {
                        return;
                    };
                    let result = model
                        .update(cx, |model, cx| {
                            model.discard_all(&repo_path, Some(paths), cx)
                        })
                        .await;
                    Self::apply_operation_result(&this, &result, None, true, cx).await;
                });
            })
            .ok();
        })
        .detach();
    }

    /// `handleIgnore`: añade el archivo a `.gitignore`.
    pub fn ignore_file(&mut self, file: &ChangedFile, cx: &mut Context<Self>) {
        let Some(repo_path) = self.repo_path(cx) else {
            return;
        };
        let model = self.model.clone();
        let repo_id = self.repo_id.clone();
        let file_path = file.path.clone();
        self.run_action(BusyAction::Ignore, cx, async move |this, cx| {
            model
                .update(cx, |model, cx| {
                    model.add_to_ignore(&repo_path, &file_path, cx)
                })
                .await;
            let outcome = model
                .update(cx, |model, cx| {
                    model.sync_repository_read_model(
                        repo_id.as_str(),
                        SyncRequest::new()
                            .status(true)
                            .changes_count(true)
                            .operation("ignore"),
                        cx,
                    )
                })
                .await;
            let Some(status) = outcome.and_then(|outcome| outcome.status) else {
                return;
            };
            let reconciled = this.update(cx, |this, cx| {
                this.set_files(status.files);
                this.absorb_status(cx);
                this.reconcile_selected_file(Some(&file_path), cx)
            });
            if let Ok(reconciled) = reconciled {
                reconciled.await;
            }
        });
    }

    /// `handleReveal`: enseña el archivo en el explorador del sistema.
    pub fn reveal_file(&mut self, file: &ChangedFile, cx: &mut Context<Self>) {
        let Some(repo_path) = self.repo_path(cx) else {
            return;
        };
        let full_path = format!("{repo_path}/{}", file.path);
        self.model
            .update(cx, |model, cx| model.show_item_in_folder(&full_path, cx))
            .detach();
    }

    /// `copyPath`: la ruta del archivo, relativa al repositorio, al portapapeles.
    pub fn copy_path(&mut self, file: &ChangedFile, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(file.path.clone()));
        self.model.update(cx, |model, cx| {
            model.notify_success("Path copied to clipboard", cx);
        });
    }

    /// `toggleDiffView`.
    pub fn toggle_diff_view(&mut self, cx: &mut Context<Self>) {
        self.model
            .update(cx, |model, cx| model.toggle_diff_view(cx))
            .detach();
    }

    /// `handleToggleIgnoreWhitespace`: cambia lo que se le pide a git, así que
    /// hay que volver a pedir el diff.
    pub fn toggle_ignore_whitespace(&mut self, cx: &mut Context<Self>) {
        self.line_anchor = self
            .diff_viewer
            .update(cx, |viewer, _| viewer.line_anchor());
        self.model
            .update(cx, |model, cx| model.toggle_ignore_whitespace(cx))
            .detach();
        if let Some(file) = self.selected_file.clone() {
            self.load_diff(Some(file), cx).detach();
        }
    }

    /// "Whole file": el diff con el archivo entero; hay que volver a pedirlo.
    pub fn toggle_whole_file(&mut self, cx: &mut Context<Self>) {
        self.line_anchor = self
            .diff_viewer
            .update(cx, |viewer, _| viewer.line_anchor());
        self.model
            .update(cx, |model, cx| model.toggle_whole_file(cx))
            .detach();
        if let Some(file) = self.selected_file.clone() {
            self.load_diff(Some(file), cx).detach();
        }
    }

    /// "Mark as resolved" del resolvedor: si trae un aviso (quedan conflictos
    /// o marcadores, o se borra el archivo), se confirma antes.
    fn confirm_resolve(
        &mut self,
        resolution: ConflictResolution,
        warning: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(warning) = warning else {
            self.handle_resolve(resolution, cx);
            return;
        };
        let deleting = resolution == ConflictResolution::Delete;
        let confirmed = self.model.update(cx, |model, cx| {
            model.confirm(
                ConfirmOptions {
                    title: Some(if deleting {
                        "Delete file".to_string()
                    } else {
                        "Conflicts remain".to_string()
                    }),
                    description: Some(warning),
                    confirm_text: Some(if deleting {
                        "Delete and resolve".to_string()
                    } else {
                        "Mark as resolved".to_string()
                    }),
                    cancel_text: Some("Cancel".to_string()),
                    variant: deleting.then(|| "destructive".to_string()),
                    ..Default::default()
                },
                cx,
            )
        });
        cx.spawn(async move |this, cx| {
            if confirmed.await {
                this.update(cx, |this, cx| this.handle_resolve(resolution, cx))
                    .ok();
            }
        })
        .detach();
    }

    /// `handleResolve`: guarda el archivo resuelto (o se queda con una
    /// versión, o lo borra), lo prepara y pasa al siguiente conflicto.
    pub fn handle_resolve(&mut self, resolution: ConflictResolution, cx: &mut Context<Self>) {
        let (Some(file), Some(repo_path)) = (self.selected_file.clone(), self.repo_path(cx)) else {
            return;
        };
        self.is_resolving_conflict = true;
        cx.notify();

        let model = self.model.clone();
        cx.spawn(async move |this, cx| {
            let current_path = file.path;
            let result = model
                .update(cx, |model, cx| {
                    model.resolve_conflict_with(&repo_path, &current_path, resolution, cx)
                })
                .await;

            if result.success() {
                // Lo decidido sobre este archivo ya está guardado.
                this.update(cx, |this, cx| {
                    this.resolver.update(cx, |resolver, cx| resolver.reset(cx));
                })
                .ok();
                Self::apply_operation_result(&this, &result, None, false, cx).await;

                let next_pending = result
                    .result()
                    .and_then(|result| result.conflict_state.pending_files.first().cloned());
                let reconciled = this.update(cx, |this, cx| {
                    let next_file = next_pending
                        .and_then(|path| this.files.iter().find(|file| file.path == path).cloned());
                    match next_file {
                        Some(next_file) => {
                            this.select_file(&next_file, cx);
                            Task::ready(())
                        }
                        None => this.reconcile_selected_file(Some(&current_path), cx),
                    }
                });
                if let Ok(reconciled) = reconciled {
                    reconciled.await;
                }
            }

            this.update(cx, |this, cx| {
                this.is_resolving_conflict = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// `handleToggleAmend`.
    pub fn set_amend(&mut self, amend: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.amend = amend;
        cx.notify();
        if !amend {
            return;
        }
        // Enmendar parte del mensaje anterior (la plantilla sin tocar cuenta
        // como nada escrito).
        let empty = self.message_is_untouched(cx);
        if let Some(last_commit) = self
            .last_commit
            .as_ref()
            .filter(|commit| empty && !commit.message.is_empty())
        {
            let (title, body) = logic::split_commit_message(&logic::amend_message(last_commit));
            // `%b` acaba en salto de línea: no es parte del cuerpo.
            let body = body.trim_end().to_string();
            self.set_commit_message_parts(title, body, window, cx);
        }
    }

    /// Escribe asunto y cuerpo (enmendar, vaciar tras el commit).
    fn set_commit_message_parts(
        &mut self,
        title: String,
        body: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.commit_title
            .update(cx, |state, cx| state.set_value(title, window, cx));
        self.commit_body
            .update(cx, |state, cx| state.set_value(body, window, cx));
    }

    /// "Sign (SSH/GPG)": se recuerda para este repositorio.
    pub fn set_sign(&mut self, sign: bool, cx: &mut Context<Self>) {
        self.sign = sign;
        if let Some(path) = self.repo_path(cx) {
            self.model
                .update(cx, |model, cx| model.set_commit_sign(&path, sign, cx));
        }
        cx.notify();
    }

    /// `pushAfterCommit = $event`.
    pub fn set_push_after_commit(&mut self, push_after_commit: bool, cx: &mut Context<Self>) {
        self.push_after_commit = push_after_commit;
        cx.notify();
    }

    /// `handleCommit`: el commit y, si se pidió, el push.
    pub fn commit(&mut self, cx: &mut Context<Self>) {
        if !self.is_committable(cx) {
            return;
        }
        let Some(repo_path) = self.repo_path(cx) else {
            return;
        };

        let should_push = self.push_after_commit;
        let action = if should_push {
            BusyAction::CommitPush
        } else {
            BusyAction::Commit
        };
        let model = self.model.clone();
        let repo_id = self.repo_id.clone();
        let message = self.commit_message(cx).to_string();
        let options = CommitOptions {
            amend: self.amend,
            sign: self.sign,
            ..Default::default()
        };
        let preferred_path = self.selected_path();

        self.run_action(action, cx, async move |this, cx| {
            let commit_result = model
                .update(cx, |model, cx| {
                    model.commit(&repo_path, &message, options, should_push, cx)
                })
                .await;
            Self::apply_operation_result(&this, &commit_result, preferred_path, true, cx).await;

            if !commit_result.success() {
                if should_push {
                    let message = commit_result.message().to_string();
                    model.update(cx, |model, cx| {
                        model.notify_error(
                            if message.is_empty() {
                                "Could not create the commit."
                            } else {
                                &message
                            },
                            cx,
                        );
                    });
                }
                return;
            }

            let mut sync_already_applied = false;
            let mut push_failure_message = None;

            if should_push {
                let push_result = model
                    .update(cx, |model, cx| {
                        let options = PushOptions {
                            remote: "origin".to_string(),
                            auth_token: model.git_auth_token(),
                            ..Default::default()
                        };
                        model.push(&repo_path, options, true, cx)
                    })
                    .await;

                if push_result.succeeded_or_noop() {
                    let mut request = SyncRequest::new()
                        .status(true)
                        .branches(true)
                        .changes_count(true);
                    if let Some(operation) = push_result.operation() {
                        request = request.operation(operation);
                    }
                    model
                        .update(cx, |model, cx| {
                            model.sync_repository_read_model(repo_id.as_str(), request, cx)
                        })
                        .await;
                    sync_already_applied = true;
                } else {
                    let message = push_result.message();
                    push_failure_message = Some(if message.is_empty() {
                        "Push failed after commit".to_string()
                    } else {
                        message.to_string()
                    });
                }
            }

            if !sync_already_applied {
                // Lo que el original dejaba a `handleCommitCreated` de
                // `CommitView.vue`: releer las ramas (el commit mueve la rama
                // y su `ahead`) y el estado si el resultado no lo traía.
                let request = SyncRequest::new()
                    .status(commit_result.status().is_none())
                    .branches(true)
                    .operation_or(commit_result.operation(), "commit")
                    .result(commit_result.clone());
                model
                    .update(cx, |model, cx| {
                        model.sync_repository_read_model(repo_id.as_str(), request, cx)
                    })
                    .await;
                sync_already_applied = true;
            }

            this.update_in(cx, |this, window, cx| {
                // Como git: el siguiente commit vuelve a partir de la plantilla
                // (y se mira si sigue siendo la misma).
                this.fill_with_template(window, cx);
                this.refresh_commit_template(cx);
                cx.emit(ChangesViewEvent::CommitCreated {
                    result: Box::new(commit_result),
                    sync_already_applied,
                });
                cx.notify();
            })
            .ok();

            model.update(cx, |model, cx| {
                if let Some(details) = push_failure_message {
                    model.notify(
                        NotifyType::Error,
                        "Commit created, but push failed.",
                        NotifyOptions {
                            details: Some(details),
                            ..Default::default()
                        },
                        cx,
                    );
                } else if should_push {
                    model.notify_success("Commit and push completed successfully.", cx);
                }
            });
        });
    }

    // ------------------------------------------------------------------
    // Menú contextual
    // ------------------------------------------------------------------

    /// Ejecuta una entrada del menú contextual de `file`.
    pub fn run_file_menu_action(
        &mut self,
        action: FileMenuAction,
        file: &ChangedFile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            FileMenuAction::OpenHere => self.open_file(file_descriptor(file), true, cx),
            FileMenuAction::OpenInNewTab => self.open_file(file_descriptor(file), false, cx),
            FileMenuAction::Unstage => self.unstage_file(file, cx),
            FileMenuAction::Stage => self.stage_file(file, cx),
            FileMenuAction::Discard => self.discard_file(file, cx),
            FileMenuAction::Ignore => self.ignore_file(file, cx),
            FileMenuAction::Blame => {
                open_blame_dialog(&self.repo_id, &file.path, window, cx);
            }
            FileMenuAction::FileHistory => self.open_file(
                FileTabDescriptor::history(file.path.clone(), None),
                false,
                cx,
            ),
            FileMenuAction::OpenInEditor => self.open_file_in_editor(file, cx),
            FileMenuAction::Reveal => self.reveal_file(file, cx),
            FileMenuAction::CopyPath => self.copy_path(file, cx),
            FileMenuAction::StashSelected => self.stash_selected_files(file, cx),
        }
    }

    /// La línea del diff a la vista con la que abrir en el editor el archivo
    /// seleccionado (la de la selección o la primera visible).
    pub fn editor_line(&self, cx: &mut App) -> Option<u32> {
        self.selected_file.as_ref()?;
        self.diff_viewer
            .update(cx, |viewer, _| viewer.editor_line())
    }

    /// "Open in editor" de la cabecera de una pestaña de archivo: el archivo
    /// que se está viendo, en la línea a la vista.
    pub fn open_selected_in_editor(&mut self, cx: &mut Context<Self>) {
        if let Some(file) = self.selected_file.clone() {
            self.open_file_in_editor(&file, cx);
        }
    }

    /// "Open in editor": el archivo del working tree en el editor externo. Si
    /// es el seleccionado, en la línea del diff a la vista.
    pub fn open_file_in_editor(&mut self, file: &ChangedFile, cx: &mut Context<Self>) {
        let Some(repo_path) = self.repo_path(cx) else {
            return;
        };
        let line = if self.selected_file.as_ref().map(|selected| &selected.path) == Some(&file.path)
        {
            self.editor_line(cx)
        } else {
            None
        };
        let path = file.path.clone();
        self.model
            .update(cx, |model, cx| {
                model.open_working_tree_file_in_editor(&repo_path, &path, line, cx)
            })
            .detach();
    }

    /// `contextMenuItems`: el menú contextual de un archivo de la lista. Si
    /// el archivo es parte de una selección múltiple (`selection`), el de la
    /// selección entera.
    fn file_menu(
        this: &WeakEntity<Self>,
        file: &ChangedFile,
        selection: &[ChangedFile],
        cx: &App,
    ) -> Vec<MenuItem> {
        let Some(view) = this.upgrade() else {
            return Vec::new();
        };
        let (in_file_tab, locked) = {
            let view = view.read(cx);
            (view.is_file_tab(), view.is_interaction_locked())
        };
        if selection.len() > 1 {
            return Self::selection_menu(this, selection, locked);
        }

        file_menu_entries(file, in_file_tab, locked)
            .into_iter()
            .map(|entry| match entry {
                FileMenuEntry::Separator => MenuItem::separator(),
                FileMenuEntry::Action { action, disabled } => {
                    let this = this.clone();
                    let file = file.clone();
                    let item = MenuItem::new(action.label())
                        .icon(action.icon())
                        .disabled(disabled)
                        .on_click(move |window, cx| {
                            this.update(cx, |this, cx| {
                                this.run_file_menu_action(action, &file, window, cx);
                            })
                            .ok();
                        });
                    if action.is_danger() {
                        item.danger()
                    } else {
                        item
                    }
                }
            })
            .collect()
    }

    /// El menú de una selección múltiple: preparar o quitar del stage,
    /// descartar y guardar en un stash, todo sobre la selección.
    fn selection_menu(
        this: &WeakEntity<Self>,
        selection: &[ChangedFile],
        locked: bool,
    ) -> Vec<MenuItem> {
        let count = logic::files_count_label(selection.len());
        let staged = selection
            .first()
            .is_some_and(|file| !file.index.trim().is_empty() && !is_untracked(file));
        let item = |label: String, icon, kind: FileActionKind| {
            let this = this.clone();
            MenuItem::new(label)
                .icon(icon)
                .disabled(locked)
                .on_click(move |_, cx| {
                    this.update(cx, |this, cx| match kind {
                        FileActionKind::Stage => this.stage_selection(cx),
                        FileActionKind::Unstage => this.unstage_selection(cx),
                        FileActionKind::Discard => this.discard_selection(cx),
                    })
                    .ok();
                })
        };
        let mut items = vec![MenuItem::header(format!("{count} selected"))];
        if staged {
            items.push(item(
                format!("Unstage {count}"),
                FileMenuAction::Unstage.icon(),
                FileActionKind::Unstage,
            ));
        } else {
            items.push(item(
                format!("Stage {count}"),
                FileMenuAction::Stage.icon(),
                FileActionKind::Stage,
            ));
            items.push(
                item(
                    format!("Discard {count}"),
                    FileMenuAction::Discard.icon(),
                    FileActionKind::Discard,
                )
                .danger(),
            );
        }
        items.push(MenuItem::separator());
        let paths: Vec<String> = selection.iter().map(|file| file.path.clone()).collect();
        let stash_this = this.clone();
        items.push(
            MenuItem::new(format!("Stash {count}…"))
                .icon(FileMenuAction::StashSelected.icon())
                .disabled(locked)
                .on_click(move |_, cx| {
                    stash_this
                        .update(cx, |this, cx| {
                            let paths = paths.clone();
                            this.model
                                .update(cx, |model, cx| model.handle_stash_paths(paths, cx));
                        })
                        .ok();
                }),
        );
        items
    }

    // ------------------------------------------------------------------
    // Ancho de la lista
    // ------------------------------------------------------------------

    fn container_geometry(&self) -> (f64, f64) {
        let bounds = self.bounds.get();
        (
            f64::from(f32::from(bounds.origin.x)),
            f64::from(f32::from(bounds.size.width)),
        )
    }

    fn start_resize(&mut self, cx: &mut Context<Self>) {
        self.resizing = true;
        if self.is_file_tab() {
            self.model
                .update(cx, |model, cx| model.start_file_list_resize(cx));
        }
        cx.notify();
    }

    fn drag_resize(&mut self, pointer_x: f64, cx: &mut Context<Self>) {
        if !self.resizing {
            return;
        }
        let (origin_x, container_width) = self.container_geometry();
        if self.is_file_tab() {
            let changed = self.model.update(cx, |model, _| {
                model.drag_file_list_resize(pointer_x, origin_x, container_width)
            });
            if !changed {
                return;
            }
        } else if self.is_list_only() {
            self.local_sidebar_width =
                clamp_local_changes_width(pointer_x - origin_x, container_width);
        } else {
            self.local_sidebar_width = clamp_list_width(pointer_x - origin_x, container_width);
        }
        cx.notify();
    }

    fn stop_resize(&mut self, cx: &mut Context<Self>) {
        if !self.resizing {
            return;
        }
        self.resizing = false;
        // El ancho de las pestañas de archivo es de todas y se guarda al soltar;
        // el de "Local Changes", también (en su propia preferencia).
        if self.is_file_tab() {
            self.model
                .update(cx, |model, cx| model.stop_file_list_resize(cx))
                .detach();
        } else if self.is_list_only() {
            let width = self.local_sidebar_width;
            self.model
                .update(cx, |model, cx| {
                    model.save_local_changes_width(Some(width), cx)
                })
                .detach();
        }
        cx.notify();
    }

    fn reset_width(&mut self, cx: &mut Context<Self>) {
        // `@dblclick="fileTab && resetFileListWidth()"`
        if self.is_file_tab() {
            self.model
                .update(cx, |model, cx| model.reset_file_list_width(cx))
                .detach();
        } else if self.is_list_only() {
            self.local_sidebar_width = DEFAULT_LOCAL_CHANGES_WIDTH;
            self.model
                .update(cx, |model, cx| model.save_local_changes_width(None, cx))
                .detach();
            cx.notify();
        }
    }

    // ------------------------------------------------------------------
    // Pintado
    // ------------------------------------------------------------------

    fn render_sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity().downgrade();
        let can_change = self.shown_lifecycle_allows_changes() && !self.is_interaction_locked();
        let file_handler = |handler: fn(&mut Self, &ChangedFile, &mut Context<Self>)| {
            let this = this.clone();
            move |file: &ChangedFile, _: &mut Window, cx: &mut App| {
                this.update(cx, |this, cx| handler(this, file, cx)).ok();
            }
        };
        let handler = |handler: fn(&mut Self, &mut Context<Self>)| {
            let this = this.clone();
            move |_: &mut Window, cx: &mut App| {
                this.update(cx, handler).ok();
            }
        };

        let sections = self.file_sections().clone();
        let file_tab = self.is_file_tab();
        let flat = self.shown_model.local_changes.flat_list && !file_tab;
        ChangesSidebar::new("changes-list")
            .source(self.source())
            .loading(self.loading_changes)
            .has_changes(!self.files.is_empty())
            .conflict_state(Some(self.shown_model.conflict_state.clone()))
            .busy_action(self.busy_action)
            .conflicted_files(sections.conflicted)
            .unstaged_files(sections.unstaged)
            .untracked_files(sections.untracked)
            .staged_files(sections.staged)
            .unstaged_stats(self.unstaged_stats.clone())
            .staged_stats(self.staged_stats.clone())
            // En "Local Changes" el filete lo pone el divisor, a toda la
            // altura de la columna (lista y caja de commit).
            .right_border(!self.is_list_only())
            .selected_file_path(self.selected_path())
            .selected_section(self.selected_section())
            .multi_selection(self.multi_selection.clone())
            .flat(flat)
            .stageable(can_change)
            .unstageable(can_change)
            .discardable(can_change)
            .on_select_file(file_handler(Self::select_file))
            .on_open_file(file_handler(Self::open_file_in_new_tab))
            .on_stage_file(file_handler(Self::stage_file))
            .on_unstage_file(file_handler(Self::unstage_file))
            .on_discard_file(file_handler(Self::discard_file))
            .on_file_action({
                let this = this.clone();
                move |action, _, cx| {
                    this.update(cx, |this, cx| this.handle_file_action(action, cx))
                        .ok();
                }
            })
            .when(!file_tab, |sidebar| {
                let this = this.clone();
                sidebar.on_multi_select(move |selection, _, cx| {
                    this.update(cx, |this, cx| this.select_many(selection, cx))
                        .ok();
                })
            })
            .when(!file_tab, |sidebar| {
                let model = self.model.clone();
                sidebar.on_layout_change(move |flat, _, cx| {
                    model.update(cx, |model, cx| {
                        model.set_changes_list_flat(*flat, cx).detach()
                    });
                })
            })
            .on_move_selection(file_handler(Self::move_selection))
            .track_focus(&self.list_focus)
            .on_stage_all(handler(Self::stage_all))
            .on_unstage_all(handler(Self::unstage_all))
            .on_discard_all(handler(Self::discard_all))
            .file_menu({
                let this = this.clone();
                move |file, selection, _, cx| Self::file_menu(&this, file, selection, cx)
            })
            .head_hash(
                self.shown_model
                    .head_short_hash
                    .clone()
                    .filter(|hash| !hash.is_empty()),
            )
            .on_open_terminal({
                let model = self.model.clone();
                move |_, cx| model.update(cx, |model, cx| model.handle_open_terminal(cx).detach())
            })
            .on_open_folder({
                let model = self.model.clone();
                move |_, cx| model.update(cx, |model, cx| model.handle_open_explorer(cx).detach())
            })
    }

    /// `.changes-resizer`
    fn render_resizer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let highlight = with_alpha(palette.primary, 0.42);
        let resizing = self.resizing;
        let this = cx.entity().downgrade();

        div()
            .id("changes-resizer")
            .test_support()
            .relative()
            .flex_shrink_0()
            .w(px(RESIZER_WIDTH as f32))
            .h_full()
            .cursor_col_resize()
            .bg(if resizing {
                highlight
            } else {
                palette.card.into()
            })
            // Señal: un filete de 1px como estructura.
            .flex()
            .justify_center()
            .child(div().w(px(1.)).h_full().bg(palette.border))
            .hover(move |style| style.bg(highlight))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    // `@mousedown.prevent`: no selecciona texto al arrastrar.
                    window.prevent_default();
                    if event.click_count >= 2 {
                        this.reset_width(cx);
                    } else {
                        this.start_resize(cx);
                    }
                }),
            )
            .child(
                // Mientras dura el arrastre el puntero se sigue en toda la
                // ventana, no solo sobre el divisor.
                canvas(
                    |_, _, _| (),
                    move |_, _, window, _| {
                        if !resizing {
                            return;
                        }
                        window.on_mouse_event({
                            let this = this.clone();
                            move |event: &MouseMoveEvent, phase, _, cx| {
                                if phase.bubble() {
                                    let pointer_x = f64::from(f32::from(event.position.x));
                                    this.update(cx, |this, cx| this.drag_resize(pointer_x, cx))
                                        .ok();
                                }
                            }
                        });
                        window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                            if phase.bubble() {
                                this.update(cx, |this, cx| this.stop_resize(cx)).ok();
                            }
                        });
                    },
                )
                .absolute()
                .size_full(),
            )
    }

    fn render_diff_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let preferences = self.shown_model.diff_preferences;
        let side = self.selected_side();
        let can_change = self.shown_lifecycle_allows_changes() && !self.is_interaction_locked();
        // "Discard" solo sobre el lado sin preparar a la vista.
        let can_discard = can_change && side == Some(DiffSide::Unstaged);
        // El estado del lado que se ve: el del índice en Staged, el del
        // working tree en Unstaged.
        let status = |file: &ChangedFile| -> String {
            match side {
                Some(DiffSide::Staged) => file.index.trim().to_string(),
                _ if file.working_dir.trim().is_empty() => file.index.trim().to_string(),
                _ => file.working_dir.trim().to_string(),
            }
        };
        let line_stats = self
            .selected_file
            .as_ref()
            .and_then(|file| self.file_line_stats(&file.path, side == Some(DiffSide::Staged)));

        let panel = DiffPanel::new(&self.diff_viewer)
            .file_opt(self.selected_file.as_ref().map(|file| {
                (
                    SharedString::from(file.path.clone()),
                    SharedString::from(status(file)),
                )
            }))
            .side(side)
            .both_sides(!self.is_file_tab() && self.selected_has_both_sides())
            .line_stats(line_stats)
            .stageable(can_change)
            .on_stage(cx.listener(|this, _, _, cx| this.stage_selected(cx)))
            .on_unstage(cx.listener(|this, _, _, cx| this.unstage_selected(cx)))
            .on_switch_side(cx.listener(|this, side: &DiffSide, _, cx| {
                this.switch_side(*side, cx);
            }))
            .diff(self.file_diff.clone())
            .loading(self.loading_diff)
            .too_large(self.diff_too_large.then_some(self.diff_size))
            .view_mode(preferences.diff_view_mode)
            .ignore_whitespace(preferences.ignore_whitespace)
            .whole_file(preferences.whole_file)
            .whole_file_fell_back(self.diff_whole_file_fell_back)
            .image(
                self.selected_file
                    .as_ref()
                    .filter(|file| is_image_path(&file.path))
                    .map(|_| self.image_viewer.clone()),
            )
            .hunk_action(logic::hunk_action(
                self.selected_file.as_ref(),
                preferences.ignore_whitespace,
            ))
            .discardable(can_discard)
            .on_toggle_view(cx.listener(|this, _, _, cx| this.toggle_diff_view(cx)))
            .on_toggle_ignore_whitespace(
                cx.listener(|this, _, _, cx| this.toggle_ignore_whitespace(cx)),
            )
            .on_toggle_whole_file(cx.listener(|this, _, _, cx| this.toggle_whole_file(cx)))
            .on_discard(cx.listener(|this, _, _, cx| this.discard_selected(cx)));

        if !self.is_conflict() {
            // Sin conflicto a la vista el original desmontaba el resolvedor, y
            // con él sus borradores.
            self.resolver.update(cx, |resolver, cx| resolver.reset(cx));
            return panel;
        }

        // El resolvedor recibe lo mismo que le pasaba `RepositoryDiffPanel`.
        let conflict_state = self.shown_model.conflict_state.clone();
        let selected = self.selected_path().unwrap_or_default();
        let total = if conflict_state.initial_count > 0 {
            conflict_state.initial_count
        } else {
            conflict_state.pending_count
        };
        self.resolver.update(cx, |resolver, cx| {
            resolver.set_files(conflict_state.pending_files.iter().cloned(), cx);
            resolver.set_counts(
                conflict_state.pending_count,
                conflict_state.resolved_count,
                total,
                cx,
            );
            // Las versiones son del archivo que las pidió: mientras llegan las
            // del elegido, el resolvedor espera.
            let versions = self
                .conflict_versions
                .clone()
                .filter(|_| self.content_path.as_deref() == Some(selected.as_str()));
            resolver.set_versions(selected.clone(), versions, cx);
            resolver.set_load_error(
                (!self.file_error.is_empty()).then(|| self.file_error.clone()),
                cx,
            );
            resolver.set_busy(self.is_resolving_conflict, cx);
            resolver.set_rebase(self.shown_model.operation_in_progress == Some("rebase"), cx);
            // Con la lista de cambios a la vista, sus conflictos ya están ahí.
            resolver.set_show_files(self.is_file_tab() && !self.files_open, cx);
        });
        panel.conflict(self.resolver.clone())
    }

    fn render_commit_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let shown = &self.shown_model;
        let branch = shown.branch.clone().unwrap_or_default();
        let upstream = shown.upstream.clone().unwrap_or_default();
        let pushed = shown.amend_is_pushed;
        let committable = logic::can_commit(
            shown.lifecycle,
            &self.commit_message(cx),
            logic::staged_count(&self.files),
            self.amend,
        ) && !self.is_interaction_locked();
        let commit_busy = self.busy_action.is_some_and(BusyAction::is_commit);

        let staged_count = logic::staged_count(&self.files);
        CommitPanel::new(&self.commit_title, &self.commit_body)
            // Los atajos se enseñan mientras no haya nada preparado (hasta
            // que se cierran); luego viven en los tooltips de las casillas.
            .shortcut_hints(!shown.local_changes.hints_dismissed && staged_count == 0)
            .on_dismiss_hints({
                let model = self.model.clone();
                move |_, cx| model.update(cx, |model, cx| model.dismiss_changes_hints(cx).detach())
            })
            .push_after_commit(self.push_after_commit)
            .committable(committable)
            .busy(commit_busy)
            .disabled(self.is_interaction_locked() && !commit_busy)
            .staged_count(staged_count)
            .branch(branch)
            .upstream(upstream)
            .amend(self.amend)
            .operation_in_progress(shown.operation_in_progress)
            .amendable(self.last_commit.is_some())
            .amend_is_pushed(pushed)
            .on_amend_change(cx.listener(|this, amend: &bool, window, cx| {
                this.set_amend(*amend, window, cx);
            }))
            .on_push_after_commit_change(cx.listener(|this, push: &bool, _, cx| {
                this.set_push_after_commit(*push, cx);
            }))
            .sign(self.sign)
            .on_sign_change(cx.listener(|this, sign: &bool, _, cx| {
                this.set_sign(*sign, cx);
            }))
            .on_commit({
                let this = cx.entity().downgrade();
                move |_, cx| {
                    this.update(cx, |this, cx| this.commit(cx)).ok();
                }
            })
            .fixup_targets(self.fixup_targets.clone())
            .on_fixup({
                let this = cx.entity().downgrade();
                move |target, _, cx| {
                    this.update(cx, |this, cx| this.commit_fixup(target, cx))
                        .ok();
                }
            })
    }
}

impl ChangesView {
    /// El panel derecho con varios archivos elegidos: cuántos, sus cifras y
    /// las acciones en lote de su sección.
    fn render_multi_summary(
        &mut self,
        selection: Rc<MultiSelection>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = *cx.palette();
        let files = self.multi_selected_files();
        let stats: &HashMap<_, _> = if selection.section == ChangesSection::Staged {
            &self.staged_stats
        } else {
            &self.unstaged_stats
        };
        let (additions, deletions) = files.iter().filter_map(|file| stats.get(&file.path)).fold(
            (0, 0),
            |(additions, deletions), stats| {
                (additions + stats.additions, deletions + stats.deletions)
            },
        );
        let summary: SharedString =
            logic::selection_summary(files.len(), additions, deletions).into();
        let count = logic::files_count_label(files.len());
        let enabled = self.shown_lifecycle_allows_changes() && !self.is_interaction_locked();
        let button = |id: &'static str, label: String, icon: mergeon_ui::Lucide| {
            mergeon_ui::button(id)
                .icon(icon)
                .label(label)
                .disabled(!enabled)
        };
        let actions: Vec<AnyElement> = match selection.section {
            ChangesSection::Unstaged => vec![
                button(
                    "multi-stage",
                    format!("Stage {count}"),
                    mergeon_ui::Lucide::Plus,
                )
                .on_click(cx.listener(|this, _, _, cx| this.stage_selection(cx)))
                .into_any_element(),
                button(
                    "multi-discard",
                    format!("Discard {count}"),
                    mergeon_ui::Lucide::Trash,
                )
                .destructive_outline()
                .on_click(cx.listener(|this, _, _, cx| this.discard_selection(cx)))
                .into_any_element(),
            ],
            ChangesSection::Staged => vec![
                button(
                    "multi-unstage",
                    format!("Unstage {count}"),
                    mergeon_ui::Lucide::Minus,
                )
                .on_click(cx.listener(|this, _, _, cx| this.unstage_selection(cx)))
                .into_any_element(),
            ],
            ChangesSection::Conflict => Vec::new(),
        };
        // Los primeros archivos, para saber de un vistazo qué se eligió.
        const SHOWN: usize = 8;
        let more = files.len().saturating_sub(SHOWN);
        let list = files.iter().take(SHOWN).map(|file| {
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .min_w_0()
                .child(mergeon_ui::status_badge(logic::effective_status(file)))
                .child(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_size(px(12.5))
                        .text_color(palette.text_2)
                        .child(file.path.clone()),
                )
        });
        div()
            .id("changes-multi-summary")
            .test_support()
            .aria_label(summary.clone())
            .flex()
            .flex_col()
            .flex_1()
            .items_center()
            .justify_center()
            .p(px(24.))
            .border_l_1()
            .border_color(palette.border)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(14.))
                    .w_full()
                    .max_w(px(420.))
                    .child(
                        div()
                            .id("changes-multi-title")
                            .test_support()
                            .aria_label(summary.clone())
                            .text_size(px(15.))
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .text_color(palette.text_1)
                            .child(summary),
                    )
                    .child(div().text_size(px(12.)).text_color(palette.text_3).child(
                        match selection.section {
                            ChangesSection::Staged => "Selected in Staged",
                            _ => "Selected in Unstaged",
                        },
                    ))
                    .child(div().flex().flex_col().gap(px(6.)).children(list).when(
                        more > 0,
                        |this| {
                            this.child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(palette.text_3)
                                    .child(format!("and {more} more")),
                            )
                        },
                    ))
                    .child(div().flex().flex_wrap().gap(px(8.)).children(actions)),
            )
            .into_any_element()
    }
}

impl Render for ChangesView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        let palette = *cx.palette();
        let file_tab = self.is_file_tab();
        let list_only = self.is_list_only();
        let show_list = !file_tab || self.files_open;
        // Con menos de 900px de ventana, en "Local Changes" las dos columnas se
        // apilan (la lista con la caja de commit, y debajo el diff).
        let stacked = list_only && window.viewport_size().width <= px(STACKED_LAYOUT_MAX_WIDTH);
        let sidebar_width = px(self.sidebar_width_with(self.shown_model.file_list_width) as f32);
        let bounds = self.bounds.clone();

        // Sin nada enfocado, las teclas son de la lista (como las flechas del
        // historial en la vista de commits).
        if !file_tab && self.shown_active() && window.focused(cx).is_none() {
            window.focus(&self.list_focus, cx);
        }

        // `.file-review-state`
        let review_state = |text: SharedString| {
            div()
                .id("file-review-state")
                .test_support()
                .aria_label(text.clone())
                .flex_none()
                .p(px(24.))
                .text_size(px(13.))
                .text_color(palette.muted_foreground)
                .child(text)
        };
        let message: Option<SharedString> = if !self.file_error.is_empty() {
            Some(self.file_error.clone())
        } else {
            self.tab
                .as_ref()
                .filter(|_| self.selected_file.is_none())
                .map(|tab| {
                    format!(
                        "No changes to display for this file in {}.",
                        if tab.source == TabSource::Index {
                            "Staged"
                        } else {
                            "Local"
                        }
                    )
                    .into()
                })
        };
        // Con varios archivos elegidos, su resumen en lugar del diff.
        let multi = self
            .multi_selection
            .clone()
            .filter(|_| !file_tab && message.is_none());
        let multi_summary = multi.map(|selection| self.render_multi_summary(selection, cx));
        let showing_multi = multi_summary.is_some();
        // En "Local Changes", sin archivo elegido el diff deja un aviso discreto.
        let no_selection =
            list_only && !showing_multi && self.selected_file.is_none() && message.is_none();
        let empty_diff = no_selection.then(|| {
            let text: SharedString = "Select a file to see its changes".into();
            div()
                .id("changes-no-selection")
                .test_support()
                .aria_label(text.clone())
                .flex()
                .flex_1()
                .items_center()
                .justify_center()
                .p(px(24.))
                .text_size(px(13.))
                .text_color(palette.text_3)
                .child(text)
        });

        // `.changes-main-column`: el diff (en "Local Changes", la columna derecha).
        let main_column = div()
            .id("changes-main")
            .test_support()
            .flex()
            .flex_col()
            .min_w_0()
            .min_h_0()
            .map(|this| {
                if stacked {
                    this.flex_1()
                        .w_full()
                        .min_h(px(160.))
                        .overflow_hidden()
                        .border_t_1()
                        .border_color(palette.border)
                } else {
                    this.flex_1().h_full().overflow_hidden()
                }
            })
            .when(list_only, |this| this.bg(palette.background))
            .children(message.map(review_state))
            .children(empty_diff)
            .children(multi_summary)
            .when(
                !no_selection && !showing_multi && self.file_error.is_empty(),
                |this| this.child(self.render_diff_panel(cx)),
            )
            // En la vista completa la caja de commit va bajo el diff.
            .when(self.mode == ChangesMode::Full, |this| {
                this.child(self.render_commit_panel(cx))
            });

        // La lista y, en "Local Changes", la caja de commit debajo.
        let list_column = show_list.then(|| {
            div()
                .id("changes-files-column")
                .flex()
                .flex_col()
                .min_w_0()
                .map(|column| {
                    if stacked {
                        // Sin `min_h` propio: mide al menos la lista mínima
                        // más la caja de commit, y el diff se queda el resto.
                        column.flex_1().w_full()
                    } else {
                        column.flex_none().min_h_0().h_full().w(sidebar_width)
                    }
                })
                .when(list_only, |column| column.bg(palette.card))
                .child(
                    div()
                        .flex_1()
                        .min_h(px(if stacked { 180. } else { 100. }))
                        .min_w_0()
                        .child(self.render_sidebar(cx)),
                )
                .when(list_only, |column| {
                    column.child(self.render_commit_panel(cx))
                })
        });

        // `.changes-view`
        div()
            .id("changes-view")
            .test_support()
            .relative()
            .flex()
            .when(stacked, |this| this.flex_col())
            .flex_1()
            .size_full()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .child(
                // Dónde está la vista: el divisor mide contra su borde.
                canvas(move |area, _, _| bounds.set(area), |_, _, _, _| {})
                    .absolute()
                    .size_full(),
            )
            .when_some(list_column, |this, column| {
                this.child(column)
                    .when(!stacked, |this| this.child(self.render_resizer(cx)))
            })
            .child(main_column)
    }
}
