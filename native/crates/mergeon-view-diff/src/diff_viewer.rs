//! Visor de diffs (`src/components/repository/FileDiffViewer.vue`): vista
//! unificada y partida, números de línea, cabeceras de bloque con su acción de
//! stage, resaltado de lo que cambió dentro de la línea y coloreado de sintaxis.
//!
//! Las dos vistas salen del mismo análisis (`mergeon_model::diff`): así no
//! pueden discrepar sobre lo que cambió.

use std::collections::HashMap;
use std::ops::{Range, RangeInclusive};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::base::Button as BaseButton;

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Bounds, Context, ElementId, EventEmitter, FocusHandle,
    Focusable, Font, FontWeight, Hsla, InteractiveElement as _, IntoElement, KeyDownEvent,
    MouseButton, MouseDownEvent, ParentElement as _, Pixels, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Task, TestSupportExt as _, TextRun, Window,
    canvas as paint_canvas, div, point, px, rems,
};
use mergeon_model::diff::{
    DiffLine, DiffLineType, InlineRange, SplitCell, SplitCellType, SplitRow, mark_inline_changes,
    parse_unified_diff, to_split_rows,
};
use mergeon_model::diff_preferences::DiffViewMode;
use mergeon_model::patch::{PatchDirection, count_hunks, extract_hunk_patch, extract_lines_patch};
use mergeon_ui::{ActivePaletteExt as _, MergeonStyled as _, Radius, SpinnerColor, SpinnerSize};

use crate::canvas::{
    self, CanvasDelegate, CanvasState, Cell, Fill, FillEdge, GridLayout, Label, LabelAlign,
    Metrics, Reveal, RevealMode, RowHeights, RowSpec, WrapMeasure,
};
use crate::change_markers::{
    ChangeMarker, MARKER_STRIP_WIDTH, StripBounds, change_markers, marker_at, paint_markers,
};
use crate::find::{
    self, FindInViewer, FindSource, FindState, FindStatus, NextChange, PreviousChange, Searchable,
};
use crate::highlight::{
    Language, LineSegment, Token, build_line_segments, highlight_tokens, language_for_path,
};
use crate::text::{display_columns, display_offset, expand_tabs, source_range};
use crate::typography::{CodeColors, code_font_family, italic, medium, plain_runs, syntax_runs};

/// Botón de bloque de Señal (`.hunk .hb`): 20px de alto, 8px de relleno,
/// texto de 11,5px.
const HUNK_BUTTON_HEIGHT: f32 = 20.;
const HUNK_BUTTON_PADDING: f32 = 8.;
const HUNK_BUTTON_TEXT: f32 = 11.5;

/// Separación entre los botones de una cabecera de bloque o de la barra de
/// líneas elegidas.
const HUNK_BUTTON_GAP: f32 = 4.;

/// De qué lado está el diff a la vista y, por tanto, qué se ofrece sobre sus
/// bloques y líneas: sin preparar (`Stage`: preparar y descartar) o preparado
/// (`Unstage`: quitar del stage). En el detalle de un commit no hay nada que
/// preparar y no se ofrece ninguna.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HunkAction {
    Stage,
    Unstage,
}

/// Lo que se le pide a git con un parche de un bloque o de unas líneas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PatchAction {
    /// Al índice (`git apply --cached`).
    Stage,
    /// Fuera del índice (`git apply --cached --reverse`).
    Unstage,
    /// Fuera del working tree (`git apply --reverse`): no se puede deshacer.
    Discard,
}

impl PatchAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stage => "stage",
            Self::Unstage => "unstage",
            Self::Discard => "discard",
        }
    }

    /// Botón de la cabecera de un bloque.
    pub fn hunk_label(self) -> &'static str {
        match self {
            Self::Stage => "Stage hunk",
            Self::Unstage => "Unstage hunk",
            Self::Discard => "Discard hunk",
        }
    }

    pub fn hunk_tooltip(self) -> &'static str {
        match self {
            Self::Stage => "Stage only this hunk",
            Self::Unstage => "Unstage only this hunk",
            Self::Discard => "Discard this hunk from the working tree",
        }
    }

    /// Botón de la barra de las líneas elegidas.
    pub fn lines_label(self) -> &'static str {
        match self {
            Self::Stage => "Stage lines",
            Self::Unstage => "Unstage lines",
            Self::Discard => "Discard lines",
        }
    }

    pub fn lines_tooltip(self) -> &'static str {
        match self {
            Self::Stage => "Stage only the selected lines (S)",
            Self::Unstage => "Unstage only the selected lines (U)",
            Self::Discard => "Discard the selected lines from the working tree (D)",
        }
    }

    /// La tecla que lo hace sobre las líneas elegidas, con el foco en el visor.
    pub fn key(self) -> &'static str {
        match self {
            Self::Stage => "s",
            Self::Unstage => "u",
            Self::Discard => "d",
        }
    }

    /// Se aplica con `git apply --reverse`.
    pub fn is_reverse(self) -> bool {
        self != Self::Stage
    }

    /// Toca el working tree (y no el índice): descartar.
    pub fn touches_worktree(self) -> bool {
        self == Self::Discard
    }

    /// Cómo se recortan las líneas para esta acción
    /// (`mergeon_model::patch::extract_lines_patch`).
    pub fn direction(self) -> PatchDirection {
        if self.is_reverse() {
            PatchDirection::Reverse
        } else {
            PatchDirection::Forward
        }
    }
}

impl HunkAction {
    /// `'stage'` / `'unstage'`, los valores de la prop `hunkAction` del original.
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "stage" => Some(Self::Stage),
            "unstage" => Some(Self::Unstage),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stage => "stage",
            Self::Unstage => "unstage",
        }
    }

    /// Texto del botón principal.
    pub fn label(self) -> &'static str {
        self.primary().hunk_label()
    }

    /// Texto de ayuda del botón principal (el `title` del original).
    pub fn tooltip(self) -> &'static str {
        self.primary().hunk_tooltip()
    }

    /// Quitar del stage es aplicar el parche al revés (`git apply --reverse`).
    pub fn is_reverse(self) -> bool {
        self == Self::Unstage
    }

    /// La acción del botón principal: preparar o quitar del stage.
    pub fn primary(self) -> PatchAction {
        match self {
            Self::Stage => PatchAction::Stage,
            Self::Unstage => PatchAction::Unstage,
        }
    }

    /// Lo que se ofrece sobre un bloque o unas líneas, en el orden en que se
    /// pintan (la principal, a la derecha). Lo preparado no se descarta.
    pub fn patch_actions(self) -> &'static [PatchAction] {
        match self {
            Self::Stage => &[PatchAction::Discard, PatchAction::Stage],
            Self::Unstage => &[PatchAction::Unstage],
        }
    }
}

/// Lo desplazado desde el principio, en píxeles (`scrollTop` / `scrollLeft`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[non_exhaustive]
pub struct ScrollPosition {
    pub top: f32,
    pub left: f32,
}

impl ScrollPosition {
    pub fn new(top: f32, left: f32) -> Self {
        Self { top, left }
    }

    pub(crate) fn from_point(position: gpui_kit::Point<Pixels>) -> Self {
        Self {
            top: f32::from(position.y),
            left: f32::from(position.x),
        }
    }

    pub(crate) fn to_point(self) -> gpui_kit::Point<Pixels> {
        point(px(self.left), px(self.top))
    }
}

/// Una línea del archivo y a qué altura del visor se veía. Al cambiar a otro
/// diff del mismo archivo ("Whole file", que trae todo el archivo o solo los
/// bloques) el desplazamiento en píxeles no dice nada; la línea sí:
/// [`DiffViewer::line_anchor`] la toma y [`DiffViewer::scroll_to_line`] la
/// vuelve a poner donde estaba.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct LineAnchor {
    /// El archivo del diff del que se tomó: en otro no significa nada.
    pub file_path: SharedString,
    /// Número de la línea en la versión nueva del archivo.
    pub line: u32,
    /// Del borde superior del visor al de la fila de esa línea (negativo si
    /// la fila asomaba cortada por arriba).
    pub offset: f32,
    /// Desplazamiento horizontal, que se conserva tal cual.
    pub left: f32,
}

/// La fila más cercana a `from` que tiene número (`number(fila)`): esa fila
/// si lo tiene, si no la siguiente y, al final del diff, la anterior. Como
/// [`crate::nearest_new_line`], pero devuelve la fila.
fn nearest_numbered_row(
    count: usize,
    from: usize,
    number: impl Fn(usize) -> Option<u32>,
) -> Option<usize> {
    let from = from.min(count);
    (from..count)
        .find(|row| number(*row).is_some())
        .or_else(|| (0..from).rev().find(|row| number(*row).is_some()))
}

/// La fila que muestra la línea `line` (de la versión nueva) o, si no está
/// en este diff (un hueco entre bloques), la de número más cercano; a igual
/// distancia, la anterior.
fn row_for_line(count: usize, line: u32, number: impl Fn(usize) -> Option<u32>) -> Option<usize> {
    let mut best: Option<(u32, usize)> = None;
    for row in 0..count {
        let Some(num) = number(row) else {
            continue;
        };
        let distance = num.abs_diff(line);
        if distance == 0 {
            return Some(row);
        }
        if best.is_none_or(|(closest, _)| distance < closest) {
            best = Some((distance, row));
        }
    }
    best.map(|(_, row)| row)
}

/// Se pidió aplicar la acción a un bloque.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct HunkRequest {
    /// Posición del bloque (`@@`) dentro del diff, desde cero.
    pub index: usize,
    pub action: PatchAction,
    /// Parche con la cabecera del archivo y solo ese bloque
    /// (`mergeon_model::patch::extract_hunk_patch`), listo para `git apply`
    /// según `action`. `None` si el diff no trae cabecera: no hay parche que
    /// aplicar.
    pub patch: Option<String>,
}

