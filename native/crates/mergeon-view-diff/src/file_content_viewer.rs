//! Contenido de un archivo tal como estaba en un commit
//! (`src/components/repository/FileContentViewer.vue`): números de línea, el
//! mismo coloreado que el diff y un aviso en vez del volcado cuando el archivo
//! es binario o enorme.

use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Div, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _, Pixels, Render,
    SharedString, Styled as _, Task, TestSupportExt as _, Window, div, px, rems,
};
use mergeon_model::format::format_size;
use mergeon_ui::{ActivePaletteExt as _, Lucide, SpinnerColor, SpinnerSize};

use crate::canvas::{
    self, CanvasDelegate, CanvasState, Cell, Fill, GridLayout, Label, LabelAlign, Metrics,
    RowHeights, RowSpec,
};
use crate::diff_viewer::{PARSING_INDICATOR_DELAY, SYNC_PARSE_BYTES, ScrollPosition};
use crate::find::{self, FindInViewer, FindSource, FindState, FindStatus, Searchable};
use crate::highlight::{Language, Token, highlight_tokens, language_for_path};
use crate::text::{display_columns, display_offset, expand_tabs, source_range};
use crate::typography::{CodeColors, code_font_family, syntax_runs};

/// El archivo que se muestra (`getFileAtCommit`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct FileContent {
    /// Vacío si el archivo es binario o demasiado grande.
    pub content: SharedString,
    /// Bytes del archivo.
    pub size: u64,
    pub binary: bool,
    pub too_large: bool,
}

impl FileContent {
    /// Un archivo de texto; el tamaño es el de su contenido.
    pub fn text(content: impl Into<SharedString>) -> Self {
        let content = content.into();
        Self {
            size: content.len() as u64,
            content,
            binary: false,
            too_large: false,
        }
    }

    /// Un archivo binario: se anuncia, no se vuelca.
    pub fn binary(size: u64) -> Self {
        Self {
            size,
            binary: true,
            ..Self::default()
        }
    }

    /// Un archivo demasiado grande para mostrarlo.
    pub fn too_large(size: u64) -> Self {
        Self {
            size,
            too_large: true,
            ..Self::default()
        }
    }

    /// Tamaño real del archivo cuando no coincide con el del contenido.
    pub fn with_size(mut self, size: u64) -> Self {
        self.size = size;
        self
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum FileContentViewerEvent {
    /// Clic en el botón de cerrar de la cabecera (`@close`).
    Close,
    /// El contenido se desplazó (`@view-scroll`).
    Scrolled(ScrollPosition),
}

struct LineText {
    display: SharedString,
    tokens: Vec<Token>,
}

/// Archivos hasta este tamaño se indexan en el acto (lo mismo que el diff):
/// no hay ni un frame sin contenido. Los más grandes, fuera del hilo de la
/// interfaz.
const SYNC_INDEX_BYTES: usize = SYNC_PARSE_BYTES;

/// Rango de cada línea dentro de `content` (sin el retorno de carro de un
/// final de línea de Windows) y columnas de la más larga. No toca la
/// interfaz: se puede llamar fuera del hilo principal.
fn index_lines(content: &str) -> (Vec<Range<usize>>, usize) {
    let mut lines = Vec::new();
    let mut max_columns = 0;
    let mut start = 0;
    for line in content.split('\n') {
        let shown = line.strip_suffix('\r').unwrap_or(line);
        lines.push(start..start + shown.len());
        max_columns = max_columns.max(display_columns(shown));
        start += line.len() + 1;
    }
    // El texto termina en salto de línea: el último elemento sobra.
    if content.is_empty() || content.ends_with('\n') {
        lines.pop();
    }
    (lines, max_columns)
}

/// Un archivo grande que se está indexando en segundo plano.
struct PendingIndex {
    generation: u64,
    /// El desplazamiento que se restaure mientras tanto: se aplica al acabar.
    scroll: ScrollPosition,
    /// Pasó [`PARSING_INDICATOR_DELAY`]: se muestra que se está cargando.
    slow: bool,
    /// Soltarlas cancela el indexado y el aviso.
    _index: Task<()>,
    _indicator: Task<()>,
}

#[derive(Debug, Clone, PartialEq)]
struct LayoutKey {
    metrics: Metrics,
    measured_width: Pixels,
}

/// Visor del contenido de un archivo.
///
/// Se crea una vez y se le dan los datos con [`Self::set_file`],
/// [`Self::set_loading`] y [`Self::set_error`]. Ocupa todo el espacio de su
/// contenedor.
pub struct FileContentViewer {
    canvas: CanvasState,
    path: SharedString,
    file: Option<FileContent>,
    loading: bool,
    error: SharedString,
    header_hidden: bool,
    language: Option<Language>,
    /// Rango de cada línea dentro del contenido (compartido con la
    /// búsqueda, que puede correr en segundo plano).
    lines: Arc<Vec<Range<usize>>>,
    max_columns: usize,
    cache: Vec<Option<Rc<LineText>>>,
    layout: Option<(LayoutKey, GridLayout)>,
    colors: Option<CodeColors>,
    find: FindState,
    /// Cambia con cada archivo nuevo: la búsqueda se repite.
    document_generation: u64,
    /// El archivo (grande) cuyas líneas se están indexando.
    pending: Option<PendingIndex>,
    /// Cuenta los indexados lanzados: uno viejo que acabe tarde no pisa al
    /// archivo nuevo.
    index_generation: u64,
}

impl EventEmitter<FileContentViewerEvent> for FileContentViewer {}

impl Focusable for FileContentViewer {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.canvas.focus.clone()
    }
}

impl FileContentViewer {
    pub fn new(_: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            canvas: CanvasState::new(cx),
            path: SharedString::default(),
            file: None,
            loading: false,
            error: SharedString::default(),
            header_hidden: false,
            language: None,
            lines: Arc::default(),
            max_columns: 0,
            cache: Vec::new(),
            layout: None,
            colors: None,
            find: FindState::default(),
            document_generation: 0,
            pending: None,
            index_generation: 0,
        }
    }

