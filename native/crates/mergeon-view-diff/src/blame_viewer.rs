//! Blame de un archivo (`src/components/repository/BlameViewer.vue`): quién
//! tocó cada línea por última vez, en qué commit y cuándo.
//!
//! El original pedía el blame él mismo (`useGit().blame`). Aquí el visor solo
//! pinta: quien lo aloja pide el blame y le pasa el resultado con
//! [`BlameViewer::set_state`].
//!
//! Navegable: el hash y el autor de cada bloque (líneas seguidas del mismo
//! commit) son un blanco con tooltip (asunto, autor y fecha). El clic avisa
//! [`BlameViewerEvent::OpenCommit`]; el menú contextual ofrece además
//! "Blame previous revision" ([`BlameViewerEvent::BlamePrevious`]) cuando git
//! dijo de qué revisión venía la línea. Qué hacer con eso lo decide quien
//! aloja el visor.

use std::ops::Range;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{
    AnyElement, App, ClipboardItem, Context, Div, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, WeakEntity, Window, div, point, px, rems,
};
use mergeon_model::format::{DateInput, format_date_time, format_short_date, format_size};
use mergeon_ui::menu::{MenuItem, MenuTriggerExt as _};
use mergeon_ui::theme::with_alpha;
use mergeon_ui::{ActivePaletteExt as _, Lucide, SpinnerColor};

use crate::canvas::{
    self, CanvasDelegate, CanvasState, Cell, Fill, FillEdge, GridLayout, Label, LabelAlign,
    Metrics, RowHeights, RowSpec,
};
use crate::text::{display_columns, expand_tabs, source_range};
use crate::typography::{CodeColors, code_font_family, plain_runs};

/// Una línea del blame.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct BlameLine {
    pub short_hash: SharedString,
    pub author: SharedString,
    /// Fecha de autor en milisegundos desde época; `None` si git no la dio.
    pub date: Option<i64>,
    /// Asunto del commit.
    pub summary: SharedString,
    /// Texto de la línea.
    pub content: SharedString,
    /// Hash completo del commit (para navegar a él); vacío si no se sabe.
    pub hash: SharedString,
    /// De qué revisión venía la línea: el padre del commit y la ruta que
    /// tenía el archivo en él (`previous` de `git blame --porcelain`).
    pub previous: Option<(SharedString, SharedString)>,
}

impl BlameLine {
    pub fn new(
        short_hash: impl Into<SharedString>,
        author: impl Into<SharedString>,
        date: Option<i64>,
        summary: impl Into<SharedString>,
        content: impl Into<SharedString>,
    ) -> Self {
        Self {
            short_hash: short_hash.into(),
            author: author.into(),
            date,
            summary: summary.into(),
            content: content.into(),
            hash: SharedString::default(),
            previous: None,
        }
    }

    /// El hash completo del commit de la línea.
    pub fn commit(mut self, hash: impl Into<SharedString>) -> Self {
        self.hash = hash.into();
        self
    }

    /// La revisión anterior de la línea: el padre del commit y la ruta.
    pub fn previous(
        mut self,
        hash: impl Into<SharedString>,
        path: impl Into<SharedString>,
    ) -> Self {
        self.previous = Some((hash.into(), path.into()));
        self
    }

    /// Una línea que todavía no está en ningún commit (`0000000…`).
    pub fn is_uncommitted(&self) -> bool {
        let hash = if self.hash.is_empty() {
            &self.short_hash
        } else {
            &self.hash
        };
        hash.is_empty() || hash.bytes().all(|byte| byte == b'0')
    }
}

/// Lo que el visor pide a quien lo aloja.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlameViewerEvent {
    /// Clic en el hash o el autor de un bloque: ir a ese commit.
    OpenCommit { hash: SharedString },
    /// "Blame previous revision": el blame de `path` en `hash` (el padre del
    /// commit de la línea).
    BlamePrevious {
        hash: SharedString,
        path: SharedString,
    },
}

/// Lo que muestra el visor.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum BlameState {
    /// Pidiendo el blame ("Loading blame...").
    #[default]
    Loading,
    /// La petición falló ("Error: …").
    Error(SharedString),
    /// El archivo es demasiado grande; lleva sus bytes.
    TooLarge(u64),
    Lines(Vec<BlameLine>),
}

struct BlameRow {
    line: BlameLine,
    date: SharedString,
    display: SharedString,
}

#[derive(Debug, Clone, PartialEq)]
struct LayoutKey {
    metrics: Metrics,
    measured_width: Pixels,
}

