//! "Repo tree" (`src/components/repository/CommitTreeTab.vue`): el repositorio
//! tal como estaba en un commit, no solo lo que ese commit tocó. Un nivel por
//! carpeta, cargado cuando se abre (el árbol entero de un repositorio grande no
//! cabe de una vez), con los archivos cambiados marcados en su sitio.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::rc::Rc;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::scroll::{Scrollbar, ScrollbarAxis};
use gpui_kit::component::{VirtualListScrollHandle, v_virtual_list};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, ClickEvent, Context, Entity, EventEmitter, Pixels, SharedString, Size,
    Subscription, Window, div, px, relative, rems, size,
};
use mergeon_app::AppModel;
use mergeon_backend::{FileAtCommit, FileChange, TreeEntries, TreeEntry};
use mergeon_git_query::TreeEntryType;
use mergeon_model::format::format_size;
use mergeon_model::workspace_tabs::FileTabDescriptor;
use mergeon_ui::menu::{MenuItem, MenuTriggerExt as _};
use mergeon_ui::{ActivePaletteExt as _, IconTone, Lucide, SpinnerColor, SpinnerSize};
use mergeon_view_diff::{FileContent, FileContentViewer, FileContentViewerEvent, code_font_family};

/// `min-height: 1.72rem` de `.tree-item`: las filas miden lo mismo.
const TREE_ROW_HEIGHT: f32 = 1.72 * 16.;
/// Alto de la línea "N more entries not shown".
const OMITTED_ROW_HEIGHT: f32 = 2.03 * 16.;

/// Lo que el árbol pide a quien lo aloja. `OpenFile` es el clic y
/// `OpenFileNew` el doble clic; dentro de una pestaña de archivo el clic la
/// reutiliza.
#[derive(Debug, Clone, PartialEq)]
pub enum CommitTreeTabEvent {
    OpenFile(FileTabDescriptor),
    OpenFileNew(FileTabDescriptor),
}

/// Una fila visible del árbol.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct TreeRow {
    pub entry: TreeEntry,
    pub depth: usize,
    /// La carpeta no se pudo leer.
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct Level {
    entries: Vec<TreeEntry>,
    omitted: usize,
    /// Carpetas hijas que fallaron al abrirse, por ruta.
    errors: HashMap<String, String>,
}

type MenuProvider = Rc<dyn Fn(&FileTabDescriptor, &mut Window, &mut App) -> Vec<MenuItem>>;

/// El árbol del repositorio en un commit, con el visor del archivo abierto a
/// su derecha si no es "externo".
pub struct CommitTreeTab {
    model: Entity<AppModel>,
    external_files: bool,
    repo_path: String,
    commit_hash: String,
    /// Estado de cada archivo que tocó el commit, por ruta.
    changed_statuses: HashMap<String, String>,
    context_menu: Option<MenuProvider>,

    /// Cambia con cada commit: las respuestas de uno anterior se descartan.
    generation: u64,
    levels: HashMap<String, Level>,
    expanded: HashSet<String>,
    loading_paths: HashSet<String>,
    loading_root: bool,
    root_error: String,
    rows: Rc<Vec<TreeRow>>,
    item_sizes: Rc<Vec<Size<Pixels>>>,
    scroll: VirtualListScrollHandle,

    selected_path: String,
    opened_file: Option<FileAtCommit>,
    loading_file: bool,
    file_error: String,
    viewer: Entity<FileContentViewer>,
    _subscription: Subscription,
}

impl EventEmitter<CommitTreeTabEvent> for CommitTreeTab {}

