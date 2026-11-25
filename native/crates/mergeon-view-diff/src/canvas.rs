//! Lienzo de código: la superficie que comparten el diff, el contenido de un
//! archivo y el blame.
//!
//! Es un elemento propio y no una lista de `div`s porque los tres visores piden
//! lo mismo y GPUI no lo da hecho: filas virtualizadas, un medianil que se queda
//! a la vista al desplazar en horizontal (`position: sticky`), texto que se
//! selecciona y se copia a través de filas que no están montadas, y un diff de
//! decenas de miles de líneas sin construir un elemento por línea.
//!
//! El visor (un [`CanvasDelegate`]) solo describe: cuánto mide cada fila
//! ([`GridLayout`]) y qué lleva la fila `n` ([`RowSpec`]: fondos, etiquetas del
//! medianil y celdas de texto). El lienzo decide qué filas caen en la ventana,
//! las pinta y se ocupa del scroll, del cursor y de la selección.
//!
//! Solo se piden y se dan forma las filas visibles; el resto del documento no
//! cuesta nada por frame.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hash, Hasher};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::scroll::{Scrollbar, ScrollbarAxis, ScrollbarHandle, ScrollbarMode};
use gpui_kit::{
    AnyElement, App, AvailableSpace, BackgroundExecutor, Bounds, ClipboardItem, ContentMask,
    Context, Corners, CursorStyle, DispatchPhase, Div, Element, ElementId, Entity, FocusHandle,
    Font, GlobalElementId, Hitbox, HitboxBehavior, Hsla, InspectorElementId,
    InteractiveElement as _, IntoElement, KeyDownEvent, LayoutId, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels, Point, ScrollWheelEvent, ShapedLine,
    SharedString, Size, Stateful, Style, Styled as _, TextAlign, TextRun, Window, div, fill, point,
    px, quad, relative, size, transparent_black,
};

use crate::typography::code_font;

/// Cuerpo del código respecto del rem (`font-size: 0.75rem`).
const CODE_FONT_REMS: f32 = 0.75;
/// Señal: código en Geist Mono 12/20.
const CODE_LINE_HEIGHT: f32 = 20. / 12.;
/// Radio del resaltado por palabra (`--r-1`, 2px).
const MARK_RADIUS: Pixels = px(2.);
/// Lo que avanza por frame el scroll al arrastrar una selección fuera del visor.
const MAX_AUTOSCROLL_STEP: f32 = 48.;

/// Medidas del frame: la ventana del visor y la tipografía del código.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Metrics {
    pub viewport: Size<Pixels>,
    pub rem: Pixels,
    pub font: Font,
    pub font_size: Pixels,
    pub line_height: Pixels,
    /// Ancho de un carácter (`1ch`): la fuente es monoespaciada.
    pub ch: Pixels,
    pub ascent: Pixels,
    pub descent: Pixels,
}

impl Metrics {
    pub(crate) fn new(viewport: Size<Pixels>, window: &Window, cx: &App) -> Self {
        let rem = window.rem_size();
        let font = code_font(cx);
        let font_size = rem * CODE_FONT_REMS;
        let text_system = window.text_system();
        let font_id = text_system.resolve_font(&font);
        let ch = text_system
            .ch_advance(font_id, font_size)
            .unwrap_or(font_size * 0.6);

        Self {
            viewport,
            rem,
            font_size,
            line_height: font_size * CODE_LINE_HEIGHT,
            ch,
            ascent: text_system.ascent(font_id, font_size),
            descent: text_system.descent(font_id, font_size).abs(),
            font,
        }
    }

    /// `n` rems en píxeles.
    pub(crate) fn rems(&self, rems: f32) -> Pixels {
        self.rem * rems
    }

    /// Alto de la caja de un trozo de texto en línea (lo que cubre su fondo).
    pub(crate) fn content_height(&self) -> Pixels {
        self.ascent + self.descent
    }
}

/// Alto de cada fila y dónde empieza.
#[derive(Debug, Clone)]
pub(crate) enum RowHeights {
    Uniform {
        count: usize,
        height: Pixels,
    },
    /// Borde superior de cada fila, más el inferior de la última (`n + 1`).
    Variable(Rc<[Pixels]>),
}

impl RowHeights {
    pub(crate) fn from_heights(heights: impl IntoIterator<Item = Pixels>) -> Self {
        let mut tops = vec![Pixels::ZERO];
        let mut top = Pixels::ZERO;
        for height in heights {
            top += height;
            tops.push(top);
        }
        Self::Variable(tops.into())
    }

    pub(crate) fn count(&self) -> usize {
        match self {
            Self::Uniform { count, .. } => *count,
            Self::Variable(tops) => tops.len().saturating_sub(1),
        }
    }

    pub(crate) fn top(&self, ix: usize) -> Pixels {
        match self {
            Self::Uniform { height, .. } => *height * ix as f32,
            Self::Variable(tops) => tops.get(ix).copied().unwrap_or_else(|| self.total()),
        }
    }

    pub(crate) fn height(&self, ix: usize) -> Pixels {
        match self {
            Self::Uniform { height, .. } => *height,
            Self::Variable(_) => self.top(ix + 1) - self.top(ix),
        }
    }

    pub(crate) fn total(&self) -> Pixels {
        match self {
            Self::Uniform { count, height } => *height * *count as f32,
            Self::Variable(tops) => tops.last().copied().unwrap_or_default(),
        }
    }

    /// La fila que contiene `y`; fuera del documento, la primera o la última.
    pub(crate) fn row_at(&self, y: Pixels) -> usize {
        let count = self.count();
        if count == 0 || y <= Pixels::ZERO {
            return 0;
        }
        let row = match self {
            Self::Uniform { height, .. } if *height > Pixels::ZERO => (y / *height) as usize,
            Self::Uniform { .. } => 0,
            Self::Variable(tops) => tops.partition_point(|top| *top <= y).saturating_sub(1),
        };
        row.min(count - 1)
    }

    /// Las filas que tocan la franja vertical `range`.
    pub(crate) fn visible(&self, range: Range<Pixels>) -> Range<usize> {
        if self.count() == 0 || range.end <= range.start {
            return 0..0;
        }
        let first = self.row_at(range.start);
        let mut last = self.row_at(range.end);
        if self.top(last) >= range.end && last > first {
            last -= 1;
        }
        first..last + 1
    }
}

/// Geometría del documento para las medidas de un frame.
#[derive(Debug, Clone)]
pub(crate) struct GridLayout {
    pub rows: RowHeights,
    /// Ancho que se puede recorrer en horizontal.
    pub scroll_width: Pixels,
    /// Ancho que ocupa cada fila: hasta dónde llegan sus fondos y su hover.
    pub row_width: Pixels,
    /// Ancho de la franja que no se desplaza en horizontal (el medianil).
    pub sticky_width: Pixels,
}

/// Qué parte del alto de la fila cubre un fondo.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum FillEdge {
    Full,
    /// Línea de este grosor pegada arriba (`border-top`).
    Top(Pixels),
    /// Línea de este grosor pegada abajo (`border-bottom`).
    Bottom(Pixels),
}

/// Un rectángulo de color dentro de una fila. `x` se mide desde el borde
/// izquierdo de la fila, o del visor si es `sticky`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Fill {
    pub x: Range<Pixels>,
    pub edge: FillEdge,
    pub color: Hsla,
    pub sticky: bool,
}

impl Fill {
    pub(crate) fn new(x: Range<Pixels>, color: impl Into<Hsla>) -> Self {
        Self {
            x,
            edge: FillEdge::Full,
            color: color.into(),
            sticky: false,
        }
    }

    pub(crate) fn edge(mut self, edge: FillEdge) -> Self {
        self.edge = edge;
        self
    }

