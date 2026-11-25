//! Archivos que tocó un commit (`src/components/repository/CommitChangesTab.vue`):
//! la lista plana "Changed Files" con estado y cifras +/− y, cuando la lista no
//! vive dentro de una pestaña de archivo, el diff del archivo elegido a su
//! derecha.

use std::rc::Rc;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, Bounds, Context, Entity, EventEmitter, FontWeight, MouseButton, Pixels, Point,
    SharedString, UniformListScrollHandle, Window, canvas, div, px, relative, rems, uniform_list,
};
use mergeon_backend::FileChange;
use mergeon_model::collate::locale_compare;
use mergeon_model::format::format_size;
use mergeon_ui::menu::{MenuItem, MenuTriggerExt as _};
use mergeon_ui::theme::with_alpha;
use mergeon_ui::{ActivePaletteExt as _, SpinnerColor, SpinnerSize};
use mergeon_view_diff::{
    DiffViewMode, DiffViewer, FileTreeRow, ImageDiffViewer, LineAnchor, file_stats,
    image_text_toggle, is_svg_path, whole_file_tooltip,
};

use crate::resizer::{drag_tracker, resizer};

/// Ancho inicial de la lista de archivos junto al diff (`filesPaneWidth`).
pub const DEFAULT_FILES_PANE_WIDTH: f32 = 260.;
const MIN_FILES_PANE_WIDTH: f32 = 160.;
const MAX_FILES_PANE_WIDTH: f32 = 420.;

/// `clampFilesPaneWidth`.
pub fn clamp_files_pane_width(width: f32) -> f32 {
    width.clamp(MIN_FILES_PANE_WIDTH, MAX_FILES_PANE_WIDTH)
}

/// Lo que la lista pide a quien la aloja. `SelectFile` es el clic y `OpenFile`
/// el doble clic; quien aloja la lista decide qué hace cada uno (en Commits
/// los dos abren pestaña, en una pestaña de archivo el clic la reutiliza).
///
/// El cambio Unified | Split ya no vive aquí: es el segmentado del bloque de
/// cifras del inspector (`CommitInspector::on_diff_view_mode`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommitChangesTabEvent {
    SelectFile(FileChange),
    OpenFile(FileChange),
    ToggleIgnoreWhitespace,
    /// "Whole file": quien aloja la lista cambia la preferencia y vuelve a
    /// pedir el diff.
    ToggleWholeFile,
}

/// Una fila de la lista plana: el nombre y, en gris, su carpeta.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct FileRow {
    pub name: String,
    pub dir: String,
    pub file: FileChange,
}

/// `fileRows`: lista plana, ordenada por ruta. En un commit interesa qué
/// archivos tocó, no recorrer carpetas; el árbol completo vive en "Repo tree".
pub fn file_rows(files: &[FileChange]) -> Vec<FileRow> {
    let mut sorted: Vec<&FileChange> = files.iter().collect();
    sorted.sort_by(|left, right| locale_compare(&left.path, &right.path));
    sorted
        .into_iter()
        .map(|file| {
            let (dir, name) = match file.path.rfind('/') {
                Some(separator) => (&file.path[..separator], &file.path[separator + 1..]),
                None => ("", file.path.as_str()),
            };
            FileRow {
                name: name.to_string(),
                dir: dir.to_string(),
                file: file.clone(),
            }
        })
        .collect()
}

