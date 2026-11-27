//! Buscar dentro de un visor (diff o archivo): la barra pequeña arriba a la
//! derecha (campo, "3 / 17", ↑ ↓, Aa, .*, ×), el resaltado de las
//! coincidencias y el salto a la actual.
//!
//! La búsqueda corre sobre las líneas ya analizadas del visor, nunca en el
//! render: al cambiar la consulta, las opciones o el documento. Se guarda por
//! consulta y generación del documento (repetirla no busca otra vez) y, en
//! documentos grandes, corre en segundo plano; una búsqueda vieja que acabe
//! tarde no pisa a la nueva.
//!
//! Teclado (contexto [`CONTEXT`], el del visor entero): `mod+f` abre la barra
//! (o vuelve a ella), Enter / Shift+Enter van a la siguiente / anterior,
//! Esc cierra. Con el foco en el visor, `mod+f` gana a "Search commits" (ver
//! `mergeon_model::keyboard_shortcuts::VIEWER_SHORTCUTS`).

use std::ops::Range;
use std::sync::Arc;

use gpui_kit::component::input::{Enter, Escape};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, Focusable as _, Hsla,
    InteractiveElement as _, IntoElement, KeyBinding, KeyDownEvent, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Task,
    TestSupportExt as _, Window, actions, div, px,
};
use mergeon_model::diff::{DiffLine, DiffLineType, SplitCellType, SplitRow};
use mergeon_model::keyboard_shortcuts::{VIEWER_SHORTCUTS, ViewerAction};
use mergeon_model::text_search::{SearchQuery, SearchResults, search_lines};
use mergeon_ui::theme::with_alpha;
use mergeon_ui::{ActivePaletteExt as _, InputEvent, InputState, Lucide, MergeonStyled as _};

use crate::canvas::{CanvasDelegate, Reveal, RevealMode};
use crate::text::display_offset;

actions!(
    mergeon_view_diff,
    [
        /// Abre la barra de búsqueda del visor (o vuelve a su campo).
        FindInViewer,
        /// Siguiente bloque de cambios del diff.
        NextChange,
        /// Bloque de cambios anterior.
        PreviousChange,
    ]
);

/// Contexto de teclado de los visores de código.
pub const CONTEXT: &str = "CodeViewer";

/// Por encima de estas filas la búsqueda va en segundo plano.
const SYNC_SEARCH_ROWS: usize = 20_000;

/// `mod+shift+f7` → `secondary-shift-f7`.
fn keystroke(combo: &str) -> String {
    combo
        .split('+')
        .map(|part| if part == "mod" { "secondary" } else { part })
        .collect::<Vec<_>>()
        .join("-")
}

pub(crate) fn init(cx: &mut App) {
    let bindings = VIEWER_SHORTCUTS
        .iter()
        .map(|binding| {
            let keys = keystroke(binding.combo);
            match binding.action {
                ViewerAction::Find => KeyBinding::new(&keys, FindInViewer, Some(CONTEXT)),
                ViewerAction::NextChange => KeyBinding::new(&keys, NextChange, Some(CONTEXT)),
                ViewerAction::PreviousChange => {
                    KeyBinding::new(&keys, PreviousChange, Some(CONTEXT))
                }
            }
        })
        .collect::<Vec<_>>();
    cx.bind_keys(bindings);
}

/// Las líneas en las que se busca, sin atarse al hilo de la interfaz.
#[derive(Clone)]
pub(crate) enum FindSource {
    /// Vista unificada del diff: una fila por línea.
    Unified(Arc<Vec<DiffLine>>),
    /// Vista partida: cada fila tiene el lado viejo (columna 0) y el nuevo (1).
    Split(Arc<Vec<SplitRow>>),
    /// Un archivo: el texto y el rango de cada línea.
    Text {
        content: SharedString,
        lines: Arc<Vec<Range<usize>>>,
    },
}

impl FindSource {
    fn rows(&self) -> usize {
        match self {
            Self::Unified(lines) => lines.len(),
            Self::Split(rows) => rows.len(),
            Self::Text { lines, .. } => lines.len(),
        }
    }

