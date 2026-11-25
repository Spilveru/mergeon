//! Lista del historial (`src/components/repository/CommitHistoryList.vue`):
//! una fila por commit con su grafo, los badges de sus refs, el mensaje y la
//! línea de autor, hash y fecha.
//!
//! El original montaba todas las filas; aquí la lista está virtualizada: solo
//! se construyen las filas a la vista, tenga el historial 200 commits o 20.000.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::scroll::{Scrollbar, ScrollbarAxis};
use gpui_kit::component::{ActiveTheme as _, VirtualListScrollHandle, v_virtual_list};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, AvailableSpace, ClickEvent, Context, Div, EventEmitter, FontWeight, Hsla,
    Pixels, Role, ScrollStrategy, SharedString, Size, Stateful, TextRun, WeakEntity, Window,
    canvas, div, font, point, px, relative, rems, size,
};
use mergeon_model::commit_view_utils::{
    CommitRef, CommitRow, FitRefBadgesOptions, FittedBadges, HistoryCommit as _, RefBadge, RefType,
    badge_title, fit_ref_badges, group_ref_badges, is_head_commit, membership_title,
};
use mergeon_ui::menu::{MenuItem, MenuTriggerExt as _};
use mergeon_ui::theme::with_alpha;
use mergeon_ui::{ActivePaletteExt as _, IconTone, Lucide, Radius, SpinnerColor, SpinnerSize};
use mergeon_view_diff::code_font_family;

use crate::colors::css_color;
use crate::graph::{
    GRAPH_HEIGHT, GraphPaint, WorkingLink, fitted_graph_width, paint_working_changes,
};
use crate::monogram::monogram;

/// Alto fijo de una fila: el grafo se dibuja a escala 1:1 contra este alto,
/// así que la fila no puede crecer con su contenido.
pub const ROW_HEIGHT: f32 = GRAPH_HEIGHT;
/// `--load-more-height`.
const LOAD_MORE_HEIGHT: f32 = 40.;
/// `rootMargin: '120px'` del centinela: la página siguiente se pide cuando
/// faltan estas filas para llegar al final.
const LOAD_MORE_LOOKAHEAD_ROWS: usize = 3;
/// Si el scroll está a menos de esto del principio, el usuario "está en la
/// punta" y sí quiere ver lo que llegue (`FOLLOW_TIP_SCROLL_THRESHOLD`).
const FOLLOW_TIP_SCROLL_THRESHOLD: f32 = 4.;

// Columnas de la fila (Señal: grafo · mensaje · autor · fecha · hash). Autor y
// fecha solo aparecen cuando la lista es lo bastante ancha; el hash, siempre.
const HASH_COLUMN: f32 = 66.;
const DATE_COLUMN: f32 = 86.;
const AUTHOR_COLUMN: f32 = 112.;
const SHOW_DATE_FROM: f32 = 380.;
const SHOW_AUTHOR_FROM: f32 = 520.;
/// Alto de la cabecera de columnas.
const COLUMN_HEADER_HEIGHT: f32 = 24.;
/// Ancho mínimo de la columna del grafo (`Math.max(g.W + 6, 56)` del diseño):
/// con uno o dos carriles el grafo mide 24–38px y la etiqueta "GRAPH" de la
/// cabecera se montaba sobre "MESSAGE".
pub const GRAPH_COLUMN_MIN_WIDTH: f32 = 56.;
/// Sangría de la etiqueta "GRAPH" (`padding-left: 10px` del diseño).
pub const GRAPH_LABEL_INSET: f32 = 10.;
/// Monograma del autor en la columna Author.
const MONOGRAM_SIZE: f32 = 16.;
/// Separación a la derecha de cada columna.
const COLUMN_GAP: f32 = 10.;
/// Caracteres del hash que van en tinta (`c4e1`a07).
const HASH_LEAD: usize = 4;

// Cuántos badges caben en cada fila: el ancho útil sale del ancho de la lista
// menos grafo, columnas y un mínimo para el mensaje, para que una fila con
// muchas ramas siga diciendo algo del commit.
const MESSAGE_MIN_WIDTH: f64 = 8. * 16.;
const ROW_HORIZONTAL_SPACE: f64 = 2. + COLUMN_GAP as f64 + 6.;
/// Pill de ref (mono 11, alto 18): padding de 6 a cada lado, borde, y el
/// cuadradito o ícono con su separación.
const BADGE_ICON: f64 = 11.;
const BADGE_OVERHEAD: f64 = 6. * 2. + 2. + BADGE_ICON + 5.;
/// La cabeza `HEAD` de la pill partida.
const HEAD_SEGMENT_WIDTH: f64 = 40.;
const REMOTE_MARK_WIDTH: f64 = BADGE_ICON + 4.;
const BADGE_GAP: f64 = 4.;
const MORE_CHIP_WIDTH: f64 = 2.2 * 16.;
const BADGE_FONT_SIZE: f32 = 11.;
const PILL_HEIGHT: f32 = 18.;

/// Lo que la lista pide a quien la aloja (`defineEmits`).
#[derive(Debug, Clone, PartialEq)]
pub enum CommitHistoryListEvent {
    /// Clic en una fila (`select-commit`).
    SelectCommit(CommitRow),
    /// Doble clic (`checkout-commit`).
    CheckoutCommit(CommitRow),
    /// El final de la lista está a la vista y hay más historial (`load-more`).
    LoadMore,
    /// Clic en la fila de cambios sin commitear: ir a Local Changes.
    OpenWorkingChanges,
    /// Ctrl/⌘+clic: añadir o quitar el commit de la selección múltiple.
    ToggleCommit(CommitRow),
    /// Shift+clic: seleccionar el rango desde el ancla hasta este commit.
    ExtendSelection(CommitRow),
    /// Doble clic en la etiqueta de una rama (local o remota): checkout de
    /// esa rama, por el mismo flujo que el doble clic en el sidebar.
    CheckoutRef {
        commit: CommitRow,
        name: String,
        kind: RefType,
    },
}

/// Cambios sin commitear del working tree, para la fila que va encima del
/// historial («5 uncommitted changes · 2 staged»).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WorkingChanges {
    /// Archivos con cambios (preparados o no).
    pub total: usize,
    /// Cuántos de ellos tienen algo en el índice.
    pub staged: usize,
}

impl WorkingChanges {
    /// `None` si no hay nada sin commitear.
    pub fn new(total: usize, staged: usize) -> Option<Self> {
        (total > 0).then_some(Self {
            total,
            staged: staged.min(total),
        })
    }

    /// El texto de la fila: «5 uncommitted changes · 2 staged».
    pub fn label(&self) -> String {
        let noun = if self.total == 1 { "change" } else { "changes" };
        format!(
            "{} uncommitted {noun} \u{b7} {} staged",
            self.total, self.staged
        )
    }
}