    pub(crate) fn sticky(mut self) -> Self {
        self.sticky = true;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LabelAlign {
    Left,
    Right,
}

/// Texto que no se selecciona: números de línea y metadatos del blame.
#[derive(Debug, Clone)]
pub(crate) struct Label {
    pub text: SharedString,
    /// Caja en la que se alinea, desde el borde de la fila (o del visor).
    pub x: Range<Pixels>,
    /// Borde superior de su línea dentro de la fila.
    pub top: Pixels,
    pub line_height: Pixels,
    pub align: LabelAlign,
    pub color: Hsla,
    pub font_size: Pixels,
    pub sticky: bool,
    /// Recorta con puntos suspensivos lo que no quepa en la caja.
    pub truncate: bool,
}

/// Texto seleccionable de una fila.
#[derive(Debug, Clone)]
pub(crate) struct Cell {
    /// Columna de selección: una selección no salta de una columna a otra (en
    /// la vista partida, del lado viejo al nuevo).
    pub column: usize,
    /// Dónde empieza el texto, desde el borde de la fila.
    pub x: Pixels,
    /// Borde superior del primer renglón dentro de la fila.
    pub top: Pixels,
    /// Ancho al que se parte en renglones; `None` si no se parte.
    pub wrap_width: Option<Pixels>,
    pub text: SharedString,
    /// Colores del texto; tienen que cubrirlo entero.
    pub runs: Vec<TextRun>,
    /// Fondos redondeados tras un trozo del texto (lo que cambió en la línea).
    pub marks: Vec<(Range<usize>, Hsla)>,
}

/// Lo que lleva una fila.
#[derive(Debug, Clone, Default)]
pub(crate) struct RowSpec {
    pub fills: Vec<Fill>,
    pub labels: Vec<Label>,
    pub cells: Vec<Cell>,
}

/// Posición dentro del texto de una columna.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct TextPosition {
    pub row: usize,
    /// Offset en bytes del texto pintado de la celda.
    pub offset: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Selection {
    pub column: usize,
    pub anchor: TextPosition,
    pub head: TextPosition,
}

impl Selection {
    pub(crate) fn start(&self) -> TextPosition {
        self.anchor.min(self.head)
    }

    pub(crate) fn end(&self) -> TextPosition {
        self.anchor.max(self.head)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    /// El trozo `[desde, hasta)` de la fila `row` que cae dentro, para una
    /// celda de `len` bytes.
    fn range_in_row(&self, row: usize, len: usize) -> Option<Range<usize>> {
        let (start, end) = (self.start(), self.end());
        if row < start.row || row > end.row {
            return None;
        }
        let from = if row == start.row {
            start.offset.min(len)
        } else {
            0
        };
        let to = if row == end.row {
            end.offset.min(len)
        } else {
            len
        };
        Some(from..to.max(from))
    }
}

/// Cómo se trae una fila a la vista.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RevealMode {
    /// Si no se ve entera, centrada (una coincidencia de la búsqueda).
    Center,
    /// Arriba, con un par de líneas de contexto encima (un bloque de cambios).
    Top,
    /// Con su borde superior a esta distancia del borde superior del visor,
    /// sin tocar el desplazamiento horizontal (volver a la línea que se leía
    /// en otro diff del mismo archivo).
    At(Pixels),
}

/// Una petición de llevar el visor hasta un trozo de texto. Se resuelve en el
/// siguiente frame, que es cuando se conocen los altos y el ancho.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Reveal {
    pub row: usize,
    /// Celda de la fila (en la vista partida, 0 es el lado viejo).
    pub column: usize,
    /// Bytes del texto pintado de la celda que tienen que verse.
    pub range: Range<usize>,
    pub mode: RevealMode,
}

struct ScrollState {
    /// Como en GPUI: cero o negativo.
    offset: Point<Pixels>,
    content: Size<Pixels>,
    viewport: Bounds<Pixels>,
}

/// Desplazamiento del lienzo. Se comparte con las barras de scroll de gpui-kit.
#[derive(Clone)]
pub(crate) struct CanvasScroll(Rc<RefCell<ScrollState>>);

impl CanvasScroll {
    fn new() -> Self {
        Self(Rc::new(RefCell::new(ScrollState {
            offset: Point::default(),
            content: Size::default(),
            viewport: Bounds::default(),
        })))
    }

    fn max_offset(state: &ScrollState) -> Point<Pixels> {
        point(
            (state.content.width - state.viewport.size.width).max(Pixels::ZERO),
            (state.content.height - state.viewport.size.height).max(Pixels::ZERO),
        )
    }

    fn clamp(state: &ScrollState, offset: Point<Pixels>) -> Point<Pixels> {
        let max = Self::max_offset(state);
        point(
            offset.x.clamp(-max.x, Pixels::ZERO),
            offset.y.clamp(-max.y, Pixels::ZERO),
        )
    }

    /// Fija la geometría del frame y devuelve el desplazamiento ya dentro de
    /// sus límites.
    fn set_geometry(&self, viewport: Bounds<Pixels>, content: Size<Pixels>) -> Point<Pixels> {
        let mut state = self.0.borrow_mut();
        state.viewport = viewport;
        state.content = content;
        state.offset = Self::clamp(&state, state.offset);
        state.offset
    }

    /// Desplaza `delta` (positivo hacia el principio, como la rueda) y dice si
    /// se movió.
    pub(crate) fn scroll_by(&self, delta: Point<Pixels>) -> bool {
        let mut state = self.0.borrow_mut();
        let next = Self::clamp(&state, state.offset + delta);
        let moved = next != state.offset;
        state.offset = next;
        moved
    }

    /// Lo desplazado desde el principio (`scrollLeft`, `scrollTop`).
    pub(crate) fn position(&self) -> Point<Pixels> {
        let offset = self.0.borrow().offset;
        point(-offset.x, -offset.y)
    }

    /// Restaura un desplazamiento. Si el contenido aún no se ha medido, se
    /// ajusta a sus límites en el primer frame.
    pub(crate) fn set_position(&self, position: Point<Pixels>) {
        self.0.borrow_mut().offset =
            point(-position.x.max(Pixels::ZERO), -position.y.max(Pixels::ZERO));
    }

    pub(crate) fn viewport(&self) -> Bounds<Pixels> {
        self.0.borrow().viewport
    }
}

impl ScrollbarHandle for CanvasScroll {
    fn viewport_bounds(&self) -> Bounds<Pixels> {
        self.0.borrow().viewport
    }

    fn offset(&self) -> Point<Pixels> {
        self.0.borrow().offset
    }

    fn set_offset(&self, offset: Point<Pixels>) {
        self.0.borrow_mut().offset = offset;
    }

    fn content_size(&self) -> Size<Pixels> {
        self.0.borrow().content
    }
}

/// Estado del lienzo que vive entre frames; lo guarda el visor.
pub(crate) struct CanvasState {
    pub scroll: CanvasScroll,
    pub focus: FocusHandle,
    selection: Option<Selection>,
    selecting: bool,
    /// Líneas enteras elegidas desde el medianil: fila ancla y fila extremo.
    line_selection: Option<(usize, usize)>,
    selecting_lines: bool,
    /// Filas que se van a construir en este frame (se fija antes de pedirlas).
    visible_rows: Range<usize>,
    hovered_row: Option<usize>,
    /// Lo último que se avisó con [`CanvasDelegate::scrolled`].
    reported_position: Point<Pixels>,
    /// Borde derecho de la fila más ancha pintada hasta ahora: corrige la
    /// estimación por columnas cuando una línea trae caracteres más anchos.
    measured_width: Pixels,
    /// Radio de las esquinas del visor: los fondos que las tocan se redondean.
    corner_radius: Pixels,
    frame: Option<Rc<CanvasFrame>>,
    /// Lo que hay que traer a la vista en el siguiente frame.
    reveal: Option<Reveal>,
    /// Celdas y etiquetas ya formadas, para no volver a dar forma a lo que
    /// ya se vio al desplazarse. Sobrevive a los cambios de documento.
    shapes: ShapeCache,
}

impl CanvasState {
    pub(crate) fn new(cx: &mut App) -> Self {
        Self {
            scroll: CanvasScroll::new(),
            focus: cx.focus_handle(),
            selection: None,
            selecting: false,
            line_selection: None,
            selecting_lines: false,
            visible_rows: 0..0,
            hovered_row: None,
            reported_position: Point::default(),
            measured_width: Pixels::ZERO,
            corner_radius: Pixels::ZERO,
            frame: None,
            reveal: None,
            shapes: ShapeCache::new(cx),
        }
    }

    /// Lleva el visor hasta `reveal` en el siguiente frame.
    pub(crate) fn reveal(&mut self, reveal: Reveal) {
        self.reveal = Some(reveal);
    }

    pub(crate) fn set_corner_radius(&mut self, radius: Pixels) {
        self.corner_radius = radius;
    }

    pub(crate) fn selection(&self) -> Option<Selection> {
        self.selection.filter(|selection| !selection.is_empty())
    }

    pub(crate) fn clear_selection(&mut self) {
        self.selection = None;
        self.selecting = false;
    }

    pub(crate) fn select(&mut self, selection: Selection) {
        self.selection = Some(selection);
        self.selecting = false;
    }

    pub(crate) fn measured_width(&self) -> Pixels {
        self.measured_width
    }

    /// Las líneas elegidas desde el medianil: `(ancla, extremo)`, en filas.
    pub(crate) fn line_selection(&self) -> Option<(usize, usize)> {
        self.line_selection
    }

    pub(crate) fn set_line_selection(&mut self, selection: Option<(usize, usize)>) {
        self.line_selection = selection;
        self.selecting_lines = false;
        if selection.is_some() {
            self.clear_selection();
        }
    }

    /// Las filas que se están construyendo en este frame.
    pub(crate) fn visible_rows(&self) -> Range<usize> {
        self.visible_rows.clone()
    }