/// Se pidió aplicar una acción a las líneas elegidas.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct LinesRequest {
    pub action: PatchAction,
    /// Las altas y bajas elegidas: posiciones en [`DiffViewer::lines`].
    pub lines: Vec<usize>,
    /// Parche recortado a esas líneas
    /// (`mergeon_model::patch::extract_lines_patch`), con las cabeceras de
    /// bloque recalculadas. `None` si no se pudo construir.
    pub patch: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DiffViewerEvent {
    /// Clic en "Stage hunk" / "Unstage hunk" / "Discard hunk" (`@apply-hunk`).
    /// Descartar no se puede deshacer: quien lo recibe tiene que confirmarlo.
    ApplyHunk(HunkRequest),
    /// Clic en "Stage lines" / "Unstage lines" / "Discard lines" (o su tecla).
    ApplyLines(LinesRequest),
    /// El visor se desplazó (`@scroll` de `.diff-viewer`).
    Scrolled(ScrollPosition),
}

/// El bloque al que pertenece la cabecera de una línea.
#[derive(Debug, Clone, Copy, Default)]
struct HunkInfo {
    /// Posición del bloque, solo en las cabeceras `@@`.
    index: Option<usize>,
    /// En las cabeceras, la última fila de su bloque.
    end: usize,
}

/// Diffs hasta este tamaño se analizan en el acto: tardan menos de un
/// milisegundo y así no hay ni un frame con el diff anterior.
pub(crate) const SYNC_PARSE_BYTES: usize = 64 * 1024;

/// Lo que puede tardar el análisis en segundo plano antes de que el visor
/// muestre que está trabajando: si acaba antes, el diff nuevo aparece sin
/// pasar por el estado de carga (sin parpadeo).
pub(crate) const PARSING_INDICATOR_DELAY: Duration = Duration::from_millis(30);

/// El diff ya analizado: lo que se calcula (en segundo plano, si es grande)
/// cada vez que cambia el texto.
#[derive(Default)]
struct ParsedDiff {
    lines: Vec<DiffLine>,
    hunks: Vec<HunkInfo>,
    /// Filas de la vista partida, si se pidieron al analizar.
    split_rows: Option<Vec<SplitRow>>,
    /// Columnas de la línea más larga y de la cabecera de bloque más larga.
    max_columns: usize,
    max_hunk_columns: usize,
    /// Cifras del número de línea más alto.
    gutter_digits: usize,
    hunk_count: usize,
    /// Marcas de cambios de la vista unificada y, si se pidieron sus filas,
    /// de la partida.
    unified_markers: Vec<ChangeMarker>,
    split_markers: Option<Vec<ChangeMarker>>,
}

/// Marcas de la vista unificada: altas y bajas de cada línea.
fn unified_change_markers(lines: &[DiffLine]) -> Vec<ChangeMarker> {
    let rows: Vec<(bool, bool)> = lines
        .iter()
        .map(|line| {
            (
                line.kind == DiffLineType::Add,
                line.kind == DiffLineType::Del,
            )
        })
        .collect();
    change_markers(&rows)
}

/// Marcas de la vista partida: bajas a la izquierda, altas a la derecha.
fn split_change_markers(rows: &[SplitRow]) -> Vec<ChangeMarker> {
    let rows: Vec<(bool, bool)> = rows
        .iter()
        .map(|row| {
            (
                row.right.kind == SplitCellType::Add,
                row.left.kind == SplitCellType::Del,
            )
        })
        .collect();
    change_markers(&rows)
}

/// Analiza un diff: líneas, lo que cambió dentro de cada una, bloques y
/// medidas. No toca la interfaz: se puede llamar fuera del hilo principal.
fn parse_diff(diff: &str, with_split_rows: bool) -> ParsedDiff {
    let mut parsed = parse_unified_diff(diff);
    // Un archivo con finales de línea de Windows trae un retorno de carro
    // al final de cada línea: no se pinta (el navegador tampoco lo hacía).
    for line in &mut parsed {
        if line.content.ends_with('\r') {
            line.content.pop();
        }
    }
    let lines = mark_inline_changes(&parsed);
    drop(parsed);

    // Cada cabecera `@@` lleva su posición: es lo que identifica al bloque
    // cuando se pide prepararlo.
    let mut hunks = vec![HunkInfo::default(); lines.len()];
    let mut hunk_index = 0;
    let mut header = None;
    let mut max_columns = 0;
    let mut max_hunk_columns = 0;
    let mut highest = 0;
    for (row, line) in lines.iter().enumerate() {
        if line.kind == DiffLineType::Hunk {
            hunks[row].index = Some(hunk_index);
            hunk_index += 1;
            header = Some(row);
        }
        if let Some(header) = header {
            hunks[header].end = row;
        }
        let columns = display_columns(&line.content);
        max_columns = max_columns.max(columns);
        if line.kind == DiffLineType::Hunk {
            max_hunk_columns = max_hunk_columns.max(columns);
        }
        highest = highest
            .max(line.old_num.unwrap_or(0))
            .max(line.new_num.unwrap_or(0));
    }

    let split_rows = with_split_rows.then(|| to_split_rows(&lines));
    ParsedDiff {
        unified_markers: unified_change_markers(&lines),
        split_markers: split_rows.as_deref().map(split_change_markers),
        split_rows,
        hunks,
        lines,
        max_columns,
        max_hunk_columns,
        // El medianil mide lo que ocupa el número más largo del diff, no un
        // ancho fijo: un archivo de 80 líneas no reserva sitio para cinco
        // cifras.
        gutter_digits: highest.to_string().len().max(2),
        hunk_count: count_hunks(diff),
    }
}

/// Un diff que se está analizando en segundo plano. Mientras tanto se sigue
/// viendo el anterior (sin acciones: sus bloques ya no son los del diff
/// pedido).
struct PendingDiff {
    diff: SharedString,
    file_path: SharedString,
    generation: u64,
    /// Dónde quedará el scroll al aplicarlo: arriba, o lo que se restaure
    /// con `set_scroll_position` mientras tanto.
    scroll: ScrollPosition,
    /// La línea a la que llevar el visor al aplicarlo
    /// ([`DiffViewer::scroll_to_line`]); manda sobre `scroll`.
    anchor: Option<LineAnchor>,
    /// Pasó [`PARSING_INDICATOR_DELAY`]: el diff anterior se atenúa.
    slow: bool,
    /// Soltarlas cancela el análisis y el aviso.
    _parse: Task<()>,
    _indicator: Task<()>,
}

/// Las filas de la vista partida de un diff grande, calculándose en segundo
/// plano al pasar a ella. Mientras tanto se sigue viendo la vista unificada
/// (y todo lo que depende de la vista, como la búsqueda, las marcas o la
/// línea de [`DiffViewer::line_anchor`], es de ella).
struct SplitBuild {
    generation: u64,
    /// Pasó [`PARSING_INDICATOR_DELAY`]: la vista unificada se atenúa.
    slow: bool,
    /// La línea a la que llevar el visor al pasar a la vista partida
    /// ([`DiffViewer::scroll_to_line`] mientras tanto).
    anchor: Option<LineAnchor>,
    /// Soltarlas cancela el cálculo y el aviso.
    _build: Task<()>,
    _indicator: Task<()>,
}

/// Lo que se pinta de una línea, calculado la primera vez que entra en la
/// ventana: colorear es lo más caro que hace esta vista.
struct LineText {
    display: SharedString,
    /// Trozos de sintaxis, en offsets de `display`.
    tokens: Vec<Token>,
    /// Lo que cambió dentro de la línea, en offsets de `display`.
    inline: Option<Range<usize>>,
}

fn prepare_line(
    content: &str,
    inline: Option<InlineRange>,
    language: Option<Language>,
    code: bool,
) -> LineText {
    let tokens = if code {
        highlight_tokens(content, language)
    } else {
        Vec::new()
    };

    match expand_tabs(content) {
        None => LineText {
            display: SharedString::from(content.to_string()),
            tokens,
            inline: inline.map(|[from, to]| from..to),
        },
        Some(display) => LineText {
            tokens: tokens
                .into_iter()
                .map(|(range, kind)| {
                    (
                        display_offset(content, range.start)..display_offset(content, range.end),
                        kind,
                    )
                })
                .collect(),
            inline: inline
                .map(|[from, to]| display_offset(content, from)..display_offset(content, to)),
            display: SharedString::from(display),
        },
    }
}

#[derive(Debug, Clone, PartialEq)]
struct LayoutKey {
    mode: DiffViewMode,
    action: Option<HunkAction>,
    hunk_buttons: bool,
    metrics: Metrics,
    measured_width: Pixels,
}

/// De qué dependen los altos de las filas: la vista, la tipografía y, en la
/// vista partida, el ancho al que se parte el texto. No el alto del visor ni
/// lo ancho que resultó una línea: recorrer todo el diff solo hace falta
/// cuando cambia esto (o el diff).
#[derive(Debug, Clone, PartialEq)]
struct RowsKey {
    mode: DiffViewMode,
    font: Font,
    font_size: Pixels,
    line_height: Pixels,
    ch: Pixels,
    wrap_width: Option<Pixels>,
}

impl RowsKey {
    fn new(mode: DiffViewMode, metrics: &Metrics, wrap_width: Option<Pixels>) -> Self {
        Self {
            mode,
            font: metrics.font.clone(),
            font_size: metrics.font_size,
            line_height: metrics.line_height,
            ch: metrics.ch,
            wrap_width,
        }
    }
}

/// Los altos de las filas, de la caché si nada de lo que los decide cambió.
fn cached_rows(
    cache: &mut Option<(RowsKey, RowHeights)>,
    key: RowsKey,
    compute: impl FnOnce() -> RowHeights,
) -> RowHeights {
    if let Some((cached, rows)) = cache
        && *cached == key
    {
        return rows.clone();
    }
    let rows = compute();
    *cache = Some((key, rows.clone()));
    rows
}