/// Ancho de la columna de metadatos (`.line-meta`).
const META_WIDTH: f32 = 220.;
/// Ancho de la columna del hash (`.meta-hash`).
const HASH_WIDTH: f32 = 50.;
/// Ancho de la columna del número de línea (`.line-number`).
const NUMBER_WIDTH: f32 = 40.;
/// `line-height` que hereda el visor: el del cuerpo del diálogo (`text-sm`,
/// 20/14).
const LINE_HEIGHT: f32 = 20. / 14.;

/// Visor de blame. Ocupa todo el espacio de su contenedor.
pub struct BlameViewer {
    canvas: CanvasState,
    state: BlameState,
    rows: Vec<BlameRow>,
    max_columns: usize,
    layout: Option<(LayoutKey, GridLayout)>,
    colors: Option<CodeColors>,
    /// Alto de fila del último layout: lo usan los blancos de cada bloque.
    row_height: Pixels,
}

impl EventEmitter<BlameViewerEvent> for BlameViewer {}

impl Focusable for BlameViewer {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.canvas.focus.clone()
    }
}

impl BlameViewer {
    /// Empieza en [`BlameState::Loading`].
    pub fn new(_: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            canvas: CanvasState::new(cx),
            state: BlameState::Loading,
            rows: Vec::new(),
            max_columns: 0,
            layout: None,
            colors: None,
            row_height: Pixels::ZERO,
        }
    }

    pub fn set_state(&mut self, state: BlameState, cx: &mut Context<Self>) {
        if self.state == state {
            return;
        }
        self.rows.clear();
        self.max_columns = 0;
        if let BlameState::Lines(lines) = &state {
            for line in lines {
                let mut line = line.clone();
                if let Some(shown) = line.content.strip_suffix('\r') {
                    line.content = SharedString::from(shown.to_string());
                }
                self.max_columns = self.max_columns.max(display_columns(&line.content));
                self.rows.push(BlameRow {
                    date: line
                        .date
                        .map(|date| format_short_date(DateInput::EpochMs(date)))
                        .unwrap_or_default()
                        .into(),
                    display: expand_tabs(&line.content)
                        .map_or_else(|| line.content.clone(), SharedString::from),
                    line,
                });
            }
        }
        self.state = state;
        self.layout = None;
        self.canvas.reset_content();
        self.canvas.scroll.set_position(point(px(0.), px(0.)));
        cx.notify();
    }

    pub fn state(&self) -> &BlameState {
        &self.state
    }

    pub fn line_count(&self) -> usize {
        self.rows.len()
    }

    /// Los bloques de líneas seguidas del mismo commit que caen en `rows`:
    /// `(primera fila, filas)`. Las líneas sin commit no forman bloque.
    pub fn blocks_in(&self, rows: Range<usize>) -> Vec<(usize, usize)> {
        let end = rows.end.min(self.rows.len());
        let mut blocks: Vec<(usize, usize)> = Vec::new();
        let mut ix = rows.start;
        while ix < end {
            let line = &self.rows[ix].line;
            let mut next = ix + 1;
            while next < end && self.rows[next].line.short_hash == line.short_hash {
                next += 1;
            }
            if !line.is_uncommitted() {
                blocks.push((ix, next - ix));
            }
            ix = next;
        }
        blocks
    }

    /// La línea `ix`, si existe.
    pub fn line(&self, ix: usize) -> Option<&BlameLine> {
        self.rows.get(ix).map(|row| &row.line)
    }

    /// El clic en un bloque: avisa que se quiere ir a su commit.
    pub fn open_commit_at(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(line) = self.line(ix).filter(|line| !line.is_uncommitted()) else {
            return;
        };
        let hash = if line.hash.is_empty() {
            line.short_hash.clone()
        } else {
            line.hash.clone()
        };
        cx.emit(BlameViewerEvent::OpenCommit { hash });
    }

    /// "Blame previous revision" sobre la línea `ix`.
    pub fn blame_previous_at(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some((hash, path)) = self.line(ix).and_then(|line| line.previous.clone()) else {
            return;
        };
        cx.emit(BlameViewerEvent::BlamePrevious { hash, path });
    }

    /// Blancos transparentes sobre la columna de metadatos, uno por bloque
    /// visible: tooltip, clic y menú. Solo los de las filas a la vista.
    fn block_targets(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let row_height = self.row_height;
        if row_height <= Pixels::ZERO || self.rows.is_empty() {
            return Vec::new();
        }
        let scroll = self.canvas.scroll.position();
        let viewport = self.canvas.scroll.viewport().size.height;
        let first = (scroll.y / row_height).floor().max(0.) as usize;
        let last = ((scroll.y + viewport) / row_height).ceil().max(0.) as usize + 1;
        let palette = *cx.palette();
        let lit = with_alpha(palette.foreground, 0.05);
        let view: WeakEntity<Self> = cx.weak_entity();

        self.blocks_in(first..last)
            .into_iter()
            .map(|(ix, count)| {
                let line = &self.rows[ix].line;
                let date = line
                    .date
                    .map(|date| format_date_time(DateInput::EpochMs(date)))
                    .unwrap_or_default();
                let tooltip = format!(
                    "{}\n{} · {}\n{} — click to show the commit",
                    line.summary, line.author, date, line.short_hash
                );
                let (click, menu) = (view.clone(), view.clone());
                let has_previous = line.previous.is_some();
                let hash = if line.hash.is_empty() {
                    line.short_hash.to_string()
                } else {
                    line.hash.to_string()
                };
                div()
                    .absolute()
                    .top(row_height * ix as f32 - scroll.y)
                    .left(-scroll.x)
                    .w(px(META_WIDTH - 1.))
                    .h(row_height * count as f32)
                    .child(
                        div()
                            .id(SharedString::from(format!("blame-commit:{ix}")))
                            .test_support()
                            .aria_label(line.short_hash.clone())
                            .size_full()
                            .cursor_pointer()
                            .hover(move |style| style.bg(lit))
                            .tooltip(mergeon_ui::tooltip(tooltip))
                            .on_click(move |_, _, cx| {
                                let _ =
                                    click.update(cx, |viewer, cx| viewer.open_commit_at(ix, cx));
                            })
                            .context_menu_items(move |_, _| {
                                block_menu(&menu, ix, &hash, has_previous)
                            }),
                    )
                    .into_any_element()
            })
            .collect()
    }

    /// Lo que se dice en lugar de las líneas, si no hay líneas que mostrar.
    pub fn status_text(&self) -> Option<SharedString> {
        match &self.state {
            BlameState::Loading => Some("Loading blame...".into()),
            BlameState::Error(error) => Some(format!("Error: {error}").into()),
            BlameState::TooLarge(size) => {
                Some(format!("Too large to blame here, {}", format_size(*size as f64)).into())
            }
            BlameState::Lines(_) => None,
        }
    }

    /// Las filas que se dibujaron en el último frame.
    pub fn painted_rows(&self) -> Range<usize> {
        self.canvas.painted_rows()
    }

    /// El texto seleccionado, tal como se copiaría.
    pub fn selected_text(&self) -> Option<String> {
        canvas::selected_text(self)
    }

    fn row_height(metrics: &Metrics) -> Pixels {
        // Línea + `padding: 2px` arriba y abajo + `border-bottom`.
        metrics.font_size * LINE_HEIGHT + px(5.)
    }

    fn state_shell(cx: &App) -> Div {
        div()
            .flex()
            .flex_col()
            .items_center()
            .p(rems(2.))
            .text_center()
            .text_color(cx.palette().muted_foreground)
    }
}