/// Id de la fila de cambios sin commitear.
pub const WORKING_CHANGES_ROW_ID: &str = "working-changes";

/// Cómo se pinta una fila: las clases `selected`, `is-head`, `not-in-branch`
/// y `to-pull` del original.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RowFlags {
    pub selected: bool,
    /// El commit en el que está el working tree.
    pub is_head: bool,
    /// No está en la rama actual: se pinta apagado.
    pub not_in_branch: bool,
    /// Está en el upstream y falta traerlo: lleva además la marca de pull.
    pub to_pull: bool,
}

impl RowFlags {
    pub fn of(commit: &CommitRow, selected_hash: Option<&str>) -> Self {
        Self {
            selected: selected_hash.is_some_and(|hash| hash == row_key(commit)),
            is_head: is_head_commit(commit),
            not_in_branch: commit.in_current_branch == Some(false),
            to_pull: commit.in_upstream == Some(true),
        }
    }
}

/// Identidad de una fila: el hash completo (o el corto si no hay otro).
pub fn row_key(commit: &CommitRow) -> &str {
    commit.graph_hash()
}

type MenuProvider = Rc<dyn Fn(&CommitRow, &mut Window, &mut App) -> Vec<MenuItem>>;

/// Un commit de la lista sin copiarlo: los manejadores de una fila lo
/// guardan así y solo lo clonan si de verdad se hace clic (antes cada fila
/// a la vista clonaba su `CommitRow` dos o tres veces por frame).
#[derive(Clone)]
struct RowCommit {
    commits: Arc<Vec<CommitRow>>,
    index: usize,
}

impl RowCommit {
    fn get(&self) -> &CommitRow {
        &self.commits[self.index]
    }
}

/// Tope de filas con los badges ya colocados: con un historial de 20.000
/// commits recorrido entero, la tabla no crece sin fin.
const MAX_BADGE_LAYOUTS: usize = 2048;

/// Los badges ya colocados de cada fila, para no medir ni volver a encajar
/// sus nombres en cada frame al desplazar la lista o pasar el ratón.
#[derive(Default)]
struct BadgeLayoutCache {
    /// Ancho disponible para badges (bits del `f64`) y fuente con los que se
    /// calcularon: si cambian, todo lo guardado deja de valer.
    available: u64,
    font: SharedString,
    /// Por [`row_key`], con las refs con las que se calculó: un commit que
    /// llega con otras refs (otra rama, un tag nuevo) se vuelve a calcular.
    layouts: HashMap<String, (Vec<CommitRef>, Rc<FittedBadges<RefBadge>>)>,
}

/// La lista del historial. Componente controlado: quien la aloja le da los
/// commits y la selección con los `set_*` y escucha sus
/// [`CommitHistoryListEvent`].
pub struct CommitHistoryList {
    commits: Arc<Vec<CommitRow>>,
    graph_lane_count: usize,
    selected_commit_hash: Option<String>,
    /// La selección múltiple (vacía si es simple), compartida con la vista.
    multi_selected: Rc<HashSet<String>>,
    is_loading: bool,
    has_more: bool,
    loading_more: bool,
    working_changes: Option<WorkingChanges>,
    context_menu: Option<MenuProvider>,

    scroll: VirtualListScrollHandle,
    item_sizes: Rc<Vec<Size<Pixels>>>,
    /// Ancho de la lista en el último frame (`listWidth`).
    list_width: Pixels,
    /// Ancho de cada nombre de ref ya medido con la fuente de los badges.
    text_widths: RefCell<HashMap<String, f64>>,
    /// Fuente con la que se midieron `text_widths`.
    text_widths_font: RefCell<SharedString>,
    badge_layouts: RefCell<BadgeLayoutCache>,
    /// El usuario estaba mirando la punta al llegar los últimos commits.
    was_at_tip: bool,
    head_commit_hash: Option<String>,
    load_more_requested: bool,
    painted_rows: Range<usize>,
}

impl EventEmitter<CommitHistoryListEvent> for CommitHistoryList {}

/// Tope de anchos de nombres de ref guardados. Con miles de ramas y tags
/// (y repositorios que se van abriendo) la tabla crecía sin fin; medir otra
/// vez un nombre es barato, así que al llenarse se vacía.
const MAX_TEXT_WIDTHS: usize = 4096;

fn remember_text_width(widths: &mut HashMap<String, f64>, text: &str, width: f64) {
    if widths.len() >= MAX_TEXT_WIDTHS && !widths.contains_key(text) {
        widths.clear();
    }
    widths.insert(text.to_string(), width);
}

impl CommitHistoryList {
    pub fn new(_window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self {
            commits: Arc::new(Vec::new()),
            graph_lane_count: 1,
            selected_commit_hash: None,
            multi_selected: Rc::new(HashSet::new()),
            is_loading: false,
            has_more: false,
            loading_more: false,
            working_changes: None,
            context_menu: None,
            scroll: VirtualListScrollHandle::new(),
            item_sizes: Rc::new(Vec::new()),
            list_width: Pixels::ZERO,
            text_widths: RefCell::new(HashMap::new()),
            text_widths_font: RefCell::new(SharedString::default()),
            badge_layouts: RefCell::new(BadgeLayoutCache::default()),
            was_at_tip: true,
            head_commit_hash: None,
            load_more_requested: false,
            painted_rows: 0..0,
        }
    }

    // --- datos (las props del original) -----------------------------------

    /// Los commits a pintar. Al refrescar el historial la lista no se mueve
    /// bajo la vista del usuario: si no estaba arriba del todo, se ancla en la
    /// primera fila que sobrevive al refresco.
    ///
    /// Compartidos con `Arc` para que la vista pueda pasárselos al hilo de
    /// fondo (paginación) sin copiarlos.
    pub fn set_commits(&mut self, commits: Arc<Vec<CommitRow>>, cx: &mut Context<Self>) {
        if Arc::ptr_eq(&self.commits, &commits) || *self.commits == *commits {
            return;
        }

        // `captureScrollAnchor`: con todas las filas montadas, la primera fila
        // con elemento era siempre el primer commit de la lista anterior.
        let scroll_top = self.scroll_top();
        self.was_at_tip = scroll_top <= FOLLOW_TIP_SCROLL_THRESHOLD;
        let anchor = (!self.was_at_tip)
            .then(|| {
                self.commits
                    .first()
                    .map(|commit| row_key(commit).to_string())
            })
            .flatten();

        self.commits = commits;
        self.badge_layouts.borrow_mut().layouts.clear();
        self.rebuild_item_sizes();
        self.load_more_requested = false;

        // `restoreScrollAnchor`
        if let Some(anchor) = anchor
            && let Some(index) = self.index_of(&anchor)
        {
            self.set_scroll_top(scroll_top + index as f32 * ROW_HEIGHT);
        }

        // Al cambiar de rama (o al commitear) la posición nueva puede quedar
        // fuera de la ventana visible: se trae a la vista, sin moverla si ya se
        // veía. Solo si el usuario estaba mirando la punta: leyendo historia
        // vieja, un fetch o un pull no deben devolverlo al principio.
        let head = self
            .commits
            .iter()
            .find(|commit| is_head_commit(*commit))
            .map(|commit| row_key(commit).to_string());
        if head != self.head_commit_hash {
            self.head_commit_hash = head;
            if self.was_at_tip
                && let Some(hash) = self.head_commit_hash.clone()
            {
                self.scroll_row_into_view(&hash);
            }
        }

        cx.notify();
    }