    /// El documento cambió: lo medido y lo seleccionado ya no valen.
    pub(crate) fn reset_content(&mut self) {
        self.clear_selection();
        self.line_selection = None;
        self.selecting_lines = false;
        self.hovered_row = None;
        self.measured_width = Pixels::ZERO;
        self.frame = None;
    }

    /// Las filas que se pintaron en el último frame.
    pub(crate) fn painted_rows(&self) -> Range<usize> {
        self.frame
            .as_ref()
            .and_then(|frame| {
                let first = frame.rows.first()?.ix;
                let last = frame.rows.last()?.ix;
                Some(first..last + 1)
            })
            .unwrap_or(0..0)
    }
}

/// El visor que pinta sobre un lienzo.
pub(crate) trait CanvasDelegate: Sized + 'static {
    fn canvas(&self) -> &CanvasState;

    fn canvas_mut(&mut self) -> &mut CanvasState;

    /// Geometría del documento para estas medidas. Se llama en cada frame:
    /// tiene que salir de una caché mientras no cambien ni los datos ni las
    /// medidas.
    fn layout(
        &mut self,
        metrics: &Metrics,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> GridLayout;

    /// Qué lleva la fila `ix`. Solo se pide para las filas visibles.
    fn row(
        &mut self,
        ix: usize,
        hovered: bool,
        layout: &GridLayout,
        metrics: &Metrics,
        cx: &mut Context<Self>,
    ) -> RowSpec;

    /// Control pegado al borde derecho de la fila (la acción de un bloque).
    fn row_action(
        &mut self,
        _ix: usize,
        _hovered: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        None
    }

    /// Texto original de un trozo (en offsets del texto pintado) de la celda
    /// `column` de una fila; `None` si la fila no tiene esa celda.
    fn copy_text(&self, row: usize, column: usize, display: Range<usize>) -> Option<String>;

    /// El desplazamiento cambió.
    fn scrolled(&mut self, _cx: &mut Context<Self>) {}

    /// Si un clic (o un arrastre) en el medianil elige líneas enteras en vez de
    /// empezar una selección de texto.
    fn selects_lines(&self) -> bool {
        false
    }

    /// Una tecla con el foco en el visor, antes que las del lienzo. `true` si
    /// la atendió.
    fn key_down(
        &mut self,
        _event: &KeyDownEvent,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> bool {
        false
    }
}

/// El texto seleccionado, con una línea por fila.
pub(crate) fn selected_text<V: CanvasDelegate>(view: &V) -> Option<String> {
    let selection = view.canvas().selection()?;
    let (start, end) = (selection.start(), selection.end());
    let lines: Vec<String> = (start.row..=end.row)
        .filter_map(|row| {
            let from = if row == start.row { start.offset } else { 0 };
            let to = if row == end.row {
                end.offset
            } else {
                usize::MAX
            };
            view.copy_text(row, selection.column, from..to)
        })
        .collect();
    Some(lines.join("\n"))
}

/// Teclado del lienzo: copiar, seleccionar todo y desplazarse.
pub(crate) fn handle_key_down<V: CanvasDelegate>(
    view: &mut V,
    event: &KeyDownEvent,
    row_count: usize,
    window: &mut Window,
    cx: &mut Context<V>,
) {
    let keystroke = &event.keystroke;
    let secondary = keystroke.modifiers.secondary();
    let line = window.line_height();
    let page = view.canvas().scroll.viewport().size.height;

    let scroll = match keystroke.key.as_str() {
        "c" if secondary => {
            if let Some(text) = selected_text(view) {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
            cx.stop_propagation();
            return;
        }
        "a" if secondary => {
            let column = view
                .canvas()
                .selection
                .map_or(0, |selection| selection.column);
            view.canvas_mut().select(Selection {
                column,
                anchor: TextPosition { row: 0, offset: 0 },
                head: TextPosition {
                    row: row_count.saturating_sub(1),
                    offset: usize::MAX,
                },
            });
            cx.stop_propagation();
            cx.notify();
            return;
        }
        "escape" if view.canvas().selection.is_some() || view.canvas().line_selection.is_some() => {
            view.canvas_mut().clear_selection();
            view.canvas_mut().line_selection = None;
            cx.stop_propagation();
            cx.notify();
            return;
        }
        "up" => point(Pixels::ZERO, line),
        "down" => point(Pixels::ZERO, -line),
        "left" => point(line, Pixels::ZERO),
        "right" => point(-line, Pixels::ZERO),
        "pageup" => point(Pixels::ZERO, page),
        "pagedown" => point(Pixels::ZERO, -page),
        "home" => point(Pixels::ZERO, Pixels::MAX),
        "end" => point(Pixels::ZERO, -Pixels::MAX),
        _ => return,
    };

    if view.canvas().scroll.scroll_by(scroll) {
        cx.stop_propagation();
        cx.notify();
    }
}

/// La superficie del visor: el lienzo, sus barras de scroll y el foco de
/// teclado. `id` identifica al visor dentro de quien lo aloja.
pub(crate) fn surface<V: CanvasDelegate>(
    id: impl Into<ElementId>,
    row_count: usize,
    canvas: &CanvasState,
    cx: &mut Context<V>,
) -> Stateful<Div> {
    let view = cx.entity();
    let (focus, scroll) = (canvas.focus.clone(), canvas.scroll.clone());

    div()
        .id(id)
        .relative()
        .size_full()
        .overflow_hidden()
        .track_focus(&focus)
        .on_key_down(cx.listener(move |view, event: &KeyDownEvent, window, cx| {
            if view.key_down(event, window, cx) {
                cx.stop_propagation();
                return;
            }
            handle_key_down(view, event, row_count, window, cx);
        }))
        .child(CodeCanvas { view })
        .child(
            div().absolute().inset_0().child(
                // Como las barras del original (`::-webkit-scrollbar`): a la
                // vista siempre que el contenido no cabe.
                Scrollbar::new(&scroll)
                    .id("scrollbar")
                    .axis(ScrollbarAxis::Both)
                    .mode(ScrollbarMode::Always)
                    .viewport_from_layout(),
            ),
        )
}

struct Segment {
    range: Range<usize>,
    line: ShapedLine,
}

struct FrameCell {
    column: usize,
    /// Esquina superior izquierda del primer renglón, en la ventana.
    origin: Point<Pixels>,
    len: usize,
    segments: ShapedCell,
    marks: Vec<(Range<usize>, Hsla)>,
}

impl FrameCell {
    /// Rectángulos que cubren `range`, uno por renglón.
    fn quads(&self, range: &Range<usize>, line_height: Pixels) -> Vec<Bounds<Pixels>> {
        let mut quads = Vec::new();
        for (line_ix, segment) in self.segments.iter().enumerate() {
            let from = range.start.max(segment.range.start);
            let to = range.end.min(segment.range.end);
            if from >= to {
                continue;
            }
            let left = segment.line.x_for_index(from - segment.range.start);
            let right = segment.line.x_for_index(to - segment.range.start);
            quads.push(Bounds::new(
                point(
                    self.origin.x + left,
                    self.origin.y + line_height * line_ix as f32,
                ),
                size(right - left, line_height),
            ));
        }
        quads
    }

    fn offset_at(&self, position: Point<Pixels>, line_height: Pixels) -> usize {
        let Some(last) = self.segments.len().checked_sub(1) else {
            return 0;
        };
        let line_ix = if position.y <= self.origin.y || line_height <= Pixels::ZERO {
            0
        } else {
            (((position.y - self.origin.y) / line_height) as usize).min(last)
        };
        let segment = &self.segments[line_ix];
        segment.range.start
            + segment
                .line
                .closest_index_for_x(position.x - self.origin.x)
                .min(segment.range.len())
    }

    /// El texto de la celda, tal como se pintó.
    fn text(&self) -> String {
        self.segments
            .iter()
            .map(|segment| segment.line.text.as_ref())
            .collect()
    }
}

struct FrameLabel {
    line: ShapedLabel,
    origin: Point<Pixels>,
    line_height: Pixels,
    sticky: bool,
}

struct FrameRow {
    ix: usize,
    top: Pixels,
    height: Pixels,
    fills: Vec<Fill>,
    labels: Vec<FrameLabel>,
    cells: Vec<FrameCell>,
}

/// Lo que se pintó en un frame: con esto se resuelve dónde cae el cursor.
pub(crate) struct CanvasFrame {
    bounds: Bounds<Pixels>,
    offset: Point<Pixels>,
    line_height: Pixels,
    ch: Pixels,
    row_width: Pixels,
    /// Ancho del medianil fijo de la izquierda.
    gutter_width: Pixels,
    rows: Vec<FrameRow>,
    action_bounds: Vec<Bounds<Pixels>>,
}

impl CanvasFrame {
    fn row_left(&self) -> Pixels {
        self.bounds.left() + self.offset.x
    }

    /// La fila bajo el cursor, si está sobre alguna.
    fn row_under(&self, position: Point<Pixels>) -> Option<usize> {
        if position.x < self.row_left() || position.x >= self.row_left() + self.row_width {
            return None;
        }
        self.rows
            .iter()
            .find(|row| position.y >= row.top && position.y < row.top + row.height)
            .map(|row| row.ix)
    }

    /// La posición de texto más cercana a un punto. `column` fija la columna
    /// (al arrastrar una selección); sin ella se elige la que esté bajo el punto.
    fn position_at(
        &self,
        position: Point<Pixels>,
        column: Option<usize>,
    ) -> Option<(usize, TextPosition)> {
        let first = self.rows.first()?;
        let last = self.rows.last()?;
        if position.y < first.top {
            let column = column.or_else(|| first.cells.first().map(|cell| cell.column))?;
            return Some((
                column,
                TextPosition {
                    row: first.ix,
                    offset: 0,
                },
            ));
        }
        if position.y >= last.top + last.height {
            let column = column.or_else(|| last.cells.first().map(|cell| cell.column))?;
            return Some((
                column,
                TextPosition {
                    row: last.ix,
                    offset: usize::MAX,
                },
            ));
        }

        let row = self
            .rows
            .iter()
            .find(|row| position.y < row.top + row.height)?;
        let cell = match column {
            Some(column) => row.cells.iter().find(|cell| cell.column == column),
            None => row
                .cells
                .iter()
                .rev()
                .find(|cell| cell.origin.x <= position.x)
                .or(row.cells.first()),
        };
        let column = column.or(cell.map(|cell| cell.column))?;
        let offset = cell.map_or(0, |cell| cell.offset_at(position, self.line_height));
        Some((
            column,
            TextPosition {
                row: row.ix,
                offset,
            },
        ))
    }

    fn cell(&self, row: usize, column: usize) -> Option<&FrameCell> {
        self.rows
            .iter()
            .find(|candidate| candidate.ix == row)?
            .cells
            .iter()
            .find(|cell| cell.column == column)
    }

    /// Si el punto cae sobre el medianil fijo.
    fn over_gutter(&self, position: Point<Pixels>) -> bool {
        self.bounds.contains(&position)
            && position.x < self.bounds.left() + self.gutter_width.min(self.bounds.size.width)
    }

    /// La fila a la altura del punto; por encima o por debajo de lo pintado,
    /// la primera o la última.
    fn row_at_y(&self, y: Pixels) -> Option<usize> {
        let first = self.rows.first()?;
        let last = self.rows.last()?;
        if y < first.top {
            return Some(first.ix);
        }
        Some(
            self.rows
                .iter()
                .find(|row| y < row.top + row.height)
                .unwrap_or(last)
                .ix,
        )
    }

    fn over_action(&self, position: Point<Pixels>) -> bool {
        self.action_bounds
            .iter()
            .any(|bounds| bounds.contains(&position))
    }
}

fn is_word(character: char) -> bool {
    character.is_alphanumeric() || character == '_' || character == '$'
}

/// La palabra (o el signo) que hay en `offset`.
fn word_range(text: &str, offset: usize) -> Range<usize> {
    let offset = offset.min(text.len());
    let Some(character) = text[offset..]
        .chars()
        .next()
        .or_else(|| text[..offset].chars().next_back())
    else {
        return 0..0;
    };
    if !is_word(character) {
        let start = if offset < text.len() {
            offset
        } else {
            offset - character.len_utf8()
        };
        return start..start + character.len_utf8();
    }

    let start = text[..offset]
        .char_indices()
        .rev()
        .take_while(|(_, character)| is_word(*character))
        .last()
        .map_or(offset, |(at, _)| at);
    let end = text[offset..]
        .char_indices()
        .find(|(_, character)| !is_word(*character))
        .map_or(text.len(), |(at, _)| offset + at);
    start..end
}

/// Las esquinas de `bounds` que coinciden con una esquina del visor se
/// redondean con él: GPUI recorta en rectángulo y el fondo asomaría.
fn corner_radii(
    bounds: Bounds<Pixels>,
    viewport: Bounds<Pixels>,
    radius: Pixels,
) -> Corners<Pixels> {
    if radius <= Pixels::ZERO {
        return Corners::default();
    }
    let near = |a: Pixels, b: Pixels| (a - b).abs() < radius;
    let left = near(bounds.left(), viewport.left());
    let right = near(bounds.right(), viewport.right());
    let top = near(bounds.top(), viewport.top());
    let bottom = near(bounds.bottom(), viewport.bottom());
    let radius = radius.min(bounds.size.height).min(bounds.size.width);
    let pick = |on: bool| if on { radius } else { Pixels::ZERO };

    Corners {
        top_left: pick(left && top),
        top_right: pick(right && top),
        bottom_right: pick(right && bottom),
        bottom_left: pick(left && bottom),
    }
}

fn slice_runs(runs: &[TextRun], range: &Range<usize>) -> Vec<TextRun> {
    let mut sliced = Vec::new();
    let mut start = 0;
    for run in runs {
        let end = start + run.len;
        let from = range.start.max(start);
        let to = range.end.min(end);
        if from < to {
            sliced.push(TextRun {
                len: to - from,
                ..run.clone()
            });
        }
        start = end;
    }
    sliced
}

/// Mide dónde se parte una línea en la vista partida. Lo usan el visor (para
/// saber cuánto mide cada fila) y el lienzo (para pintarla): tienen que
/// coincidir.
pub(crate) struct WrapMeasure {
    text_system: std::sync::Arc<gpui_kit::WindowTextSystem>,
    font_id: gpui_kit::FontId,
    font_size: Pixels,
    ch: f32,
    widths: std::collections::HashMap<char, f32>,
}

impl WrapMeasure {
    pub(crate) fn new(metrics: &Metrics, window: &Window) -> Self {
        let text_system = window.text_system().clone();
        Self {
            font_id: text_system.resolve_font(&metrics.font),
            text_system,
            font_size: metrics.font_size,
            ch: f32::from(metrics.ch),
            widths: std::collections::HashMap::new(),
        }
    }

    /// Offsets donde empieza cada renglón a partir del segundo; vacío si la
    /// línea cabe entera.
    pub(crate) fn breaks(&mut self, text: &str, wrap_width: Pixels) -> Vec<usize> {
        let wrap_width = f32::from(wrap_width);
        // La fuente es monoespaciada y ningún carácter mide más `ch` que
        // bytes ocupa: casi todas las líneas se resuelven aquí.
        if text.len() as f32 * self.ch <= wrap_width {
            return Vec::new();
        }

        let Self {
            text_system,
            font_id,
            font_size,
            ch,
            widths,
        } = self;
        crate::text::wrap_offsets(text, wrap_width, |character| {
            if character.is_ascii() {
                *ch
            } else {
                *widths.entry(character).or_insert_with(|| {
                    f32::from(text_system.layout_width(*font_id, *font_size, character))
                })
            }
        })
    }

    /// Renglones que ocupa la línea.
    pub(crate) fn line_count(&mut self, text: &str, wrap_width: Pixels) -> usize {
        self.breaks(text, wrap_width).len() + 1
    }
}

/// Cuántas formas guarda cada generación de [`ShapeCache`] (de celdas y de
/// etiquetas, por separado): unas decenas de pantallas de filas. Con dos
/// generaciones, lo visto hace poco sobrevive.
const SHAPE_CACHE_CAPACITY: usize = 4096;

/// Lo que identifica un texto formado: el texto, sus estilos (fuente y color
/// de cada tramo), el cuerpo y, si se parte en renglones, el ancho. El hash
/// se calcula una vez al crearla (y barato): buscar en la caché no puede
/// costar lo que cuesta dar forma.
#[derive(Clone)]
struct ShapeKey {
    hash: u64,
    text: SharedString,
    runs: Vec<TextRun>,
    font_size: Pixels,
    wrap_width: Option<Pixels>,
}

impl ShapeKey {
    fn new(
        text: SharedString,
        runs: Vec<TextRun>,
        font_size: Pixels,
        wrap_width: Option<Pixels>,
    ) -> Self {
        // Solo una parte de lo que se compara (la familia, de la primera
        // fuente): basta para repartir, y la igualdad mira todo.
        let mut hasher = FxHasher::default();
        hasher.write(text.as_bytes());
        hasher.write_u32(f32::from(font_size).to_bits());
        hasher.write_u32(wrap_width.map_or(u32::MAX, |width| f32::from(width).to_bits()));
        if let Some(run) = runs.first() {
            hasher.write(run.font.family.as_bytes());
        }
        for run in &runs {
            hasher.write_usize(run.len);
            run.color.hash(&mut hasher);
            hasher.write_u32(run.font.weight.0.to_bits());
            run.font.style.hash(&mut hasher);
        }
        Self {
            hash: hasher.finish(),
            text,
            runs,
            font_size,
            wrap_width,
        }
    }
}

impl PartialEq for ShapeKey {
    fn eq(&self, other: &Self) -> bool {
        self.hash == other.hash
            && self.font_size == other.font_size
            && self.wrap_width == other.wrap_width
            && self.text == other.text
            && self.runs == other.runs
    }
}

impl Eq for ShapeKey {}

impl Hash for ShapeKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_u64(self.hash);
    }
}

/// El hash de rustc (`FxHasher`): multiplicar y rotar, ocho bytes por paso.
/// No resiste a quien busque colisiones a propósito, que aquí no hay.
#[derive(Default)]
struct FxHasher {
    hash: u64,
}

impl FxHasher {
    const SEED: u64 = 0x51_7c_c1_b7_27_22_0a_95;