/// El menú de un bloque del blame.
fn block_menu(
    view: &WeakEntity<BlameViewer>,
    ix: usize,
    hash: &str,
    has_previous: bool,
) -> Vec<MenuItem> {
    let (show, previous) = (view.clone(), view.clone());
    let hash = hash.to_string();
    vec![
        MenuItem::new("Show commit in history")
            .icon(Lucide::GitCommitHorizontal)
            .on_click(move |_, cx| {
                let _ = show.update(cx, |viewer, cx| viewer.open_commit_at(ix, cx));
            }),
        MenuItem::new("Blame previous revision")
            .icon(Lucide::RotateCcwClock)
            // La primera versión de la línea no tiene revisión anterior.
            .disabled(!has_previous)
            .on_click(move |_, cx| {
                let _ = previous.update(cx, |viewer, cx| viewer.blame_previous_at(ix, cx));
            }),
        MenuItem::separator(),
        MenuItem::new("Copy commit hash")
            .icon(Lucide::Copy)
            .on_click(move |_, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(hash.clone()));
            }),
    ]
}

impl CanvasDelegate for BlameViewer {
    fn canvas(&self) -> &CanvasState {
        &self.canvas
    }

    fn canvas_mut(&mut self) -> &mut CanvasState {
        &mut self.canvas
    }