    /// Carriles del grafo de toda la lista: un único ancho para que los
    /// carriles queden alineados entre filas.
    pub fn set_graph_lane_count(&mut self, lane_count: usize, cx: &mut Context<Self>) {
        let lane_count = lane_count.max(1);
        if self.graph_lane_count != lane_count {
            self.graph_lane_count = lane_count;
            cx.notify();
        }
    }

    /// La fila seleccionada (por su [`row_key`]). La selección se trae a la
    /// vista: un salto desde el sidebar o la búsqueda puede apuntar a un
    /// commit fuera de la ventana visible.
    pub fn set_selected_commit_hash(&mut self, hash: Option<&str>, cx: &mut Context<Self>) {
        if self.selected_commit_hash.as_deref() == hash {
            return;
        }
        self.selected_commit_hash = hash.map(str::to_string);
        if let Some(hash) = self.selected_commit_hash.clone() {
            self.scroll_row_into_view(&hash);
        }
        cx.notify();
    }

    /// Los commits de la selección múltiple (por su [`row_key`]); vacío con
    /// selección simple. Se pintan seleccionados además del principal.
    pub fn set_multi_selection(&mut self, keys: Rc<HashSet<String>>, cx: &mut Context<Self>) {
        if Rc::ptr_eq(&self.multi_selected, &keys)
            || (self.multi_selected.is_empty() && keys.is_empty())
        {
            return;
        }
        self.multi_selected = keys;
        cx.notify();
    }

    /// Cuántos commits pinta seleccionados la selección múltiple.
    pub fn multi_selection_len(&self) -> usize {
        self.multi_selected.len()
    }

    /// Primera carga del historial en curso: sin filas, se pinta el spinner.
    pub fn set_loading(&mut self, loading: bool, cx: &mut Context<Self>) {
        if self.is_loading != loading {
            self.is_loading = loading;
            cx.notify();
        }
    }

    pub fn set_has_more(&mut self, has_more: bool, cx: &mut Context<Self>) {
        if self.has_more != has_more {
            self.has_more = has_more;
            self.rebuild_item_sizes();
            cx.notify();
        }
    }

    pub fn set_loading_more(&mut self, loading_more: bool, cx: &mut Context<Self>) {
        if self.loading_more != loading_more {
            self.loading_more = loading_more;
            // Si tras cargar una página el final sigue a la vista (lista más
            // corta que el contenedor), el siguiente frame pide la siguiente.
            if !loading_more {
                self.load_more_requested = false;
            }
            self.rebuild_item_sizes();
            cx.notify();
        }
    }

    /// Cambios sin commitear: con `Some`, una fila más encima del primer
    /// commit (no seleccionable: el clic lleva a Local Changes). Lo que se
    /// estaba viendo no se mueve cuando la fila aparece o desaparece.
    pub fn set_working_changes(&mut self, changes: Option<WorkingChanges>, cx: &mut Context<Self>) {
        if self.working_changes == changes {
            return;
        }
        let lead_before = self.lead_rows();
        self.working_changes = changes;
        let lead_after = self.lead_rows();
        if lead_before != lead_after {
            let scroll_top = self.scroll_top();
            if scroll_top > FOLLOW_TIP_SCROLL_THRESHOLD {
                let shift = (lead_after as f32 - lead_before as f32) * ROW_HEIGHT;
                self.set_scroll_top(scroll_top + shift);
            }
            self.rebuild_item_sizes();
        }
        cx.notify();
    }

    /// Entradas del menú contextual de una fila (`open-context-menu`): se
    /// piden cada vez que el menú se abre.
    pub fn set_context_menu(
        &mut self,
        items: impl Fn(&CommitRow, &mut Window, &mut App) -> Vec<MenuItem> + 'static,
    ) {
        self.context_menu = Some(Rc::new(items));
    }

    // --- lectura ------------------------------------------------------------

    pub fn commits(&self) -> &Arc<Vec<CommitRow>> {
        &self.commits
    }

    pub fn graph_lane_count(&self) -> usize {
        self.graph_lane_count
    }

    pub fn selected_commit_hash(&self) -> Option<&str> {
        self.selected_commit_hash.as_deref()
    }

    pub fn is_loading(&self) -> bool {
        self.is_loading
    }

    pub fn has_more(&self) -> bool {
        self.has_more
    }

    pub fn is_loading_more(&self) -> bool {
        self.loading_more
    }

    pub fn working_changes(&self) -> Option<WorkingChanges> {
        self.working_changes
    }

    /// Ancho de la columna del grafo: el del grafo, pero nunca menos de lo que
    /// pide la etiqueta "GRAPH" de la cabecera.
    pub fn graph_column_width(&self) -> f32 {
        self.graph_width().max(GRAPH_COLUMN_MIN_WIDTH)
    }

    /// Ancho del grafo, igual en todas las filas y limitado a una parte de la
    /// lista (`fitted_graph_width`) para que el texto de la fila siempre quepa.
    pub fn graph_width(&self) -> f32 {
        fitted_graph_width(self.graph_lane_count, f32::from(self.list_width))
    }