/// Una fila de "Changed Files". Fuera de `render` porque la pinta la lista
/// virtual: los eventos llegan a la vista por su `WeakEntity`.
fn file_row_element(
    row: &FileRow,
    selected_path: Option<&str>,
    context_menu: &Option<MenuProvider>,
    this: &gpui_kit::WeakEntity<CommitChangesTab>,
) -> AnyElement {
    let file = row.file.clone();
    let selected = selected_path == Some(file.path.as_str());
    let (select, open, menu_file) = (file.clone(), file.clone(), file.clone());
    let (select_to, open_to) = (this.clone(), this.clone());
    let tree_row = FileTreeRow::file(
        SharedString::from(format!("file:{}", file.path)),
        row.name.clone(),
    )
    .path(file.path.clone())
    .dir(row.dir.clone())
    .selected(selected)
    .leading(mergeon_ui::status_badge(&file.status).on_ink(selected))
    .trailing(
        div().flex_shrink_0().children(
            file_stats(file.additions as usize, file.deletions as usize)
                .map(|stats| stats.bars(true).on_ink(selected)),
        ),
    )
    .on_activate(move |_, _, cx| {
        select_to
            .update(cx, |_, cx| {
                cx.emit(CommitChangesTabEvent::SelectFile(select.clone()))
            })
            .ok();
    })
    .on_open_new(move |_, _, cx| {
        open_to
            .update(cx, |_, cx| {
                cx.emit(CommitChangesTabEvent::OpenFile(open.clone()))
            })
            .ok();
    });

    // La fila compartida no se registra para las pruebas: su envoltura sí.
    let wrapper = div()
        .id(file_row_id(&file.path))
        .test_support()
        .aria_label(SharedString::from(file.path.clone()))
        .aria_selected(selected)
        .flex_shrink_0();
    match context_menu.clone() {
        Some(menu) => wrapper
            .child(tree_row.context_menu_items(move |window, cx| menu(&menu_file, window, cx)))
            .into_any_element(),
        None => wrapper.child(tree_row).into_any_element(),
    }
}

/// Id del elemento de la fila de un archivo, para encontrarla en la ventana.
pub fn file_row_id(path: &str) -> SharedString {
    SharedString::from(format!("file-row:{path}"))
}

type MenuProvider = Rc<dyn Fn(&FileChange, &mut Window, &mut App) -> Vec<MenuItem>>;

/// Lista de archivos de un commit, con su diff al lado si no es "externa".
/// Componente controlado: recibe los datos con los `set_*` y avisa con
/// [`CommitChangesTabEvent`].
pub struct CommitChangesTab {
    external_files: bool,
    /// La cabecera "Changed Files  N" sobre la lista.
    show_header: bool,
    loading_details: bool,
    /// `Rc`: la lista virtual las lee desde su closure sin copiarlas.
    rows: Rc<Vec<FileRow>>,
    /// Los archivos tal como llegaron, para no reordenarlos si no cambiaron.
    source_files: Vec<FileChange>,
    file_count: usize,
    files_scroll: UniformListScrollHandle,
    selected_path: Option<String>,
    loading_diff: bool,
    file_diff: SharedString,
    diff_too_large: bool,
    diff_whole_file_fell_back: bool,
    diff_size: u64,
    diff_view_mode: DiffViewMode,
    ignore_whitespace: bool,
    whole_file: bool,
    /// La línea que se leía al pulsar "Whole file": el diff que llega
    /// después vuelve a ella (el desplazamiento en píxeles de un diff no vale
    /// en el otro).
    line_anchor: Option<LineAnchor>,
    context_menu: Option<MenuProvider>,
    /// Línea bajo la lista (la comparación: "and N more files").
    files_note: Option<SharedString>,

    viewer: Entity<DiffViewer>,
    /// Antes y después de una imagen: quien aloja la lista lo llena
    /// (`image_viewer()`) y se pinta en lugar del diff si la ruta es de una
    /// imagen.
    image_viewer: Entity<ImageDiffViewer>,
    files_pane_width: f32,
    is_resizing: bool,
    /// Borde izquierdo de la rejilla, para traducir el ratón a un ancho.
    grid_bounds: Bounds<Pixels>,
}

impl EventEmitter<CommitChangesTabEvent> for CommitChangesTab {}