impl CommitTreeTab {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let viewer = cx.new(|cx| FileContentViewer::new(window, cx));
        let subscription = cx.subscribe(&viewer, |this, _, event: &FileContentViewerEvent, cx| {
            if matches!(event, FileContentViewerEvent::Close) {
                this.close_file(cx);
            }
        });
        Self {
            model: AppModel::global(cx),
            external_files: false,
            repo_path: String::new(),
            commit_hash: String::new(),
            changed_statuses: HashMap::new(),
            context_menu: None,
            generation: 0,
            levels: HashMap::new(),
            expanded: HashSet::new(),
            loading_paths: HashSet::new(),
            loading_root: false,
            root_error: String::new(),
            rows: Rc::new(Vec::new()),
            item_sizes: Rc::new(Vec::new()),
            scroll: VirtualListScrollHandle::new(),
            selected_path: String::new(),
            opened_file: None,
            loading_file: false,
            file_error: String::new(),
            viewer,
            _subscription: subscription,
        }
    }

    // --- datos ----------------------------------------------------------------

    /// Repositorio y commit del árbol. Al cambiar cualquiera de los dos el
    /// árbol empieza de cero.
    pub fn set_source(&mut self, repo_path: &str, commit_hash: &str, cx: &mut Context<Self>) {
        if self.repo_path == repo_path && self.commit_hash == commit_hash {
            return;
        }
        self.repo_path = repo_path.to_string();
        self.commit_hash = commit_hash.to_string();
        self.reset(cx);
    }

    /// Los archivos que tocó este commit, para marcarlos dentro del árbol.
    pub fn set_commit_files(&mut self, files: &[FileChange], cx: &mut Context<Self>) {
        let statuses: HashMap<String, String> = files
            .iter()
            .map(|file| (file.path.clone(), file.status.clone()))
            .collect();
        if statuses != self.changed_statuses {
            self.changed_statuses = statuses;
            cx.notify();
        }
    }

    /// El árbol vive junto a un visor que no es el suyo (una pestaña de
    /// archivo): abrir un archivo solo avisa, sin leerlo aquí.
    pub fn set_external_files(&mut self, external: bool, cx: &mut Context<Self>) {
        if self.external_files != external {
            self.external_files = external;
            cx.notify();
        }
    }

    /// Entradas del menú contextual de un archivo (`file-context-menu`).
    pub fn set_file_context_menu(
        &mut self,
        items: impl Fn(&FileTabDescriptor, &mut Window, &mut App) -> Vec<MenuItem> + 'static,
    ) {
        self.context_menu = Some(Rc::new(items));
    }

    // --- lectura --------------------------------------------------------------

    /// Las filas a la vista: solo lo que está abierto, que es lo único cargado.
    pub fn rows(&self) -> &[TreeRow] {
        &self.rows
    }

    pub fn is_expanded(&self, path: &str) -> bool {
        self.expanded.contains(path)
    }

    pub fn is_loading(&self, path: &str) -> bool {
        self.loading_paths.contains(path)
    }

    pub fn is_loading_root(&self) -> bool {
        self.loading_root
    }

    pub fn selected_path(&self) -> &str {
        &self.selected_path
    }

    /// `omittedTotal`: entradas que no entraron por pasar del límite.
    pub fn omitted_total(&self) -> usize {
        self.levels.values().map(|level| level.omitted).sum()
    }

    /// Estado del archivo en este commit, si lo tocó.
    pub fn changed_status(&self, row: &TreeRow) -> Option<&str> {
        if row.entry.kind == TreeEntryType::Tree {
            return None;
        }
        self.changed_statuses
            .get(&row.entry.path)
            .map(String::as_str)
    }

    /// El texto del estado sin árbol: "Loading tree...", el error de la raíz
    /// o "No files at this commit".
    pub fn status_text(&self) -> Option<SharedString> {
        if self.loading_root {
            Some("Loading tree...".into())
        } else if !self.root_error.is_empty() {
            Some(self.root_error.clone().into())
        } else if self.rows.is_empty() {
            Some("No files at this commit".into())
        } else {
            None
        }
    }

    /// El visor del archivo abierto (cuando el árbol no es externo).
    pub fn viewer(&self) -> &Entity<FileContentViewer> {
        &self.viewer
    }

    /// `fileDescriptor(row)`.
    pub fn file_descriptor(&self, path: &str) -> FileTabDescriptor {
        FileTabDescriptor::tree(path, &self.commit_hash)
    }

    // --- acciones -------------------------------------------------------------

    /// `activate(row)`: una carpeta se pliega o despliega; un archivo se abre;
    /// un submódulo se enseña pero no se abre.
    pub fn activate(&mut self, path: &str, cx: &mut Context<Self>) {
        let Some(kind) = self
            .rows
            .iter()
            .find(|row| row.entry.path == path)
            .map(|row| row.entry.kind)
        else {
            return;
        };
        match kind {
            TreeEntryType::Tree => self.toggle_folder(path, cx),
            TreeEntryType::Commit => {}
            TreeEntryType::Blob => self.open_file(path, cx),
        }
    }

    /// `closeFile`.
    pub fn close_file(&mut self, cx: &mut Context<Self>) {
        self.selected_path.clear();
        self.opened_file = None;
        self.file_error.clear();
        self.sync_viewer(cx);
        cx.notify();
    }

    // --- interno --------------------------------------------------------------

    /// `reset`: otro commit, otro árbol.
    fn reset(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        self.loading_paths.clear();
        self.loading_root = false;
        self.loading_file = false;
        self.levels.clear();
        self.expanded.clear();
        self.root_error.clear();
        self.rebuild_rows();
        self.close_file(cx);

        if self.repo_path.is_empty() || self.commit_hash.is_empty() {
            return;
        }

        self.loading_root = true;
        self.load_level(String::new(), cx);
    }

    /// `loadLevel(directory)`.
    fn load_level(&mut self, directory: String, cx: &mut Context<Self>) {
        if self.levels.contains_key(&directory) || self.loading_paths.contains(&directory) {
            return;
        }

        let version = self.generation;
        self.loading_paths.insert(directory.clone());
        let read = self.model.update(cx, |model, cx| {
            model.get_tree_entries(&self.repo_path, &self.commit_hash, &directory, cx)
        });
        cx.notify();

        cx.spawn(async move |this, cx| {
            let level = read.await;
            let _ = this.update(cx, |this, cx| {
                if version != this.generation {
                    return;
                }
                match level {
                    Ok(TreeEntries { entries, omitted }) => {
                        this.levels.insert(
                            directory.clone(),
                            Level {
                                entries,
                                omitted,
                                errors: HashMap::new(),
                            },
                        );
                    }
                    Err(error) => {
                        // Una carpeta ilegible marca su rama; el resto del
                        // árbol sigue navegable.
                        let message = if error.message().is_empty() {
                            "Could not read this directory".to_string()
                        } else {
                            error.message().to_string()
                        };
                        if directory.is_empty() {
                            this.root_error = message;
                        } else {
                            let parent = directory
                                .rfind('/')
                                .map_or("", |separator| &directory[..separator]);
                            if let Some(level) = this.levels.get_mut(parent) {
                                level.errors.insert(directory.clone(), message);
                            }
                        }
                    }
                }
                this.loading_paths.remove(&directory);
                if directory.is_empty() {
                    this.loading_root = false;
                }
                this.rebuild_rows();
                cx.notify();
            });
        })
        .detach();
    }

    /// `toggleFolder(row)`.
    fn toggle_folder(&mut self, path: &str, cx: &mut Context<Self>) {
        if self.expanded.remove(path) {
            self.rebuild_rows();
            cx.notify();
            return;
        }

        self.expanded.insert(path.to_string());
        self.rebuild_rows();
        cx.notify();
        self.load_level(path.to_string(), cx);
    }

    /// `openFile(row)`.
    fn open_file(&mut self, path: &str, cx: &mut Context<Self>) {
        if self.external_files {
            self.selected_path = path.to_string();
            cx.emit(CommitTreeTabEvent::OpenFile(self.file_descriptor(path)));
            cx.notify();
            return;
        }

        let version = self.generation;
        let path = path.to_string();
        self.selected_path = path.clone();
        self.opened_file = None;
        self.file_error.clear();
        self.loading_file = true;
        let read = self.model.update(cx, |model, cx| {
            model.get_file_at_commit(&self.repo_path, &self.commit_hash, &path, cx)
        });
        self.sync_viewer(cx);
        cx.notify();

        cx.spawn(async move |this, cx| {
            let file = read.await;
            let _ = this.update(cx, |this, cx| {
                if version != this.generation || this.selected_path != path {
                    return;
                }
                match file {
                    Ok(file) => this.opened_file = Some(file),
                    Err(error) => {
                        this.file_error = if error.message().is_empty() {
                            "Could not read this file".to_string()
                        } else {
                            error.message().to_string()
                        };
                    }
                }
                this.loading_file = false;
                this.sync_viewer(cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn sync_viewer(&mut self, cx: &mut Context<Self>) {
        let path = self.selected_path.clone();
        let content = self.opened_file.as_ref().map(file_content);
        let (loading, error) = (self.loading_file, self.file_error.clone());
        self.viewer.update(cx, |viewer, cx| {
            viewer.set_loading(loading, cx);
            viewer.set_error(error, cx);
            viewer.set_file(path, content, cx);
        });
    }

    /// Aplana solo lo que está abierto, que es lo único que hay cargado.
    fn rebuild_rows(&mut self) {
        fn walk(
            levels: &HashMap<String, Level>,
            expanded: &HashSet<String>,
            directory: &str,
            depth: usize,
            output: &mut Vec<TreeRow>,
        ) {
            let Some(level) = levels.get(directory) else {
                return;
            };
            for entry in &level.entries {
                output.push(TreeRow {
                    entry: entry.clone(),
                    depth,
                    error: level.errors.get(&entry.path).cloned(),
                });
                if entry.kind == TreeEntryType::Tree && expanded.contains(&entry.path) {
                    walk(levels, expanded, &entry.path, depth + 1, output);
                }
            }
        }

        let mut rows = Vec::new();
        walk(&self.levels, &self.expanded, "", 0, &mut rows);

        let mut sizes = vec![size(px(0.), px(TREE_ROW_HEIGHT)); rows.len()];
        if self.omitted_total() > 0 {
            sizes.push(size(px(0.), px(OMITTED_ROW_HEIGHT)));
        }
        self.rows = Rc::new(rows);
        self.item_sizes = Rc::new(sizes);
    }

    fn render_rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let rows = self.rows.clone();
        range
            .map(|index| match rows.get(index) {
                Some(row) => self.render_row(row, cx),
                None => self.render_omitted(cx),
            })
            .collect()
    }

    fn render_omitted(&self, cx: &App) -> AnyElement {
        div()
            .id("tree-omitted")
            .test_support()
            .h(px(OMITTED_ROW_HEIGHT))
            .py(rems(0.5))
            .px(rems(0.65))
            .text_size(rems(0.6875))
            .text_color(cx.palette().muted_foreground)
            .child(format!("{} more entries not shown", self.omitted_total()))
            .into_any_element()
    }

    fn render_row(&self, row: &TreeRow, cx: &mut Context<Self>) -> AnyElement {
        let palette = *cx.palette();
        let entry = &row.entry;
        let is_tree = entry.kind == TreeEntryType::Tree;
        let selected = !is_tree && self.selected_path == entry.path;
        let icon = match entry.kind {
            TreeEntryType::Commit => Lucide::Package,
            TreeEntryType::Blob => Lucide::FileCodeCorner,
            TreeEntryType::Tree if self.expanded.contains(&entry.path) => Lucide::FolderOpen,
            TreeEntryType::Tree => Lucide::Folder,
        };

        // Lo que aporta el árbol frente a la lista de cambios: ver el cambio
        // dentro del proyecto, no aislado.
        let trailing: Option<AnyElement> = if let Some(status) = self.changed_status(row) {
            Some(mergeon_ui::status_badge(status).into_any_element())
        } else if self.loading_paths.contains(&entry.path) {
            Some(
                mergeon_ui::spinner()
                    .size(SpinnerSize::Sm)
                    .color(SpinnerColor::Primary)
                    .into_any_element(),
            )
        } else if let Some(error) = &row.error {
            Some(
                div()
                    .id("tree-error")
                    .test_support()
                    .flex_shrink_0()
                    .text_size(rems(0.6875))
                    .text_color(palette.destructive)
                    .tooltip(mergeon_ui::tooltip(error.clone()))
                    .child("failed")
                    .into_any_element(),
            )
        } else {
            entry.size.map(|size| {
                div()
                    .flex_shrink_0()
                    .font_family(code_font_family(cx))
                    .text_size(rems(0.6875))
                    .text_color(palette.muted_foreground)
                    .child(format_size(size as f64))
                    .into_any_element()
            })
        };

        let path = entry.path.clone();
        let item = div()
            .id(SharedString::from(format!("tree:{}", entry.path)))
            .test_support()
            .aria_label(SharedString::from(entry.name.clone()))
            .aria_selected(selected)
            .when(is_tree, |item| {
                item.aria_expanded(self.expanded.contains(&entry.path))
            })
            .flex()
            .items_center()
            .gap(rems(0.45))
            .h(px(TREE_ROW_HEIGHT))
            .w_full()
            .py(rems(0.22))
            .pr(rems(0.65))
            .pl(rems(row.depth as f32 * 1.05 + 0.6))
            .text_size(rems(0.8125))
            .text_color(palette.foreground)
            .cursor_pointer()
            .hover(move |style| style.bg(palette.accent))
            .when(selected, |item| item.bg(palette.accent))
            .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                if !event.standard_click() {
                    return;
                }
                this.activate(&path, cx);
                if event.click_count() == 2
                    && this
                        .rows
                        .iter()
                        .any(|row| row.entry.path == path && row.entry.kind != TreeEntryType::Tree)
                {
                    cx.emit(CommitTreeTabEvent::OpenFileNew(this.file_descriptor(&path)));
                }
            }))
            .child(div().flex().flex_shrink_0().items_center().child(
                mergeon_ui::icon(icon).size(px(14.)).tone(if is_tree {
                    IconTone::Muted
                } else {
                    IconTone::Default
                }),
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(entry.name.clone()),
            )
            .children(trailing);

        match (is_tree, self.context_menu.clone()) {
            (false, Some(menu)) => {
                let descriptor = self.file_descriptor(&entry.path);
                item.context_menu_items(move |window, cx| menu(&descriptor, window, cx))
                    .into_any_element()
            }
            _ => item.into_any_element(),
        }
    }
}

/// El contenido de un archivo, como lo espera el visor.
pub(crate) fn file_content(file: &FileAtCommit) -> FileContent {
    if file.binary {
        FileContent::binary(file.size)
    } else if file.too_large {
        FileContent::too_large(file.size)
    } else {
        FileContent::text(file.content.clone()).with_size(file.size)
    }
}

impl Render for CommitTreeTab {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();

        // `flex: 1` del árbol frente al `flex: 2` del visor.
        let pane_open = !self.selected_path.is_empty() && !self.external_files;
        let tree = div()
            .id("tree-container")
            .relative()
            .min_w_0()
            .h_full()
            .overflow_hidden()
            .bg(palette.card)
            .map(|tree| {
                if pane_open {
                    tree.flex_none().w(relative(1. / 3.))
                } else {
                    tree.flex_1()
                }
            });
        let tree = match self.status_text() {
            Some(status) => {
                let is_error = !self.loading_root && !self.root_error.is_empty();
                tree.child(
                    div()
                        .id("tree-state")
                        .test_support()
                        .aria_label(status.clone())
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .h_full()
                        .p(rems(1.5))
                        .text_size(rems(0.8125))
                        .text_color(if is_error {
                            palette.destructive
                        } else {
                            palette.muted_foreground
                        })
                        .when(self.loading_root, |state| {
                            state.child(
                                div()
                                    .mb_2()
                                    .child(mergeon_ui::spinner().color(SpinnerColor::Primary)),
                            )
                        })
                        .child(status),
                )
            }
            None => tree
                .child(
                    v_virtual_list(
                        cx.entity(),
                        "tree-rows",
                        self.item_sizes.clone(),
                        |tab, range, _, cx| tab.render_rows(range, cx),
                    )
                    .track_scroll(&self.scroll),
                )
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .child(Scrollbar::new(&self.scroll).axis(ScrollbarAxis::Vertical)),
                ),
        };

        div()
            .id("tree-tab")
            .flex()
            .size_full()
            .min_h_0()
            .line_height(relative(1.5))
            .child(tree)
            .when(pane_open, |panes| {
                panes.child(
                    div()
                        .flex()
                        .flex_1()
                        .min_w_0()
                        .min_h_0()
                        .border_l_1()
                        .border_color(palette.border)
                        .child(self.viewer.clone()),
                )
            })
    }
}