    fn search(&self, query: &regex::Regex) -> SearchResults {
        match self {
            Self::Unified(lines) => search_lines(
                lines
                    .iter()
                    .enumerate()
                    .filter(|(_, line)| line.kind != DiffLineType::Note)
                    .map(|(row, line)| (row, 0, line.content.as_str())),
                query,
            ),
            Self::Split(rows) => search_lines(
                rows.iter().enumerate().flat_map(|(row, split)| {
                    [(0, &split.left), (1, &split.right)]
                        .into_iter()
                        .filter(|(_, cell)| {
                            !matches!(cell.kind, SplitCellType::Empty | SplitCellType::Note)
                        })
                        .map(move |(column, cell)| (row, column, cell.content.as_str()))
                }),
                query,
            ),
            Self::Text { content, lines } => search_lines(
                lines.iter().enumerate().map(|(row, range)| {
                    let line = content.get(range.clone()).unwrap_or_default();
                    (row, 0, line)
                }),
                query,
            ),
        }
    }
}

/// Estado de la búsqueda de un visor.
#[derive(Default)]
pub(crate) struct FindState {
    open: bool,
    input: Option<Entity<InputState>>,
    case_sensitive: bool,
    regex: bool,
    results: Arc<SearchResults>,
    current: Option<usize>,
    /// La regex no compila.
    error: bool,
    /// Lo último buscado: consulta y generación del documento.
    searched: Option<(SearchQuery, u64)>,
    /// Cuenta las búsquedas lanzadas: una vieja no pisa a la nueva.
    generation: u64,
    pending: Option<Task<()>>,
    _subscription: Option<Subscription>,
}

impl FindState {
    pub(crate) fn is_open(&self) -> bool {
        self.open
    }

    pub(crate) fn query_text(&self, cx: &App) -> SharedString {
        self.input
            .as_ref()
            .map(|input| input.read(cx).value())
            .unwrap_or_default()
    }

    fn query(&self, cx: &App) -> SearchQuery {
        SearchQuery::new(self.query_text(cx).to_string())
            .case_sensitive(self.case_sensitive)
            .regex(self.regex)
    }

    /// Fondos de las coincidencias de una celda, en offsets del texto
    /// pintado. `display` pasa de bytes de la línea original a los del texto
    /// pintado (los tabuladores se expanden).
    pub(crate) fn marks(
        &self,
        row: usize,
        column: usize,
        source: &str,
        colors: (Hsla, Hsla),
    ) -> Vec<(Range<usize>, Hsla)> {
        if !self.open || self.results.matches.is_empty() {
            return Vec::new();
        }
        let range = self.results.in_row(row);
        let start = range.start;
        self.results.matches[range]
            .iter()
            .enumerate()
            .filter(|(_, found)| found.column == column)
            .map(|(offset, found)| {
                let color = if self.current == Some(start + offset) {
                    colors.1
                } else {
                    colors.0
                };
                (
                    display_offset(source, found.range.start)
                        ..display_offset(source, found.range.end),
                    color,
                )
            })
            .collect()
    }
}

/// Cómo va la búsqueda de un visor (para quien lo aloja y para los tests).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct FindStatus {
    /// La barra está abierta.
    pub open: bool,
    pub query: String,
    pub case_sensitive: bool,
    pub regex: bool,
    /// La regex no compila.
    pub invalid: bool,
    /// Una búsqueda en segundo plano sigue en curso.
    pub searching: bool,
    /// Coincidencias: fila, columna y bytes de la línea original.
    pub matches: Vec<(usize, usize, Range<usize>)>,
    /// Había más de las que se cuentan.
    pub truncated: bool,
    /// La actual, como posición en `matches`.
    pub current: Option<usize>,
    /// Lo que dice el contador ("3 / 17").
    pub counter: String,
}

impl FindStatus {
    pub(crate) fn of(find: &FindState, cx: &App) -> Self {
        Self {
            open: find.open,
            query: find.query_text(cx).to_string(),
            case_sensitive: find.case_sensitive,
            regex: find.regex,
            invalid: find.error,
            searching: find.pending.is_some(),
            matches: find
                .results
                .matches
                .iter()
                .map(|found| (found.row, found.column, found.range.clone()))
                .collect(),
            truncated: find.results.truncated,
            current: find.current,
            counter: find.results.counter(find.current),
        }
    }
}