    fn add(&mut self, word: u64) {
        self.hash = (self.hash.rotate_left(5) ^ word).wrapping_mul(Self::SEED);
    }
}

impl Hasher for FxHasher {
    fn write(&mut self, bytes: &[u8]) {
        let (chunks, rest) = bytes.as_chunks::<8>();
        for chunk in chunks {
            self.add(u64::from_le_bytes(*chunk));
        }
        if !rest.is_empty() {
            let mut word = [0; 8];
            word[..rest.len()].copy_from_slice(rest);
            self.add(u64::from_le_bytes(word));
        }
        self.add(bytes.len() as u64);
    }

    fn write_u32(&mut self, value: u32) {
        self.add(u64::from(value));
    }

    fn write_u64(&mut self, value: u64) {
        self.add(value);
    }

    fn write_usize(&mut self, value: usize) {
        self.add(value as u64);
    }

    fn finish(&self) -> u64 {
        // Mezcla final: la tabla usa los bits altos para sus etiquetas.
        let mut hash = self.hash;
        hash ^= hash >> 33;
        hash = hash.wrapping_mul(0xff51_afd7_ed55_8ccd);
        hash ^= hash >> 33;
        hash
    }
}

/// Para las tablas de la caché: la clave ya trae su hash.
#[derive(Default)]
struct KeyHasher(u64);

impl Hasher for KeyHasher {
    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 = self.0.rotate_left(8) ^ u64::from(*byte);
        }
    }

    fn write_u64(&mut self, value: u64) {
        self.0 = value;
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

type ShapeMap<V> = HashMap<ShapeKey, V, BuildHasherDefault<KeyHasher>>;

/// Renglones de una celda ya formados. `Arc` y no `Rc`: lo que sale de la
/// caché se suelta en segundo plano.
type ShapedCell = Arc<[Segment]>;

/// Una etiqueta ya formada. En un `Arc`: `ShapedLine` lleva sus tramos de
/// color en línea (más de un kilobyte) y copiarla en cada frame y en cada
/// tabla de la caché cuesta más que buscarla.
type ShapedLabel = Arc<ShapedLine>;

/// Una caché de dos generaciones: al llenarse la actual pasa a ser la
/// anterior, y lo que no se volvió a usar de la anterior se tira.
struct Generations<V> {
    current: ShapeMap<V>,
    previous: ShapeMap<V>,
}

impl<V> Default for Generations<V> {
    fn default() -> Self {
        Self {
            current: ShapeMap::default(),
            previous: ShapeMap::default(),
        }
    }
}

impl<V: Clone + Send + 'static> Generations<V> {
    fn get_or_insert(
        &mut self,
        key: ShapeKey,
        misses: &mut usize,
        dropper: Option<&BackgroundExecutor>,
        shape: impl FnOnce(&ShapeKey) -> V,
    ) -> V {
        if let Some(value) = self.current.get(&key) {
            return value.clone();
        }
        let value = self.previous.remove(&key).unwrap_or_else(|| {
            *misses += 1;
            shape(&key)
        });
        if self.current.len() >= SHAPE_CACHE_CAPACITY {
            let full = std::mem::replace(
                &mut self.current,
                ShapeMap::with_capacity_and_hasher(SHAPE_CACHE_CAPACITY, Default::default()),
            );
            let discarded = std::mem::replace(&mut self.previous, full);
            // Soltar miles de líneas formadas cuesta milisegundos: fuera del
            // hilo de la interfaz.
            match dropper {
                Some(executor) => executor.spawn(async move { drop(discarded) }).detach(),
                None => drop(discarded),
            }
        }
        self.current.insert(key, value.clone());
        value
    }
}