    fn layout(&mut self, metrics: &Metrics, _: &mut Window, cx: &mut Context<Self>) -> GridLayout {
        self.colors = Some(CodeColors::new(cx));
        // Los blancos de los bloques se colocan con este alto: si cambia (el
        // primer frame, otro tamaño de letra), otro render.
        let row_height = Self::row_height(metrics);
        if row_height != self.row_height {
            self.row_height = row_height;
            cx.notify();
        }
        let key = LayoutKey {
            metrics: metrics.clone(),
            measured_width: self.canvas.measured_width(),
        };
        if let Some((cached_key, layout)) = &self.layout
            && *cached_key == key
        {
            return layout.clone();
        }

        let text_left = px(META_WIDTH + NUMBER_WIDTH + 8.);
        let content = (text_left + metrics.ch * self.max_columns as f32 + px(8.))
            .max((self.canvas.measured_width() + px(8.)).ceil());
        let layout = GridLayout {
            rows: RowHeights::Uniform {
                count: self.rows.len(),
                height: Self::row_height(metrics),
            },
            scroll_width: content.max(metrics.viewport.width),
            // Las filas miden lo que el visor: el código largo las desborda.
            row_width: metrics.viewport.width,
            sticky_width: Pixels::ZERO,
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
        let Some(row) = self.rows.get(ix) else {
            return RowSpec::default();
        };
        let zero = Pixels::ZERO;
        let one = px(1.);
        let meta = px(META_WIDTH);
        let number_right = meta + px(NUMBER_WIDTH);
        let soft_border = with_alpha(colors.border, 0.45);
        let dim = with_alpha(colors.muted_foreground, 0.7);
        let line_height = metrics.font_size * LINE_HEIGHT;
        let small = metrics.rems(0.6875);
        let top = px(2.);

        let mut spec = RowSpec::default();
        if hovered {
            spec.fills
                .push(Fill::new(zero..layout.row_width, colors.secondary));
        }
        spec.fills.push(Fill::new(zero..meta, colors.secondary));
        spec.fills.push(Fill::new(meta - one..meta, colors.border));
        spec.fills.push(Fill::new(meta..number_right, colors.card));
        spec.fills
            .push(Fill::new(number_right - one..number_right, soft_border));
        spec.fills
            .push(Fill::new(zero..layout.row_width, soft_border).edge(FillEdge::Bottom(one)));

        let label =
            |text: SharedString, x: Range<Pixels>, align, color, font_size, truncate| Label {
                text,
                x,
                top,
                line_height,
                align,
                color,
                font_size,
                sticky: false,
                truncate,
            };
        // Hash, autor y fecha, separados 8px, dentro del relleno de 8px.
        let date_width = metrics.ch * row.date.chars().count() as f32 * (0.6875 / 0.75);
        let meta_right = meta - one - px(8.);
        let author_left = px(8. + HASH_WIDTH + 8.);
        let author_right = (meta_right - date_width - px(8.)).max(author_left);
        spec.labels.push(label(
            row.line.short_hash.clone(),
            px(8.)..px(8. + HASH_WIDTH),
            LabelAlign::Left,
            colors.primary,
            metrics.font_size,
            false,
        ));
        spec.labels.push(label(
            row.line.author.clone(),
            author_left..author_right,
            LabelAlign::Left,
            colors.secondary_foreground,
            metrics.font_size,
            true,
        ));
        spec.labels.push(label(
            row.date.clone(),
            author_right..meta_right,
            LabelAlign::Right,
            dim,
            small,
            false,
        ));
        spec.labels.push(label(
            (ix + 1).to_string().into(),
            meta..number_right - one - px(8.),
            LabelAlign::Right,
            dim,
            metrics.font_size,
            false,
        ));

        spec.cells.push(Cell {
            column: 0,
            x: number_right + px(8.),
            top,
            wrap_width: None,
            text: row.display.clone(),
            runs: plain_runs(row.display.len(), &metrics.font, colors.foreground),
            marks: Vec::new(),
        });
        spec
    }

    /// Los blancos de los bloques siguen al desplazamiento.
    fn scrolled(&mut self, cx: &mut Context<Self>) {
        if !self.rows.is_empty() {
            cx.notify();
        }
    }

    fn copy_text(&self, row: usize, _: usize, display: Range<usize>) -> Option<String> {
        let content: &str = &self.rows.get(row)?.line.content;
        content
            .get(source_range(content, display))
            .map(str::to_string)
    }
}

impl Render for BlameViewer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let row_count = self.rows.len();

        let body = match (&self.state, self.status_text()) {
            (BlameState::Loading, Some(status)) => Self::state_shell(cx)
                .id("blame-loading")
                .child(mergeon_ui::spinner().color(SpinnerColor::Primary))
                .child(status)
                .into_any_element(),
            (_, Some(status)) => Self::state_shell(cx)
                .id("blame-status")
                .child(status)
                .into_any_element(),
            (_, None) => div()
                .relative()
                .size_full()
                .child(canvas::surface("blame-rows", row_count, &self.canvas, cx))
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .overflow_hidden()
                        // Los menús de los bloques, con la letra de la
                        // interfaz y no la del código.
                        .font_family(cx.theme().font_family.clone())
                        .children(self.block_targets(cx)),
                )
                .into_any_element(),
        };

        div()
            .size_full()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .bg(palette.card)
            .font_family(code_font_family(cx))
            .text_size(rems(0.75))
            .child(body)
    }
}