    /// El texto del estado sin filas: "Loading commits..." o "No commits found".
    pub fn status_text(&self) -> Option<&'static str> {
        if !self.commits.is_empty() {
            None
        } else if self.is_loading {
            Some("Loading commits...")
        } else {
            Some("No commits found")
        }
    }

    /// Filas construidas en el último frame.
    pub fn painted_rows(&self) -> Range<usize> {
        self.painted_rows.clone()
    }

    /// Desplazamiento vertical, en píxeles desde el principio.
    pub fn scroll_top(&self) -> f32 {
        -f32::from(self.scroll.offset().y)
    }

    pub fn set_scroll_top(&mut self, top: f32) {
        let offset = self.scroll.offset();
        self.scroll.set_offset(point(offset.x, px(-top.max(0.))));
    }

    /// Dónde está (o estaría, si se desplazara hasta ella) la fila de un
    /// commit, en coordenadas de la ventana, según el último frame.
    pub fn row_bounds(&self, hash: &str) -> Option<gpui_kit::Bounds<Pixels>> {
        let index = self.index_of(hash)? + self.lead_rows();
        let viewport = self.scroll.bounds();
        let top = viewport.top() + px(index as f32 * ROW_HEIGHT) + self.scroll.offset().y;
        Some(gpui_kit::Bounds::new(
            point(viewport.left(), top),
            size(viewport.size.width, px(ROW_HEIGHT)),
        ))
    }

    /// Las clases de la fila de ese commit.
    pub fn row_flags(&self, commit: &CommitRow) -> RowFlags {
        let mut flags = RowFlags::of(commit, self.selected_commit_hash.as_deref());
        flags.selected |= self.multi_selected.contains(row_key(commit));
        flags
    }

    /// Los badges que caben en la fila de ese commit con el ancho actual de la
    /// lista, y cuántos quedan resumidos en el chip "+N". `measure_text` da el
    /// ancho en píxeles de un nombre con la fuente de los badges.
    pub fn badge_layout(
        &self,
        commit: &CommitRow,
        measure_text: impl Fn(&str) -> f64,
    ) -> FittedBadges<RefBadge> {
        self.fit_badges(commit, self.badge_space(), measure_text)
    }

    /// [`Self::badge_layout`] recordado por fila: se calcula una vez por
    /// commit y se reutiliza mientras no cambien el ancho disponible, la
    /// fuente (`font`) ni las refs del commit. Es lo que usa cada fila al
    /// pintarse.
    pub fn cached_badge_layout(
        &self,
        commit: &CommitRow,
        font: &SharedString,
        measure_text: impl Fn(&str) -> f64,
    ) -> Rc<FittedBadges<RefBadge>> {
        let available = self.badge_space();
        let mut cache = self.badge_layouts.borrow_mut();
        if cache.available != available.to_bits() || cache.font != *font {
            cache.available = available.to_bits();
            cache.font = font.clone();
            cache.layouts.clear();
        }
        let key = row_key(commit);
        if let Some((refs, layout)) = cache.layouts.get(key)
            && *refs == commit.refs
        {
            return layout.clone();
        }
        let layout = Rc::new(self.fit_badges(commit, available, measure_text));
        if cache.layouts.len() >= MAX_BADGE_LAYOUTS && !cache.layouts.contains_key(key) {
            cache.layouts.clear();
        }
        cache
            .layouts
            .insert(key.to_string(), (commit.refs.clone(), layout.clone()));
        layout
    }

    /// Filas con los badges ya colocados (tests).
    #[doc(hidden)]
    pub fn cached_badge_layouts(&self) -> usize {
        self.badge_layouts.borrow().layouts.len()
    }

    /// Ancho que queda para los badges en una fila.
    fn badge_space(&self) -> f64 {
        let list_width = f64::from(f32::from(self.list_width));
        if list_width > 0. {
            list_width
                - f64::from(self.graph_column_width())
                - f64::from(self.columns_width())
                - ROW_HORIZONTAL_SPACE
                - MESSAGE_MIN_WIDTH
        } else {
            0.
        }
    }

    fn fit_badges(
        &self,
        commit: &CommitRow,
        available: f64,
        measure_text: impl Fn(&str) -> f64,
    ) -> FittedBadges<RefBadge> {
        let badges = group_ref_badges(&commit.refs);
        fit_ref_badges(
            &badges,
            available,
            |badge| {
                BADGE_OVERHEAD
                    + measure_text(&badge.name)
                    + if badge.is_head {
                        HEAD_SEGMENT_WIDTH
                    } else {
                        0.
                    }
                    + badge.remotes.as_ref().map_or(0, Vec::len) as f64 * REMOTE_MARK_WIDTH
            },
            FitRefBadgesOptions {
                gap: BADGE_GAP,
                more_width: MORE_CHIP_WIDTH,
            },
        )
    }

    /// Qué columnas opcionales caben: (autor, fecha).
    fn visible_columns(&self) -> (bool, bool) {
        visible_columns_for(self.list_width)
    }

    /// Ancho de las columnas a la derecha del mensaje.
    fn columns_width(&self) -> f32 {
        let (author, date) = self.visible_columns();
        HASH_COLUMN + if date { DATE_COLUMN } else { 0. } + if author { AUTHOR_COLUMN } else { 0. }
    }

    // --- interno ------------------------------------------------------------

    fn index_of(&self, hash: &str) -> Option<usize> {
        self.commits
            .iter()
            .position(|commit| row_key(commit) == hash)
    }

    fn has_load_more_row(&self) -> bool {
        !self.commits.is_empty() && (self.has_more || self.loading_more)
    }

    /// Filas de la lista virtual antes del primer commit: la de cambios sin
    /// commitear, si la hay. Los índices de commit no cuentan con ella.
    fn lead_rows(&self) -> usize {
        usize::from(self.working_changes.is_some() && !self.commits.is_empty())
    }

    fn rebuild_item_sizes(&mut self) {
        let mut sizes = vec![size(px(0.), px(ROW_HEIGHT)); self.commits.len() + self.lead_rows()];
        if self.has_load_more_row() {
            sizes.push(size(px(0.), px(LOAD_MORE_HEIGHT)));
        }
        self.item_sizes = Rc::new(sizes);
    }

    /// `scrollIntoView({ block: 'nearest' })`.
    fn scroll_row_into_view(&mut self, hash: &str) {
        if let Some(index) = self.index_of(hash) {
            self.scroll
                .scroll_to_item(index + self.lead_rows(), ScrollStrategy::Nearest);
        }
    }

    fn measure_text(&self, text: &str, window: &Window, cx: &App) -> f64 {
        let family = &cx.theme().mono_font_family;
        // Otra fuente de código: los anchos medidos con la anterior no valen.
        if *self.text_widths_font.borrow() != *family {
            *self.text_widths_font.borrow_mut() = family.clone();
            self.text_widths.borrow_mut().clear();
        }
        if let Some(width) = self.text_widths.borrow().get(text) {
            return *width;
        }
        let mut badge_font = font(family.clone());
        badge_font.weight = FontWeight::MEDIUM;
        let line = window.text_system().shape_line(
            SharedString::from(text.to_string()),
            px(BADGE_FONT_SIZE),
            &[TextRun {
                len: text.len(),
                font: badge_font,
                color: Hsla::default(),
                background_color: None,
                underline: None,
                strikethrough: None,
            }],
            None,
        );
        let width = f64::from(f32::from(line.width));
        remember_text_width(&mut self.text_widths.borrow_mut(), text, width);
        width
    }

    fn render_rows(
        &mut self,
        range: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        // Los índices de la lista virtual cuentan la fila de cambios sin
        // commitear; los de los commits, no.
        let lead = self.lead_rows();
        let len = self.commits.len();
        self.painted_rows =
            range.start.saturating_sub(lead).min(len)..range.end.saturating_sub(lead).min(len);

        if self.has_more
            && !self.loading_more
            && !self.load_more_requested
            && range.end + LOAD_MORE_LOOKAHEAD_ROWS >= len + lead
        {
            self.load_more_requested = true;
            cx.emit(CommitHistoryListEvent::LoadMore);
        }

        let link = self.working_link_target(cx);
        let commits = self.commits.clone();
        range
            .map(|index| {
                if index < lead {
                    return self.render_working_changes(link, cx);
                }
                let index = index - lead;
                match commits.get(index) {
                    Some(_) => {
                        let working_link = link.and_then(|(head_index, lane, color)| {
                            match index.cmp(&head_index) {
                                std::cmp::Ordering::Less => Some(WorkingLink::through(lane, color)),
                                std::cmp::Ordering::Equal => {
                                    Some(WorkingLink::into_head(lane, color))
                                }
                                std::cmp::Ordering::Greater => None,
                            }
                        });
                        let commit = RowCommit {
                            commits: commits.clone(),
                            index,
                        };
                        self.render_row(commit, working_link, window, cx)
                    }
                    None => self.render_load_more(cx),
                }
            })
            .collect()
    }

    /// Dónde está HEAD (índice, carril y color de su carril) para unirlo con
    /// la fila de cambios sin commitear. `None` sin esa fila o sin HEAD.
    fn working_link_target(&self, cx: &App) -> Option<(usize, usize, Hsla)> {
        self.working_changes?;
        let index = self.commits.iter().position(is_head_commit)?;
        let commit = &self.commits[index];
        let lane = commit.graph.as_ref().map_or(0, |graph| graph.lane);
        let color = commit
            .graph
            .as_ref()
            .map(|graph| &*graph.color)
            .filter(|color| !color.is_empty())
            .unwrap_or(commit.color.as_str());
        Some((index, lane, css_color(color, cx)))
    }

    /// La fila de cambios sin commitear (Señal `.crow.wip`): texto en cursiva
    /// apagada, «now» en la fecha, «—» en el hash y, en el grafo, el nodo
    /// hueco punteado sobre HEAD.
    fn render_working_changes(
        &self,
        link: Option<(usize, usize, Hsla)>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = *cx.palette();
        let Some(changes) = self.working_changes else {
            return div().into_any_element();
        };
        let label = changes.label();
        let graph_width = self.graph_column_width();
        let (show_author, show_date) = self.visible_columns();
        let mono = code_font_family(cx);
        let card: Hsla = palette.card.into();
        let lit: Hsla = palette.bg_hover.into();
        let node: Hsla = palette.text_3.into();
        let list_bounds = self.scroll.bounds();
        let lane = link.map_or(0, |(_, lane, _)| lane);
        let link = link.map(|(_, lane, color)| WorkingLink::out_of_working_changes(lane, color));
        let column = |width: f32| {
            div()
                .flex_shrink_0()
                .w(px(width))
                .pr(px(COLUMN_GAP))
                .overflow_hidden()
                .truncate()
        };

        div()
            .id(WORKING_CHANGES_ROW_ID)
            .test_support()
            .role(Role::ListItem)
            .aria_label(SharedString::from(label.clone()))
            .relative()
            .flex()
            .flex_shrink_0()
            .items_center()
            .w_full()
            .h(px(ROW_HEIGHT))
            .overflow_hidden()
            .pr(px(2.))
            .whitespace_nowrap()
            .cursor_pointer()
            .hover(move |style| style.bg(lit))
            .tooltip(mergeon_ui::tooltip("Open Local Changes"))
            .on_click(cx.listener(|_, event: &ClickEvent, _, cx| {
                // No es un commit: no entra en la selección múltiple, y un
                // Ctrl/Shift+clic sobre ella no hace nada.
                let modifiers = event.modifiers();
                if event.standard_click() && !modifiers.secondary() && !modifiers.shift {
                    cx.emit(CommitHistoryListEvent::OpenWorkingChanges);
                }
            }))
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _| {
                        let mouse = window.mouse_position();
                        let hovered = mouse.y >= bounds.top()
                            && mouse.y < bounds.bottom()
                            && mouse.x >= list_bounds.left()
                            && mouse.x < list_bounds.right();
                        let surface = if hovered { lit } else { card };
                        paint_working_changes(bounds, link, lane, node, surface, window);
                    },
                )
                .flex_shrink_0()
                .w(px(graph_width))
                .h(px(GRAPH_HEIGHT)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .pr(px(COLUMN_GAP))
                    .truncate()
                    .italic()
                    .text_color(palette.text_2)
                    .child(label),
            )
            .when(show_author, |row| row.child(column(AUTHOR_COLUMN)))
            .when(show_date, |row| {
                row.child(
                    column(DATE_COLUMN)
                        .font_family(mono.clone())
                        .text_size(px(11.5))
                        .text_color(palette.text_3)
                        .child("now"),
                )
            })
            .child(
                column(HASH_COLUMN)
                    .font_family(mono)
                    .text_size(px(11.5))
                    .text_color(palette.text_3)
                    .child("\u{2014}"),
            )
            .into_any_element()
    }

    fn render_load_more(&self, cx: &App) -> AnyElement {
        let palette = cx.palette();
        div()
            .flex()
            .items_center()
            .justify_center()
            .gap(rems(0.45))
            .h(px(LOAD_MORE_HEIGHT))
            .w_full()
            .text_size(rems(0.8125))
            .text_color(palette.muted_foreground)
            .when(self.loading_more, |row| {
                row.child(
                    mergeon_ui::spinner()
                        .size(SpinnerSize::Sm)
                        .color(SpinnerColor::Primary),
                )
                .child("Loading more…")
            })
            .into_any_element()
    }

    fn render_row(
        &self,
        row_commit: RowCommit,
        working_link: Option<WorkingLink>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let commit = row_commit.get();
        let palette = *cx.palette();
        let flags = self.row_flags(commit);
        let key = row_key(commit).to_string();
        let graph_width = self.graph_column_width();
        let selected = flags.selected;

        // El fondo del nodo de merge y del anillo de HEAD sigue al de la fila.
        let card: Hsla = palette.card.into();
        let lit: Hsla = palette.bg_hover.into();
        let ink: Hsla = palette.sel_bg.into();
        let graph = GraphPaint::new(commit.graph.as_ref(), &commit.color, flags.is_head, cx)
            .selected(selected, cx)
            .working_link(working_link);
        let lane_color = graph.dot_color;
        let list_bounds = self.scroll.bounds();

        let layout =
            if commit.refs.is_empty() {
                None
            } else {
                let family = cx.theme().mono_font_family.clone();
                Some(self.cached_badge_layout(commit, &family, |text| {
                    self.measure_text(text, window, cx)
                }))
            };

        let label = SharedString::from(commit.message.clone());
        let title = membership_title(commit.in_current_branch, commit.in_upstream);
        let select = row_commit.clone();
        let menu_commit = row_commit.clone();
        let menu = self.context_menu.clone();

        let row = div()
            .id(SharedString::from(format!("commit:{key}")))
            .test_support()
            .role(Role::ListItem)
            .aria_label(label)
            .aria_selected(selected)
            .relative()
            .flex()
            .flex_shrink_0()
            .items_center()
            .w_full()
            .h(px(ROW_HEIGHT))
            .overflow_hidden()
            .pr(px(2.))
            .whitespace_nowrap()
            .text_color(palette.text_1)
            .cursor_pointer()
            // Señal: hover inmediato y la fila seleccionada invertida a tinta.
            .when(!selected, |row| row.hover(move |style| style.bg(lit)))
            .when(selected, |row| row.bg(ink).text_color(palette.sel_fg))
            // Todavía no está en tu copia de trabajo: se apaga en vez de
            // esconderse, para que se vea que existe pero no es tuyo.
            .when(flags.not_in_branch && !selected, |row| row.opacity(0.55))
            .when_some(title, |row, title| row.tooltip(mergeon_ui::tooltip(title)))
            .on_click(cx.listener(move |_, event: &ClickEvent, _, cx| {
                if !event.standard_click() {
                    return;
                }
                let modifiers = event.modifiers();
                let commit = select.get();
                if modifiers.secondary() {
                    cx.emit(CommitHistoryListEvent::ToggleCommit(commit.clone()));
                    return;
                }
                if modifiers.shift {
                    cx.emit(CommitHistoryListEvent::ExtendSelection(commit.clone()));
                    return;
                }
                cx.emit(CommitHistoryListEvent::SelectCommit(commit.clone()));
                if event.click_count() == 2 {
                    cx.emit(CommitHistoryListEvent::CheckoutCommit(commit.clone()));
                }
            }))
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _| {
                        let mouse = window.mouse_position();
                        let hovered = mouse.y >= bounds.top()
                            && mouse.y < bounds.bottom()
                            && mouse.x >= list_bounds.left()
                            && mouse.x < list_bounds.right();
                        let surface = if selected {
                            ink
                        } else if hovered {
                            lit
                        } else {
                            card
                        };
                        graph.paint(bounds, surface, window);
                    },
                )
                .flex_shrink_0()
                .w(px(graph_width))
                .h(px(GRAPH_HEIGHT)),
            )
            .child(self.render_info(
                &row_commit,
                flags,
                lane_color,
                layout.as_deref(),
                cx.entity().downgrade(),
                cx,
            ))
            .children(self.render_columns(commit, selected, cx))
            // La marca naranja de 2px va superpuesta y no como borde: un borde
            // correría la fila y el grafo dejaría de alinearse con el resto.
            .when(selected, |row| {
                row.child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .bottom_0()
                        .w(px(2.))
                        .bg(palette.primary),
                )
            });

        match menu {
            Some(menu) => row
                .context_menu_items(move |window, cx| menu(menu_commit.get(), window, cx))
                .into_any_element(),
            None => row.into_any_element(),
        }
    }

    /// Refs y mensaje, en una sola línea.
    fn render_info(
        &self,
        row_commit: &RowCommit,
        flags: RowFlags,
        lane_color: Hsla,
        layout: Option<&FittedBadges<RefBadge>>,
        this: WeakEntity<Self>,
        cx: &App,
    ) -> impl IntoElement {
        let palette = *cx.palette();
        let selected = flags.selected;
        let commit = row_commit.get();

        // Las refs van antes del mensaje: son lo que identifica la fila. Nunca
        // se recorta un nombre; lo que no cabe se resume en "+N".
        let refs = layout.filter(|_| !commit.refs.is_empty()).map(|layout| {
            div()
                .flex()
                .flex_shrink_0()
                .items_center()
                .gap(px(BADGE_GAP as f32))
                .children(layout.visible.iter().enumerate().map(|(index, badge)| {
                    let pill = div()
                        .id(("ref", index))
                        .test_support()
                        .aria_label(SharedString::from(badge.name.clone()))
                        .flex()
                        .flex_shrink_0()
                        .child(ref_badge(badge, lane_color, selected, cx));
                    // Doble clic en una rama: checkout de esa rama. El primer
                    // clic sigue llegando a la fila (la selecciona); el doble
                    // no, para que no pida además el checkout del commit.
                    if !matches!(badge.kind, RefType::Local | RefType::Remote) {
                        return pill.into_any_element();
                    }
                    let (this, commit, name, kind) = (
                        this.clone(),
                        row_commit.clone(),
                        badge.name.clone(),
                        badge.kind,
                    );
                    pill.on_click(move |event: &ClickEvent, _, cx| {
                        if !event.standard_click() || event.click_count() != 2 {
                            return;
                        }
                        cx.stop_propagation();
                        let _ = this.update(cx, |_, cx| {
                            cx.emit(CommitHistoryListEvent::CheckoutRef {
                                commit: commit.get().clone(),
                                name: name.clone(),
                                kind,
                            });
                        });
                    })
                    .into_any_element()
                }))
                .when(layout.hidden > 0, |refs| {
                    // Todas las refs del commit, no solo las que no caben.
                    let title = refs_tooltip(&commit.refs);
                    refs.child(
                        pill_shell("ref-more", cx)
                            .test_support()
                            .border_dashed()
                            .border_color(if selected {
                                palette.sel_fg_2
                            } else {
                                palette.border_control
                            })
                            .text_color(if selected {
                                palette.sel_fg_2
                            } else {
                                palette.text_3
                            })
                            .tooltip(mergeon_ui::tooltip(title))
                            .child(format!("+{}", layout.hidden)),
                    )
                })
        });

        div()
            .flex()
            .flex_1()
            .min_w_0()
            .items_center()
            .gap(px(6.))
            .pr(px(COLUMN_GAP))
            .overflow_hidden()
            .when(flags.to_pull, |info| {
                info.child(
                    div()
                        .id("pull-marker")
                        .test_support()
                        .flex()
                        .flex_shrink_0()
                        .items_center()
                        .tooltip(mergeon_ui::tooltip("In your upstream, not pulled yet"))
                        .child(
                            mergeon_ui::icon(Lucide::ArrowDownToLine)
                                .size(px(12.))
                                .tone(if selected {
                                    IconTone::Current
                                } else {
                                    IconTone::Active
                                }),
                        ),
                )
            })
            .children(refs)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(commit.message.clone()),
            )
    }

    /// Cabecera de columnas de Señal (24px, mono en mayúsculas, `text-3`),
    /// alineada con las columnas de las filas.
    ///
    /// Qué columnas lleva depende del ancho de la lista, y ese ancho cambia
    /// al arrastrar el divisor: la cabecera se arma en el prepaint del mismo
    /// frame, con el ancho que tiene ahora. Armada en `render` usaba el ancho
    /// medido en el frame anterior y se quedaba desfasada de las filas (que sí
    /// se arman después de medirlo) hasta el siguiente repintado.
    fn render_column_header(&self) -> impl IntoElement {
        let graph_width = self.graph_column_width();
        canvas(
            move |bounds, window, cx| {
                let mut header = column_header(bounds.size.width, graph_width, cx);
                header.layout_as_root(
                    size(
                        AvailableSpace::Definite(bounds.size.width),
                        AvailableSpace::Definite(bounds.size.height),
                    ),
                    window,
                    cx,
                );
                header.prepaint_at(bounds.origin, window, cx);
                header
            },
            |_, mut header, window, cx| header.paint(window, cx),
        )
        .flex_shrink_0()
        .w_full()
        .h(px(COLUMN_HEADER_HEIGHT))
    }

    /// Autor, fecha y hash, en columnas fijas a la derecha.
    fn render_columns(&self, commit: &CommitRow, selected: bool, cx: &App) -> Vec<AnyElement> {
        let palette = *cx.palette();
        let mono = code_font_family(cx);
        let (show_author, show_date) = self.visible_columns();
        let quiet = if selected {
            palette.sel_fg_2
        } else {
            palette.text_3
        };
        let column = |width: f32| {
            div()
                .flex_shrink_0()
                .w(px(width))
                .pr(px(COLUMN_GAP))
                .overflow_hidden()
                .truncate()
        };

        let mut columns = Vec::with_capacity(3);
        if show_author {
            // Señal `.who`: monograma de iniciales y el nombre.
            columns.push(
                column(AUTHOR_COLUMN)
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_color(if selected {
                        palette.sel_fg_2
                    } else {
                        palette.text_2
                    })
                    .child(monogram(&commit.author, MONOGRAM_SIZE, 8.5, selected, cx))
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .truncate()
                            .child(commit.author.clone()),
                    )
                    .into_any_element(),
            );
        }
        if show_date {
            columns.push(
                column(DATE_COLUMN)
                    .font_family(mono.clone())
                    .text_size(px(11.5))
                    .text_color(quiet)
                    .child(compact_date(commit))
                    .into_any_element(),
            );
        }
        // Hash partido: los primeros caracteres en tinta, el resto apagado.
        let lead = commit
            .hash
            .char_indices()
            .nth(HASH_LEAD)
            .map_or(commit.hash.len(), |(index, _)| index);
        columns.push(
            column(HASH_COLUMN)
                .flex()
                .font_family(mono)
                .text_size(px(11.5))
                .child(
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(if selected {
                            palette.sel_fg
                        } else {
                            palette.text_1
                        })
                        .child(commit.hash[..lead].to_string()),
                )
                .child(
                    div()
                        .text_color(quiet)
                        .child(commit.hash[lead..].to_string()),
                )
                .into_any_element(),
        );
        columns
    }
}

