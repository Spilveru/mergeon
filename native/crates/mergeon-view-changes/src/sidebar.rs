//! La lista de cambios locales
//! (`src/components/repository/RepositoryChangesSidebar.vue`): las secciones
//! "Conflicted Files", "Unstaged" (con lo que aún no se sigue) y "Staged",
//! cada una como árbol plegable (o lista plana), con sus contadores y sus
//! acciones en lote.
//!
//! Señal: cada archivo lleva una casilla que lo prepara o lo quita del stage,
//! cada carpeta una casilla para toda la carpeta y cada sección una casilla
//! de sección (vacía, marcada o mixta) y su "Stage all" / "Unstage all". Con
//! el foco en la lista, Espacio prepara o quita el archivo elegido, D lo
//! descarta y las flechas mueven la selección
//! (`mergeon_model::keyboard_shortcuts::CHANGES_LIST_SHORTCUTS`). Tras
//! preparar o descartar desde el teclado o la casilla, la selección pasa al
//! siguiente archivo de la misma sección ([`FileAction::next`]).
//!
//! Selección múltiple dentro de una sección: `Shift+clic` (rango),
//! `Ctrl/Cmd+clic` (alternar) y `Ctrl/Cmd+A`. Espacio, D y el menú contextual
//! actúan entonces sobre toda la selección.
//!
//! El banner de la operación en curso (rebase, merge…) no se pinta aquí: vive
//! bajo la barra de herramientas. El estado de conflictos solo alimenta los
//! contadores y la barra de progreso.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::rc::Rc;

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, ElementId, Entity, FocusHandle, FontWeight, InteractiveElement as _,
    IntoElement, KeyDownEvent, ParentElement as _, RenderOnce, ScrollStrategy, SharedString,
    StatefulInteractiveElement as _, Styled as _, TestSupportExt as _, UniformListScrollHandle,
    Window, div, px, relative, rems, uniform_list,
};
use mergeon_git_query::LineStats;
use mergeon_gitcmd::ConflictState;
use mergeon_model::file_tree::{
    FileTreeOptions, FileTreeRow as TreeRow, build_file_tree_rows_with, parent_folder,
};
use mergeon_model::keyboard_shortcuts::{
    CHANGES_LIST_SHORTCUTS, ChangesListAction, KeyEvent, is_mac_platform, matches_combo,
};
use mergeon_ui::menu::{MenuItem, MenuTriggerExt as _};
use mergeon_ui::theme::with_alpha;
use mergeon_ui::{ActivePaletteExt as _, IconTone, Lucide, Radius, SpinnerColor};
use mergeon_view_diff::{FileTreeRow, file_stats};

use crate::logic::{BusyAction, ChangedFile, SidebarSource, effective_status, is_partially_staged};
use crate::parts::PlainButton;

type FileHandler = Rc<dyn Fn(&ChangedFile, &mut Window, &mut App)>;
type Handler = Rc<dyn Fn(&mut Window, &mut App)>;
type MenuBuilder = Rc<dyn Fn(&ChangedFile, &[ChangedFile], &mut Window, &mut App) -> Vec<MenuItem>>;
type ActionHandler = Rc<dyn Fn(&FileAction, &mut Window, &mut App)>;
type MultiHandler = Rc<dyn Fn(&MultiSelection, &mut Window, &mut App)>;
type LayoutHandler = Rc<dyn Fn(&bool, &mut Window, &mut App)>;
type PathSet = Rc<HashSet<String>>;
/// Cifras `+N −N` por ruta.
pub type StatsByPath = Rc<HashMap<String, LineStats>>;

/// Una sección de la lista.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChangesSection {
    /// "Conflicted Files".
    Conflict,
    /// "Unstaged" (con lo que no se sigue).
    Unstaged,
    /// "Staged".
    Staged,
}

impl ChangesSection {
    /// El prefijo de los ids de sus filas (`unstaged:file:src/a.js`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Conflict => "conflict",
            Self::Unstaged => "unstaged",
            Self::Staged => "staged",
        }
    }
}

/// Lo que se pide sobre uno o varios archivos desde la lista.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileActionKind {
    Stage,
    Unstage,
    /// Quien lo recibe pide confirmación.
    Discard,
}

/// Preparar, quitar del stage o descartar archivos desde la lista (Espacio,
/// D, la casilla de un archivo o la de una carpeta).
#[derive(Debug, Clone, PartialEq)]
pub struct FileAction {
    pub kind: FileActionKind,
    /// La sección de donde salen.
    pub section: ChangesSection,
    pub files: Vec<ChangedFile>,
    /// Adónde pasa la selección: el siguiente archivo de la misma sección
    /// tras los tocados (o el anterior, si eran los últimos). `None` si la
    /// selección no está entre ellos o la sección se queda vacía (entonces
    /// la selección sigue al archivo a la otra sección).
    pub next: Option<ChangedFile>,
}

/// Varios archivos elegidos de una misma sección.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultiSelection {
    pub section: ChangesSection,
    /// Sus rutas, en el orden en que se eligieron.
    pub paths: Vec<String>,
}

impl MultiSelection {
    pub fn contains(&self, section: ChangesSection, path: &str) -> bool {
        self.section == section && self.paths.iter().any(|selected| selected == path)
    }
}

/// Lista de cambios locales. Es un componente controlado: se construye en cada
/// `render` con los archivos de cada sección y avisa con sus `on_*`. Lo único
/// que guarda entre frames es qué carpetas están plegadas, el desplazamiento
/// de cada sección y la fila desde la que se extiende un rango.
///
/// ```ignore
/// ChangesSidebar::new("changes-list")
///     .has_changes(!files.is_empty())
///     .conflicted_files(conflicted_files(&files))
///     .unstaged_files(unstaged_files(&files))
///     .untracked_files(untracked_files(&files))
///     .staged_files(staged_files(&files))
///     .selected_file_path(selected.map(|file| file.path.clone()))
///     .on_select_file(|file, window, cx| { /* ... */ })
///     .on_file_action(|action, window, cx| { /* preparar, quitar, descartar */ })
///     .on_stage_all(|window, cx| { /* ... */ })
/// ```
#[derive(IntoElement)]
pub struct ChangesSidebar {
    id: ElementId,
    source: SidebarSource,
    loading: bool,
    has_changes: bool,
    conflict_state: Option<ConflictState>,
    busy_action: Option<BusyAction>,
    conflicted_files: Rc<Vec<ChangedFile>>,
    unstaged_files: Rc<Vec<ChangedFile>>,
    untracked_files: Rc<Vec<ChangedFile>>,
    staged_files: Rc<Vec<ChangedFile>>,
    unstaged_stats: Option<StatsByPath>,
    staged_stats: Option<StatsByPath>,
    right_border: bool,
    selected_file_path: Option<SharedString>,
    selected_section: Option<ChangesSection>,
    multi_selection: Option<Rc<MultiSelection>>,
    flat: bool,
    can_stage: bool,
    can_unstage: bool,
    can_discard: bool,
    on_select_file: Option<FileHandler>,
    on_open_file: Option<FileHandler>,
    on_stage_file: Option<FileHandler>,
    on_unstage_file: Option<FileHandler>,
    on_discard_file: Option<FileHandler>,
    on_file_action: Option<ActionHandler>,
    on_multi_select: Option<MultiHandler>,
    on_move_selection: Option<FileHandler>,
    on_stage_all: Option<Handler>,
    on_unstage_all: Option<Handler>,
    on_discard_all: Option<Handler>,
    on_layout_change: Option<LayoutHandler>,
    file_menu: Option<MenuBuilder>,
    head_hash: Option<SharedString>,
    on_open_terminal: Option<Handler>,
    on_open_folder: Option<Handler>,
    focus_handle: Option<FocusHandle>,
}