/// Visor de un diff unificado de git.
///
/// Guarda el diff ya analizado, el desplazamiento, la selección y la caché de
/// coloreado; quien lo aloja lo crea una vez (`cx.new(|cx| DiffViewer::new(window,
/// cx))`), le da los datos con [`Self::set_diff`] y lo pinta como hijo
/// (`.child(viewer.clone())`). Ocupa todo el espacio de su contenedor.
pub struct DiffViewer {
    canvas: CanvasState,
    diff: SharedString,
    file_path: SharedString,
    language: Option<Language>,
    /// La vista que se ve. Solo difiere de `requested_mode` mientras se
    /// calculan en segundo plano las filas de la vista partida.
    shown_mode: DiffViewMode,
    /// La vista pedida con [`Self::set_view_mode`].
    requested_mode: DiffViewMode,
    /// Las filas de la vista partida en camino (diff grande).
    split_build: Option<SplitBuild>,
    split_generation: u64,
    hunk_action: Option<HunkAction>,
    /// Botones "Stage hunk" / "Discard hunk" en las cabeceras `@@`. Sin ellos
    /// (el diff del archivo entero, que es un solo bloque) siguen las líneas
    /// sueltas.
    hunk_buttons: bool,
    /// Compartidas con la búsqueda, que puede correr en segundo plano.
    lines: Arc<Vec<DiffLine>>,
    hunks: Vec<HunkInfo>,
    split_rows: Option<Arc<Vec<SplitRow>>>,
    /// Columnas de la línea más larga y de la cabecera de bloque más larga.
    max_columns: usize,
    max_hunk_columns: usize,
    /// Cifras del número de línea más alto.
    gutter_digits: usize,
    hunk_count: usize,
    /// El diff pedido mientras se analiza en segundo plano.
    pending: Option<PendingDiff>,
    /// Cuenta los análisis lanzados: uno viejo que acabe tarde no pisa al
    /// nuevo.
    generation: u64,
    unified_cache: Vec<Option<Rc<LineText>>>,
    split_cache: HashMap<(usize, usize), Rc<LineText>>,
    layout: Option<(LayoutKey, GridLayout)>,
    /// Altos de las filas, que sobreviven a cambios de alto del visor y de la
    /// línea más ancha medida.
    rows: Option<(RowsKey, RowHeights)>,
    /// Ancho de los botones de bloque, por acción y fuente.
    action_width: Option<((Option<HunkAction>, SharedString), Pixels)>,
    colors: Option<CodeColors>,
    /// La barra de búsqueda y sus coincidencias.
    find: FindState,
    /// Cambia con cada diff analizado y con cada cambio de vista: lo que se
    /// busca y los bloques de cambios dependen de ello.
    document_generation: u64,
    /// Primera fila de cada bloque de cambios en la vista actual, por
    /// generación del documento.
    change_starts: Option<(u64, Rc<Vec<usize>>)>,
    /// El último bloque al que se saltó con F7 / Shift+F7.
    change_cursor: Option<usize>,
    /// Marcas de cambios junto a la barra de desplazamiento, por vista.
    unified_markers: Rc<Vec<ChangeMarker>>,
    split_markers: Option<Rc<Vec<ChangeMarker>>>,
    /// Dónde quedó la tira de marcas en el último frame.
    marker_strip: StripBounds,
}

impl EventEmitter<DiffViewerEvent> for DiffViewer {}

impl Focusable for DiffViewer {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.canvas.focus.clone()
    }
}