/// Colores del resaltado: las coincidencias y la actual (más contraste).
pub(crate) fn match_colors(cx: &App) -> (Hsla, Hsla) {
    let palette = cx.palette();
    (
        with_alpha(palette.warning, 0.38),
        with_alpha(palette.primary, 0.62),
    )
}

/// Un visor en el que se puede buscar.
pub(crate) trait Searchable: CanvasDelegate {
    fn find(&self) -> &FindState;
    fn find_mut(&mut self) -> &mut FindState;
    /// Las líneas en las que buscar; `None` si no hay documento.
    fn find_source(&mut self) -> Option<FindSource>;
    /// Cambia cada vez que cambia lo que se busca (otro documento u otra vista).
    fn document_generation(&self) -> u64;
    /// La línea original de una celda (para pasar a offsets pintados).
    fn source_text(&self, row: usize, column: usize) -> Option<&str>;
}

/// `mod+f`: abre la barra con el foco en su campo (y lo seleccionado en el
/// visor como consulta, si es una sola línea).
pub(crate) fn open<V: Searchable>(view: &mut V, window: &mut Window, cx: &mut Context<V>) {
    let input = match view.find().input.clone() {
        Some(input) => input,
        None => {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder("Find"));
            let subscription = cx.subscribe_in(
                &input,
                window,
                |view: &mut V, _, event: &InputEvent, _, cx| {
                    if matches!(event, InputEvent::Change) && view.find().open {
                        search(view, true, cx);
                    }
                },
            );
            let find = view.find_mut();
            find.input = Some(input.clone());
            find._subscription = Some(subscription);
            input
        }
    };

    let selected = crate::canvas::selected_text(view).filter(|text| !text.contains('\n'));
    view.find_mut().open = true;
    input.update(cx, |state, cx| {
        if let Some(selected) = selected.filter(|text| !text.is_empty()) {
            state.set_value(selected, window, cx);
        }
        state.select_all(window, cx);
    });
    let handle = input.read(cx).focus_handle(cx);
    window.focus(&handle, cx);
    // `set_value` no avisa: se busca aquí.
    search(view, true, cx);
    cx.notify();
}

/// Esc o ×: cierra la barra y devuelve el foco al visor.
pub(crate) fn close<V: Searchable>(view: &mut V, window: &mut Window, cx: &mut Context<V>) {
    if !view.find().open {
        return;
    }
    let find = view.find_mut();
    find.open = false;
    find.generation += 1;
    find.pending = None;
    find.searched = None;
    find.results = Arc::default();
    find.current = None;
    window.focus(&view.canvas().focus.clone(), cx);
    cx.notify();
}

/// El documento cambió: se vuelve a buscar sin mover el visor.
pub(crate) fn refresh<V: Searchable>(view: &mut V, cx: &mut Context<V>) {
    if view.find().open {
        search(view, false, cx);
    }
}

pub(crate) fn toggle_case<V: Searchable>(view: &mut V, cx: &mut Context<V>) {
    let find = view.find_mut();
    find.case_sensitive = !find.case_sensitive;
    search(view, true, cx);
    cx.notify();
}

pub(crate) fn toggle_regex<V: Searchable>(view: &mut V, cx: &mut Context<V>) {
    let find = view.find_mut();
    find.regex = !find.regex;
    search(view, true, cx);
    cx.notify();
}