impl CommitChangesTab {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            external_files: false,
            show_header: true,
            loading_details: false,
            rows: Rc::new(Vec::new()),
            source_files: Vec::new(),
            file_count: 0,
            files_scroll: UniformListScrollHandle::new(),
            selected_path: None,
            loading_diff: false,
            file_diff: SharedString::default(),
            diff_too_large: false,
            diff_whole_file_fell_back: false,
            diff_size: 0,
            diff_view_mode: DiffViewMode::default(),
            ignore_whitespace: false,
            whole_file: false,
            line_anchor: None,
            context_menu: None,
            files_note: None,
            viewer: cx.new(|cx| DiffViewer::new(window, cx)),
            image_viewer: cx.new(ImageDiffViewer::new),
            files_pane_width: DEFAULT_FILES_PANE_WIDTH,
            is_resizing: false,
            grid_bounds: Bounds::default(),
        }
    }

    // --- datos ----------------------------------------------------------------

    /// La lista vive junto a un visor que no es el suyo (una pestaña de
    /// archivo, o Commits dentro del espacio de trabajo): solo se pinta la
    /// lista, a todo el ancho.
    pub fn set_external_files(&mut self, external: bool, cx: &mut Context<Self>) {
        if self.external_files != external {
            self.external_files = external;
            cx.notify();
        }
    }

    /// Pinta (por defecto) o no la cabecera "Changed Files  N" de la lista.
    /// El inspector de Commits la quita: su bloque de cifras ya dice cuántos
    /// archivos son.
    pub fn set_show_header(&mut self, show: bool, cx: &mut Context<Self>) {
        if self.show_header != show {
            self.show_header = show;
            cx.notify();
        }
    }

    pub fn set_loading_details(&mut self, loading: bool, cx: &mut Context<Self>) {
        if self.loading_details != loading {
            self.loading_details = loading;
            cx.notify();
        }
    }

    pub fn set_commit_files(&mut self, files: &[FileChange], cx: &mut Context<Self>) {
        // Se llama cada vez que el modelo de la app avisa de un cambio, sea el
        // que sea: ordenar miles de rutas con `locale_compare` en cada aviso
        // hacía que hasta pasar el ratón por la lista costara cientos de ms.
        if files == self.source_files.as_slice() {
            return;
        }
        self.source_files = files.to_vec();
        let rows = file_rows(files);
        if rows != *self.rows {
            self.file_count = files.len();
            self.rows = Rc::new(rows);
            cx.notify();
        }
    }

    /// Ruta del archivo elegido (`selectedFile`).
    pub fn set_selected_path(&mut self, path: Option<&str>, cx: &mut Context<Self>) {
        if self.selected_path.as_deref() != path {
            self.selected_path = path.map(str::to_string);
            self.line_anchor = None;
            self.sync_viewer(cx);
            cx.notify();
        }
    }

    /// `loadingDiff`.
    pub fn set_loading_diff(&mut self, loading: bool, cx: &mut Context<Self>) {
        if self.loading_diff != loading {
            self.loading_diff = loading;
            cx.notify();
        }
    }

    /// `fileDiff`, `diffTooLarge` y `diffSize`.
    pub fn set_diff(
        &mut self,
        diff: impl Into<SharedString>,
        too_large: bool,
        size: u64,
        cx: &mut Context<Self>,
    ) {
        let diff = diff.into();
        if self.file_diff != diff || self.diff_too_large != too_large || self.diff_size != size {
            self.file_diff = diff;
            self.diff_too_large = too_large;
            self.diff_size = size;
            self.sync_viewer(cx);
            if let Some(anchor) = self.line_anchor.take() {
                self.viewer
                    .update(cx, |viewer, cx| viewer.scroll_to_line(&anchor, cx));
            }
            cx.notify();
        }
    }

    /// `diffViewMode` e `ignoreWhitespace`.
    pub fn set_diff_preferences(
        &mut self,
        view_mode: DiffViewMode,
        ignore_whitespace: bool,
        cx: &mut Context<Self>,
    ) {
        if self.diff_view_mode != view_mode || self.ignore_whitespace != ignore_whitespace {
            self.diff_view_mode = view_mode;
            self.ignore_whitespace = ignore_whitespace;
            self.sync_viewer(cx);
            cx.notify();
        }
    }

    /// "Whole file": el diff trae el archivo entero (el botón se enciende).
    pub fn set_whole_file(&mut self, whole_file: bool, cx: &mut Context<Self>) {
        if self.whole_file != whole_file {
            self.whole_file = whole_file;
            cx.notify();
        }
    }

    /// "Whole file" superó el tope de tamaño y cayó al diff normal por bloques.
    pub fn set_whole_file_fell_back(&mut self, fell_back: bool, cx: &mut Context<Self>) {
        if self.diff_whole_file_fell_back != fell_back {
            self.diff_whole_file_fell_back = fell_back;
            cx.notify();
        }
    }

    pub fn is_whole_file(&self) -> bool {
        self.whole_file
    }

    /// El visor de las dos versiones de una imagen elegida.
    pub fn image_viewer(&self) -> &Entity<ImageDiffViewer> {
        &self.image_viewer
    }

    /// Entradas del menú contextual de un archivo (`file-context-menu`).
    pub fn set_file_context_menu(
        &mut self,
        items: impl Fn(&FileChange, &mut Window, &mut App) -> Vec<MenuItem> + 'static,
    ) {
        self.context_menu = Some(Rc::new(items));
    }

    /// Una nota bajo la lista de archivos (p. ej. cuántos quedaron fuera).
    pub fn set_files_note(&mut self, note: Option<SharedString>, cx: &mut Context<Self>) {
        if self.files_note != note {
            self.files_note = note;
            cx.notify();
        }
    }

    // --- lectura --------------------------------------------------------------

    pub fn is_external_files(&self) -> bool {
        self.external_files
    }

    pub fn is_loading_details(&self) -> bool {
        self.loading_details
    }

    /// Las filas, en el orden en que se pintan.
    pub fn rows(&self) -> &[FileRow] {
        &self.rows
    }

    pub fn selected_path(&self) -> Option<&str> {
        self.selected_path.as_deref()
    }

    pub fn diff_view_mode(&self) -> DiffViewMode {
        self.diff_view_mode
    }

    pub fn is_ignoring_whitespace(&self) -> bool {
        self.ignore_whitespace
    }

    /// El visor del diff del archivo elegido.
    pub fn viewer(&self) -> &Entity<DiffViewer> {
        &self.viewer
    }

    pub fn files_pane_width(&self) -> f32 {
        self.files_pane_width
    }

    pub fn is_resizing(&self) -> bool {
        self.is_resizing
    }

    /// Texto del estado sin lista o sin diff, si lo hay: "Loading changes...",
    /// "No files changed", "Select a file to view changes", "Loading diff..." o
    /// "Diff too large to display here (…)".
    pub fn status_text(&self) -> Option<SharedString> {
        if self.loading_details {
            Some("Loading changes...".into())
        } else if self.rows.is_empty() {
            Some("No files changed".into())
        } else if self.external_files {
            None
        } else if self.selected_path.is_none() {
            Some("Select a file to view changes".into())
        } else if self.loading_diff {
            Some("Loading diff...".into())
        } else if self.diff_too_large {
            Some(
                format!(
                    "Diff too large to display here (over {})",
                    format_size(self.diff_size as f64)
                )
                .into(),
            )
        } else {
            None
        }
    }

    // --- interno --------------------------------------------------------------

    fn sync_viewer(&mut self, cx: &mut Context<Self>) {
        let path = self.selected_path.clone().unwrap_or_default();
        let diff = self.file_diff.clone();
        let mode = self.diff_view_mode;
        self.viewer.update(cx, |viewer, cx| {
            viewer.set_diff(diff, path, cx);
            viewer.set_view_mode(mode, cx);
        });
    }

    fn start_resize(&mut self, cx: &mut Context<Self>) {
        self.is_resizing = true;
        cx.notify();
    }

    fn handle_resize(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        if !self.is_resizing {
            return;
        }
        let width = clamp_files_pane_width(f32::from(position.x - self.grid_bounds.left()));
        if width != self.files_pane_width {
            self.files_pane_width = width;
            cx.notify();
        }
    }

    fn stop_resize(&mut self, cx: &mut Context<Self>) {
        if self.is_resizing {
            self.is_resizing = false;
            cx.notify();
        }
    }

    fn header(cx: &App) -> gpui_kit::Div {
        let palette = cx.palette();
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .justify_between()
            .min_h(rems(2.05))
            .py(rems(0.42))
            .px(rems(0.65))
            .border_b_1()
            .border_color(palette.border)
            .bg(palette.accent)
    }

    fn placeholder(text: impl Into<SharedString>, spinner: bool, cx: &App) -> AnyElement {
        div()
            .flex()
            .items_center()
            .justify_center()
            .size_full()
            .text_color(cx.palette().muted_foreground)
            .when(spinner, |state| {
                state.child(
                    mergeon_ui::spinner()
                        .size(SpinnerSize::Sm)
                        .color(SpinnerColor::Primary),
                )
            })
            .child(text.into())
            .into_any_element()
    }

    fn render_files(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        // Lista virtual: solo se construyen las filas visibles. Antes se armaban
        // todas en cada render y un commit con miles de archivos hacía que
        // desplazar, pasar el ratón o elegir uno costara cientos de ms.
        let rows = self.rows.clone();
        let selected_path = self.selected_path.clone();
        let context_menu = self.context_menu.clone();
        let this = cx.entity().downgrade();
        let list = uniform_list("files-list", rows.len(), move |range, _, _| {
            rows[range.start..range.end.min(rows.len())]
                .iter()
                .map(|row| file_row_element(row, selected_path.as_deref(), &context_menu, &this))
                .collect()
        })
        .track_scroll(&self.files_scroll)
        .size_full();

        div()
            .flex()
            .flex_col()
            .min_w_0()
            .min_h_0()
            .h_full()
            .border_r_1()
            .border_color(palette.border)
            .bg(palette.card)
            .when(self.show_header, |files| {
                files.child(
                    Self::header(cx)
                        .child(
                            div()
                                .text_size(rems(0.75))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(palette.foreground)
                                .child("Changed Files"),
                        )
                        .child(
                            div()
                                .id("file-count")
                                .test_support()
                                .text_size(rems(0.75))
                                .text_color(palette.muted_foreground)
                                .child(self.file_count.to_string()),
                        ),
                )
            })
            .child(
                div()
                    .id("files-tree")
                    .when(!self.show_header, |tree| tree.pt(px(4.)))
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(list)
                    .vertical_scrollbar(&self.files_scroll),
            )
            .when_some(self.files_note.clone(), |files, note| {
                files.child(
                    div()
                        .id("files-note")
                        .test_support()
                        .flex_shrink_0()
                        .px(rems(0.65))
                        .py(px(6.))
                        .border_t_1()
                        .border_color(palette.border_hair)
                        .text_size(rems(0.75))
                        .text_color(palette.text_3)
                        .child(note),
                )
            })
    }

    fn render_diff_pane(&self, path: &str, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let name = path.rsplit('/').next().unwrap_or(path).to_string();
        let ignoring = self.ignore_whitespace;
        let whole_file = self.whole_file;
        // Un SVG con "Text" elegido se ve como texto, con sus interruptores.
        let image = self.image_viewer.read(cx).shows_image(path);
        let svg_toggle = is_svg_path(path).then(|| image_text_toggle(&self.image_viewer, cx));

        let body = if image {
            self.image_viewer.clone().into_any_element()
        } else if self.loading_diff {
            Self::placeholder("Loading diff...", true, cx)
        } else if self.diff_too_large {
            Self::placeholder(
                format!(
                    "Diff too large to display here (over {})",
                    format_size(self.diff_size as f64)
                ),
                false,
                cx,
            )
        } else {
            self.viewer.clone().into_any_element()
        };

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .h_full()
            .child(
                Self::header(cx)
                    .child(
                        div()
                            .id("diff-title")
                            .min_w_0()
                            .truncate()
                            .text_size(rems(0.8125))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(palette.foreground)
                            .tooltip(mergeon_ui::tooltip(path.to_string()))
                            .child(name),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_shrink_0()
                            .items_center()
                            .gap_1()
                            .children(svg_toggle)
                            .when(!image, |actions| actions.child(
                                mergeon_ui::button("toggle-whole-file")
                                    .ghost()
                                    .sm()
                                    .label("Whole file")
                                    .tooltip(whole_file_tooltip(whole_file))
                                    .when(whole_file, |button| {
                                        button
                                            .bg(with_alpha(palette.primary, 0.14))
                                            .text_color(palette.primary_text)
                                    })
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.line_anchor = this
                                            .viewer
                                            .update(cx, |viewer, _| viewer.line_anchor());
                                        cx.emit(CommitChangesTabEvent::ToggleWholeFile);
                                    })),
                            ))
                            .when(!image, |actions| actions.child(
                                mergeon_ui::button("toggle-ignore-whitespace")
                                    .ghost()
                                    .sm()
                                    .label("Ignore spaces")
                                    .tooltip(if ignoring {
                                        "Ignoring whitespace — click to bring it back"
                                    } else {
                                        "Hide whitespace-only changes, so a reindented file shows just what really changed"
                                    })
                                    // Encendido se distingue del resto de acciones.
                                    .when(ignoring, |button| {
                                        button
                                            .bg(with_alpha(palette.primary, 0.14))
                                            .text_color(palette.primary_text)
                                    })
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.line_anchor = this
                                            .viewer
                                            .update(cx, |viewer, _| viewer.line_anchor());
                                        cx.emit(CommitChangesTabEvent::ToggleIgnoreWhitespace);
                                    })),
                            )),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .bg(palette.card)
                    .child(body),
            )
            .when(!image && self.diff_whole_file_fell_back, |this| {
                this.child(
                    div()
                        .id("diff-whole-file-fallback")
                        .test_support()
                        .flex_none()
                        .py(rems(0.35))
                        .px(rems(0.65))
                        .border_t_1()
                        .border_color(palette.border)
                        .text_size(rems(0.6875))
                        .text_color(palette.muted_foreground)
                        .child("File too large for Whole file — showing changes only."),
                )
            })
    }
}