/// Qué columnas opcionales caben en una lista de este ancho: (autor, fecha).
/// Sin medir todavía (ancho 0) se muestran todas, como en una lista ancha.
fn visible_columns_for(width: Pixels) -> (bool, bool) {
    let width = f32::from(width);
    let measured = width > 0.;
    (
        !measured || width >= SHOW_AUTHOR_FROM,
        !measured || width >= SHOW_DATE_FROM,
    )
}

/// La cabecera de columnas para una lista de `width` (ver
/// [`CommitHistoryList::render_column_header`]).
fn column_header(width: Pixels, graph_width: f32, cx: &App) -> AnyElement {
    let palette = *cx.palette();
    let (show_author, show_date) = visible_columns_for(width);
    let label = |width: Option<f32>, text: &'static str| {
        div()
            .id(SharedString::from(format!(
                "column:{}",
                text.to_lowercase()
            )))
            .test_support()
            .flex_shrink_0()
            .pr(px(COLUMN_GAP))
            .overflow_hidden()
            .map(|this| match width {
                Some(width) => this.w(px(width)),
                None => this.flex_1().min_w_0(),
            })
            .child(text)
    };
    div()
        .id("commit-columns")
        .flex()
        .flex_shrink_0()
        .w(width)
        .items_center()
        .h(px(COLUMN_HEADER_HEIGHT))
        .pr(px(2.))
        .border_b_1()
        .border_color(palette.border)
        .bg(palette.card)
        .font_family(cx.theme().mono_font_family.clone())
        .text_size(px(10.5))
        .font_weight(FontWeight::MEDIUM)
        .text_color(palette.text_3)
        .whitespace_nowrap()
        .child(label(Some(graph_width), "GRAPH").pl(px(GRAPH_LABEL_INSET)))
        .child(label(None, "MESSAGE"))
        .when(show_author, |this| {
            this.child(label(Some(AUTHOR_COLUMN), "AUTHOR"))
        })
        .when(show_date, |this| {
            this.child(label(Some(DATE_COLUMN), "DATE"))
        })
        .child(label(Some(HASH_COLUMN), "HASH"))
        .into_any_element()
}