impl DiffViewer {
    pub fn new(_: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            canvas: CanvasState::new(cx),
            diff: SharedString::default(),
            file_path: SharedString::default(),
            language: None,
            shown_mode: DiffViewMode::Unified,
            requested_mode: DiffViewMode::Unified,
            split_build: None,
            split_generation: 0,
            hunk_action: None,
            hunk_buttons: true,
            lines: Arc::default(),
            hunks: Vec::new(),
            split_rows: None,
            max_columns: 0,
            max_hunk_columns: 0,
            gutter_digits: 2,
            hunk_count: 0,
            pending: None,
            generation: 0,
            unified_cache: Vec::new(),
            split_cache: HashMap::new(),
            layout: None,
            rows: None,
            action_width: None,
            colors: None,
            find: FindState::default(),
            document_generation: 0,
            change_starts: None,
            change_cursor: None,
            unified_markers: Rc::default(),
            split_markers: None,
            marker_strip: StripBounds::default(),
        }
    }

    /// Cambia el diff que se muestra. `file_path` decide el lenguaje con el que
    /// se colorea. No hace nada si los dos son los que ya había; si cambian, el
    /// desplazamiento vuelve al principio y la selección se pierde.
    ///
    /// Un diff grande se analiza en segundo plano: mientras tanto se sigue
    /// viendo el anterior (atenuado si tarda) y [`Self::is_parsing`] dice
    /// `true`. Si llega otro antes de que acabe, el análisis viejo se descarta.
    pub fn set_diff(
        &mut self,
        diff: impl Into<SharedString>,
        file_path: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        let diff = diff.into();
        let file_path = file_path.into();
        if diff == *self.diff() && file_path == *self.file_path() {
            return;
        }

        // Volver al diff que se está viendo (o cambiar solo la ruta) no pide
        // analizar nada: se suelta lo pendiente.
        if diff == self.diff {
            self.pending = None;
            if file_path != self.file_path {
                self.language = language_for_path(&file_path);
                self.file_path = file_path;
                self.reset_line_texts();
            }
            // Se pidió la vista partida mientras llegaba el otro diff.
            if self.sync_view_mode(cx) {
                self.document_changed(cx);
            }
            cx.notify();
            return;
        }

        // Las filas de la vista partida solo si es la que se ve: en la
        // unificada son casi la mitad del análisis y no se usan. Si luego se
        // pasa a la partida, se calculan entonces (en segundo plano si el
        // diff es grande).
        let with_split_rows = self.requested_mode == DiffViewMode::Split;
        if diff.len() <= SYNC_PARSE_BYTES {
            self.pending = None;
            let parsed = parse_diff(&diff, with_split_rows);
            self.apply_parsed(diff, file_path, parsed, ScrollPosition::default(), cx);
            return;
        }

        // Las filas partidas del diff que se ve ya no hacen falta.
        self.split_build = None;
        self.generation += 1;
        let generation = self.generation;
        let text = diff.clone();
        let parsing = cx.background_spawn(async move { parse_diff(&text, with_split_rows) });
        let parse = cx.spawn(async move |this, cx| {
            let parsed = parsing.await;
            this.update(cx, |this, cx| this.finish_parse(generation, parsed, cx))
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
        self.pending = Some(PendingDiff {
            diff,
            file_path,
            generation,
            scroll: ScrollPosition::default(),
            anchor: None,
            slow: false,
            _parse: parse,
            _indicator: indicator,
        });
        // Lo elegido era del diff anterior.
        self.canvas.set_line_selection(None);
        cx.notify();
    }

    /// `true` mientras un diff pedido con [`Self::set_diff`] se analiza en
    /// segundo plano.
    pub fn is_parsing(&self) -> bool {
        self.pending.is_some()
    }

    fn finish_parse(&mut self, generation: u64, parsed: ParsedDiff, cx: &mut Context<Self>) {
        if self
            .pending
            .as_ref()
            .is_none_or(|pending| pending.generation != generation)
        {
            return;
        }
        let Some(pending) = self.pending.take() else {
            return;
        };
        self.apply_parsed(pending.diff, pending.file_path, parsed, pending.scroll, cx);
        if let Some(anchor) = pending.anchor {
            self.reveal_line(&anchor, cx);
            // Si la vista partida aún está en camino, también en ella.
            if let Some(build) = &mut self.split_build {
                build.anchor = Some(anchor);
            }
        }
    }

    /// `true` mientras se calculan en segundo plano las filas de la vista
    /// partida de un diff grande (se sigue viendo la unificada).
    pub fn is_building_split_rows(&self) -> bool {
        self.split_build.is_some()
    }

    /// Ya están calculadas las filas de la vista partida del diff que se ve
    /// (tests: en la unificada no se calculan).
    #[doc(hidden)]
    pub fn has_split_rows(&self) -> bool {
        self.split_rows.is_some()
    }

    /// Lleva la vista que se ve a la pedida. La unificada, o la partida con
    /// sus filas ya calculadas (o un diff pequeño, que las calcula en el
    /// acto), cambian enseguida; un diff grande las calcula en segundo plano
    /// y, mientras tanto, se sigue viendo la unificada. Dice si cambió la
    /// vista que se ve (quien llama avisa del documento nuevo).
    fn sync_view_mode(&mut self, cx: &mut Context<Self>) -> bool {
        let mode = self.requested_mode;
        if mode == DiffViewMode::Split && self.split_rows.is_none() {
            if self.split_build.is_some() {
                return false;
            }
            if self.diff.len() > SYNC_PARSE_BYTES {
                // Con otro diff en camino, sus filas se piden al llegar.
                if self.pending.is_none() {
                    self.start_split_build(cx);
                }
                return false;
            }
            self.split_rows();
        }
        self.split_build = None;
        if self.shown_mode == mode {
            return false;
        }
        self.shown_mode = mode;
        self.layout = None;
        self.rows = None;
        self.canvas.reset_content();
        true
    }

    /// Calcula fuera del hilo de la interfaz las filas (y las marcas) de la
    /// vista partida del diff que se ve.
    fn start_split_build(&mut self, cx: &mut Context<Self>) {
        self.split_generation += 1;
        let generation = self.split_generation;
        let lines = self.lines.clone();
        let building = cx.background_spawn(async move {
            let rows = to_split_rows(&lines);
            let markers = split_change_markers(&rows);
            (rows, markers)
        });
        let build = cx.spawn(async move |this, cx| {
            let (rows, markers) = building.await;
            this.update(cx, |this, cx| {
                this.finish_split_build(generation, rows, markers, cx)
            })
            .ok();
        });
        let indicator = cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(PARSING_INDICATOR_DELAY)
                .await;
            this.update(cx, |this, cx| {
                if let Some(build) = &mut this.split_build
                    && build.generation == generation
                {
                    build.slow = true;
                    cx.notify();
                }
            })
            .ok();
        });
        self.split_build = Some(SplitBuild {
            generation,
            slow: false,
            anchor: None,
            _build: build,
            _indicator: indicator,
        });
    }

    fn finish_split_build(
        &mut self,
        generation: u64,
        rows: Vec<SplitRow>,
        markers: Vec<ChangeMarker>,
        cx: &mut Context<Self>,
    ) {
        // Otro diff u otra vista entretanto: estas filas ya no sirven.
        let Some(build) = self
            .split_build
            .take_if(|build| build.generation == generation)
        else {
            return;
        };
        self.split_rows = Some(Arc::new(rows));
        self.split_markers = Some(Rc::new(markers));
        if self.sync_view_mode(cx) {
            self.document_changed(cx);
        }
        if let Some(anchor) = build.anchor {
            self.reveal_line(&anchor, cx);
        }
        cx.notify();
    }

    fn apply_parsed(
        &mut self,
        diff: SharedString,
        file_path: SharedString,
        parsed: ParsedDiff,
        scroll: ScrollPosition,
        cx: &mut Context<Self>,
    ) {
        // Las filas partidas que se calculaban eran del diff anterior.
        self.split_build = None;
        self.diff = diff;
        self.language = language_for_path(&file_path);
        self.file_path = file_path;
        let ParsedDiff {
            lines,
            hunks,
            split_rows,
            max_columns,
            max_hunk_columns,
            gutter_digits,
            hunk_count,
            unified_markers,
            split_markers,
        } = parsed;
        self.unified_markers = Rc::new(unified_markers);
        self.split_markers = split_markers.map(Rc::new);
        self.lines = Arc::new(lines);
        self.hunks = hunks;
        self.split_rows = split_rows.map(Arc::new);
        self.max_columns = max_columns;
        self.max_hunk_columns = max_hunk_columns;
        self.gutter_digits = gutter_digits;
        self.hunk_count = hunk_count;
        self.rows = None;
        self.canvas.scroll.set_position(scroll.to_point());
        self.reset_line_texts();
        self.sync_view_mode(cx);
        self.document_changed(cx);
        cx.notify();
    }

    /// Lo que se ve es otro documento (otro diff u otra vista): la búsqueda
    /// se repite y los bloques de cambios se recalculan.
    fn document_changed(&mut self, cx: &mut Context<Self>) {
        self.document_generation += 1;
        self.change_cursor = None;
        find::refresh(self, cx);
    }

    /// Lo pintado ya no vale (otro diff u otro lenguaje).
    fn reset_line_texts(&mut self) {
        self.unified_cache = vec![None; self.lines.len()];
        self.split_cache.clear();
        self.layout = None;
        self.canvas.reset_content();
    }

    /// `Unified` o `Split`. Las filas de la vista partida se calculan al
    /// pasar a ella: en el acto si el diff es pequeño; si es grande, en
    /// segundo plano ([`Self::is_building_split_rows`]), y hasta que estén se
    /// sigue viendo la unificada (atenuada si tarda).
    pub fn set_view_mode(&mut self, mode: DiffViewMode, cx: &mut Context<Self>) {
        if self.requested_mode == mode {
            return;
        }
        self.requested_mode = mode;
        if self.sync_view_mode(cx) {
            self.document_changed(cx);
        }
        cx.notify();
    }

    /// Acción que se ofrece en cada bloque (solo en la vista unificada);
    /// `None` no ofrece ninguna.
    pub fn set_hunk_action(&mut self, action: Option<HunkAction>, cx: &mut Context<Self>) {
        if self.hunk_action == action {
            return;
        }
        self.hunk_action = action;
        self.layout = None;
        self.canvas.set_line_selection(None);
        cx.notify();
    }

    /// Si las cabeceras `@@` llevan los botones de bloque (por defecto sí).
    /// Con el archivo entero ("Whole file") el diff es un único bloque:
    /// "Stage hunk" sería el archivo entero y "Discard hunk" lo descartaría
    /// todo, así que se quitan y quedan las líneas sueltas, que sí se
    /// aplican bien sobre ese diff.
    pub fn set_hunk_buttons(&mut self, show: bool, cx: &mut Context<Self>) {
        if self.hunk_buttons == show {
            return;
        }
        self.hunk_buttons = show;
        self.layout = None;
        cx.notify();
    }

    pub fn has_hunk_buttons(&self) -> bool {
        self.hunk_buttons
    }

    /// Restaura un desplazamiento guardado (p. ej. al volver a una pestaña).
    /// Con un diff analizándose, se aplica cuando aparezca.
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

    /// La primera línea que se ve (de la versión nueva; si arriba solo hay
    /// líneas borradas o una cabecera, la siguiente que exista en el archivo)
    /// y a qué altura está. `None` si aún no se pintó, si se está analizando
    /// otro diff o si el diff no tiene líneas de la versión nueva.
    pub fn line_anchor(&mut self) -> Option<LineAnchor> {
        if self.pending.is_some() {
            return None;
        }
        let rows = match (&self.layout, &self.rows) {
            (Some((_, layout)), _) => layout.rows.clone(),
            (None, Some((_, rows))) => rows.clone(),
            (None, None) => return None,
        };
        let position = self.canvas.scroll.position();
        let top = rows.row_at(position.y);
        let row = match self.shown_mode {
            DiffViewMode::Unified => {
                let lines = &self.lines;
                nearest_numbered_row(lines.len(), top, |row| lines[row].new_num)?
            }
            DiffViewMode::Split => {
                let split = self.split_rows();
                nearest_numbered_row(split.len(), top, |row| split[row].right.num)?
            }
        };
        let line = self.new_line_at(row)?;
        Some(LineAnchor {
            file_path: self.file_path.clone(),
            line,
            offset: f32::from(rows.top(row) - position.y),
            left: f32::from(position.x),
        })
    }

    /// Lleva el visor a la línea de `anchor`, a la misma altura a la que se
    /// veía; si este diff no la tiene (los bloques sin ella), a la más
    /// cercana. Solo si el diff es del mismo archivo. Con un diff
    /// analizándose, se aplica cuando aparezca (y manda sobre
    /// [`Self::set_scroll_position`]).
    pub fn scroll_to_line(&mut self, anchor: &LineAnchor, cx: &mut Context<Self>) {
        if *self.file_path() != anchor.file_path {
            return;
        }
        match &mut self.pending {
            Some(pending) => pending.anchor = Some(anchor.clone()),
            None => {
                self.reveal_line(anchor, cx);
                // Con la vista partida en camino, se vuelve a llevar a la
                // línea cuando aparezca (sus filas son otras).
                if let Some(build) = &mut self.split_build {
                    build.anchor = Some(anchor.clone());
                }
            }
        }
        cx.notify();
    }

    fn new_line_at(&mut self, row: usize) -> Option<u32> {
        match self.shown_mode {
            DiffViewMode::Unified => self.lines.get(row)?.new_num,
            DiffViewMode::Split => self.split_rows().get(row)?.right.num,
        }
    }

    fn reveal_line(&mut self, anchor: &LineAnchor, cx: &mut Context<Self>) {
        let row = match self.shown_mode {
            DiffViewMode::Unified => {
                let lines = &self.lines;
                row_for_line(lines.len(), anchor.line, |row| lines[row].new_num)
            }
            DiffViewMode::Split => {
                let split = self.split_rows();
                row_for_line(split.len(), anchor.line, |row| split[row].right.num)
            }
        };
        let Some(row) = row else {
            return;
        };
        let top = self.canvas.scroll.position().y;
        self.canvas.scroll.set_position(point(px(anchor.left), top));
        self.canvas.reveal(Reveal {
            row,
            column: usize::from(self.shown_mode == DiffViewMode::Split),
            range: 0..0,
            mode: RevealMode::At(px(anchor.offset)),
        });
        cx.notify();
    }

    /// El último diff pedido con [`Self::set_diff`] (aunque aún se esté
    /// analizando).
    pub fn diff(&self) -> &SharedString {
        self.pending
            .as_ref()
            .map_or(&self.diff, |pending| &pending.diff)
    }

    pub fn file_path(&self) -> &SharedString {
        self.pending
            .as_ref()
            .map_or(&self.file_path, |pending| &pending.file_path)
    }

    /// La vista pedida con [`Self::set_view_mode`] (la partida puede estar
    /// aún calculándose: ver [`Self::is_building_split_rows`]).
    pub fn view_mode(&self) -> DiffViewMode {
        self.requested_mode
    }

    pub fn hunk_action(&self) -> Option<HunkAction> {
        self.hunk_action
    }

    /// Lenguaje con el que se colorea, deducido de la ruta.
    pub fn language(&self) -> Option<Language> {
        self.language
    }

    /// `true` si no hay diff que mostrar ("No changes to display").
    pub fn is_empty(&self) -> bool {
        self.diff().is_empty()
    }

    /// Las líneas del diff, con lo que cambió dentro de cada una ya marcado.
    /// Mientras un diff nuevo se analiza, las del que se ve.
    pub fn lines(&self) -> impl ExactSizeIterator<Item = &DiffLine> {
        self.lines.iter()
    }

    /// Las filas de la vista partida.
    pub fn split_rows(&mut self) -> Arc<Vec<SplitRow>> {
        if self.split_rows.is_none() {
            self.split_rows = Some(Arc::new(to_split_rows(&self.lines)));
        }
        let rows = self.split_rows.clone().unwrap_or_default();
        if self.split_markers.is_none() {
            self.split_markers = Some(Rc::new(split_change_markers(&rows)));
        }
        rows
    }

    /// Las marcas de cambios de la vista actual (la tira junto a la barra de
    /// desplazamiento). Calculadas al analizar el diff: pedirlas no recorre
    /// nada.
    pub fn change_markers(&mut self) -> Rc<Vec<ChangeMarker>> {
        match self.shown_mode {
            DiffViewMode::Unified => self.unified_markers.clone(),
            DiffViewMode::Split => {
                self.split_rows();
                self.split_markers.clone().unwrap_or_default()
            }
        }
    }

    /// Lleva el visor al bloque de cambios de la marca más cercana a
    /// `fraction` de la tira (0 arriba, 1 abajo), como un clic en ella.
    /// Devuelve la fila a la que saltó.
    pub fn jump_to_marker(&mut self, fraction: f32, cx: &mut Context<Self>) -> Option<usize> {
        if self.pending.is_some() {
            return None;
        }
        let markers = self.change_markers();
        let row = marker_at(&markers, fraction)?.row;
        self.reveal_change(row, cx);
        Some(row)
    }

    fn reveal_change(&mut self, row: usize, cx: &mut Context<Self>) {
        self.change_cursor = Some(row);
        self.canvas.reveal(Reveal {
            row,
            column: usize::from(self.shown_mode == DiffViewMode::Split),
            range: 0..0,
            mode: RevealMode::Top,
        });
        cx.notify();
    }

    /// Bloques (`@@`) del diff.
    pub fn hunk_count(&self) -> usize {
        self.hunk_count
    }

    /// Cifras que reserva el medianil: las del número de línea más alto, y
    /// nunca menos de dos.
    pub fn gutter_digits(&self) -> usize {
        self.gutter_digits
    }

    /// Filas de la vista actual.
    pub fn row_count(&mut self) -> usize {
        match self.shown_mode {
            DiffViewMode::Unified => self.lines.len(),
            DiffViewMode::Split => self.split_rows().len(),
        }
    }

    /// Las filas que se dibujaron en el último frame: solo las que caben en la
    /// ventana del visor.
    pub fn painted_rows(&self) -> Range<usize> {
        self.canvas.painted_rows()
    }

    /// Textos (celdas y números) a los que se tuvo que dar forma porque no
    /// estaban en la caché del lienzo. Para los tests de rendimiento.
    #[doc(hidden)]
    pub fn shaped_text_count(&self) -> usize {
        self.canvas.shape_misses()
    }

    /// Alto total del contenido con las medidas del último frame, para un
    /// contenedor que quiera ajustarse a él (el diálogo de un stash).
    pub fn content_height(&self) -> Option<Pixels> {
        self.layout
            .as_ref()
            .map(|(_, layout)| layout.rows.total() + px(2.))
    }

    /// Los trozos que se pintan de la línea `index` de la vista unificada: su
    /// color de sintaxis y si son lo que cambió.
    pub fn line_segments(&self, index: usize) -> Vec<LineSegment> {
        self.lines
            .get(index)
            .map(|line| build_line_segments(line, self.language))
            .unwrap_or_default()
    }

    /// La línea (de la versión nueva del archivo) con la que abrirlo en un
    /// editor: la del principio de la selección o, sin selección, la primera
    /// fila que se ve. `None` si el diff no tiene ninguna o si se está
    /// analizando otro (lo que se ve es del diff anterior).
    pub fn editor_line(&mut self) -> Option<u32> {
        if self.pending.is_some() {
            return None;
        }
        let from = match self.canvas.selection() {
            Some(selection) => selection.start().row,
            None => self.painted_rows().start,
        };
        let numbers: Vec<Option<u32>> = match self.shown_mode {
            DiffViewMode::Unified => self.lines().map(|line| line.new_num).collect(),
            DiffViewMode::Split => self.split_rows().iter().map(|row| row.right.num).collect(),
        };
        crate::editor_line::nearest_new_line(&numbers, from)
    }

    /// El texto seleccionado, tal como se copiaría.
    pub fn selected_text(&self) -> Option<String> {
        canvas::selected_text(self)
    }

    /// Selecciona todo el texto (en la vista partida, el del lado nuevo).
    pub fn select_all(&mut self, cx: &mut Context<Self>) {
        let last = self.row_count().saturating_sub(1);
        let column = usize::from(self.shown_mode == DiffViewMode::Split);
        self.canvas.select(canvas::Selection {
            column,
            anchor: canvas::TextPosition { row: 0, offset: 0 },
            head: canvas::TextPosition {
                row: last,
                offset: usize::MAX,
            },
        });
        cx.notify();
    }

    /// Pide aplicar la acción principal al bloque `index`, como el clic en
    /// su botón.
    pub fn apply_hunk(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(action) = self.hunk_action else {
            return;
        };
        self.apply_hunk_action(index, action.primary(), cx);
    }

    /// Pide aplicar `action` al bloque `index`, si es una de las que se
    /// ofrecen.
    pub fn apply_hunk_action(&mut self, index: usize, action: PatchAction, cx: &mut Context<Self>) {
        // Con otro diff analizándose, los bloques a la vista ya no son los del
        // diff pedido.
        if self.pending.is_some() || !self.hunk_buttons || !self.patch_actions().contains(&action) {
            return;
        }
        cx.emit(DiffViewerEvent::ApplyHunk(HunkRequest {
            index,
            action,
            patch: extract_hunk_patch(&self.diff, index),
        }));
    }

    /// Lo que se ofrece sobre los bloques y las líneas elegidas.
    pub fn patch_actions(&self) -> &'static [PatchAction] {
        self.hunk_action.map_or(&[], HunkAction::patch_actions)
    }

    /// Si se pueden elegir líneas desde el medianil: solo en la vista
    /// unificada, con alguna acción de bloque y sin otro diff analizándose.
    pub fn selects_lines(&self) -> bool {
        self.shown_mode == DiffViewMode::Unified
            && self.hunk_action.is_some()
            && self.pending.is_none()
    }

    /// Elige las filas `rows` de la vista unificada, como un arrastre por el
    /// medianil. Una sola cabecera `@@` elige su bloque entero.
    pub fn select_lines(&mut self, rows: RangeInclusive<usize>, cx: &mut Context<Self>) {
        if !self.selects_lines() || self.lines.is_empty() {
            return;
        }
        let last = self.lines.len() - 1;
        self.canvas
            .set_line_selection(Some(((*rows.start()).min(last), (*rows.end()).min(last))));
        cx.notify();
    }

    // ------------------------------------------------------------------
    // Buscar en el diff (`mod+f` con el foco en el visor)
    // ------------------------------------------------------------------

    /// Abre la barra de búsqueda (o vuelve a su campo), con lo seleccionado
    /// como consulta si es una sola línea.
    pub fn open_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        find::open(self, window, cx);
    }

    /// Cierra la barra y devuelve el foco al visor.
    pub fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        find::close(self, window, cx);
    }

    /// Siguiente coincidencia (Enter), dando la vuelta.
    pub fn find_next(&mut self, cx: &mut Context<Self>) {
        find::navigate(self, true, cx);
    }

    /// Coincidencia anterior (Shift+Enter).
    pub fn find_previous(&mut self, cx: &mut Context<Self>) {
        find::navigate(self, false, cx);
    }

    /// Cómo va la búsqueda: si la barra está abierta, la consulta, las
    /// coincidencias y cuál es la actual.
    pub fn find_status(&self, cx: &App) -> FindStatus {
        FindStatus::of(&self.find, cx)
    }

    // ------------------------------------------------------------------
    // Bloques de cambios (F7 / Shift+F7, Alt+↓ / Alt+↑)
    // ------------------------------------------------------------------

    /// Primera fila de cada bloque de altas y bajas seguidas, en la vista
    /// actual. Se calcula al navegar (no al pintar) y se guarda por
    /// documento.
    pub fn change_starts(&mut self) -> Rc<Vec<usize>> {
        if let Some((generation, starts)) = &self.change_starts
            && *generation == self.document_generation
        {
            return starts.clone();
        }
        let changed: Vec<bool> = match self.shown_mode {
            DiffViewMode::Unified => self
                .lines
                .iter()
                .map(|line| line.kind.is_change())
                .collect(),
            DiffViewMode::Split => {
                let is_change =
                    |kind: SplitCellType| matches!(kind, SplitCellType::Add | SplitCellType::Del);
                self.split_rows()
                    .iter()
                    .map(|row| is_change(row.left.kind) || is_change(row.right.kind))
                    .collect()
            }
        };
        let starts: Vec<usize> = changed
            .iter()
            .enumerate()
            .filter(|(row, change)| **change && (*row == 0 || !changed[row - 1]))
            .map(|(row, _)| row)
            .collect();
        let starts = Rc::new(starts);
        self.change_starts = Some((self.document_generation, starts.clone()));
        starts
    }

    /// Lleva el visor al siguiente bloque de cambios (dando la vuelta) y
    /// devuelve su primera fila.
    pub fn next_change(&mut self, cx: &mut Context<Self>) -> Option<usize> {
        self.jump_to_change(true, cx)
    }

    /// Lleva el visor al bloque de cambios anterior.
    pub fn previous_change(&mut self, cx: &mut Context<Self>) -> Option<usize> {
        self.jump_to_change(false, cx)
    }

    fn jump_to_change(&mut self, forward: bool, cx: &mut Context<Self>) -> Option<usize> {
        // Con otro diff analizándose, lo que se ve ya no es lo pedido.
        if self.pending.is_some() {
            return None;
        }
        let starts = self.change_starts();
        let painted = self.painted_rows();
        // Desde el último bloque al que se saltó si sigue a la vista; si no,
        // desde lo que se ve.
        let cursor = self.change_cursor.filter(|row| painted.contains(row));
        let target = if forward {
            let after = cursor.map_or(painted.start as isize - 1, |row| row as isize);
            starts
                .iter()
                .copied()
                .find(|row| *row as isize > after)
                .or_else(|| starts.first().copied())
        } else {
            let before = cursor.unwrap_or(painted.start);
            starts
                .iter()
                .rev()
                .copied()
                .find(|row| *row < before)
                .or_else(|| starts.last().copied())
        }?;
        self.reveal_change(target, cx);
        Some(target)
    }

    pub fn clear_line_selection(&mut self, cx: &mut Context<Self>) {
        if self.canvas.line_selection().is_some() {
            self.canvas.set_line_selection(None);
            cx.notify();
        }
    }

    /// Las filas que cubre la selección de líneas, ya en orden.
    fn line_selection_rows(&self) -> Option<RangeInclusive<usize>> {
        if !self.selects_lines() {
            return None;
        }
        let (anchor, head) = self.canvas.line_selection()?;
        let (start, end) = (anchor.min(head), anchor.max(head));
        let end = end.min(self.lines.len().checked_sub(1)?);
        // Un clic en el medianil de una cabecera elige todo su bloque.
        if start == end
            && let Some(hunk) = self.hunks.get(start)
            && hunk.index.is_some()
        {
            return Some(start..=hunk.end);
        }
        Some(start..=end)
    }

    fn is_line_selected(&self, row: usize) -> bool {
        self.lines
            .get(row)
            .is_some_and(|line| line.kind.is_change())
            && self
                .line_selection_rows()
                .is_some_and(|rows| rows.contains(&row))
    }

    /// Las altas y bajas elegidas desde el medianil, como posiciones en
    /// [`Self::lines`].
    pub fn selected_lines(&self) -> Vec<usize> {
        self.line_selection_rows()
            .map(|rows| {
                rows.filter(|row| self.lines[*row].kind.is_change())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Pide aplicar `action` a las líneas elegidas, como el clic en su botón.
    pub fn apply_lines(&mut self, action: PatchAction, cx: &mut Context<Self>) {
        if self.pending.is_some() || !self.patch_actions().contains(&action) {
            return;
        }
        let lines = self.selected_lines();
        if lines.is_empty() {
            return;
        }
        let patch = extract_lines_patch(&self.diff, &lines, action.direction());
        cx.emit(DiffViewerEvent::ApplyLines(LinesRequest {
            action,
            lines,
            patch,
        }));
    }

    /// La fila sobre la que va la barra de las líneas elegidas: la primera
    /// elegida que esté a la vista.
    fn lines_bar_row(&self) -> Option<usize> {
        let rows = self.line_selection_rows()?;
        let visible = self.canvas.visible_rows();
        let from = (*rows.start()).max(visible.start);
        let to = (*rows.end()).min(visible.end.saturating_sub(1));
        (from..=to).find(|row| self.lines[*row].kind.is_change())
    }

    fn colors(&self, cx: &App) -> CodeColors {
        self.colors.unwrap_or_else(|| CodeColors::new(cx))
    }

    /// `calc(Nch + 0.6rem)`.
    fn gutter_width(&self, metrics: &Metrics) -> Pixels {
        metrics.ch * self.gutter_digits as f32 + metrics.rems(0.6)
    }

    fn unified_line(&mut self, ix: usize) -> Option<Rc<LineText>> {
        if let Some(cached) = self.unified_cache.get(ix)?.clone() {
            return Some(cached);
        }
        let line = self.lines.get(ix)?;
        let code = !matches!(line.kind, DiffLineType::Hunk | DiffLineType::Note);
        let text = Rc::new(prepare_line(
            &line.content,
            line.inline,
            self.language,
            code,
        ));
        self.unified_cache[ix] = Some(text.clone());
        Some(text)
    }

    fn split_cell(&mut self, row: usize, side: usize, cell: &SplitCell) -> Rc<LineText> {
        if let Some(cached) = self.split_cache.get(&(row, side)) {
            return cached.clone();
        }
        let code = !matches!(
            cell.kind,
            SplitCellType::Hunk | SplitCellType::Note | SplitCellType::Empty
        );
        let text = Rc::new(prepare_line(
            &cell.content,
            cell.inline,
            self.language,
            code,
        ));
        self.split_cache.insert((row, side), text.clone());
        text
    }

    /// Ancho de los botones de bloque: texto de 11px más su relleno, su borde
    /// y la separación entre ellos.
    fn action_width(&mut self, window: &mut Window, cx: &App) -> Pixels {
        let key = (self.hunk_action, cx.theme().font_family.clone());
        if let Some((cached, width)) = &self.action_width
            && *cached == key
        {
            return *width;
        }
        let actions = self.patch_actions();
        let gaps = px(HUNK_BUTTON_GAP) * actions.len().saturating_sub(1) as f32;
        let width = actions
            .iter()
            .map(|action| self.button_width(action.hunk_label(), window, cx))
            .fold(gaps, |total, width| total + width);
        self.action_width = Some((key, width));
        width
    }

    fn button_width(&self, label: &'static str, window: &mut Window, cx: &App) -> Pixels {
        let label = SharedString::from(label);
        let run = TextRun {
            len: label.len(),
            font: Font {
                weight: FontWeight::MEDIUM,
                ..gpui_kit::font(cx.theme().font_family.clone())
            },
            color: Default::default(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let text = window
            .text_system()
            .shape_line(label, px(HUNK_BUTTON_TEXT), &[run], None);
        text.width().ceil() + px(HUNK_BUTTON_PADDING * 2. + 2.)
    }

    fn unified_layout(&mut self, metrics: &Metrics, window: &mut Window, cx: &App) -> GridLayout {
        let numbers = self.gutter_width(metrics) * 2.;
        let line_width = |columns: usize| numbers + metrics.ch * columns as f32 + px(16.);
        // Crece con la línea más larga para que todas las filas midan lo
        // mismo: si no, el fondo de una línea corta se cortaría al desplazar.
        let mut width = line_width(self.max_columns);
        if self.hunk_action.is_some() && self.hunk_buttons && self.hunk_count() > 0 {
            width = width.max(line_width(self.max_hunk_columns) + self.action_width(window, cx));
        }
        // La estimación por columnas se queda corta con caracteres anchos.
        width = width.max((self.canvas.measured_width() + px(8.)).ceil());
        // Nunca más angosta que el panel: los fondos de las líneas y de las
        // cabeceras de hunk llegan hasta el borde derecho, como en el visor de
        // archivos, aunque todas las líneas sean cortas.
        width = width.max(metrics.viewport.width);

        let hunk_height = metrics.line_height + px(2.);
        let lines = &self.lines;
        let key = RowsKey::new(self.shown_mode, metrics, None);
        let rows = cached_rows(&mut self.rows, key, || {
            RowHeights::from_heights(lines.iter().map(|line| {
                if line.kind == DiffLineType::Hunk {
                    hunk_height
                } else {
                    metrics.line_height
                }
            }))
        });
        GridLayout {
            rows,
            scroll_width: width,
            row_width: width,
            sticky_width: numbers,
        }
    }

    /// Ancho de cada mitad y ancho al que se parte su texto.
    fn split_geometry(&self, metrics: &Metrics) -> (Pixels, Pixels) {
        let pane = metrics.viewport.width / 2.;
        let wrap = (pane - px(1.) - self.gutter_width(metrics) - px(8.)).max(metrics.ch);
        (pane, wrap)
    }

    fn split_layout(&mut self, metrics: &Metrics, window: &mut Window) -> GridLayout {
        let split_rows = self.split_rows();
        let (_, wrap) = self.split_geometry(metrics);
        // Partir cada fila en renglones es lo caro: una vez por ancho.
        let key = RowsKey::new(self.shown_mode, metrics, Some(wrap));
        let rows = cached_rows(&mut self.rows, key, || {
            let mut measure = WrapMeasure::new(metrics, window);
            let mut lines_of = |cell: &SplitCell| match expand_tabs(&cell.content) {
                Some(display) => measure.line_count(&display, wrap),
                None => measure.line_count(&cell.content, wrap),
            };
            RowHeights::from_heights(split_rows.iter().map(|row| {
                let lines = lines_of(&row.left).max(lines_of(&row.right));
                let borders = if row.left.kind == SplitCellType::Hunk {
                    px(2.)
                } else {
                    Pixels::ZERO
                };
                metrics.line_height * lines as f32 + borders
            }))
        });

        GridLayout {
            rows,
            scroll_width: metrics.viewport.width,
            row_width: metrics.viewport.width,
            sticky_width: Pixels::ZERO,
        }
    }

    fn unified_row(
        &mut self,
        ix: usize,
        hovered: bool,
        layout: &GridLayout,
        metrics: &Metrics,
        cx: &App,
    ) -> RowSpec {
        let colors = self.colors(cx);
        let Some(text) = self.unified_line(ix) else {
            return RowSpec::default();
        };
        let line = &self.lines[ix];
        let kind = line.kind;
        let is_hunk = kind == DiffLineType::Hunk;
        let gutter = self.gutter_width(metrics);
        let numbers = gutter * 2.;
        let one = px(1.);
        let zero = Pixels::ZERO;

        let mut spec = RowSpec::default();

        // `.diff-line:hover` pesa más que el color de alta, baja o cabecera.
        let background = if hovered {
            Some(colors.hover)
        } else {
            match kind {
                DiffLineType::Add => Some(colors.addition),
                DiffLineType::Del => Some(colors.deletion),
                DiffLineType::Hunk => Some(colors.hunk),
                _ => None,
            }
        };
        if let Some(background) = background {
            spec.fills
                .push(Fill::new(zero..layout.row_width, background));
        }

        // El medianil se queda a la vista al desplazar a la derecha. Señal: el
        // de las líneas añadidas o quitadas va algo más saturado, con los
        // números en el color del signo; sin filetes entre columnas.
        // Una línea elegida desde el medianil se marca como las filas
        // seleccionadas de Señal: medianil en tinta, números en el color de su
        // signo sobre la tinta y marca naranja de 2px a la izquierda.
        let line_selected = self.is_line_selected(ix);
        let (gutter_fill, number_color) = match kind {
            DiffLineType::Add if line_selected => (colors.selection, colors.selection_add),
            DiffLineType::Del if line_selected => (colors.selection, colors.selection_del),
            DiffLineType::Add => (colors.addition_gutter, colors.addition_sign),
            DiffLineType::Del => (colors.deletion_gutter, colors.deletion_sign),
            DiffLineType::Hunk => (colors.hunk, colors.gutter_text),
            _ => (colors.gutter, colors.gutter_text),
        };
        spec.fills
            .push(Fill::new(zero..numbers, gutter_fill).sticky());
        if line_selected {
            spec.fills
                .push(Fill::new(zero..px(2.), colors.selection_mark).sticky());
        }
        if is_hunk {
            for edge in [FillEdge::Top(one), FillEdge::Bottom(one)] {
                spec.fills
                    .push(Fill::new(zero..layout.row_width, colors.border).edge(edge));
                spec.fills
                    .push(Fill::new(zero..numbers, colors.border).edge(edge).sticky());
            }
        }

        let top = if is_hunk { one } else { zero };
        let number = |value: Option<u32>, right: Pixels| {
            value.map(|value| Label {
                text: value.to_string().into(),
                x: zero..right,
                top,
                line_height: metrics.line_height,
                align: LabelAlign::Right,
                color: number_color,
                font_size: metrics.font_size,
                sticky: true,
                truncate: false,
            })
        };
        spec.labels
            .extend(number(line.old_num, gutter - one - px(4.)));
        spec.labels
            .extend(number(line.new_num, numbers - one - px(4.)));

        let len = text.display.len();
        let (runs, mark) = match kind {
            DiffLineType::Hunk => (
                plain_runs(len, &medium(&metrics.font), colors.hunk_text),
                None,
            ),
            DiffLineType::Note => (
                plain_runs(len, &italic(&metrics.font), colors.muted_foreground),
                None,
            ),
            DiffLineType::Add => (
                syntax_runs(
                    len,
                    &text.tokens,
                    &metrics.font,
                    colors.text,
                    &colors.syntax,
                ),
                Some(colors.addition_mark),
            ),
            DiffLineType::Del => (
                syntax_runs(
                    len,
                    &text.tokens,
                    &metrics.font,
                    colors.text,
                    &colors.syntax,
                ),
                Some(colors.deletion_mark),
            ),
            DiffLineType::Ctx => (
                syntax_runs(
                    len,
                    &text.tokens,
                    &metrics.font,
                    colors.text,
                    &colors.syntax,
                ),
                None,
            ),
        };
        let mut marks: Vec<(Range<usize>, gpui_kit::Hsla)> =
            text.inline.clone().zip(mark).into_iter().collect();
        marks.extend(
            self.find
                .marks(ix, 0, &line.content, find::match_colors(cx)),
        );
        spec.cells.push(Cell {
            column: 0,
            x: numbers + px(8.),
            top,
            wrap_width: None,
            text: text.display.clone(),
            runs,
            marks,
        });

        spec
    }

    fn split_row(&mut self, ix: usize, metrics: &Metrics, cx: &App) -> RowSpec {
        let colors = self.colors(cx);
        let rows = self.split_rows();
        let Some(row) = rows.get(ix) else {
            return RowSpec::default();
        };
        let gutter = self.gutter_width(metrics);
        let (pane, wrap) = self.split_geometry(metrics);
        let one = px(1.);
        let mut spec = RowSpec::default();

        for (side, cell) in [&row.left, &row.right].into_iter().enumerate() {
            let left = pane * side as f32;
            let right = left + pane;
            let is_hunk = cell.kind == SplitCellType::Hunk;

            // Verde/rojo por convención de git, a baja opacidad, con una guía
            // lateral que marca el signo sin teñir toda la línea.
            match cell.kind {
                SplitCellType::Add => {
                    spec.fills.push(Fill::new(left..right, colors.addition));
                    spec.fills
                        .push(Fill::new(left..left + px(2.), colors.addition_guide));
                }
                SplitCellType::Del => {
                    spec.fills.push(Fill::new(left..right, colors.deletion));
                    spec.fills
                        .push(Fill::new(left..left + px(2.), colors.deletion_guide));
                }
                SplitCellType::Hunk => spec.fills.push(Fill::new(left..right, colors.hunk)),
                SplitCellType::Empty => spec.fills.push(Fill::new(left..right, colors.empty)),
                SplitCellType::Ctx | SplitCellType::Note => {}
            }
            let (gutter_fill, number_color) = match cell.kind {
                SplitCellType::Add => (colors.addition_gutter, colors.addition_sign),
                SplitCellType::Del => (colors.deletion_gutter, colors.deletion_sign),
                SplitCellType::Hunk => (colors.hunk, colors.gutter_text),
                SplitCellType::Empty => (colors.empty, colors.gutter_text),
                _ => (colors.split_gutter, colors.gutter_text),
            };
            spec.fills.push(Fill::new(left..left + gutter, gutter_fill));
            spec.fills
                .push(Fill::new(right - one..right, colors.border));
            if is_hunk {
                for edge in [FillEdge::Top(one), FillEdge::Bottom(one)] {
                    spec.fills
                        .push(Fill::new(left..right, colors.border).edge(edge));
                }
            }

            let top = if is_hunk { one } else { Pixels::ZERO };
            if let Some(number) = cell.num {
                spec.labels.push(Label {
                    text: number.to_string().into(),
                    x: left..left + gutter - one - px(4.),
                    top,
                    line_height: metrics.line_height,
                    align: LabelAlign::Right,
                    color: number_color,
                    font_size: metrics.font_size,
                    sticky: false,
                    truncate: false,
                });
            }
            if cell.kind == SplitCellType::Empty {
                continue;
            }

            let text = self.split_cell(ix, side, cell);
            let len = text.display.len();
            let (runs, mark) = match cell.kind {
                SplitCellType::Hunk => (
                    plain_runs(len, &medium(&metrics.font), colors.hunk_text),
                    None,
                ),
                SplitCellType::Note => (
                    plain_runs(len, &italic(&metrics.font), colors.muted_foreground),
                    None,
                ),
                SplitCellType::Add => (
                    syntax_runs(
                        len,
                        &text.tokens,
                        &metrics.font,
                        colors.text,
                        &colors.syntax,
                    ),
                    Some(colors.addition_mark),
                ),
                SplitCellType::Del => (
                    syntax_runs(
                        len,
                        &text.tokens,
                        &metrics.font,
                        colors.text,
                        &colors.syntax,
                    ),
                    Some(colors.deletion_mark),
                ),
                _ => (
                    syntax_runs(
                        len,
                        &text.tokens,
                        &metrics.font,
                        colors.text,
                        &colors.syntax,
                    ),
                    None,
                ),
            };
            let mut marks: Vec<(Range<usize>, gpui_kit::Hsla)> =
                text.inline.clone().zip(mark).into_iter().collect();
            marks.extend(
                self.find
                    .marks(ix, side, &cell.content, find::match_colors(cx)),
            );
            spec.cells.push(Cell {
                column: side,
                x: left + gutter + px(4.),
                top,
                wrap_width: Some(wrap),
                text: text.display.clone(),
                runs,
                marks,
            });
        }

        spec
    }

    /// Botón pequeño de Señal (`.hunk .hb`): el de las cabeceras de bloque y
    /// el de la barra de las líneas elegidas.
    fn small_button(
        &self,
        id: ElementId,
        label: &'static str,
        tooltip: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
        on_click: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> AnyElement {
        let palette = *cx.palette();
        let focus = window
            .use_keyed_state(id.clone(), cx, |_, cx| cx.focus_handle())
            .read(cx)
            .clone();
        let focused = focus.is_focused(window);

        BaseButton::new(id)
            .track_focus(&focus)
            .accessibility_label(label)
            .flex()
            .flex_shrink_0()
            .items_center()
            .h(px(HUNK_BUTTON_HEIGHT))
            .px(px(HUNK_BUTTON_PADDING))
            .border_1()
            .border_color(if focused {
                palette.border_ink
            } else {
                palette.border_base
            })
            .radius(Radius::Xs, cx)
            .bg(palette.bg_base)
            .text_color(palette.text_1)
            .font_family(cx.theme().font_family.clone())
            .text_size(px(HUNK_BUTTON_TEXT))
            .font_weight(FontWeight::MEDIUM)
            .whitespace_nowrap()
            .cursor_pointer()
            .hover(move |style| style.bg(palette.bg_hover))
            .active(move |style| style.bg(palette.bg_pressed))
            .tooltip(mergeon_ui::tooltip(tooltip))
            .on_click(cx.listener(move |this, _, _, cx| on_click(this, cx)))
            .child(label)
            .into_any_element()
    }

    /// "Discard hunk" y "Stage hunk" (o solo "Unstage hunk"), a la derecha de
    /// la cabecera del bloque (Señal: `.hunk .hb`, botones pequeños siempre
    /// visibles). El principal conserva el id `("hunk-action", índice)`.
    fn hunk_buttons(
        &self,
        index: usize,
        action: HunkAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let buttons: Vec<AnyElement> = action
            .patch_actions()
            .iter()
            .map(|&patch_action| {
                let id: ElementId = if patch_action == action.primary() {
                    ("hunk-action", index).into()
                } else {
                    ("hunk-discard", index).into()
                };
                self.small_button(
                    id,
                    patch_action.hunk_label(),
                    patch_action.hunk_tooltip(),
                    window,
                    cx,
                    move |this, cx| this.apply_hunk_action(index, patch_action, cx),
                )
            })
            .collect();
        div()
            .flex()
            .items_center()
            .gap(px(HUNK_BUTTON_GAP))
            .children(buttons)
            .into_any_element()
    }

    /// La barra de las líneas elegidas: "Discard lines" y "Stage lines" (o
    /// "Unstage lines"), sobre la primera elegida que se vea. Ids de sus
    /// botones: `lines-discard`, `lines-stage`, `lines-unstage`.
    fn lines_bar(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let buttons: Vec<AnyElement> = self
            .patch_actions()
            .iter()
            .map(|&action| {
                self.small_button(
                    match action {
                        PatchAction::Stage => "lines-stage",
                        PatchAction::Unstage => "lines-unstage",
                        PatchAction::Discard => "lines-discard",
                    }
                    .into(),
                    action.lines_label(),
                    action.lines_tooltip(),
                    window,
                    cx,
                    move |this, cx| this.apply_lines(action, cx),
                )
            })
            .collect();
        div()
            .id("lines-bar")
            .flex()
            .items_center()
            .gap(px(HUNK_BUTTON_GAP))
            .children(buttons)
            .into_any_element()
    }
}

impl CanvasDelegate for DiffViewer {
    fn canvas(&self) -> &CanvasState {
        &self.canvas
    }

    fn canvas_mut(&mut self) -> &mut CanvasState {
        &mut self.canvas
    }

    fn layout(
        &mut self,
        metrics: &Metrics,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> GridLayout {
        self.colors = Some(CodeColors::new(cx));
        let key = LayoutKey {
            mode: self.shown_mode,
            action: self.hunk_action,
            hunk_buttons: self.hunk_buttons,
            metrics: metrics.clone(),
            measured_width: self.canvas.measured_width(),
        };
        if let Some((cached_key, layout)) = &self.layout
            && *cached_key == key
        {
            return layout.clone();
        }

        let layout = match self.shown_mode {
            DiffViewMode::Unified => self.unified_layout(metrics, window, cx),
            DiffViewMode::Split => self.split_layout(metrics, window),
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
        match self.shown_mode {
            DiffViewMode::Unified => self.unified_row(ix, hovered, layout, metrics, cx),
            // La vista partida no resalta la fila bajo el cursor.
            DiffViewMode::Split => self.split_row(ix, metrics, cx),
        }
    }

    fn row_action(
        &mut self,
        ix: usize,
        _hovered: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.shown_mode != DiffViewMode::Unified {
            return None;
        }
        let action = self.hunk_action?;
        // Con otro diff analizándose, lo que se ve no admite acciones.
        if self.pending.is_some() {
            return None;
        }
        if let Some(index) = self.hunks.get(ix)?.index {
            return self
                .hunk_buttons
                .then(|| self.hunk_buttons(index, action, window, cx));
        }
        (self.lines_bar_row() == Some(ix)).then(|| self.lines_bar(window, cx))
    }

    fn selects_lines(&self) -> bool {
        DiffViewer::selects_lines(self)
    }

    /// `S` / `U` / `D` sobre las líneas elegidas; `Esc` cierra la búsqueda.
    fn key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let keystroke = &event.keystroke;
        let modifiers = keystroke.modifiers;
        if keystroke.key == "escape" && !modifiers.modified() && self.find.is_open() {
            find::close(self, window, cx);
            return true;
        }
        // Shift sí vale: `S`, `U` y `D` en mayúscula hacen lo mismo (las
        // pistas las enseñan así).
        if modifiers.control || modifiers.alt || modifiers.platform || modifiers.function {
            return false;
        }
        let Some(&action) = self
            .patch_actions()
            .iter()
            .find(|action| keystroke.key.eq_ignore_ascii_case(action.key()))
        else {
            return false;
        };
        if self.selected_lines().is_empty() {
            return false;
        }
        self.apply_lines(action, cx);
        true
    }

    fn copy_text(&self, row: usize, column: usize, display: Range<usize>) -> Option<String> {
        let content: &str = match self.shown_mode {
            DiffViewMode::Unified => &self.lines.get(row)?.content,
            DiffViewMode::Split => {
                let row = self.split_rows.as_ref()?.get(row)?;
                let cell = if column == 0 { &row.left } else { &row.right };
                if cell.kind == SplitCellType::Empty {
                    return None;
                }
                &cell.content
            }
        };
        content
            .get(source_range(content, display))
            .map(str::to_string)
    }

    fn scrolled(&mut self, cx: &mut Context<Self>) {
        // Mover el diff anterior mientras se analiza otro no es el scroll del
        // diff pedido.
        if self.pending.is_none() {
            cx.emit(DiffViewerEvent::Scrolled(self.scroll_position()));
        }
    }
}

impl Searchable for DiffViewer {
    fn find(&self) -> &FindState {
        &self.find
    }

    fn find_mut(&mut self) -> &mut FindState {
        &mut self.find
    }

    fn find_source(&mut self) -> Option<FindSource> {
        if self.lines.is_empty() {
            return None;
        }
        Some(match self.shown_mode {
            DiffViewMode::Unified => FindSource::Unified(self.lines.clone()),
            DiffViewMode::Split => FindSource::Split(self.split_rows()),
        })
    }

    fn document_generation(&self) -> u64 {
        self.document_generation
    }

    fn source_text(&self, row: usize, column: usize) -> Option<&str> {
        match self.shown_mode {
            DiffViewMode::Unified => self.lines.get(row).map(|line| line.content.as_str()),
            DiffViewMode::Split => {
                let row = self.split_rows.as_ref()?.get(row)?;
                let cell = if column == 0 { &row.left } else { &row.right };
                Some(cell.content.as_str())
            }
        }
    }
}

/// Lo que dice el visor cuando el parche no tiene líneas que pintar.
fn empty_diff_message(diff: &str) -> String {
    let header = |prefix: &str| {
        diff.lines()
            .find_map(|line| line.strip_prefix(prefix))
            .map(str::to_string)
    };
    if let Some(from) = header("rename from ") {
        return format!("Renamed from {from}, without changes to its content.");
    }
    if let Some(from) = header("copy from ") {
        return format!("Copied from {from}, without changes to its content.");
    }
    if let (Some(old), Some(new)) = (header("old mode "), header("new mode ")) {
        return format!("File mode changed from {old} to {new}.");
    }
    if diff.lines().any(|line| line.starts_with("Binary files ")) {
        return "Binary file changed.".to_string();
    }
    "No changes to display".to_string()
}

impl Render for DiffViewer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let radius = Radius::Xl.px(cx);
        // Los fondos de las filas se redondean por dentro del borde.
        self.canvas.set_corner_radius(radius - px(1.));
        let row_count = self.row_count();
        // La tira solo si hay algo que marcar y el diff a la vista es el pedido.
        let markers = Some(self.change_markers())
            .filter(|markers| !markers.is_empty() && self.pending.is_none());

        div()
            .relative()
            .size_full()
            .min_w_0()
            .min_h_0()
            .flex_1()
            .overflow_hidden()
            .bg(palette.card)
            .border_1()
            .border_color(palette.border)
            .rounded(radius)
            .font_family(code_font_family(cx))
            .text_size(rems(0.75))
            .key_context(find::CONTEXT)
            .on_action(cx.listener(|this, _: &FindInViewer, window, cx| {
                this.open_find(window, cx);
            }))
            .on_action(cx.listener(|this, _: &NextChange, _, cx| {
                this.next_change(cx);
            }))
            .on_action(cx.listener(|this, _: &PreviousChange, _, cx| {
                this.previous_change(cx);
            }))
            .map(|this| {
                let slow = self.pending.as_ref().is_some_and(|pending| pending.slow)
                    || self.split_build.as_ref().is_some_and(|build| build.slow);
                let pending = self.pending.is_some();
                // Un parche sin hunks (renombre o cambio de modo sin tocar el
                // contenido) tampoco tiene filas: se dice qué pasó en vez de
                // dejar el panel en blanco.
                if (self.diff.is_empty() || row_count == 0) && !pending {
                    this.child(
                        div()
                            .id("diff-placeholder")
                            .p(rems(2.))
                            .text_center()
                            .text_color(palette.muted_foreground)
                            .child(empty_diff_message(&self.diff)),
                    )
                } else if self.diff.is_empty() {
                    // Nada que atenuar: un indicador discreto, solo si tarda.
                    this.when(slow, |this| this.child(parsing_indicator()))
                } else {
                    // Mientras se analiza otro diff se sigue viendo este; si
                    // tarda, atenuado y con el indicador encima.
                    this.child(
                        div()
                            .relative()
                            .size_full()
                            .child(
                                div()
                                    .flex()
                                    .size_full()
                                    .when(slow, |this| this.opacity(0.45))
                                    .child(div().flex_1().min_w_0().h_full().child(
                                        canvas::surface("diff-rows", row_count, &self.canvas, cx),
                                    ))
                                    .when_some(markers, |this, markers| {
                                        this.child(self.marker_strip(markers, cx))
                                    }),
                            )
                            .when(slow, |this| this.child(parsing_indicator())),
                    )
                }
            })
            .children(find::render_bar(self, cx))
    }
}

impl DiffViewer {
    /// La tira de marcas de cambios a la derecha de la barra de
    /// desplazamiento. Pintar recorre solo las marcas (acotadas, ver
    /// [`crate::change_markers::MARKER_RESOLUTION`]).
    fn marker_strip(&self, markers: Rc<Vec<ChangeMarker>>, cx: &mut Context<Self>) -> AnyElement {
        let palette = *cx.palette();
        let (additions, deletions): (Hsla, Hsla) =
            (palette.diff_add_sign.into(), palette.diff_del_sign.into());
        let strip = self.marker_strip.clone();
        let clicked = self.marker_strip.clone();
        // Un diff más corto que el visor ocupa solo su parte de la tira: las
        // marcas quedan a la altura de sus filas, no estiradas hasta abajo.
        let viewport = self.canvas.scroll.viewport().size.height;
        let fill = match self.content_height() {
            Some(content) if viewport > Pixels::ZERO && content < viewport => content / viewport,
            _ => 1.,
        };
        div()
            .id("change-markers")
            .test_support()
            .flex_shrink_0()
            .w(px(MARKER_STRIP_WIDTH))
            .h_full()
            .border_l_1()
            .border_color(palette.border_hair)
            .cursor_pointer()
            .tooltip(mergeon_ui::tooltip("Changes in this file — click to jump"))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    let bounds: Bounds<Pixels> = clicked.get();
                    if bounds.size.height <= Pixels::ZERO {
                        return;
                    }
                    let fraction = (event.position.y - bounds.top()) / bounds.size.height;
                    this.jump_to_marker(fraction.clamp(0., 1.), cx);
                    cx.stop_propagation();
                }),
            )
            .child(
                paint_canvas(
                    move |bounds, _, _| {
                        let used = Bounds::new(
                            bounds.origin,
                            gpui_kit::size(bounds.size.width, bounds.size.height * fill),
                        );
                        strip.set(used);
                        used
                    },
                    move |_, used, window, _| {
                        paint_markers(&markers, used, additions, deletions, window);
                    },
                )
                .size_full(),
            )
            .into_any_element()
    }
}

/// "Analizando un diff grande": un spinner pequeño arriba a la derecha.
fn parsing_indicator() -> impl IntoElement {
    div()
        .id("diff-parsing")
        .absolute()
        .top(px(8.))
        .right(px(12.))
        .child(
            mergeon_ui::spinner()
                .size(SpinnerSize::Sm)
                .color(SpinnerColor::Primary),
        )
}

#[cfg(test)]
mod empty_diff_message_tests {
    use super::empty_diff_message;

    #[test]
    fn says_what_happened_when_a_patch_has_no_lines() {
        let rename = "diff --git a/src/app.js b/src/entry.js\nsimilarity index 100%\nrename from src/app.js\nrename to src/entry.js\n";
        assert_eq!(
            empty_diff_message(rename),
            "Renamed from src/app.js, without changes to its content."
        );
        let mode = "diff --git a/run.sh b/run.sh\nold mode 100644\nnew mode 100755\n";
        assert_eq!(
            empty_diff_message(mode),
            "File mode changed from 100644 to 100755."
        );
        let binary = "diff --git a/a.png b/a.png\nindex 1..2 100644\nBinary files a/a.png and b/a.png differ\n";
        assert_eq!(empty_diff_message(binary), "Binary file changed.");
        assert_eq!(empty_diff_message(""), "No changes to display");
    }
}