/// Caché de `shape_line` entre frames para las celdas y etiquetas del
/// lienzo. GPUI solo recuerda lo del frame anterior; aquí se guarda lo visto
/// en las últimas miles de formas, así que volver por un tramo ya recorrido
/// (o un frame sin cambios) no da forma a nada. La clave es el contenido, así
/// que no hace falta vaciarla al cambiar de documento: lo que ya no se pinta
/// sale solo por el tope.
#[derive(Default)]
pub(crate) struct ShapeCache {
    cells: Generations<ShapedCell>,
    labels: Generations<ShapedLabel>,
    /// Donde se sueltan las generaciones descartadas.
    dropper: Option<BackgroundExecutor>,
    /// Formas que hubo que calcular (para los tests).
    misses: usize,
}

impl ShapeCache {
    fn new(cx: &App) -> Self {
        Self {
            dropper: Some(cx.background_executor().clone()),
            ..Self::default()
        }
    }

    fn cell(&mut self, key: ShapeKey, shape: impl FnOnce(&ShapeKey) -> ShapedCell) -> ShapedCell {
        self.cells
            .get_or_insert(key, &mut self.misses, self.dropper.as_ref(), shape)
    }

    fn label(
        &mut self,
        key: ShapeKey,
        shape: impl FnOnce(&ShapeKey) -> ShapedLabel,
    ) -> ShapedLabel {
        self.labels
            .get_or_insert(key, &mut self.misses, self.dropper.as_ref(), shape)
    }
}

impl CanvasState {
    /// Textos a los que se dio forma desde que se creó el visor (los que no
    /// salieron de la caché).
    pub(crate) fn shape_misses(&self) -> usize {
        self.shapes.misses
    }
}

/// Lo que el pintado de una celda necesita saber del frame.
#[derive(Clone, Copy)]
struct CellStyle {
    selection: Option<Selection>,
    selection_color: Hsla,
    /// Alto de la caja de un trozo de texto: lo que cubre el fondo de una marca.
    content_height: Pixels,
}

/// El elemento que pinta el lienzo de un visor.
pub(crate) struct CodeCanvas<V: CanvasDelegate> {
    view: Entity<V>,
}

pub(crate) struct CanvasPrepaint {
    frame: Rc<CanvasFrame>,
    hitbox: Hitbox,
    gutter_hitbox: Option<Hitbox>,
    actions: Vec<AnyElement>,
    selection: Option<Selection>,
    selecting: bool,
    hovered_row: Option<usize>,
    corner_radius: Pixels,
    content_height: Pixels,
}

impl<V: CanvasDelegate> IntoElement for CodeCanvas<V> {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl<V: CanvasDelegate> CodeCanvas<V> {
    fn shape_cell(
        cell: Cell,
        row_left: Pixels,
        row_top: Pixels,
        metrics: &Metrics,
        shapes: &mut ShapeCache,
        window: &mut Window,
    ) -> FrameCell {
        let len = cell.text.len();
        let key = ShapeKey::new(cell.text, cell.runs, metrics.font_size, cell.wrap_width);
        let segments = shapes.cell(key, |key| {
            let text_system = window.text_system().clone();
            let breaks = match key.wrap_width {
                Some(width) => WrapMeasure::new(metrics, window).breaks(&key.text, width),
                None => Vec::new(),
            };
            if breaks.is_empty() {
                return Arc::from([Segment {
                    range: 0..len,
                    line: text_system.shape_line(key.text.clone(), key.font_size, &key.runs, None),
                }]);
            }
            let mut segments = Vec::with_capacity(breaks.len() + 1);
            let mut start = 0;
            for end in breaks.into_iter().chain([len]) {
                let range = start..end;
                let text = SharedString::from(key.text[range.clone()].to_string());
                let runs = slice_runs(&key.runs, &range);
                segments.push(Segment {
                    line: text_system.shape_line(text, key.font_size, &runs, None),
                    range,
                });
                start = end;
            }
            segments.into()
        });

        FrameCell {
            column: cell.column,
            origin: point(row_left + cell.x, row_top + cell.top),
            len,
            segments,
            marks: cell.marks,
        }
    }