    /// Cambia el archivo que se muestra. `path` decide el lenguaje con el que se
    /// colorea; `None` deja el visor sin archivo ("Empty file"). No hace nada
    /// si ruta y archivo son los que ya había.
    pub fn set_file(
        &mut self,
        path: impl Into<SharedString>,
        file: Option<FileContent>,
        cx: &mut Context<Self>,
    ) {
        let path = path.into();
        if path == self.path && file == self.file {
            return;
        }

        self.language = language_for_path(&path);
        self.path = path;
        self.file = file;

        // Un binario o un archivo enorme se anuncian; volcarlos no ayuda.
        let content = self
            .file
            .as_ref()
            .filter(|file| !file.binary && !file.too_large)
            .map(|file| file.content.clone())
            .unwrap_or_default();

        if content.len() <= SYNC_INDEX_BYTES {
            self.pending = None;
            let (lines, max_columns) = index_lines(&content);
            self.apply_index(lines, max_columns, ScrollPosition::default(), cx);
            return;
        }

        // Partir en líneas y medir varios MB se hace fuera del hilo de la
        // interfaz. Mientras tanto no hay líneas (las del archivo anterior no
        // son de este contenido).
        self.index_generation += 1;
        let generation = self.index_generation;
        let indexing = cx.background_spawn(async move { index_lines(&content) });
        let index = cx.spawn(async move |this, cx| {
            let (lines, max_columns) = indexing.await;
            this.update(cx, |this, cx| {
                this.finish_index(generation, lines, max_columns, cx)
            })
            .ok();
        });
        let indicator = cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(PARSING_INDICATOR_DELAY)
                .await;
            this.update(cx, |this, cx| {
                if let Some(pending) = &mut this.pending
                    && pending.generation == generation
                {
                    pending.slow = true;
                    cx.notify();
                }
            })
            .ok();
        });
        self.pending = Some(PendingIndex {
            generation,
            scroll: ScrollPosition::default(),
            slow: false,
            _index: index,
            _indicator: indicator,
        });
        self.apply_index(Vec::new(), 0, ScrollPosition::default(), cx);
    }

    /// `true` mientras las líneas de un archivo grande se indexan en segundo
    /// plano.
    pub fn is_indexing(&self) -> bool {
        self.pending.is_some()
    }

    fn finish_index(
        &mut self,
        generation: u64,
        lines: Vec<Range<usize>>,
        max_columns: usize,
        cx: &mut Context<Self>,
    ) {
        // Otro archivo pedido entretanto: estas líneas no son suyas.
        let Some(pending) = self
            .pending
            .take_if(|pending| pending.generation == generation)
        else {
            return;
        };
        self.apply_index(lines, max_columns, pending.scroll, cx);
    }

    fn apply_index(
        &mut self,
        lines: Vec<Range<usize>>,
        max_columns: usize,
        scroll: ScrollPosition,
        cx: &mut Context<Self>,
    ) {
        self.max_columns = max_columns;
        self.lines = Arc::new(lines);
        self.cache = vec![None; self.lines.len()];
        self.layout = None;
        self.canvas.reset_content();
        self.canvas.scroll.set_position(scroll.to_point());
        self.document_generation += 1;
        find::refresh(self, cx);
        cx.notify();
    }

    /// Abre la barra de búsqueda (o vuelve a su campo).
    pub fn open_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        find::open(self, window, cx);
    }

    pub fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        find::close(self, window, cx);
    }

    pub fn find_next(&mut self, cx: &mut Context<Self>) {
        find::navigate(self, true, cx);
    }

    pub fn find_previous(&mut self, cx: &mut Context<Self>) {
        find::navigate(self, false, cx);
    }

    pub fn find_status(&self, cx: &App) -> FindStatus {
        FindStatus::of(&self.find, cx)
    }

    pub fn set_loading(&mut self, loading: bool, cx: &mut Context<Self>) {
        if self.loading != loading {
            self.loading = loading;
            cx.notify();
        }
    }

    /// Mensaje de error en lugar del contenido; vacío lo quita.
    pub fn set_error(&mut self, error: impl Into<SharedString>, cx: &mut Context<Self>) {
        let error = error.into();
        if self.error != error {
            self.error = error;
            cx.notify();
        }
    }

    /// Oculta la cabecera (ruta, tamaño y cerrar), como `hide-header`.
    pub fn set_header_hidden(&mut self, hidden: bool, cx: &mut Context<Self>) {
        if self.header_hidden != hidden {
            self.header_hidden = hidden;
            cx.notify();
        }
    }

    /// Restaura un desplazamiento guardado (`scrollPosition`). Con el
    /// archivo indexándose, se aplica cuando esté.
    pub fn set_scroll_position(&mut self, position: ScrollPosition, cx: &mut Context<Self>) {
        match &mut self.pending {
            Some(pending) => pending.scroll = position,
            None => self.canvas.scroll.set_position(position.to_point()),
        }
        cx.notify();
    }

    pub fn scroll_position(&self) -> ScrollPosition {
        match &self.pending {
            Some(pending) => pending.scroll,
            None => ScrollPosition::from_point(self.canvas.scroll.position()),
        }
    }

    pub fn path(&self) -> &SharedString {
        &self.path
    }

    pub fn file(&self) -> Option<&FileContent> {
        self.file.as_ref()
    }

    pub fn is_loading(&self) -> bool {
        self.loading
    }

    pub fn error(&self) -> &SharedString {
        &self.error
    }

    pub fn is_header_hidden(&self) -> bool {
        self.header_hidden
    }

    pub fn language(&self) -> Option<Language> {
        self.language
    }

    /// Líneas del archivo (cero si es binario, enorme o está vacío).
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// El texto de la línea `index` (desde cero).
    pub fn line(&self, index: usize) -> Option<&str> {
        let content = &self.file.as_ref()?.content;
        content.get(self.lines.get(index)?.clone())
    }

    /// Las filas que se dibujaron en el último frame.
    pub fn painted_rows(&self) -> Range<usize> {
        self.canvas.painted_rows()
    }

    /// El texto seleccionado, tal como se copiaría.
    pub fn selected_text(&self) -> Option<String> {
        canvas::selected_text(self)
    }

    /// Selecciona todo el contenido.
    pub fn select_all(&mut self, cx: &mut Context<Self>) {
        self.canvas.select(canvas::Selection {
            column: 0,
            anchor: canvas::TextPosition { row: 0, offset: 0 },
            head: canvas::TextPosition {
                row: self.lines.len().saturating_sub(1),
                offset: usize::MAX,
            },
        });
        cx.notify();
    }

    /// Lo que se dice en lugar del contenido, si no hay contenido que mostrar.
    pub fn status_text(&self) -> Option<SharedString> {
        if self.loading || self.pending.is_some() {
            return None;
        }
        if !self.error.is_empty() {
            return Some(self.error.clone());
        }
        match &self.file {
            Some(file) if file.binary => {
                Some(format!("Binary file, {}", format_size(file.size as f64)).into())
            }
            Some(file) if file.too_large => {
                Some(format!("Too large to show here, {}", format_size(file.size as f64)).into())
            }
            _ if self.lines.is_empty() => Some("Empty file".into()),
            _ => None,
        }
    }

    fn line_text(&mut self, ix: usize) -> Option<Rc<LineText>> {
        if let Some(cached) = self.cache.get(ix)?.clone() {
            return Some(cached);
        }
        let source = self.line(ix)?;
        let tokens = highlight_tokens(source, self.language);
        let text = match expand_tabs(source) {
            None => LineText {
                display: SharedString::from(source.to_string()),
                tokens,
            },
            Some(display) => LineText {
                tokens: tokens
                    .into_iter()
                    .map(|(range, kind)| {
                        (
                            display_offset(source, range.start)..display_offset(source, range.end),
                            kind,
                        )
                    })
                    .collect(),
                display: SharedString::from(display),
            },
        };
        let text = Rc::new(text);
        self.cache[ix] = Some(text.clone());
        Some(text)
    }

    fn state_shell(cx: &App) -> Div {
        div()
            .flex()
            .size_full()
            .items_center()
            .justify_center()
            .p(rems(1.5))
            .text_center()
            .text_size(rems(0.8125))
            .text_color(cx.palette().muted_foreground)
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let palette = *cx.palette();
        let size = self
            .file
            .as_ref()
            .filter(|file| file.size > 0)
            .map(|file| format_size(file.size as f64));

        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(rems(0.5))
            .pt(rems(0.3))
            .pb(rems(0.3))
            .pr(rems(0.4))
            .pl(rems(0.65))
            .border_b_1()
            .border_color(palette.border)
            .child(
                // `direction: rtl`: si la ruta no cabe, se recorta por el
                // principio y queda a la vista el nombre del archivo.
                div()
                    .id("file-path")
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis_start()
                    .text_size(rems(0.75))
                    .text_color(palette.foreground)
                    .child(self.path.clone()),
            )
            .when_some(size, |this, size| {
                this.child(
                    div()
                        .flex_shrink_0()
                        .font_family(code_font_family(cx))
                        .text_size(rems(0.6875))
                        .text_color(palette.muted_foreground)
                        .child(size),
                )
            })
            .child(
                mergeon_ui::button("close-file")
                    .ghost()
                    .sm()
                    .child(mergeon_ui::icon(Lucide::X).size(px(14.)))
                    .w(rems(1.75))
                    .px(px(0.))
                    .justify_center()
                    .tooltip("Close file")
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(FileContentViewerEvent::Close))),
            )
    }
}