/// Fecha corta para la columna (Señal: «hoy 11:48», «10 nov»): lo reciente,
/// relativo y abreviado (`24 min ago`); lo demás, la fecha compacta.
pub(crate) fn compact_date(commit: &CommitRow) -> String {
    let relative = commit.date.as_str();
    if let Some(amount) = relative.strip_suffix(" ago") {
        let amount = amount
            .replace(" minutes", " min")
            .replace(" minute", " min")
            .replace(" hours", " h")
            .replace(" hour", " h")
            .replace(" days", " d")
            .replace(" day", " d");
        return format!("{amount} ago");
    }
    mergeon_model::format::format_short_date(commit.full_date.as_str())
}

/// Caja común de las pills de ref (mono 11, alto 18, radio 2).
fn pill_shell(id: impl Into<gpui_kit::ElementId>, cx: &App) -> gpui_kit::Stateful<gpui_kit::Div> {
    div()
        .id(id)
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(px(5.))
        .h(px(PILL_HEIGHT))
        .whitespace_nowrap()
        .border_1()
        .border_color(gpui_kit::transparent_black())
        .rounded(Radius::Xs.px(cx))
        .px(px(5.))
        .font_family(cx.theme().mono_font_family.clone())
        .text_size(px(BADGE_FONT_SIZE))
        .line_height(relative(1.))
        .font_weight(FontWeight::MEDIUM)
}