impl Render for CommitChangesTab {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let shell = div()
            .id("changes-tab")
            .test_support()
            .size_full()
            .min_h_0()
            .overflow_hidden()
            .line_height(relative(1.5));

        if self.loading_details {
            return shell.child(Self::placeholder("Loading changes...", true, cx));
        }
        if self.rows.is_empty() {
            return shell.child(Self::placeholder("No files changed", false, cx));
        }
        if self.external_files {
            return shell.child(self.render_files(cx));
        }

        let entity = cx.entity().downgrade();
        let measure = {
            let entity = entity.clone();
            canvas(
                move |bounds, _, cx| {
                    let _ = entity.update(cx, |tab, _| tab.grid_bounds = bounds);
                },
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0()
        };

        shell.child(
            div()
                .relative()
                .flex()
                .size_full()
                .min_h_0()
                .min_w_0()
                .child(measure)
                .child(drag_tracker(
                    self.is_resizing,
                    entity,
                    |tab, position, _, cx| tab.handle_resize(position, cx),
                    |tab, _, cx| tab.stop_resize(cx),
                ))
                .child(
                    div()
                        .flex_shrink_0()
                        .h_full()
                        .w(px(self.files_pane_width))
                        .child(self.render_files(cx)),
                )
                .child(
                    resizer("changes-resizer", self.is_resizing, cx)
                        .test_support()
                        .aria_label("Resize changed files panel")
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|tab, _, window, cx| {
                                window.prevent_default();
                                tab.start_resize(cx);
                            }),
                        ),
                )
                .map(|grid| match self.selected_path.clone() {
                    Some(path) => grid.child(self.render_diff_pane(&path, cx)),
                    None => grid.child(div().flex_1().min_w_0().child(Self::placeholder(
                        "Select a file to view changes",
                        false,
                        cx,
                    ))),
                }),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(path: &str) -> FileChange {
        FileChange {
            path: path.to_string(),
            status: "M".to_string(),
            additions: 0,
            deletions: 0,
        }
    }