impl CanvasDelegate for FileContentViewer {
    fn canvas(&self) -> &CanvasState {
        &self.canvas
    }

    fn canvas_mut(&mut self) -> &mut CanvasState {
        &mut self.canvas
    }

    fn layout(&mut self, metrics: &Metrics, _: &mut Window, cx: &mut Context<Self>) -> GridLayout {
        self.colors = Some(CodeColors::new(cx));
        let key = LayoutKey {
            metrics: metrics.clone(),
            measured_width: self.canvas.measured_width(),
        };
        if let Some((cached_key, layout)) = &self.layout
            && *cached_key == key
        {
            return layout.clone();
        }

        let gutter = metrics.rems(3.2);
        // Medianil + línea más larga del archivo + padding de `.code-text`.
        let content = (gutter + metrics.ch * self.max_columns as f32 + metrics.rems(1.))
            .max((self.canvas.measured_width() + metrics.rems(0.5)).ceil());
        let width = content.max(metrics.viewport.width);
        let layout = GridLayout {
            rows: RowHeights::Uniform {
                count: self.lines.len(),
                height: metrics.line_height,
            },
            scroll_width: width,
            row_width: width,
            sticky_width: gutter,
        };
        self.layout = Some((key, layout.clone()));
        layout
    }

    fn row(
        &mut self,
        ix: usize,
        hovered: bool,
        layout: &GridLayout,
        metrics: &Metrics,
        cx: &mut Context<Self>,
    ) -> RowSpec {
        let colors = self.colors.unwrap_or_else(|| CodeColors::new(cx));
        let Some(text) = self.line_text(ix) else {
            return RowSpec::default();
        };
        let gutter = metrics.rems(3.2);
        let zero = Pixels::ZERO;
        let mut spec = RowSpec::default();

        if hovered {
            spec.fills
                .push(Fill::new(zero..layout.row_width, colors.hover));
        }
        // Igual que en el diff: se queda a la vista al desplazar en
        // horizontal, y por eso su fondo tiene que ser opaco.
        spec.fills
            .push(Fill::new(zero..gutter, colors.gutter).sticky());
        spec.fills
            .push(Fill::new(gutter - px(1.)..gutter, colors.border).sticky());
        spec.labels.push(Label {
            text: (ix + 1).to_string().into(),
            x: zero..gutter - px(1.) - metrics.rems(0.5),
            top: zero,
            line_height: metrics.line_height,
            align: LabelAlign::Right,
            color: colors.gutter_text,
            font_size: metrics.font_size,
            sticky: true,
            truncate: false,
        });

        let len = text.display.len();
        spec.cells.push(Cell {
            column: 0,
            x: gutter + metrics.rems(0.5),
            top: zero,
            wrap_width: None,
            text: text.display.clone(),
            runs: syntax_runs(
                len,
                &text.tokens,
                &metrics.font,
                colors.foreground,
                &colors.syntax,
            ),
            marks: match self.line(ix) {
                Some(source) => self.find.marks(ix, 0, source, find::match_colors(cx)),
                None => Vec::new(),
            },
        });
        spec
    }