    fn shape_label(
        label: Label,
        row_left: Pixels,
        viewport_left: Pixels,
        row_top: Pixels,
        metrics: &Metrics,
        shapes: &mut ShapeCache,
        window: &mut Window,
    ) -> FrameLabel {
        let mut text = label.text;
        let available = label.x.end - label.x.start;
        if label.truncate {
            // La fuente es monoespaciada: caben tantos caracteres como `ch`.
            let advance =
                f32::from(metrics.ch) * f32::from(label.font_size) / f32::from(metrics.font_size);
            let fit = (f32::from(available) / advance).floor() as usize;
            if text.chars().count() > fit {
                let kept: String = text.chars().take(fit.saturating_sub(1)).collect();
                text = SharedString::from(format!("{kept}…"));
            }
        }

        let run = TextRun {
            len: text.len(),
            font: metrics.font.clone(),
            color: label.color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let key = ShapeKey::new(text, vec![run], label.font_size, None);
        let line = shapes.label(key, |key| {
            Arc::new(window.text_system().shape_line(
                key.text.clone(),
                key.font_size,
                &key.runs,
                None,
            ))
        });
        let left = if label.sticky {
            viewport_left
        } else {
            row_left
        };
        let x = match label.align {
            LabelAlign::Left => left + label.x.start,
            LabelAlign::Right => left + label.x.end - line.width(),
        };

        FrameLabel {
            line,
            origin: point(x, row_top + label.top),
            line_height: label.line_height,
            sticky: label.sticky,
        }
    }

    fn paint_fill(
        fill_spec: &Fill,
        row: &FrameRow,
        frame: &CanvasFrame,
        radius: Pixels,
        window: &mut Window,
    ) {
        let left = if fill_spec.sticky {
            frame.bounds.left()
        } else {
            frame.row_left()
        };
        let (top, height) = match fill_spec.edge {
            FillEdge::Full => (row.top, row.height),
            FillEdge::Top(thickness) => (row.top, thickness),
            FillEdge::Bottom(thickness) => (row.top + row.height - thickness, thickness),
        };
        let bounds = Bounds::new(
            point(left + fill_spec.x.start, top),
            size(fill_spec.x.end - fill_spec.x.start, height),
        );
        if bounds.size.width <= Pixels::ZERO || !bounds.intersects(&frame.bounds) {
            return;
        }
        // Las esquinas se miden contra lo que de verdad se ve del fondo.
        let visible = bounds.intersect(&frame.bounds);
        let radii = corner_radii(visible, frame.bounds, radius);
        if radii == Corners::default() {
            window.paint_quad(fill(bounds, fill_spec.color));
        } else {
            window.paint_quad(quad(
                visible,
                radii,
                fill_spec.color,
                px(0.),
                transparent_black(),
                Default::default(),
            ));
        }
    }

    fn paint_cell(
        cell: &FrameCell,
        row: &FrameRow,
        frame: &CanvasFrame,
        style: &CellStyle,
        window: &mut Window,
        cx: &mut App,
    ) {
        let line_height = frame.line_height;
        let CellStyle {
            selection,
            selection_color,
            content_height,
        } = *style;

        for (range, color) in &cell.marks {
            let inset = ((line_height - content_height) / 2.).max(Pixels::ZERO);
            for bounds in cell.quads(range, line_height) {
                let bounds = Bounds::new(
                    point(bounds.left(), bounds.top() + inset),
                    size(bounds.size.width, line_height - inset * 2.),
                );
                let radius = MARK_RADIUS.min(bounds.size.height / 2.);
                window.paint_quad(quad(
                    bounds,
                    radius,
                    *color,
                    px(0.),
                    transparent_black(),
                    Default::default(),
                ));
            }
        }

        if let Some(selection) = selection.filter(|selection| selection.column == cell.column)
            && let Some(range) = selection.range_in_row(row.ix, cell.len)
        {
            let mut quads = cell.quads(&range, line_height);
            // El salto de línea también va seleccionado: sin esto una línea
            // vacía dentro de la selección no se distinguiría.
            if row.ix < selection.end().row && range.end == cell.len {
                let last_line = cell.segments.len().saturating_sub(1);
                let end_x = cell
                    .segments
                    .last()
                    .map_or(Pixels::ZERO, |segment| segment.line.width());
                quads.push(Bounds::new(
                    point(
                        cell.origin.x + end_x,
                        cell.origin.y + line_height * last_line as f32,
                    ),
                    size(frame.ch, line_height),
                ));
            }
            for bounds in quads {
                window.paint_quad(fill(bounds, selection_color));
            }
        }

        for (line_ix, segment) in cell.segments.iter().enumerate() {
            let origin = point(cell.origin.x, cell.origin.y + line_height * line_ix as f32);
            if let Err(error) =
                segment
                    .line
                    .paint(origin, line_height, TextAlign::Left, None, window, cx)
            {
                log::debug!("no se pudo pintar una línea de código: {error}");
            }
        }
    }
}

impl<V: CanvasDelegate> Element for CodeCanvas<V> {
    type RequestLayoutState = ();
    type PrepaintState = CanvasPrepaint;

    fn id(&self) -> Option<ElementId> {
        Some("code-canvas".into())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, None, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let metrics = Metrics::new(bounds.size, window, cx);

        struct Described {
            shapes: ShapeCache,
            layout: GridLayout,
            offset: Point<Pixels>,
            rows: Vec<(usize, RowSpec, Option<AnyElement>)>,
            selection: Option<Selection>,
            selecting: bool,
            hovered_row: Option<usize>,
            corner_radius: Pixels,
        }

        let described = self.view.update(cx, |view, cx| {
            let layout = view.layout(&metrics, window, cx);
            let content = size(layout.scroll_width, layout.rows.total());
            let mut offset = view.canvas().scroll.set_geometry(bounds, content);
            if let Some(reveal) = view.canvas_mut().reveal.take()
                && reveal.row < layout.rows.count()
            {
                let target = reveal_position(view, &reveal, &layout, &metrics, bounds, cx);
                view.canvas().scroll.set_position(target);
                offset = view.canvas().scroll.set_geometry(bounds, content);
            }
            if view.canvas().scroll.position() != view.canvas().reported_position {
                view.canvas_mut().reported_position = view.canvas().scroll.position();
                view.scrolled(cx);
            }

            let hovered_row = view.canvas().hovered_row;
            let visible = layout
                .rows
                .visible(-offset.y..-offset.y + bounds.size.height);
            view.canvas_mut().visible_rows = visible.clone();
            let rows = visible
                .map(|ix| {
                    let hovered = hovered_row == Some(ix);
                    let spec = view.row(ix, hovered, &layout, &metrics, cx);
                    let action = view.row_action(ix, hovered, window, cx);
                    (ix, spec, action)
                })
                .collect();

            let canvas = view.canvas_mut();
            let shapes = std::mem::take(&mut canvas.shapes);
            Described {
                shapes,
                layout,
                offset,
                rows,
                selection: canvas.selection,
                selecting: canvas.selecting || canvas.selecting_lines,
                hovered_row,
                corner_radius: canvas.corner_radius,
            }
        });

        let Described {
            mut shapes,
            layout,
            offset,
            rows,
            selection,
            selecting,
            hovered_row,
            corner_radius,
        } = described;

        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        let gutter_hitbox = (layout.sticky_width > Pixels::ZERO).then(|| {
            window.insert_hitbox(
                Bounds::new(
                    bounds.origin,
                    size(
                        layout.sticky_width.min(bounds.size.width),
                        bounds.size.height,
                    ),
                ),
                HitboxBehavior::Normal,
            )
        });

        let row_left = bounds.left() + offset.x;
        let mut frame_rows = Vec::with_capacity(rows.len());
        let mut actions = Vec::new();
        let mut action_bounds = Vec::new();
        let mut widest = Pixels::ZERO;

        for (ix, spec, action) in rows {
            let top = bounds.top() + offset.y + layout.rows.top(ix);
            let height = layout.rows.height(ix);

            let cells: Vec<FrameCell> = spec
                .cells
                .into_iter()
                .map(|cell| {
                    let wraps = cell.wrap_width.is_some();
                    let shaped =
                        Self::shape_cell(cell, row_left, top, &metrics, &mut shapes, window);
                    if !wraps && let Some(segment) = shaped.segments.first() {
                        widest = widest.max(shaped.origin.x - row_left + segment.line.width());
                    }
                    shaped
                })
                .collect();
            let labels = spec
                .labels
                .into_iter()
                .map(|label| {
                    Self::shape_label(
                        label,
                        row_left,
                        bounds.left(),
                        top,
                        &metrics,
                        &mut shapes,
                        window,
                    )
                })
                .collect();

            if let Some(mut action) = action {
                let action_size = action.layout_as_root(
                    size(AvailableSpace::MinContent, AvailableSpace::MinContent),
                    window,
                    cx,
                );
                // `position: sticky; right: 0.4rem`: al final de la fila, o
                // pegado al borde derecho del visor si la fila sigue más allá.
                let right = (row_left + layout.row_width).min(bounds.right() - metrics.rems(0.4));
                let origin = point(
                    right - action_size.width,
                    top + (height - action_size.height) / 2.,
                );
                window.with_content_mask(Some(ContentMask { bounds }), |window| {
                    action.prepaint_at(origin, window, cx);
                });
                action_bounds.push(Bounds::new(origin, action_size));
                actions.push(action);
            }

            frame_rows.push(FrameRow {
                ix,
                top,
                height,
                fills: spec.fills,
                labels,
                cells,
            });
        }

        let frame = Rc::new(CanvasFrame {
            bounds,
            offset,
            line_height: metrics.line_height,
            ch: metrics.ch,
            row_width: layout.row_width,
            gutter_width: layout.sticky_width,
            rows: frame_rows,
            action_bounds,
        });

        let mut grew = false;
        self.view.update(cx, |view, _| {
            let canvas = view.canvas_mut();
            canvas.shapes = shapes;
            canvas.frame = Some(frame.clone());
            if widest > canvas.measured_width {
                canvas.measured_width = widest;
                grew = true;
            }
        });
        // Dentro de un frame `notify` no pide otro: se pide a la ventana.
        if grew {
            window.request_animation_frame();
        }

        CanvasPrepaint {
            frame,
            hitbox,
            gutter_hitbox,
            actions,
            selection: selection.filter(|selection| !selection.is_empty()),
            selecting,
            hovered_row,
            corner_radius,
            content_height: metrics.content_height(),
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let frame = prepaint.frame.clone();
        let cell_style = CellStyle {
            selection: prepaint.selection,
            selection_color: cx.theme().selection,
            content_height: prepaint.content_height,
        };
        let radius = prepaint.corner_radius;

        window.set_cursor_style(CursorStyle::IBeam, &prepaint.hitbox);
        if let Some(gutter) = &prepaint.gutter_hitbox {
            window.set_cursor_style(CursorStyle::Arrow, gutter);
        }

        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            // Primero lo que se desplaza; encima, lo que se queda fijo.
            for row in &frame.rows {
                for fill_spec in row.fills.iter().filter(|fill_spec| !fill_spec.sticky) {
                    Self::paint_fill(fill_spec, row, &frame, radius, window);
                }
            }
            for row in &frame.rows {
                for cell in &row.cells {
                    Self::paint_cell(cell, row, &frame, &cell_style, window, cx);
                }
                for label in row.labels.iter().filter(|label| !label.sticky) {
                    let _ = label.line.paint(
                        label.origin,
                        label.line_height,
                        TextAlign::Left,
                        None,
                        window,
                        cx,
                    );
                }
            }
            for row in &frame.rows {
                for fill_spec in row.fills.iter().filter(|fill_spec| fill_spec.sticky) {
                    Self::paint_fill(fill_spec, row, &frame, radius, window);
                }
                for label in row.labels.iter().filter(|label| label.sticky) {
                    let _ = label.line.paint(
                        label.origin,
                        label.line_height,
                        TextAlign::Left,
                        None,
                        window,
                        cx,
                    );
                }
            }
            for action in &mut prepaint.actions {
                action.paint(window, cx);
            }
        });