impl ChangesSidebar {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            source: SidebarSource::All,
            loading: false,
            has_changes: false,
            conflict_state: None,
            busy_action: None,
            conflicted_files: Rc::default(),
            unstaged_files: Rc::default(),
            untracked_files: Rc::default(),
            staged_files: Rc::default(),
            unstaged_stats: None,
            staged_stats: None,
            right_border: true,
            selected_file_path: None,
            selected_section: None,
            multi_selection: None,
            flat: false,
            can_stage: true,
            can_unstage: true,
            can_discard: true,
            on_select_file: None,
            on_open_file: None,
            on_stage_file: None,
            on_unstage_file: None,
            on_discard_file: None,
            on_file_action: None,
            on_multi_select: None,
            on_move_selection: None,
            on_stage_all: None,
            on_unstage_all: None,
            on_discard_all: None,
            on_layout_change: None,
            file_menu: None,
            head_hash: None,
            on_open_terminal: None,
            on_open_folder: None,
            focus_handle: None,
        }
    }

    /// `source`: qué lado se lista (todo, solo el working tree o solo el índice).
    pub fn source(mut self, source: SidebarSource) -> Self {
        self.source = source;
        self
    }

    /// `isLoading`: "Loading changes...".
    pub fn loading(mut self, loading: bool) -> Self {
        self.loading = loading;
        self
    }

    /// `hasChanges`: sin cambios se dice "No changes detected".
    pub fn has_changes(mut self, has_changes: bool) -> Self {
        self.has_changes = has_changes;
        self
    }

    /// Hash corto de HEAD, que nombra el estado vacío ("No changes since …").
    pub fn head_hash(mut self, hash: Option<impl Into<SharedString>>) -> Self {
        self.head_hash = hash.map(Into::into);
        self
    }

    /// "Open terminal" del estado vacío (la acción de la barra de herramientas).
    pub fn on_open_terminal(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_open_terminal = Some(Rc::new(handler));
        self
    }

    /// "Open folder" del estado vacío (la acción de la barra de herramientas).
    pub fn on_open_folder(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_open_folder = Some(Rc::new(handler));
        self
    }

    /// `conflictState`: de aquí salen los contadores de la sección de conflictos.
    pub fn conflict_state(mut self, conflict_state: Option<ConflictState>) -> Self {
        self.conflict_state = conflict_state;
        self
    }

    /// `busyAction`: la mutación en curso. Con una, los botones se apagan y el
    /// de esa acción gira.
    pub fn busy_action(mut self, busy_action: Option<BusyAction>) -> Self {
        self.busy_action = busy_action;
        self
    }

    pub fn conflicted_files(mut self, files: impl Into<Rc<Vec<ChangedFile>>>) -> Self {
        self.conflicted_files = files.into();
        self
    }

    pub fn unstaged_files(mut self, files: impl Into<Rc<Vec<ChangedFile>>>) -> Self {
        self.unstaged_files = files.into();
        self
    }

    pub fn untracked_files(mut self, files: impl Into<Rc<Vec<ChangedFile>>>) -> Self {
        self.untracked_files = files.into();
        self
    }

    pub fn staged_files(mut self, files: impl Into<Rc<Vec<ChangedFile>>>) -> Self {
        self.staged_files = files.into();
        self
    }

    /// Las cifras `+N −N` (y las barritas) de los archivos sin preparar y sin
    /// seguimiento. Un archivo que no está en el mapa sale sin cifras (aún no
    /// se leyeron, o es binario).
    pub fn unstaged_stats(mut self, stats: StatsByPath) -> Self {
        self.unstaged_stats = Some(stats);
        self
    }

    /// Las cifras de los archivos preparados (ver [`Self::unstaged_stats`]).
    pub fn staged_stats(mut self, stats: StatsByPath) -> Self {
        self.staged_stats = Some(stats);
        self
    }

    /// El filete de 1px a la derecha (por defecto, sí).
    pub fn right_border(mut self, right_border: bool) -> Self {
        self.right_border = right_border;
        self
    }

    /// `selectedFilePath`.
    pub fn selected_file_path(mut self, path: Option<impl Into<SharedString>>) -> Self {
        self.selected_file_path = path.map(Into::into);
        self
    }

    /// La sección del archivo elegido. Un archivo a medio preparar está en
    /// Unstaged y en Staged: solo la fila de esta sección sale elegida, y la
    /// otra lleva una marca tenue de "el mismo archivo". Sin ella vale la
    /// fila en la que se hizo clic.
    pub fn selected_section(mut self, section: Option<ChangesSection>) -> Self {
        self.selected_section = section;
        self
    }

    /// Varios archivos elegidos (con dos o más): se pintan como elegidos en
    /// lugar de `selected_file_path`.
    pub fn multi_selection(mut self, selection: Option<Rc<MultiSelection>>) -> Self {
        self.multi_selection = selection.filter(|selection| selection.paths.len() > 1);
        self
    }

    /// Lista plana (un archivo por fila con su carpeta en gris) en lugar del
    /// árbol.
    pub fn flat(mut self, flat: bool) -> Self {
        self.flat = flat;
        self
    }

    /// `canStage` (por defecto sí).
    pub fn stageable(mut self, can_stage: bool) -> Self {
        self.can_stage = can_stage;
        self
    }

    /// `canUnstage` (por defecto sí).
    pub fn unstageable(mut self, can_unstage: bool) -> Self {
        self.can_unstage = can_unstage;
        self
    }

    /// `canDiscard` (por defecto sí).
    pub fn discardable(mut self, can_discard: bool) -> Self {
        self.can_discard = can_discard;
        self
    }

    /// `@select-file`: clic en un archivo.
    pub fn on_select_file(
        mut self,
        handler: impl Fn(&ChangedFile, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_select_file = Some(Rc::new(handler));
        self
    }

    /// `@open-file`: doble clic, que dentro de una pestaña de archivo pide
    /// otra pestaña. Llega después de los dos `on_select_file` de sus clics.
    pub fn on_open_file(
        mut self,
        handler: impl Fn(&ChangedFile, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_open_file = Some(Rc::new(handler));
        self
    }

    /// `@stage-file`: la casilla de un archivo sin preparar, o Espacio sobre
    /// él. Sin [`Self::on_file_action`]; con él, todo llega por ahí.
    pub fn on_stage_file(
        mut self,
        handler: impl Fn(&ChangedFile, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_stage_file = Some(Rc::new(handler));
        self
    }

    /// `@unstage-file`: la casilla de un archivo preparado, o Espacio sobre él.
    pub fn on_unstage_file(
        mut self,
        handler: impl Fn(&ChangedFile, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_unstage_file = Some(Rc::new(handler));
        self
    }

    /// D sobre un archivo sin preparar: quien lo recibe pide confirmación.
    pub fn on_discard_file(
        mut self,
        handler: impl Fn(&ChangedFile, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_discard_file = Some(Rc::new(handler));
        self
    }

    /// Preparar, quitar del stage o descartar uno o varios archivos (Espacio,
    /// D, la casilla de un archivo o de una carpeta), con el archivo al que
    /// pasa la selección. Con esto no se llaman `on_stage_file`,
    /// `on_unstage_file` ni `on_discard_file`.
    pub fn on_file_action(
        mut self,
        handler: impl Fn(&FileAction, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_file_action = Some(Rc::new(handler));
        self
    }

    /// `Shift+clic`, `Ctrl/Cmd+clic` o `Ctrl/Cmd+A` eligieron dos o más
    /// archivos de una sección. Sin esto, esos clics eligen solo la fila.
    pub fn on_multi_select(
        mut self,
        handler: impl Fn(&MultiSelection, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_multi_select = Some(Rc::new(handler));
        self
    }

    /// Las flechas: el archivo al que se movió la selección. Sin esto, las
    /// flechas hacen lo mismo que un clic (`on_select_file`).
    pub fn on_move_selection(
        mut self,
        handler: impl Fn(&ChangedFile, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_move_selection = Some(Rc::new(handler));
        self
    }

    /// El foco de la lista: con él, Espacio, D y las flechas son de la lista.
    /// Un clic en la lista se lo da.
    pub fn track_focus(mut self, focus_handle: &FocusHandle) -> Self {
        self.focus_handle = Some(focus_handle.clone());
        self
    }

    /// `@stage-all`.
    pub fn on_stage_all(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_stage_all = Some(Rc::new(handler));
        self
    }

    /// `@unstage-all`.
    pub fn on_unstage_all(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_unstage_all = Some(Rc::new(handler));
        self
    }

    /// `@discard-all`.
    pub fn on_discard_all(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_discard_all = Some(Rc::new(handler));
        self
    }

    /// El conmutador Árbol / Lista de la cabecera de la lista: recibe si se
    /// pidió la lista plana. Sin esto no hay conmutador.
    pub fn on_layout_change(
        mut self,
        handler: impl Fn(&bool, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_layout_change = Some(Rc::new(handler));
        self
    }

    /// `@open-context-menu`: las entradas del menú contextual de un archivo.
    /// Se evalúa cada vez que el menú se abre. El segundo argumento son los
    /// archivos elegidos si el archivo es parte de una selección múltiple
    /// (vacío si no).
    pub fn file_menu(
        mut self,
        builder: impl Fn(&ChangedFile, &[ChangedFile], &mut Window, &mut App) -> Vec<MenuItem> + 'static,
    ) -> Self {
        self.file_menu = Some(Rc::new(builder));
        self
    }
}

/// Estado que la lista guarda entre frames.
struct SidebarState {
    /// Carpetas plegadas. Las demás están desplegadas (`expandedFolders[path]
    /// !== false`).
    collapsed: HashSet<String>,
    /// Sube cada vez que cambia `collapsed`: invalida `trees`.
    collapsed_version: u64,
    /// El árbol de filas de cada sección, con los archivos (el mismo `Rc`) y la
    /// versión de `collapsed` con que se armó. Rehacerlo en cada render costaba
    /// CPU con miles de archivos aunque solo se pinten las filas visibles.
    trees: Vec<CachedTree>,
    /// Las rutas preparadas que además tienen cambios sin preparar, de la
    /// lista de preparados con que se calcularon (mismo `Rc`).
    partial: Option<(Rc<Vec<ChangedFile>>, PathSet)>,
    /// La fila elegida con el teclado o el ratón (sección y ruta): un archivo
    /// a medio preparar está en dos secciones con la misma ruta.
    cursor: Option<(ChangesSection, String)>,
    /// Desde dónde se extiende un rango con `Shift+clic`.
    anchor: Option<(ChangesSection, String)>,
    conflicted_scroll: UniformListScrollHandle,
    unstaged_scroll: UniformListScrollHandle,
    staged_scroll: UniformListScrollHandle,
}

impl SidebarState {
    fn scroll(&self, section: ChangesSection) -> &UniformListScrollHandle {
        match section {
            ChangesSection::Conflict => &self.conflicted_scroll,
            ChangesSection::Unstaged => &self.unstaged_scroll,
            ChangesSection::Staged => &self.staged_scroll,
        }
    }

    fn tree(&self, section: ChangesSection) -> Option<&CachedTree> {
        self.trees.iter().find(|tree| tree.section == section)
    }
}

/// Las rutas a medio preparar de `staged`, guardadas mientras sea el mismo `Rc`.
fn cached_partial(
    state: &Entity<SidebarState>,
    staged: &Rc<Vec<ChangedFile>>,
    cx: &mut App,
) -> Rc<HashSet<String>> {
    state.update(cx, |state, _| {
        if let Some((files, partial)) = &state.partial
            && Rc::ptr_eq(files, staged)
        {
            return partial.clone();
        }
        let partial: Rc<HashSet<String>> = Rc::new(
            staged
                .iter()
                .filter(|file| is_partially_staged(file))
                .map(|file| file.path.clone())
                .collect(),
        );
        state.partial = Some((staged.clone(), partial.clone()));
        partial
    })
}

/// El árbol de una sección tal como se armó la última vez.
struct CachedTree {
    section: ChangesSection,
    /// Los archivos de entrada (los mismos `Rc` que trajo el render).
    inputs: Vec<Rc<Vec<ChangedFile>>>,
    collapsed_version: u64,
    options: FileTreeOptions,
    /// Las entradas juntas, en orden.
    files: Rc<Vec<ChangedFile>>,
    rows: Rc<Vec<TreeRow<ChangedFile>>>,
}

/// Los archivos y el árbol de filas de `section`: los guardados si las
/// entradas son los mismos `Rc`, con las mismas opciones, y no se plegó
/// ninguna carpeta desde entonces; si no, se arman y se guardan.
fn cached_tree(
    state: &Entity<SidebarState>,
    section: ChangesSection,
    inputs: &[&Rc<Vec<ChangedFile>>],
    options: FileTreeOptions,
    cx: &mut App,
) -> (Rc<Vec<ChangedFile>>, Rc<Vec<TreeRow<ChangedFile>>>) {
    state.update(cx, |state, _| {
        let version = state.collapsed_version;
        let hit = state.trees.iter().find(|tree| {
            tree.section == section
                && tree.collapsed_version == version
                && tree.options == options
                && tree.inputs.len() == inputs.len()
                && tree
                    .inputs
                    .iter()
                    .zip(inputs)
                    .all(|(cached, input)| Rc::ptr_eq(cached, input))
        });
        if let Some(tree) = hit {
            return (tree.files.clone(), tree.rows.clone());
        }

        let files = match inputs {
            [only] => Rc::clone(only),
            _ => Rc::new(
                inputs
                    .iter()
                    .flat_map(|files| files.iter().cloned())
                    .collect(),
            ),
        };
        let rows = Rc::new(build_file_tree_rows_with(
            &files,
            |file| file.path.as_str(),
            |folder| !state.collapsed.contains(folder),
            options,
        ));
        state.trees.retain(|tree| tree.section != section);
        state.trees.push(CachedTree {
            section,
            inputs: inputs.iter().map(|files| Rc::clone(files)).collect(),
            collapsed_version: version,
            options,
            files: files.clone(),
            rows: rows.clone(),
        });
        (files, rows)
    })
}

/// El archivo de la sección que queda en el sitio de los de `acted`: el
/// primero que no se toca tras el primero tocado (con dos bloques elegidos,
/// el hueco entre ellos) o, si no hay, el anterior a ellos. Solo cuentan las
/// filas a la vista.
fn successor(rows: &[TreeRow<ChangedFile>], acted: &HashSet<&str>) -> Option<ChangedFile> {
    let files: Vec<&ChangedFile> = rows.iter().filter_map(|row| row.file.as_ref()).collect();
    let first = files
        .iter()
        .position(|file| acted.contains(file.path.as_str()))?;
    files[first + 1..]
        .iter()
        .find(|file| !acted.contains(file.path.as_str()))
        .or_else(|| {
            files[..first]
                .iter()
                .rev()
                .find(|file| !acted.contains(file.path.as_str()))
        })
        .map(|file| (*file).clone())
}

/// Quien recibe lo que se pide sobre los archivos: `on_file_action` o, sin
/// él, los avisos de un archivo (uno por archivo).
#[derive(Clone, Default)]
struct Dispatch {
    on_file_action: Option<ActionHandler>,
    on_stage: Option<FileHandler>,
    on_unstage: Option<FileHandler>,
    on_discard: Option<FileHandler>,
}

impl Dispatch {
    fn handles(&self, kind: FileActionKind) -> bool {
        self.on_file_action.is_some()
            || match kind {
                FileActionKind::Stage => self.on_stage.is_some(),
                FileActionKind::Unstage => self.on_unstage.is_some(),
                FileActionKind::Discard => self.on_discard.is_some(),
            }
    }

    fn run(&self, action: &FileAction, window: &mut Window, cx: &mut App) {
        if let Some(handler) = &self.on_file_action {
            handler(action, window, cx);
            return;
        }
        let handler = match action.kind {
            FileActionKind::Stage => &self.on_stage,
            FileActionKind::Unstage => &self.on_unstage,
            FileActionKind::Discard => &self.on_discard,
        };
        if let Some(handler) = handler {
            for file in &action.files {
                handler(file, window, cx);
            }
        }
    }
}

/// Lo que comparten todas las filas (y el teclado) en un frame.
struct Shared {
    state: Entity<SidebarState>,
    selected: Option<SharedString>,
    /// La sección de la fila elegida (ver [`ChangesSidebar::selected_section`]).
    active_section: Option<ChangesSection>,
    multi: Option<Rc<MultiSelection>>,
    on_select_file: Option<FileHandler>,
    on_open_file: Option<FileHandler>,
    on_multi_select: Option<MultiHandler>,
    file_menu: Option<MenuBuilder>,
    dispatch: Dispatch,
    /// Las carpetas plegadas, para la flecha de su fila.
    collapsed: HashSet<String>,
    /// La marca de "el mismo archivo" de la otra sección.
    same_file_color: gpui_kit::Hsla,
}

impl Shared {
    /// La fila de `path` en `section` es la elegida (sin selección múltiple).
    fn is_active(&self, section: ChangesSection, path: &str) -> bool {
        self.selected.as_deref() == Some(path)
            && self.active_section.is_none_or(|active| active == section)
    }

    /// La selección está entre `acted` (y en `section`): tras la acción
    /// pasará a otro archivo.
    fn selection_among(&self, section: ChangesSection, acted: &HashSet<&str>) -> bool {
        match &self.multi {
            Some(multi) => {
                multi.section == section
                    && multi.paths.iter().any(|path| acted.contains(path.as_str()))
            }
            None => self
                .selected
                .as_deref()
                .is_some_and(|path| acted.contains(path) && self.is_active(section, path)),
        }
    }

    /// `next` de una acción sobre `files` de `section`.
    fn next_after(
        &self,
        section: ChangesSection,
        files: &[ChangedFile],
        cx: &App,
    ) -> Option<ChangedFile> {
        let acted: HashSet<&str> = files.iter().map(|file| file.path.as_str()).collect();
        if !self.selection_among(section, &acted) {
            return None;
        }
        let state = self.state.read(cx);
        successor(&state.tree(section)?.rows, &acted)
    }

    /// Elige `paths` de `section`: uno solo como un clic, varios como
    /// selección múltiple.
    fn select_paths(
        &self,
        section: ChangesSection,
        paths: Vec<String>,
        files: &[ChangedFile],
        window: &mut Window,
        cx: &mut App,
    ) {
        match paths.as_slice() {
            [] => {}
            [one] => {
                if let (Some(handler), Some(file)) = (
                    &self.on_select_file,
                    files.iter().find(|file| file.path == *one),
                ) {
                    handler(file, window, cx);
                }
            }
            _ => {
                if let Some(handler) = &self.on_multi_select {
                    handler(&MultiSelection { section, paths }, window, cx);
                }
            }
        }
    }

    /// Los archivos de la selección múltiple, si `file` es parte de ella.
    fn multi_files(
        &self,
        section: ChangesSection,
        file: &ChangedFile,
        files: &[ChangedFile],
    ) -> Vec<ChangedFile> {
        match &self.multi {
            Some(multi) if multi.contains(section, &file.path) => files
                .iter()
                .filter(|candidate| multi.paths.contains(&candidate.path))
                .cloned()
                .collect(),
            _ => Vec::new(),
        }
    }
}

/// Lo que hace falta para pintar las filas de una sección.
#[derive(Clone)]
struct SectionRows {
    /// Prefijo de los ids de sus filas: el mismo archivo puede estar en dos
    /// secciones.
    section: ChangesSection,
    rows: Rc<Vec<TreeRow<ChangedFile>>>,
    /// Todos los archivos de la sección (también los de carpetas plegadas).
    files: Rc<Vec<ChangedFile>>,
    flat: bool,
    shared: Rc<Shared>,
    /// La casilla de cada archivo y carpeta (no la llevan los conflictos).
    check: Option<RowCheck>,
    /// Las cifras de los archivos de la sección (los conflictos no llevan).
    stats: Option<StatsByPath>,
}

/// La casilla de los archivos de una sección.
#[derive(Clone)]
struct RowCheck {
    /// En la sección de preparados la casilla está marcada.
    staged: bool,
    /// Rutas a medio preparar: su casilla sale mixta en las dos secciones.
    partial: Rc<HashSet<String>>,
    disabled: bool,
}

impl RowCheck {
    fn kind(&self) -> FileActionKind {
        if self.staged {
            FileActionKind::Unstage
        } else {
            FileActionKind::Stage
        }
    }
}

impl SectionRows {
    fn render(&self, range: Range<usize>) -> Vec<AnyElement> {
        range
            .filter_map(|ix| self.rows.get(ix))
            .map(|row| self.render_row(row))
            .collect()
    }

    /// Prepara o quita del stage `files` (la casilla de un archivo o de una
    /// carpeta), con el archivo al que pasa la selección.
    fn toggle(
        &self,
        kind: FileActionKind,
        files: Vec<ChangedFile>,
        window: &mut Window,
        cx: &mut App,
    ) {
        if files.is_empty() {
            return;
        }
        let next = self.shared.next_after(self.section, &files, cx);
        self.shared.dispatch.run(
            &FileAction {
                kind,
                section: self.section,
                files,
                next,
            },
            window,
            cx,
        );
    }

    /// La casilla de un archivo: marcada en preparados, vacía en sin preparar
    /// y mixta en las dos si el archivo está a medio preparar. El clic pide lo
    /// contrario de la sección (preparar / quitar del stage) y no elige la fila.
    fn row_checkbox(
        &self,
        check: &RowCheck,
        file: &ChangedFile,
        key: &str,
        selected: bool,
    ) -> impl IntoElement {
        let section = self.section.as_str();
        let id = SharedString::from(format!("check:{section}:{key}"));
        let kind = check.kind();
        let enabled = !check.disabled && self.shared.dispatch.handles(kind);
        let this = self.clone();
        let tip = if check.staged {
            "Unstage (Space)"
        } else {
            "Stage (Space)"
        };
        div()
            .id(SharedString::from(format!("check-tip:{section}:{key}")))
            .flex_shrink_0()
            .tooltip(mergeon_ui::tooltip(tip))
            .child(
                mergeon_ui::checkbox(id)
                    .checked(check.staged && !check.partial.contains(&file.path))
                    .mixed(check.partial.contains(&file.path))
                    .on_ink(selected)
                    .disabled(!enabled)
                    .when(enabled, |this_box| {
                        let file = file.clone();
                        this_box.on_change(move |_, window, cx| {
                            cx.stop_propagation();
                            this.toggle(kind, vec![file.clone()], window, cx);
                        })
                    }),
            )
    }

    /// La casilla de una carpeta: prepara (o quita) de un clic todo lo que
    /// la sección tiene debajo de ella, también en subcarpetas plegadas.
    fn folder_checkbox(&self, check: &RowCheck, folder: &str, key: &str) -> impl IntoElement {
        let section = self.section.as_str();
        let prefix = format!("{folder}/");
        let mixed = check.partial.iter().any(|path| path.starts_with(&prefix));
        let kind = check.kind();
        let enabled = !check.disabled && self.shared.dispatch.handles(kind);
        let this = self.clone();
        let tip = if check.staged {
            "Unstage folder"
        } else {
            "Stage folder"
        };
        div()
            .id(SharedString::from(format!("check-tip:{section}:{key}")))
            .flex_shrink_0()
            .tooltip(mergeon_ui::tooltip(tip))
            .child(
                mergeon_ui::checkbox(SharedString::from(format!("check:{section}:{key}")))
                    .checked(check.staged && !mixed)
                    .mixed(mixed)
                    .disabled(!enabled)
                    .when(enabled, |this_box| {
                        this_box.on_change(move |_, window, cx| {
                            cx.stop_propagation();
                            let files: Vec<ChangedFile> = this
                                .files
                                .iter()
                                .filter(|file| file.path.starts_with(&prefix))
                                .cloned()
                                .collect();
                            this.toggle(kind, files, window, cx);
                        })
                    }),
            )
    }

    /// Clic en un archivo: elegirlo, o con `Shift` un rango y con
    /// `Ctrl/Cmd` alternarlo en la selección múltiple.
    fn click_file(
        &self,
        file: &ChangedFile,
        modifiers: gpui_kit::Modifiers,
        window: &mut Window,
        cx: &mut App,
    ) {
        let shared = &self.shared;
        let section = self.section;
        let multi_allowed = shared.on_multi_select.is_some() && section != ChangesSection::Conflict;
        let toggle = modifiers.control || modifiers.platform;

        if multi_allowed && modifiers.shift {
            // El rango va del ancla (o de la fila elegida) a esta, entre las
            // filas a la vista de la sección.
            let visible: Vec<&str> = self
                .rows
                .iter()
                .filter_map(|row| row.file.as_ref().map(|file| file.path.as_str()))
                .collect();
            let anchor = shared
                .state
                .read(cx)
                .anchor
                .clone()
                .filter(|(anchor_section, _)| *anchor_section == section)
                .map(|(_, path)| path)
                .or_else(|| {
                    shared
                        .selected
                        .as_ref()
                        .filter(|path| shared.is_active(section, path))
                        .map(|path| path.to_string())
                });
            let from = anchor.and_then(|anchor| visible.iter().position(|path| *path == anchor));
            let to = visible.iter().position(|path| *path == file.path);
            let paths: Vec<String> = match (from, to) {
                (Some(from), Some(to)) => visible[from.min(to)..=from.max(to)]
                    .iter()
                    .map(|path| path.to_string())
                    .collect(),
                _ => vec![file.path.clone()],
            };
            if paths.len() == 1 {
                shared.state.update(cx, |state, _| {
                    state.anchor = Some((section, file.path.clone()));
                });
            }
            shared.select_paths(section, paths, &self.files, window, cx);
            return;
        }

        if multi_allowed && toggle {
            let mut paths: Vec<String> = match &shared.multi {
                Some(multi) if multi.section == section => multi.paths.clone(),
                _ => shared
                    .selected
                    .as_ref()
                    .filter(|path| shared.is_active(section, path))
                    .map(|path| vec![path.to_string()])
                    .unwrap_or_default(),
            };
            match paths.iter().position(|path| *path == file.path) {
                Some(at) => {
                    paths.remove(at);
                }
                None => paths.push(file.path.clone()),
            }
            if paths.is_empty() {
                paths.push(file.path.clone());
            }
            shared.state.update(cx, |state, _| {
                state.anchor = Some((section, file.path.clone()));
            });
            shared.select_paths(section, paths, &self.files, window, cx);
            return;
        }

        shared.state.update(cx, |state, _| {
            state.anchor = Some((section, file.path.clone()));
        });
        if let Some(handler) = &shared.on_select_file {
            handler(file, window, cx);
        }
    }

    fn render_row(&self, row: &TreeRow<ChangedFile>) -> AnyElement {
        let tree_row = FileTreeRow::from_row(row);
        let section = self.section;
        let shared = &self.shared;
        let mut same_file = false;
        let content = match &row.file {
            None => {
                let state = shared.state.clone();
                let folder = row.path.clone();
                let expanded = !shared.collapsed.contains(&folder);
                tree_row
                    .expanded(expanded)
                    .when_some(self.check.as_ref(), |this, check| {
                        this.leading(self.folder_checkbox(check, &row.path, &row.key))
                    })
                    .on_activate(move |_, _, cx| {
                        state.update(cx, |state, cx| {
                            if !state.collapsed.remove(&folder) {
                                state.collapsed.insert(folder.clone());
                            }
                            state.collapsed_version += 1;
                            cx.notify();
                        });
                    })
                    .into_any_element()
            }
            Some(file) => {
                let status = if section == ChangesSection::Conflict {
                    "U"
                } else {
                    effective_status(file)
                };
                let selected = match &shared.multi {
                    Some(multi) => multi.contains(section, &file.path),
                    None => shared.is_active(section, &file.path),
                };
                // La otra fila del mismo archivo (a medio preparar): marca
                // tenue, sin parecer elegida.
                same_file = shared.multi.is_none()
                    && !selected
                    && shared.selected.as_deref() == Some(file.path.as_str());
                let badge = mergeon_ui::status_badge(status).on_ink(selected);
                let leading = match &self.check {
                    Some(check) => div()
                        .flex()
                        .flex_shrink_0()
                        .items_center()
                        .gap(px(8.))
                        .child(self.row_checkbox(check, file, &row.key, selected))
                        .child(badge)
                        .into_any_element(),
                    None => badge.into_any_element(),
                };
                let state = shared.state.clone();
                // Solo se busca la fila que se pinta: con miles de archivos
                // nada se recorre por frame.
                let stats = self
                    .stats
                    .as_ref()
                    .and_then(|stats| stats.get(&file.path))
                    .and_then(|stats| {
                        file_stats(stats.additions as usize, stats.deletions as usize)
                    });
                let trailing = stats.map(|stats| {
                    let (additions, deletions) = stats.labels();
                    let label = [additions, deletions]
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>()
                        .join(" ");
                    div()
                        .id(SharedString::from(format!(
                            "stats:{}:{}",
                            section.as_str(),
                            file.path
                        )))
                        .test_support()
                        .aria_label(SharedString::from(label))
                        .flex_shrink_0()
                        .child(stats.bars(true).on_ink(selected))
                });
                // Un renombre dice de dónde viene: `viejo → nuevo` (con la
                // ruta vieja entera si cambió de carpeta).
                let renamed = file
                    .from
                    .as_ref()
                    .filter(|from| !from.is_empty())
                    .map(|from| {
                        let from_label = if parent_folder(from) == parent_folder(&file.path) {
                            from.rsplit('/').next().unwrap_or(from)
                        } else {
                            from.as_str()
                        };
                        (
                            format!("{from_label} \u{2192} {}", row.name),
                            format!("{from} \u{2192} {}", file.path),
                        )
                    });
                let tree_row = tree_row
                    .when(self.flat, |this| {
                        this.dir(parent_folder(&file.path).to_string())
                    })
                    .when_some(renamed, |this, (name, path)| this.name(name).path(path))
                    .selected(selected)
                    .leading(leading)
                    .when_some(trailing, |this, trailing| this.trailing(trailing))
                    .on_mouse_down(gpui_kit::MouseButton::Left, {
                        let path = file.path.clone();
                        move |_, _, cx| {
                            state.update(cx, |state, _| {
                                state.cursor = Some((section, path.clone()));
                            });
                        }
                    })
                    .on_activate({
                        let this = self.clone();
                        let file = file.clone();
                        move |event, window, cx| {
                            this.click_file(&file, event.modifiers(), window, cx)
                        }
                    })
                    .when_some(shared.on_open_file.clone(), |this, handler| {
                        let file = file.clone();
                        this.on_open_new(move |_, window, cx| handler(&file, window, cx))
                    });
                match shared.file_menu.clone() {
                    Some(menu) => {
                        let file = file.clone();
                        let this = self.clone();
                        tree_row
                            .context_menu_items(move |window, cx| {
                                let selection =
                                    this.shared.multi_files(this.section, &file, &this.files);
                                menu(&file, &selection, window, cx)
                            })
                            .into_any_element()
                    }
                    None => tree_row.into_any_element(),
                }
            }
        };

        // Envoltura sin estilo: da a la fila un id propio de su sección.
        // `aria-selected` dice qué fila es la elegida.
        div()
            .id(SharedString::from(format!(
                "{}:{}",
                section.as_str(),
                row.key
            )))
            .test_support()
            .aria_label(SharedString::from(row.path.clone()))
            .relative()
            .child(content)
            .when(same_file, |this| {
                this.child(
                    div()
                        .id(SharedString::from(format!(
                            "same-file:{}:{}",
                            section.as_str(),
                            row.key
                        )))
                        .test_support()
                        .absolute()
                        .left_0()
                        .top(px(4.))
                        .bottom(px(4.))
                        .w(px(2.))
                        .rounded(px(1.))
                        .bg(shared.same_file_color),
                )
            })
            .into_any_element()
    }
}

/// `.section-icon-btn` en rojo: "Discard all", el botón cuadrado de la
/// cabecera de "Unstaged". Mientras su acción está en curso, el ícono es un
/// cargador que gira.
fn danger_section_button(
    id: &'static str,
    title: &'static str,
    icon: Lucide,
    spinning: bool,
    disabled: bool,
    on_click: impl Fn(&mut Window, &mut App) + 'static,
) -> PlainButton {
    PlainButton::new(id)
        .label(title)
        .tooltip(title)
        .disabled(disabled)
        .disabled_opacity(0.55)
        .on_click(move |_, window, cx| {
            // `@click.stop`
            cx.stop_propagation();
            on_click(window, cx);
        })
        .styled(move |button, cx| {
            let palette = *cx.palette();
            let hover_background = with_alpha(palette.destructive, 0.12);
            button
                .flex()
                .items_center()
                .justify_center()
                .size(px(22.))
                .rounded(Radius::Sm.px(cx))
                .when(!disabled, |this| {
                    this.hover(move |style| style.bg(hover_background))
                })
        })
        .child(
            mergeon_ui::icon(if spinning { Lucide::LoaderCircle } else { icon })
                .size(px(13.))
                .tone(IconTone::Danger)
                .spin(spinning),
        )
}

/// `.section-header` (Señal: `.ch-h`): casilla de sección, título en mono y
/// mayúsculas, contador y, a la derecha, lo que la sección ofrezca.
fn section_header(
    id: &'static str,
    title: &'static str,
    count: usize,
    check: Option<AnyElement>,
    trailing: impl IntoElement,
    cx: &App,
) -> impl IntoElement {
    let palette = *cx.palette();
    div()
        .id(id)
        .test_support()
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_between()
        .gap(px(8.))
        .min_h(px(30.))
        .pl(px(14.))
        .pr(px(10.))
        .border_b_1()
        .border_color(palette.border_hair)
        .bg(palette.bg_inset)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .min_w_0()
                .children(check)
                .child(
                    // Señal: etiqueta de sección en mono, mayúsculas, `text-3`.
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .min_w_0()
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_size(px(10.5))
                        .line_height(relative(1.4))
                        .text_color(palette.text_3)
                        .child(
                            // Si no cabe, el título se parte en dos renglones
                            // (como en la lista estrecha de una pestaña de
                            // archivo).
                            div()
                                .min_w_0()
                                .font_weight(FontWeight::MEDIUM)
                                .child(title.to_uppercase()),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .id("count")
                                .test_support()
                                .aria_label(SharedString::from(count.to_string()))
                                .text_color(palette.text_3)
                                .child(count.to_string()),
                        ),
                ),
        )
        .child(trailing)
}

/// `.ch-h .act`: "Stage all" / "Unstage all", texto con su ícono a la
/// derecha de la cabecera.
fn section_text_button(
    id: &'static str,
    title: &'static str,
    icon: Lucide,
    spinning: bool,
    disabled: bool,
    on_click: impl Fn(&mut Window, &mut App) + 'static,
) -> PlainButton {
    PlainButton::new(id)
        .label(title)
        .disabled(disabled)
        .disabled_opacity(0.55)
        .on_click(move |_, window, cx| {
            cx.stop_propagation();
            on_click(window, cx);
        })
        .styled(move |button, cx| {
            let palette = *cx.palette();
            button
                .flex()
                .items_center()
                .gap(px(4.))
                .h(px(22.))
                .px(px(6.))
                .rounded(Radius::Sm.px(cx))
                .text_size(px(12.))
                .text_color(palette.text_2)
                .whitespace_nowrap()
                .when(!disabled, |this| {
                    this.hover(move |style| style.bg(palette.bg_hover).text_color(palette.text_1))
                })
        })
        .child(
            mergeon_ui::icon(if spinning { Lucide::LoaderCircle } else { icon })
                .size(px(12.))
                .tone(IconTone::Muted)
                .spin(spinning),
        )
        .child(title)
}

/// La casilla de la cabecera de una sección.
fn section_checkbox(
    id: &'static str,
    checked: bool,
    mixed: bool,
    disabled: bool,
    on_click: Option<Handler>,
) -> AnyElement {
    mergeon_ui::checkbox(id)
        .checked(checked)
        .mixed(mixed)
        .disabled(disabled || on_click.is_none())
        .when_some(on_click, |this, handler| {
            this.on_change(move |_, window, cx| {
                cx.stop_propagation();
                handler(window, cx);
            })
        })
        .into_any_element()
}

/// `.section-files`: las filas de una sección, de las que solo se construyen
/// las visibles, o el texto de sección vacía.
/// Alto de una fila de archivo o carpeta (`FileTreeRow`).
const ROW_HEIGHT: f32 = 26.;
/// Alto de la cabecera de una sección, con su filete.
const HEADER_HEIGHT: f32 = 31.;
/// Alto del aviso de sección vacía ("No staged changes").
const EMPTY_HEIGHT: f32 = 30.;

/// Señal: cada sección mide lo que su contenido (los preparados van justo
/// debajo de los sin preparar y lo que sobra queda abajo, sobre la caja de
/// commit). Si no caben, encogen en proporción a su tamaño: la sección de
/// miles de archivos cede el sitio y la de dos se sigue viendo entera. Nunca
/// bajan de su cabecera y dos filas.
fn fit_section<S: gpui_kit::Styled>(section: S, rows: usize) -> S {
    let content = if rows == 0 {
        EMPTY_HEIGHT
    } else {
        rows as f32 * ROW_HEIGHT
    };
    let minimum = if rows == 0 {
        EMPTY_HEIGHT
    } else {
        rows.min(2) as f32 * ROW_HEIGHT
    };
    section
        .flex_grow_0()
        .flex_shrink(1.)
        .flex_basis(px(HEADER_HEIGHT + content))
        .min_h(px(HEADER_HEIGHT + minimum))
}

fn section_files(
    rows: SectionRows,
    scroll: &UniformListScrollHandle,
    empty: Option<&'static str>,
    cx: &App,
) -> AnyElement {
    if rows.rows.is_empty() {
        return div()
            .flex_1()
            .min_h_0()
            .children(empty.map(|text| {
                // `.section-empty`
                div()
                    .flex()
                    .items_center()
                    .py(rems(0.5))
                    .px(rems(0.75))
                    .text_size(rems(0.8125))
                    .text_color(cx.palette().muted_foreground)
                    .child(text)
            }))
            .into_any_element();
    }

    let count = rows.rows.len();
    let id = SharedString::from(format!("{}-files", rows.section.as_str()));
    div()
        .relative()
        .flex_1()
        .min_h_0()
        .child(
            uniform_list(id, count, move |range, _, _| rows.render(range))
                .track_scroll(scroll)
                .size_full(),
        )
        .vertical_scrollbar(scroll)
        .into_any_element()
}

/// Estado vacío de Señal: el check en su marco con marcas de esquina, el
/// título, desde qué commit no hay cambios y las dos salidas habituales.
fn clean_state(
    head_hash: Option<SharedString>,
    on_open_terminal: Option<Handler>,
    on_open_folder: Option<Handler>,
    cx: &App,
) -> AnyElement {
    let palette = *cx.palette();
    let lines = div()
        .flex()
        .flex_col()
        .items_center()
        .child(match head_hash {
            Some(hash) => div()
                .flex()
                .flex_wrap()
                .justify_center()
                .child("No changes since\u{a0}")
                .child(
                    div()
                        .id("changes-empty-head")
                        .test_support()
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_color(palette.text_1)
                        .child(hash),
                )
                .child(".")
                .into_any_element(),
            None => div().child("No changes yet.").into_any_element(),
        })
        .child("Edit files or switch branches to get started.");
    let action = |id: &'static str, label: &'static str, icon: Lucide, handler: Option<Handler>| {
        handler.map(|handler| {
            mergeon_ui::button(id)
                .xs()
                .icon(icon)
                .label(label)
                .on_click(move |_, window, cx| handler(window, cx))
        })
    };
    let open_terminal = action(
        "changes-empty-terminal",
        "Open terminal",
        Lucide::SquareTerminal,
        on_open_terminal,
    )
    .map(|button| button.outline());
    let open_folder = action(
        "changes-empty-folder",
        "Open folder",
        Lucide::Folder,
        on_open_folder,
    )
    .map(|button| button.ghost());

    mergeon_ui::empty_state("Working tree clean")
        .icon(Lucide::Check)
        .rich_description(lines)
        .children(open_terminal)
        .children(open_folder)
        .into_any_element()
}

/// Una tecla de una sola letra (`d`, o `D` si llega ya en mayúscula).
fn is_letter_key(key: &str) -> bool {
    let mut chars = key.chars();
    chars.next().is_some_and(|c| c.is_alphabetic()) && chars.next().is_none()
}

/// La cabecera de la lista: cuántos archivos y el conmutador Árbol / Lista.
fn layout_toolbar(flat: bool, count: usize, handler: LayoutHandler, cx: &App) -> AnyElement {
    let palette = *cx.palette();
    let option = |id: &'static str, title: &'static str, icon: Lucide, value: bool| {
        let active = flat == value;
        let handler = handler.clone();
        PlainButton::new(id)
            .label(title)
            .tooltip(title)
            .on_click(move |_, window, cx| {
                cx.stop_propagation();
                if !active {
                    handler(&value, window, cx);
                }
            })
            .styled(move |button, cx| {
                let palette = *cx.palette();
                button.size(px(22.)).rounded(Radius::Sm.px(cx)).map(|this| {
                    if active {
                        this.bg(palette.sel_bg)
                    } else {
                        this.hover(move |style| style.bg(palette.bg_hover))
                    }
                })
            })
            .child(mergeon_ui::icon(icon).size(px(13.)).map(|icon| {
                if active {
                    icon.text_color(palette.sel_fg)
                } else {
                    icon.tone(IconTone::Muted)
                }
            }))
    };
    let label = if count == 1 {
        "1 changed file".to_string()
    } else {
        format!("{count} changed files")
    };
    div()
        .id("changes-list-toolbar")
        .test_support()
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_between()
        .min_h(px(30.))
        .pl(px(14.))
        .pr(px(8.))
        .border_b_1()
        .border_color(palette.border_hair)
        .child(
            div()
                .id("changes-list-count")
                .test_support()
                .aria_label(SharedString::from(label.clone()))
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .font_family(cx.theme().mono_font_family.clone())
                .text_size(px(10.5))
                .text_color(palette.text_3)
                .child(label.to_uppercase()),
        )
        .child(
            div()
                .id("changes-layout")
                .test_support()
                .aria_label(if flat { "List" } else { "Tree" })
                .flex()
                .flex_shrink_0()
                .items_center()
                .gap(px(2.))
                .child(option(
                    "changes-layout-tree",
                    "Tree",
                    Lucide::ListTree,
                    false,
                ))
                .child(option("changes-layout-flat", "List", Lucide::List, true)),
        )
        .into_any_element()
}

/// El progreso de los conflictos: `1 / 2` con una barra fina de lo resuelto.
fn conflict_progress(resolved: usize, total: usize, cx: &App) -> impl IntoElement {
    let palette = *cx.palette();
    let ratio = if total == 0 {
        0.
    } else {
        (resolved as f32 / total as f32).clamp(0., 1.)
    };
    let text = format!("{resolved} / {total}");
    div()
        .id("conflict-progress")
        .test_support()
        .aria_label(SharedString::from(text.clone()))
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(px(8.))
        .child(
            div()
                .id("conflict-progress-bar")
                .test_support()
                .aria_label(SharedString::from(format!("{}%", (ratio * 100.).round())))
                .relative()
                .w(px(56.))
                .h(px(3.))
                .rounded(px(2.))
                .overflow_hidden()
                .bg(palette.border_base)
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .bottom_0()
                        .w(relative(ratio))
                        .bg(palette.success),
                ),
        )
        .child(
            div()
                .text_size(rems(0.75))
                .text_color(palette.muted_foreground)
                .child(text),
        )
}

/// Las teclas de la lista con el foco en ella (`CHANGES_LIST_SHORTCUTS`, más
/// `Ctrl/Cmd+A`).
struct ListKeys {
    focus_handle: FocusHandle,
    shared: Rc<Shared>,
    /// Las secciones a la vista, en orden.
    sections: [Option<ChangesSection>; 3],
    busy: bool,
    can_stage: bool,
    can_unstage: bool,
    can_discard: bool,
    on_move: Option<FileHandler>,
}

impl ListKeys {
    /// `Ctrl/Cmd+A`: todos los archivos de la sección de la fila elegida (o
    /// la primera que tenga).
    fn select_all(&self, window: &mut Window, cx: &mut App) {
        let shared = &self.shared;
        let (section, files) = {
            let state = shared.state.read(cx);
            let current = state
                .cursor
                .as_ref()
                .map(|(section, _)| *section)
                .or(shared.multi.as_ref().map(|multi| multi.section))
                .or(shared.active_section)
                .filter(|section| *section != ChangesSection::Conflict);
            let section = current.or_else(|| {
                self.sections.iter().flatten().copied().find(|section| {
                    *section != ChangesSection::Conflict
                        && state
                            .tree(*section)
                            .is_some_and(|tree| !tree.files.is_empty())
                })
            });
            let Some(section) = section else {
                return;
            };
            let Some(tree) = state.tree(section) else {
                return;
            };
            (section, tree.files.clone())
        };
        let mut paths: Vec<String> = Vec::with_capacity(files.len());
        for file in files.iter() {
            if !paths.contains(&file.path) {
                paths.push(file.path.clone());
            }
        }
        shared.select_paths(section, paths, &files, window, cx);
    }

    fn handle(&self, event: &KeyDownEvent, window: &mut Window, cx: &mut App) {
        // Solo con el foco en la propia lista: si lo tiene una casilla o un
        // botón de dentro, Espacio es suyo.
        if !self.focus_handle.is_focused(window) {
            return;
        }
        let keystroke = &event.keystroke;
        let modifiers = keystroke.modifiers;
        if keystroke.key.eq_ignore_ascii_case("a")
            && (modifiers.control || modifiers.platform)
            && !modifiers.alt
            && !modifiers.shift
        {
            cx.stop_propagation();
            if !event.is_held && self.shared.on_multi_select.is_some() {
                self.select_all(window, cx);
            }
            return;
        }
        // Las letras valen igual en mayúscula (Shift o Bloq Mayús): las pistas
        // las enseñan así. Espacio y las flechas con Shift siguen sin ser de
        // la lista.
        let letter = is_letter_key(&keystroke.key);
        let key = KeyEvent {
            key: keystroke.key.clone(),
            meta_key: modifiers.platform,
            ctrl_key: modifiers.control,
            shift_key: modifiers.shift && !letter,
            alt_key: modifiers.alt,
            ..Default::default()
        };
        let Some(action) = CHANGES_LIST_SHORTCUTS
            .iter()
            .find(|binding| matches_combo(&key, binding.combo, is_mac_platform()))
            .map(|binding| binding.action)
        else {
            return;
        };
        cx.stop_propagation();
        // Mantener pulsada una flecha recorre la lista; Espacio y D no se repiten.
        if event.is_held
            && !matches!(
                action,
                ChangesListAction::Previous | ChangesListAction::Next
            )
        {
            return;
        }

        let shared = self.shared.clone();
        // Los archivos a la vista (sin los de carpetas plegadas), en orden.
        let entries: Vec<(ChangesSection, usize, ChangedFile)> = {
            let state = shared.state.read(cx);
            self.sections
                .iter()
                .flatten()
                .filter_map(|section| state.tree(*section))
                .flat_map(|tree| {
                    tree.rows.iter().enumerate().filter_map(move |(ix, row)| {
                        row.file.clone().map(|file| (tree.section, ix, file))
                    })
                })
                .collect()
        };
        let cursor = shared.state.read(cx).cursor.clone();
        let selected = shared.selected.as_deref();
        let current = cursor
            .filter(|(_, path)| Some(path.as_str()) == selected || shared.multi.is_some())
            .and_then(|(section, path)| {
                entries
                    .iter()
                    .position(|(s, _, file)| *s == section && file.path == path)
            })
            .or_else(|| {
                entries.iter().position(|(section, _, file)| {
                    Some(file.path.as_str()) == selected
                        && shared
                            .active_section
                            .is_none_or(|active| active == *section)
                })
            });

        // Sobre qué actúan Espacio y D: la selección múltiple o la fila.
        let targets = || -> Option<(ChangesSection, Vec<ChangedFile>)> {
            if let Some(multi) = &shared.multi {
                let state = shared.state.read(cx);
                let files = state
                    .tree(multi.section)?
                    .files
                    .iter()
                    .filter(|file| multi.paths.contains(&file.path))
                    .cloned()
                    .collect::<Vec<_>>();
                return (!files.is_empty()).then_some((multi.section, files));
            }
            let (section, _, file) = current.and_then(|ix| entries.get(ix))?;
            Some((*section, vec![file.clone()]))
        };

        match action {
            ChangesListAction::Previous | ChangesListAction::Next => {
                let next = match (action, current) {
                    (_, None) => 0,
                    (ChangesListAction::Next, Some(ix)) => {
                        (ix + 1).min(entries.len().saturating_sub(1))
                    }
                    (_, Some(ix)) => ix.saturating_sub(1),
                };
                let Some((section, row_ix, file)) = entries.get(next).cloned() else {
                    return;
                };
                if Some(next) == current && shared.multi.is_none() {
                    return;
                }
                shared.state.update(cx, |state, cx| {
                    state.cursor = Some((section, file.path.clone()));
                    state.anchor = Some((section, file.path.clone()));
                    state
                        .scroll(section)
                        .scroll_to_item(row_ix, ScrollStrategy::Nearest);
                    cx.notify();
                });
                if let Some(on_move) = &self.on_move {
                    on_move(&file, window, cx);
                }
            }
            ChangesListAction::ToggleStage => {
                let Some((section, files)) = targets() else {
                    return;
                };
                let kind = match section {
                    ChangesSection::Unstaged if self.can_stage => FileActionKind::Stage,
                    ChangesSection::Staged if self.can_unstage => FileActionKind::Unstage,
                    _ => return,
                };
                if self.busy || !shared.dispatch.handles(kind) {
                    return;
                }
                let next = shared.next_after(section, &files, cx);
                shared.dispatch.run(
                    &FileAction {
                        kind,
                        section,
                        files,
                        next,
                    },
                    window,
                    cx,
                );
            }
            ChangesListAction::Discard => {
                let Some((section, files)) = targets() else {
                    return;
                };
                if section != ChangesSection::Unstaged
                    || !self.can_discard
                    || self.busy
                    || !shared.dispatch.handles(FileActionKind::Discard)
                {
                    return;
                }
                let next = shared.next_after(section, &files, cx);
                shared.dispatch.run(
                    &FileAction {
                        kind: FileActionKind::Discard,
                        section,
                        files,
                        next,
                    },
                    window,
                    cx,
                );
            }
        }
    }
}

impl RenderOnce for ChangesSidebar {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let palette = *cx.palette();
        let root = div()
            .id(self.id.clone())
            .test_support()
            .size_full()
            .min_w_0()
            .overflow_hidden()
            .when(self.right_border, |this| {
                this.border_r_1().border_color(palette.border)
            })
            .bg(palette.card);

        // `.state-shell`
        let state_shell = || {
            div()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .h_full()
                .text_sm()
                .text_color(palette.muted_foreground)
        };

        if self.loading {
            return root.child(
                state_shell()
                    .id("changes-loading")
                    .test_support()
                    .child(mergeon_ui::spinner().color(SpinnerColor::Primary))
                    .child("Loading changes..."),
            );
        }

        if !self.has_changes {
            return root.child(
                div()
                    .id("changes-empty")
                    .test_support()
                    .flex()
                    .flex_col()
                    .justify_center()
                    .h_full()
                    .px_3()
                    .child(clean_state(
                        self.head_hash,
                        self.on_open_terminal,
                        self.on_open_folder,
                        cx,
                    )),
            );
        }

        let state = window.use_keyed_state(self.id.clone(), cx, |_, _| SidebarState {
            collapsed: HashSet::new(),
            collapsed_version: 0,
            trees: Vec::new(),
            partial: None,
            cursor: None,
            anchor: None,
            conflicted_scroll: UniformListScrollHandle::new(),
            unstaged_scroll: UniformListScrollHandle::new(),
            staged_scroll: UniformListScrollHandle::new(),
        });
        let (conflicted_scroll, unstaged_scroll, staged_scroll, cursor, collapsed) = {
            let state = state.read(cx);
            (
                state.conflicted_scroll.clone(),
                state.unstaged_scroll.clone(),
                state.staged_scroll.clone(),
                state.cursor.clone(),
                state.collapsed.clone(),
            )
        };
        // Los conflictos van siempre en árbol; las otras dos secciones, en
        // árbol compacto o en lista plana.
        let options = FileTreeOptions {
            compact_folders: true,
            flat: self.flat,
        };
        let (conflict_files, conflict_rows) = cached_tree(
            &state,
            ChangesSection::Conflict,
            &[&self.conflicted_files],
            FileTreeOptions {
                compact_folders: true,
                flat: false,
            },
            cx,
        );
        // Sin preparar y sin seguimiento van en la misma sección.
        let (pending, pending_rows) = cached_tree(
            &state,
            ChangesSection::Unstaged,
            &[&self.unstaged_files, &self.untracked_files],
            options,
            cx,
        );
        let (staged_all, staged_rows) = cached_tree(
            &state,
            ChangesSection::Staged,
            &[&self.staged_files],
            options,
            cx,
        );
        let partial = cached_partial(&state, &self.staged_files, cx);
        let busy = self.busy_action.is_some();
        let is_busy = |action: BusyAction| self.busy_action == Some(action);

        // La sección de la fila elegida: la que dice quien aloja la lista o,
        // si no, la de la fila en la que se hizo clic.
        let active_section = self.selected_section.or_else(|| {
            cursor
                .filter(|(_, path)| self.selected_file_path.as_deref() == Some(path.as_str()))
                .map(|(section, _)| section)
        });
        let shared = Rc::new(Shared {
            state: state.clone(),
            selected: self.selected_file_path.clone(),
            active_section,
            multi: self.multi_selection.clone(),
            on_select_file: self.on_select_file.clone(),
            on_open_file: self.on_open_file.clone(),
            on_multi_select: self.on_multi_select.clone(),
            file_menu: self.file_menu.clone(),
            dispatch: Dispatch {
                on_file_action: self.on_file_action.clone(),
                on_stage: self.on_stage_file.clone(),
                on_unstage: self.on_unstage_file.clone(),
                on_discard: self.on_discard_file.clone(),
            },
            collapsed,
            same_file_color: with_alpha(palette.primary, 0.55),
        });
        let section_rows = |section: ChangesSection,
                            rows: &Rc<Vec<TreeRow<ChangedFile>>>,
                            files: &Rc<Vec<ChangedFile>>,
                            check| SectionRows {
            section,
            rows: rows.clone(),
            files: files.clone(),
            flat: self.flat && section != ChangesSection::Conflict,
            shared: shared.clone(),
            check,
            stats: match section {
                ChangesSection::Unstaged => self.unstaged_stats.clone(),
                ChangesSection::Staged => self.staged_stats.clone(),
                ChangesSection::Conflict => None,
            },
        };
        let row_check = |staged: bool| {
            let allowed = if staged {
                self.can_unstage
            } else {
                self.can_stage
            };
            Some(RowCheck {
                staged,
                partial: partial.clone(),
                disabled: busy || !allowed,
            })
        };

        let conflict_section = (self.source.shows_working() && !self.conflicted_files.is_empty())
            .then(|| {
                let resolved_count = self
                    .conflict_state
                    .as_ref()
                    .map_or(0, |state| state.resolved_count);
                // `conflictState?.initialCount || conflictedFiles.length`
                let total = self
                    .conflict_state
                    .as_ref()
                    .map(|state| state.initial_count)
                    .filter(|count| *count > 0)
                    .unwrap_or(self.conflicted_files.len());
                let rows = section_rows(
                    ChangesSection::Conflict,
                    &conflict_rows,
                    &conflict_files,
                    None,
                );

                // `.conflict-section`: como las otras, mide lo que su
                // contenido y cede sitio si no cabe; con decenas de conflictos
                // Unstaged y Staged se siguen viendo.
                div()
                    .id("conflict-section")
                    .test_support()
                    .flex()
                    .flex_col()
                    .map(|this| fit_section(this, conflict_rows.len()))
                    .border_b_1()
                    .border_color(palette.border)
                    .bg(with_alpha(palette.destructive, 0.05))
                    .child(section_header(
                        "conflict-header",
                        "Conflicted Files",
                        self.conflicted_files.len(),
                        None,
                        conflict_progress(resolved_count, total, cx),
                        cx,
                    ))
                    .child(section_files(rows, &conflicted_scroll, None, cx))
            });

        let unstaged_section = self.source.shows_working().then(|| {
            let has_pending = !pending.is_empty();
            let can_stage_all = has_pending && self.can_stage && !busy;
            let stage_all = self.on_stage_all.clone().filter(|_| can_stage_all);
            // Vacía: nada de esto está preparado; mixta si algún archivo está
            // a medio preparar. Marcarla prepara todo.
            let check = section_checkbox(
                "unstaged-check",
                false,
                has_pending && pending.iter().any(|file| partial.contains(&file.path)),
                !can_stage_all,
                stage_all,
            );
            let actions = div()
                .flex()
                .flex_shrink_0()
                .items_center()
                .gap(px(2.))
                .children(
                    self.on_stage_all
                        .clone()
                        .filter(|_| has_pending && self.can_stage)
                        .map(|handler| {
                            section_text_button(
                                "stage-all",
                                "Stage all",
                                Lucide::Plus,
                                is_busy(BusyAction::StageAll),
                                busy,
                                move |window, cx| handler(window, cx),
                            )
                        }),
                )
                .children(
                    self.on_discard_all
                        .clone()
                        .filter(|_| has_pending && self.can_discard)
                        .map(|handler| {
                            danger_section_button(
                                "discard-all",
                                "Discard all",
                                Lucide::Trash,
                                is_busy(BusyAction::DiscardAll),
                                busy,
                                move |window, cx| handler(window, cx),
                            )
                        }),
                );

            // `.file-section.unstaged-section`
            div()
                .id("unstaged-section")
                .test_support()
                .flex()
                .flex_col()
                .map(|this| fit_section(this, pending_rows.len()))
                .child(section_header(
                    "unstaged-header",
                    "Unstaged",
                    pending.len(),
                    Some(check),
                    actions,
                    cx,
                ))
                .child(section_files(
                    section_rows(
                        ChangesSection::Unstaged,
                        &pending_rows,
                        &pending,
                        row_check(false),
                    ),
                    &unstaged_scroll,
                    Some("No unstaged changes"),
                    cx,
                ))
        });

        let staged_section = self.source.shows_staged().then(|| {
            let has_staged = !self.staged_files.is_empty();
            let can_unstage_all = has_staged && self.can_unstage && !busy;
            let unstage_all = self.on_unstage_all.clone().filter(|_| can_unstage_all);
            // Marcada: todo esto está preparado; mixta si algún archivo tiene
            // además cambios sin preparar. Desmarcarla lo quita todo.
            let check = section_checkbox(
                "staged-check",
                has_staged && partial.is_empty(),
                has_staged && !partial.is_empty(),
                !can_unstage_all,
                unstage_all,
            );
            let actions = div().flex().flex_shrink_0().items_center().children(
                self.on_unstage_all
                    .clone()
                    .filter(|_| has_staged && self.can_unstage)
                    .map(|handler| {
                        section_text_button(
                            "unstage-all",
                            "Unstage all",
                            Lucide::Minus,
                            is_busy(BusyAction::UnstageAll),
                            busy,
                            move |window, cx| handler(window, cx),
                        )
                    }),
            );

            // `.file-section.staged-section`
            div()
                .id("staged-section")
                .test_support()
                .flex()
                .flex_col()
                .map(|this| fit_section(this, staged_rows.len()))
                .border_t_1()
                .border_color(palette.border_hair)
                .child(section_header(
                    "staged-header",
                    "Staged",
                    self.staged_files.len(),
                    Some(check),
                    actions,
                    cx,
                ))
                .child(section_files(
                    section_rows(
                        ChangesSection::Staged,
                        &staged_rows,
                        &staged_all,
                        row_check(true),
                    ),
                    &staged_scroll,
                    Some("No staged changes"),
                    cx,
                ))
        });

        let toolbar = self.on_layout_change.clone().map(|handler| {
            let count = self.conflicted_files.len() + pending.len() + self.staged_files.len();
            layout_toolbar(self.flat, count, handler, cx)
        });

        let keyboard = self.focus_handle.clone().map(|focus_handle| ListKeys {
            focus_handle,
            shared: shared.clone(),
            sections: [
                (self.source.shows_working() && !self.conflicted_files.is_empty())
                    .then_some(ChangesSection::Conflict),
                self.source
                    .shows_working()
                    .then_some(ChangesSection::Unstaged),
                self.source.shows_staged().then_some(ChangesSection::Staged),
            ],
            busy,
            can_stage: self.can_stage,
            can_unstage: self.can_unstage,
            can_discard: self.can_discard,
            on_move: self
                .on_move_selection
                .clone()
                .or_else(|| self.on_select_file.clone()),
        });

        // `.files-container`
        root.when_some(keyboard, |root, keyboard| {
            root.track_focus(&keyboard.focus_handle.clone())
                .on_key_down(move |event, window, cx| keyboard.handle(event, window, cx))
        })
        .child(
            div()
                .flex()
                .flex_col()
                .h_full()
                .children(toolbar)
                .children(conflict_section)
                .children(unstaged_section)
                .children(staged_section),
        )
    }
}