    fn copy_text(&self, row: usize, _: usize, display: Range<usize>) -> Option<String> {
        let line = self.line(row)?;
        line.get(source_range(line, display)).map(str::to_string)
    }

    fn scrolled(&mut self, cx: &mut Context<Self>) {
        cx.emit(FileContentViewerEvent::Scrolled(self.scroll_position()));
    }

    /// `Esc` cierra la búsqueda.
    fn key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let keystroke = &event.keystroke;
        if keystroke.key == "escape" && !keystroke.modifiers.modified() && self.find.is_open() {
            find::close(self, window, cx);
            return true;
        }
        false
    }
}

impl Searchable for FileContentViewer {
    fn find(&self) -> &FindState {
        &self.find
    }

    fn find_mut(&mut self) -> &mut FindState {
        &mut self.find
    }

    fn find_source(&mut self) -> Option<FindSource> {
        let file = self
            .file
            .as_ref()
            .filter(|file| !file.binary && !file.too_large)?;
        Some(FindSource::Text {
            content: file.content.clone(),
            lines: self.lines.clone(),
        })
    }

    fn document_generation(&self) -> u64 {
        self.document_generation
    }

    fn source_text(&self, row: usize, _: usize) -> Option<&str> {
        self.line(row)
    }
}

impl Render for FileContentViewer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let status = self.status_text();
        let is_error = !self.loading && !self.error.is_empty();
        let row_count = self.lines.len();

        // Un archivo grande indexándose: nada (sin parpadeo) y, si tarda, lo
        // mismo que mientras se carga.
        let indexing = self.pending.as_ref().map(|pending| pending.slow);
        let body = if indexing == Some(false) {
            Self::state_shell(cx)
                .id("file-indexing")
                .test_support()
                .into_any_element()
        } else if self.loading || indexing == Some(true) {
            Self::state_shell(cx)
                .id("file-loading")
                .child(
                    mergeon_ui::spinner()
                        .size(SpinnerSize::Sm)
                        .color(SpinnerColor::Primary),
                )
                .into_any_element()
        } else if let Some(status) = status {
            Self::state_shell(cx)
                .id("file-status")
                .when(is_error, |this| this.text_color(palette.destructive))
                .child(status)
                .into_any_element()
        } else {
            canvas::surface("file-rows", row_count, &self.canvas, cx).into_any_element()
        };

        div()
            .flex()
            .flex_col()
            .size_full()
            .min_w_0()
            .min_h_0()
            .bg(palette.card)
            .key_context(find::CONTEXT)
            .on_action(cx.listener(|this, _: &FindInViewer, window, cx| {
                this.open_find(window, cx);
            }))
            .when(!self.header_hidden, |this| {
                this.child(self.render_header(cx))
            })
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .child(body)
                    .children(find::render_bar(self, cx)),
            )
    }
}