    // Lista plana: la carpeta va en gris junto al nombre.
    #[test]
    fn lists_files_flat_with_the_folder_next_to_the_name() {
        let rows = file_rows(&[change("src/main.js"), change("README.md")]);

        let shown: Vec<(&str, &str)> = rows
            .iter()
            .map(|row| (row.name.as_str(), row.dir.as_str()))
            .collect();
        assert_eq!(shown, [("README.md", ""), ("main.js", "src")]);
    }

    #[test]
    fn sorts_paths_without_minding_the_case() {
        let rows = file_rows(&[
            change("styles/main.css"),
            change("src/lib/util.js"),
            change("docs/old.md"),
            change("README.md"),
            change("docs/guide.md"),
            change("src/app.js"),
        ]);

        let paths: Vec<&str> = rows.iter().map(|row| row.file.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "docs/guide.md",
                "docs/old.md",
                "README.md",
                "src/app.js",
                "src/lib/util.js",
                "styles/main.css",
            ]
        );
    }

    #[test]
    fn keeps_the_files_pane_between_its_limits() {
        assert_eq!(clamp_files_pane_width(20.), 160.);
        assert_eq!(clamp_files_pane_width(300.), 300.);
        assert_eq!(clamp_files_pane_width(900.), 420.);
    }
}