/// Busca la consulta actual, salvo que ya esté buscada sobre este documento.
/// `reveal` lleva el visor a la primera coincidencia desde lo que se ve.
fn search<V: Searchable>(view: &mut V, reveal: bool, cx: &mut Context<V>) {
    let query = view.find().query(cx);
    let key = (query.clone(), view.document_generation());
    if view.find().searched.as_ref() == Some(&key) {
        return;
    }
    view.find_mut().searched = Some(key);
    view.find_mut().generation += 1;
    let generation = view.find().generation;

    let regex = match query.compile() {
        Ok(Some(regex)) => regex,
        Ok(None) => {
            apply(view, SearchResults::default(), false, false, cx);
            return;
        }
        Err(_) => {
            apply(view, SearchResults::default(), true, false, cx);
            return;
        }
    };
    let Some(source) = view.find_source() else {
        apply(view, SearchResults::default(), false, false, cx);
        return;
    };

    if source.rows() <= SYNC_SEARCH_ROWS {
        let results = source.search(&regex);
        apply(view, results, false, reveal, cx);
        return;
    }
    let searching = cx.background_spawn(async move { source.search(&regex) });
    let task = cx.spawn(async move |this, cx| {
        let results = searching.await;
        this.update(cx, |view, cx| {
            if view.find().generation == generation {
                view.find_mut().pending = None;
                apply(view, results, false, reveal, cx);
            }
        })
        .ok();
    });
    view.find_mut().pending = Some(task);
    cx.notify();
}

fn apply<V: Searchable>(
    view: &mut V,
    results: SearchResults,
    error: bool,
    reveal: bool,
    cx: &mut Context<V>,
) {
    let previous = view.find().current;
    let anchor = view.canvas().painted_rows().start;
    let find = view.find_mut();
    find.pending = None;
    find.error = error;
    find.current = if reveal {
        results.first_from(anchor)
    } else {
        previous
            .filter(|current| *current < results.matches.len())
            .or_else(|| results.first_from(anchor))
    };
    find.results = Arc::new(results);
    if reveal {
        reveal_current(view);
    }
    cx.notify();
}

fn reveal_current<V: Searchable>(view: &mut V) {
    let Some(found) = view
        .find()
        .current
        .and_then(|current| view.find().results.matches.get(current))
        .cloned()
    else {
        return;
    };
    let range = match view.source_text(found.row, found.column) {
        Some(source) => {
            display_offset(source, found.range.start)..display_offset(source, found.range.end)
        }
        None => found.range.clone(),
    };
    view.canvas_mut().reveal(Reveal {
        row: found.row,
        column: found.column,
        range,
        mode: RevealMode::Center,
    });
}

/// Enter / ↓ (`forward`) y Shift+Enter / ↑: la coincidencia siguiente o la
/// anterior, dando la vuelta, y el visor hasta ella.
pub(crate) fn navigate<V: Searchable>(view: &mut V, forward: bool, cx: &mut Context<V>) {
    let total = view.find().results.matches.len();
    if total == 0 {
        return;
    }
    let next = match view.find().current {
        Some(current) if forward => (current + 1) % total,
        Some(current) => (current + total - 1) % total,
        None if forward => 0,
        None => total - 1,
    };
    view.find_mut().current = Some(next);
    reveal_current(view);
    cx.notify();
}