        // El contenido puede haberse movido bajo un cursor quieto (rueda,
        // teclado, barra de scroll): la fila resaltada se recalcula al pintar.
        let hovered_now = prepaint
            .hitbox
            .is_hovered(window)
            .then(|| frame.row_under(window.mouse_position()))
            .flatten();
        if hovered_now != prepaint.hovered_row {
            self.view.update(cx, |view, _| {
                view.canvas_mut().hovered_row = hovered_now;
            });
            window.request_animation_frame();
        }

        // Arrastrar una selección más allá del borde desplaza el visor.
        if prepaint.selecting {
            let mouse = window.mouse_position();
            let step = |distance: Pixels| {
                px(f32::from(distance).clamp(-MAX_AUTOSCROLL_STEP, MAX_AUTOSCROLL_STEP))
            };
            let delta = point(
                if mouse.x < bounds.left() {
                    step(bounds.left() - mouse.x)
                } else if mouse.x > bounds.right() {
                    step(bounds.right() - mouse.x)
                } else {
                    Pixels::ZERO
                },
                if mouse.y < bounds.top() {
                    step(bounds.top() - mouse.y)
                } else if mouse.y > bounds.bottom() {
                    step(bounds.bottom() - mouse.y)
                } else {
                    Pixels::ZERO
                },
            );
            if delta != Point::default() {
                let frame = frame.clone();
                self.view.update(cx, |view, _| {
                    if view.canvas().scroll.scroll_by(delta) {
                        if view.canvas().selecting_lines {
                            drag_line_selection(view, &frame, mouse);
                        } else {
                            drag_selection(view, &frame, mouse);
                        }
                    }
                });
                window.request_animation_frame();
            }
        }

        let view = self.view.clone();
        let hitbox = prepaint.hitbox.clone();
        let line_height = window.line_height();

        window.on_mouse_event({
            let view = view.clone();
            let hitbox = hitbox.clone();
            move |event: &ScrollWheelEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble || !hitbox.should_handle_scroll(window) {
                    return;
                }
                let mut delta = event.delta.pixel_delta(line_height);
                if event.modifiers.shift && delta.x == Pixels::ZERO {
                    delta = point(delta.y, Pixels::ZERO);
                }
                let moved = view.update(cx, |view, cx| {
                    let moved = view.canvas().scroll.scroll_by(delta);
                    if moved {
                        cx.notify();
                    }
                    moved
                });
                // En el límite, la rueda sigue hacia quien aloja al visor.
                if moved {
                    cx.stop_propagation();
                }
            }
        });

        window.on_mouse_event({
            let view = view.clone();
            let hitbox = hitbox.clone();
            let frame = frame.clone();
            move |event: &MouseDownEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble
                    || event.button != MouseButton::Left
                    || !hitbox.is_hovered(window)
                    || frame.over_action(event.position)
                {
                    return;
                }
                view.update(cx, |view, cx| {
                    window.focus(&view.canvas().focus, cx);
                    if view.selects_lines() && frame.over_gutter(event.position) {
                        begin_line_selection(view, &frame, event);
                    } else {
                        view.canvas_mut().line_selection = None;
                        begin_selection(view, &frame, event);
                    }
                    cx.notify();
                });
            }
        });

        window.on_mouse_event({
            let view = view.clone();
            let hitbox = hitbox.clone();
            let frame = frame.clone();
            move |event: &MouseMoveEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble {
                    return;
                }
                let hovered = hitbox
                    .is_hovered(window)
                    .then(|| frame.row_under(event.position))
                    .flatten();
                view.update(cx, |view, cx| {
                    let mut changed = false;
                    if view.canvas().selecting_lines {
                        if event.pressed_button == Some(MouseButton::Left) {
                            changed = drag_line_selection(view, &frame, event.position);
                        } else {
                            view.canvas_mut().selecting_lines = false;
                            changed = true;
                        }
                    }
                    if view.canvas().selecting {
                        if event.pressed_button == Some(MouseButton::Left) {
                            changed = drag_selection(view, &frame, event.position);
                        } else {
                            // El botón se soltó fuera de la ventana.
                            view.canvas_mut().selecting = false;
                            changed = true;
                        }
                    }
                    if view.canvas().hovered_row != hovered {
                        view.canvas_mut().hovered_row = hovered;
                        changed = true;
                    }
                    if changed {
                        cx.notify();
                    }
                });
            }
        });

        window.on_mouse_event({
            move |event: &MouseUpEvent, phase, _, cx| {
                if phase != DispatchPhase::Bubble || event.button != MouseButton::Left {
                    return;
                }
                view.update(cx, |view, cx| {
                    if view.canvas().selecting || view.canvas().selecting_lines {
                        view.canvas_mut().selecting = false;
                        view.canvas_mut().selecting_lines = false;
                        cx.notify();
                    }
                });
            }
        });
    }
}

/// Hasta dónde desplazar el visor para que se vea `reveal` (posición desde
/// el principio, como `scrollTop` / `scrollLeft`).
fn reveal_position<V: CanvasDelegate>(
    view: &mut V,
    reveal: &Reveal,
    layout: &GridLayout,
    metrics: &Metrics,
    bounds: Bounds<Pixels>,
    cx: &mut Context<V>,
) -> Point<Pixels> {
    let current = view.canvas().scroll.position();
    let top = layout.rows.top(reveal.row);
    let height = layout.rows.height(reveal.row);
    let viewport = bounds.size;

    let y = match reveal.mode {
        RevealMode::At(offset) => return point(current.x, (top - offset).max(Pixels::ZERO)),
        RevealMode::Top => top - metrics.line_height * 2.,
        RevealMode::Center if top >= current.y && top + height <= current.y + viewport.height => {
            current.y
        }
        RevealMode::Center => top - (viewport.height - height) / 2.,
    };

    // En horizontal solo si el documento es más ancho que el visor (las
    // celdas que se parten en renglones nunca lo son).
    let mut x = current.x;
    if layout.scroll_width > viewport.width {
        let spec = view.row(reveal.row, false, layout, metrics, cx);
        if let Some(cell) = spec
            .cells
            .iter()
            .find(|cell| cell.column == reveal.column && cell.wrap_width.is_none())
        {
            let column_x = |offset: usize| {
                let before = cell
                    .text
                    .get(..offset.min(cell.text.len()))
                    .unwrap_or_default();
                cell.x + metrics.ch * crate::text::display_columns(before) as f32
            };
            let start = column_x(reveal.range.start);
            let end = column_x(reveal.range.end);
            let margin = metrics.ch * 4.;
            let left = current.x + layout.sticky_width;
            let right = current.x + viewport.width;
            if start < left {
                x = start - layout.sticky_width - margin;
            } else if end > right {
                x = end - viewport.width + margin;
            }
        }
    }
    point(x.max(Pixels::ZERO), y.max(Pixels::ZERO))
}