/// Pill de una ref según su tipo (Señal): HEAD partida tinta/naranja, local
/// con el cuadradito del color de su carril, remota con borde discontinuo y
/// tag con relleno gris. Sobre la fila en tinta pasan a `sel-fg`.
fn ref_badge(badge: &RefBadge, lane_color: Hsla, selected: bool, cx: &App) -> Stateful<Div> {
    let palette = *cx.palette();
    let fg = if selected {
        palette.sel_fg
    } else {
        palette.text_1
    };
    let quiet = if selected {
        palette.sel_fg_2
    } else {
        palette.text_3
    };
    let outline = if selected {
        palette.sel_fg_2
    } else {
        palette.border_control
    };
    let title = ref_badge_title(badge);
    let shell = pill_shell("pill", cx);

    // La copia remota está en este mismo commit (`main` y `origin/main`): una
    // sola etiqueta con una nube por remoto, como el sidebar, en vez de
    // repetir el nombre con prefijo.
    let synced = badge.remotes.iter().flatten().map(move |remote| {
        div()
            .id(SharedString::from(format!("ref-remote:{remote}")))
            .test_support()
            .flex()
            .items_center()
            .text_color(quiet)
            .child(
                mergeon_ui::icon(Lucide::Cloud)
                    .size(px(BADGE_ICON as f32))
                    .tone(IconTone::Current),
            )
    });

    let pill = if badge.is_head {
        // Cápsula partida: `HEAD` en tinta y el nombre de la rama en naranja
        // con texto tinta (nunca blanco sobre naranja).
        let detached = badge.kind == RefType::Head;
        shell
            .gap_0()
            .px_0()
            .border_0()
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .items_center()
                    .h_full()
                    .px(px(6.))
                    .bg(if selected {
                        palette.sel_fg
                    } else {
                        palette.text_1
                    })
                    .text_color(if selected {
                        palette.sel_bg
                    } else {
                        palette.bg_base
                    })
                    .text_size(px(10.))
                    .child("HEAD"),
            )
            .when(!detached, |pill| {
                pill.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .h_full()
                        .px(px(6.))
                        .bg(palette.primary)
                        .text_color(palette.primary_foreground)
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(badge.name.clone())
                        .children(badge.remotes.iter().flatten().map(|remote| {
                            div()
                                .id(SharedString::from(format!("ref-remote:{remote}")))
                                .test_support()
                                .flex()
                                .items_center()
                                .child(
                                    mergeon_ui::icon(Lucide::Cloud)
                                        .size(px(BADGE_ICON as f32))
                                        .tone(IconTone::Current),
                                )
                        })),
                )
            })
    } else {
        match badge.kind {
            RefType::Remote => {
                // `origin/` en gris y la rama en el color del texto.
                let (remote, branch) = badge
                    .name
                    .split_once('/')
                    .map_or(("", badge.name.as_str()), |(remote, branch)| {
                        (remote, branch)
                    });
                shell
                    .border_dashed()
                    .border_color(outline)
                    .text_color(if selected {
                        palette.sel_fg
                    } else {
                        palette.text_2
                    })
                    .child(
                        div()
                            .flex()
                            .when(!remote.is_empty(), |name| {
                                name.child(div().text_color(quiet).child(format!("{remote}/")))
                            })
                            .child(branch.to_string()),
                    )
            }
            RefType::Tag => shell
                .bg(if selected {
                    with_alpha(palette.sel_fg, 0.16)
                } else {
                    palette.bg_pressed.into()
                })
                .text_color(fg)
                .child(
                    mergeon_ui::icon(Lucide::Tag)
                        .size(px(BADGE_ICON as f32))
                        .tone(IconTone::Current),
                )
                .child(badge.name.clone()),
            RefType::Local | RefType::Head => shell
                .border_color(outline)
                .text_color(fg)
                .child(div().flex_shrink_0().size(px(6.)).bg(lane_color))
                .child(badge.name.clone())
                .children(synced),
        }
    };
    pill.when_some(title, |pill, title| {
        pill.tooltip(mergeon_ui::tooltip(title))
    })
}