/// La barra, si está abierta: va arriba a la derecha del visor.
pub(crate) fn render_bar<V: Searchable>(view: &V, cx: &mut Context<V>) -> Option<AnyElement> {
    let find = view.find();
    if !find.open {
        return None;
    }
    let input = find.input.clone()?;
    let palette = *cx.palette();
    let has_query = !find.query_text(cx).is_empty();
    let counter: SharedString = if find.error {
        "Invalid regex".into()
    } else if !has_query {
        "".into()
    } else if find.results.matches.is_empty() && find.pending.is_some() {
        "…".into()
    } else if find.results.matches.is_empty() {
        "No results".into()
    } else {
        find.results.counter(find.current).into()
    };
    let no_matches = has_query && find.results.matches.is_empty() && find.pending.is_none();

    let toggle = |id: &'static str, icon: Lucide, tooltip: &'static str, on: bool| {
        mergeon_ui::button(id)
            .ghost()
            .xs()
            .icon(icon)
            .selected(on)
            .tooltip(tooltip)
    };

    Some(
        div()
            .id("find-bar")
            .test_support()
            .absolute()
            .top(px(6.))
            .right(px(14.))
            .flex()
            .items_center()
            .gap(px(2.))
            .p(px(3.))
            .floating_surface(cx)
            .occlude()
            .capture_action(cx.listener(|view: &mut V, action: &Enter, _, cx| {
                cx.stop_propagation();
                navigate(view, !action.shift, cx);
            }))
            .capture_action(cx.listener(|view: &mut V, _: &Escape, window, cx| {
                cx.stop_propagation();
                close(view, window, cx);
            }))
            // Con el foco en uno de sus botones, Esc también la cierra.
            .on_key_down(
                cx.listener(|view: &mut V, event: &KeyDownEvent, window, cx| {
                    if event.keystroke.key == "escape" && !event.keystroke.modifiers.modified() {
                        cx.stop_propagation();
                        close(view, window, cx);
                    }
                }),
            )
            .child(
                div()
                    .id("find-input")
                    .test_support()
                    .w(px(200.))
                    .child(mergeon_ui::input(&input)),
            )
            .child(
                div()
                    .id("find-count")
                    .test_support()
                    .aria_label(counter.clone())
                    .min_w(px(64.))
                    .px(px(6.))
                    .text_size(px(11.5))
                    .font_family(crate::typography::code_font_family(cx))
                    .text_color(if no_matches || find.error {
                        palette.destructive
                    } else {
                        palette.text_3
                    })
                    .whitespace_nowrap()
                    .child(counter),
            )
            .child(
                mergeon_ui::button("find-previous")
                    .ghost()
                    .xs()
                    .icon(Lucide::ArrowUp)
                    .tooltip("Previous match (Shift+Enter)")
                    .disabled(find.results.matches.is_empty())
                    .on_click(cx.listener(|view: &mut V, _, _, cx| navigate(view, false, cx))),
            )
            .child(
                mergeon_ui::button("find-next")
                    .ghost()
                    .xs()
                    .icon(Lucide::ArrowDown)
                    .tooltip("Next match (Enter)")
                    .disabled(find.results.matches.is_empty())
                    .on_click(cx.listener(|view: &mut V, _, _, cx| navigate(view, true, cx))),
            )
            .child(
                toggle(
                    "find-case",
                    Lucide::CaseSensitive,
                    "Match case",
                    find.case_sensitive,
                )
                .on_click(cx.listener(|view: &mut V, _, _, cx| toggle_case(view, cx))),
            )
            .child(
                toggle(
                    "find-regex",
                    Lucide::Regex,
                    "Use regular expression",
                    find.regex,
                )
                .on_click(cx.listener(|view: &mut V, _, _, cx| toggle_regex(view, cx))),
            )
            .child(
                mergeon_ui::button("find-close")
                    .ghost()
                    .xs()
                    .icon(Lucide::X)
                    .tooltip("Close (Esc)")
                    .on_click(cx.listener(|view: &mut V, _, window, cx| close(view, window, cx))),
            )
            .into_any_element(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translates_viewer_combos_to_gpui_keystrokes() {
        assert_eq!(keystroke("mod+f"), "secondary-f");
        assert_eq!(keystroke("shift+f7"), "shift-f7");
        assert_eq!(keystroke("alt+down"), "alt-down");
    }

    #[test]
    fn searches_both_sides_of_the_split_view_and_skips_notes() {
        use mergeon_model::diff::{mark_inline_changes, parse_unified_diff, to_split_rows};
        let diff = "@@ -1,2 +1,2 @@\n ctx foo\n-foo old\n+foo new\n\\ No newline at end of file\n";
        let lines = mark_inline_changes(&parse_unified_diff(diff));
        let regex = SearchQuery::new("foo").compile().unwrap().unwrap();
        let unified = FindSource::Unified(Arc::new(lines.clone())).search(&regex);
        assert_eq!(unified.matches.len(), 3);
        let split = FindSource::Split(Arc::new(to_split_rows(&lines))).search(&regex);
        // El contexto sale en los dos lados.
        assert_eq!(split.matches.len(), 4);
        assert!(split.matches.iter().any(|found| found.column == 1));
        let note = SearchQuery::new("newline").compile().unwrap().unwrap();
        assert!(
            FindSource::Unified(Arc::new(lines))
                .search(&note)
                .matches
                .is_empty()
        );
    }
}