/// Clic: empieza una selección. Doble clic selecciona la palabra, triple la
/// línea, y con mayúsculas se extiende la que hay.
fn begin_selection<V: CanvasDelegate>(view: &mut V, frame: &CanvasFrame, event: &MouseDownEvent) {
    let existing = view.canvas().selection;
    let column = existing
        .filter(|_| event.modifiers.shift)
        .map(|selection| selection.column);
    let Some((column, position)) = frame.position_at(event.position, column) else {
        view.canvas_mut().clear_selection();
        return;
    };

    let canvas = view.canvas_mut();
    canvas.selecting = true;
    if let Some(mut selection) = existing.filter(|_| event.modifiers.shift) {
        selection.head = position;
        canvas.selection = Some(selection);
        return;
    }

    let (anchor, head) = match (event.click_count, frame.cell(position.row, column)) {
        (2, Some(cell)) => {
            let range = word_range(&cell.text(), position.offset);
            (
                TextPosition {
                    row: position.row,
                    offset: range.start,
                },
                TextPosition {
                    row: position.row,
                    offset: range.end,
                },
            )
        }
        (count, Some(cell)) if count >= 3 => (
            TextPosition {
                row: position.row,
                offset: 0,
            },
            TextPosition {
                row: position.row,
                offset: cell.len,
            },
        ),
        _ => (position, position),
    };
    canvas.selection = Some(Selection {
        column,
        anchor,
        head,
    });
    // Tras un doble o triple clic no hay arrastre que seguir.
    canvas.selecting = event.click_count <= 1;
}

/// Clic en el medianil: elige esa línea entera, o con mayúsculas extiende las
/// que ya había hasta ella. Suelta la selección de texto.
fn begin_line_selection<V: CanvasDelegate>(
    view: &mut V,
    frame: &CanvasFrame,
    event: &MouseDownEvent,
) {
    let Some(row) = frame.row_at_y(event.position.y) else {
        return;
    };
    let canvas = view.canvas_mut();
    canvas.clear_selection();
    let anchor = match canvas.line_selection {
        Some((anchor, _)) if event.modifiers.shift => anchor,
        _ => row,
    };
    canvas.line_selection = Some((anchor, row));
    canvas.selecting_lines = true;
}

/// Lleva el extremo de las líneas elegidas a la fila del punto; dice si cambió.
fn drag_line_selection<V: CanvasDelegate>(
    view: &mut V,
    frame: &CanvasFrame,
    position: Point<Pixels>,
) -> bool {
    let (Some((anchor, head)), Some(row)) =
        (view.canvas().line_selection, frame.row_at_y(position.y))
    else {
        return false;
    };
    if head == row {
        return false;
    }
    view.canvas_mut().line_selection = Some((anchor, row));
    true
}

/// Lleva el extremo móvil de la selección al punto dado; dice si cambió.
fn drag_selection<V: CanvasDelegate>(
    view: &mut V,
    frame: &CanvasFrame,
    position: Point<Pixels>,
) -> bool {
    let Some(mut selection) = view.canvas().selection else {
        return false;
    };
    let Some((_, head)) = frame.position_at(position, Some(selection.column)) else {
        return false;
    };
    if selection.head == head {
        return false;
    }
    selection.head = head;
    view.canvas_mut().selection = Some(selection);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uniform_rows_find_what_is_visible() {
        let rows = RowHeights::Uniform {
            count: 100,
            height: px(18.),
        };

        assert_eq!(rows.total(), px(1800.));
        assert_eq!(rows.top(3), px(54.));
        assert_eq!(rows.row_at(px(0.)), 0);
        assert_eq!(rows.row_at(px(17.9)), 0);
        assert_eq!(rows.row_at(px(18.)), 1);
        assert_eq!(rows.row_at(px(99_999.)), 99);
        assert_eq!(rows.visible(px(0.)..px(36.)), 0..2);
        assert_eq!(rows.visible(px(9.)..px(45.)), 0..3);
        assert_eq!(rows.visible(px(1790.)..px(2500.)), 99..100);
    }

    #[test]
    fn variable_rows_find_what_is_visible() {
        let rows = RowHeights::from_heights([px(20.), px(18.), px(18.), px(36.), px(18.)]);

        assert_eq!(rows.count(), 5);
        assert_eq!(rows.total(), px(110.));
        assert_eq!(rows.top(3), px(56.));
        assert_eq!(rows.height(3), px(36.));
        assert_eq!(rows.row_at(px(19.9)), 0);
        assert_eq!(rows.row_at(px(20.)), 1);
        assert_eq!(rows.row_at(px(91.)), 3);
        assert_eq!(rows.row_at(px(500.)), 4);
        assert_eq!(rows.visible(px(20.)..px(56.)), 1..3);
        assert_eq!(rows.visible(px(0.)..px(500.)), 0..5);
    }

    #[test]
    fn an_empty_document_shows_no_rows() {
        let rows = RowHeights::Uniform {
            count: 0,
            height: px(18.),
        };
        assert_eq!(rows.visible(px(0.)..px(500.)), 0..0);
        assert_eq!(RowHeights::from_heights([]).visible(px(0.)..px(500.)), 0..0);
    }

    #[test]
    fn a_selection_knows_what_it_covers_in_each_row() {
        let selection = Selection {
            column: 0,
            anchor: TextPosition { row: 4, offset: 3 },
            head: TextPosition { row: 2, offset: 5 },
        };

        assert_eq!(selection.start(), TextPosition { row: 2, offset: 5 });
        assert_eq!(selection.range_in_row(1, 10), None);
        assert_eq!(selection.range_in_row(2, 10), Some(5..10));
        assert_eq!(selection.range_in_row(3, 10), Some(0..10));
        assert_eq!(selection.range_in_row(4, 10), Some(0..3));
        assert_eq!(selection.range_in_row(4, 2), Some(0..2));
        assert_eq!(selection.range_in_row(5, 10), None);
    }

    #[test]
    fn picks_the_word_under_the_cursor() {
        let text = "call(alpha_1) + ñu";
        assert_eq!(&text[word_range(text, 1)], "call");
        assert_eq!(&text[word_range(text, 4)], "(");
        assert_eq!(&text[word_range(text, 7)], "alpha_1");
        assert_eq!(&text[word_range(text, text.len())], "ñu");
        assert_eq!(word_range("", 0), 0..0);
    }

    #[test]
    fn scrolling_stays_inside_the_content() {
        let scroll = CanvasScroll::new();
        scroll.set_geometry(
            Bounds::new(point(px(0.), px(0.)), size(px(100.), px(50.))),
            size(px(300.), px(500.)),
        );

        assert!(scroll.scroll_by(point(px(0.), px(-120.))));
        assert_eq!(scroll.position(), point(px(0.), px(120.)));
        assert!(scroll.scroll_by(point(px(-999.), px(-999.))));
        assert_eq!(scroll.position(), point(px(200.), px(450.)));
        assert!(!scroll.scroll_by(point(px(-1.), px(-1.))));

        // Una posición restaurada antes de medir se ajusta al primer frame.
        scroll.set_position(point(px(0.), px(9_000.)));
        let offset = scroll.set_geometry(
            Bounds::new(point(px(0.), px(0.)), size(px(100.), px(50.))),
            size(px(300.), px(500.)),
        );
        assert_eq!(offset, point(px(0.), px(-450.)));
    }

    #[test]
    fn cuts_the_colour_runs_of_a_wrapped_line() {
        let run = |len: usize| TextRun {
            len,
            font: gpui_kit::font("monospace"),
            color: Hsla::default(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = [run(4), run(6), run(2)];

        let lens = |range: Range<usize>| -> Vec<usize> {
            slice_runs(&runs, &range)
                .iter()
                .map(|run| run.len)
                .collect()
        };
        assert_eq!(lens(0..12), [4, 6, 2]);
        assert_eq!(lens(2..5), [2, 1]);
        assert_eq!(lens(4..10), [6]);
        assert_eq!(lens(11..12), [1]);
    }
}