/// El tooltip de una etiqueta: dónde estás y sus copias remotas
/// ([`badge_title`]) y, en las ramas, que el doble clic hace checkout.
pub fn ref_badge_title(badge: &RefBadge) -> Option<String> {
    let hint = matches!(badge.kind, RefType::Local | RefType::Remote) && !badge.is_head;
    match (badge_title(badge), hint) {
        (Some(title), true) => Some(format!("{title}\nDouble-click to check out")),
        (None, true) => Some("Double-click to check out".to_string()),
        (title, false) => title,
    }
}

/// El tooltip del chip "+N": todas las refs del commit, una por línea y con
/// lo que es cada una.
pub fn refs_tooltip(refs: &[CommitRef]) -> String {
    refs.iter()
        .map(|commit_ref| match commit_ref.kind {
            RefType::Head => "HEAD (detached)".to_string(),
            RefType::Local if commit_ref.is_head => format!("{} (HEAD)", commit_ref.name),
            RefType::Local => commit_ref.name.clone(),
            RefType::Remote => format!("{} (remote)", commit_ref.name),
            RefType::Tag => format!("{} (tag)", commit_ref.name),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

impl Render for CommitHistoryList {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let entity = cx.entity();

        let shell = div()
            .id("commit-list")
            .relative()
            .flex_1()
            .w_full()
            .h_full()
            .min_w_0()
            .overflow_hidden()
            .border_r_1()
            .border_color(palette.border)
            .bg(palette.card)
            .line_height(relative(1.5));

        if let Some(status) = self.status_text() {
            return shell.child(
                div()
                    .id("commit-list-state")
                    .test_support()
                    .aria_label(status)
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .h_full()
                    .text_size(rems(0.875))
                    .text_color(palette.muted_foreground)
                    .when(self.is_loading, |state| {
                        state.child(
                            div()
                                .mb_2()
                                .child(mergeon_ui::spinner().color(SpinnerColor::Primary)),
                        )
                    })
                    .child(status),
            );
        }

        // El ancho de la lista decide cuántos badges caben en cada fila.
        let measure = {
            let entity = entity.downgrade();
            canvas(
                move |bounds, _, cx| {
                    let _ = entity.update(cx, |list, cx| {
                        if list.list_width != bounds.size.width {
                            list.list_width = bounds.size.width;
                            cx.notify();
                        }
                    });
                },
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0()
        };

        shell
            .flex()
            .flex_col()
            .child(measure)
            .child(self.render_column_header())
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(
                        v_virtual_list(
                            entity,
                            "commit-rows",
                            self.item_sizes.clone(),
                            |list, range, window, cx| list.render_rows(range, window, cx),
                        )
                        .track_scroll(&self.scroll),
                    )
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .child(Scrollbar::new(&self.scroll).axis(ScrollbarAxis::Vertical)),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_measured_ref_widths_are_bounded() {
        let mut widths = HashMap::new();
        for index in 0..MAX_TEXT_WIDTHS {
            remember_text_width(&mut widths, &format!("branch-{index}"), 40.0);
        }
        assert_eq!(widths.len(), MAX_TEXT_WIDTHS);
        remember_text_width(&mut widths, "branch-0", 41.0);
        assert_eq!(widths.len(), MAX_TEXT_WIDTHS);
        assert_eq!(widths.get("branch-0"), Some(&41.0));

        remember_text_width(&mut widths, "main", 30.0);
        assert_eq!(widths.len(), 1);
        assert_eq!(widths.get("main"), Some(&30.0));
    }
}
