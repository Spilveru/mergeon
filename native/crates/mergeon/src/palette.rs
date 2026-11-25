//! Paleta de comandos (Ctrl+Shift+P / ⌘⇧P): un diálogo arriba y centrado con
//! un campo grande y la lista virtual de resultados (ícono, nombre con las
//! letras encontradas resaltadas, contexto atenuado y atajo a la derecha).
//!
//! Busca a la vez comandos ([`crate::commands`]), ramas del repositorio,
//! repositorios (pestañas abiertas y recientes) y archivos (`git ls-files`,
//! [`crate::file_index`]). Prefijos: `>` comandos, `@` ramas, `#`
//! repositorios, `/` archivos; sin prefijo, todo mezclado
//! (`mergeon_model::command_palette`).
//!
//! La vista no ejecuta nada: emite [`PaletteEvent::Run`] y la cáscara, que
//! guardó el repositorio activo al abrirla, lo ejecuta sobre él. Filtrar
//! nunca ocurre en el render: al cambiar la consulta (o al llegar los
//! archivos), en el acto con pocos candidatos y en segundo plano con muchos;
//! el resultado se guarda por consulta.

use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::input::{Enter, Escape, MoveDown, MovePageDown, MovePageUp, MoveUp};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, Context, Entity, EventEmitter, FontWeight, HighlightStyle, ScrollStrategy,
    SharedString, StyledText, Subscription, Task, TestSupportExt as _, UniformListScrollHandle,
    Window, div, px, uniform_list,
};
use mergeon_model::command_palette::{
    PaletteCandidate, PaletteKind, PaletteScope, RankedCandidate, parse_query, rank_with_files,
};
use mergeon_ui::{ActivePaletteExt as _, IconTone, InputEvent, InputState, Lucide};

use crate::commands::Command;
use crate::file_index::FileList;

/// Alto de una fila de resultados.
pub const ROW_HEIGHT: f32 = 34.;
/// Filas visibles antes de que la lista haga scroll.
const VISIBLE_ROWS: usize = 10;
/// Resultados que se ordenan y se pueden recorrer.
pub const RESULT_LIMIT: usize = 500;
/// Con más candidatos que esto se filtra en segundo plano.
const SYNC_RANK_LIMIT: usize = 3_000;

/// Lo que hace un resultado al elegirlo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaletteAction {
    Command(Command),
    /// Checkout de una rama local.
    Branch(String),
    /// Checkout de una rama remota (`origin`, `feature/x`).
    RemoteBranch {
        remote: String,
        branch: String,
    },
    /// Una pestaña de repositorio abierta.
    OpenRepository {
        id: String,
    },
    /// Un repositorio reciente que no está abierto.
    OpenRecent {
        path: String,
    },
    /// Un archivo del repositorio, tal como está en `head`.
    File {
        path: String,
        head: String,
    },
    /// "File history…": la pestaña de historial del archivo desde `HEAD`.
    FileHistory {
        path: String,
    },
    /// "Compare with current branch…": la comparación de la rama actual con
    /// esta ref (rama local o `remoto/rama`).
    Compare {
        reference: String,
    },
    /// "Fixup into…": un `fixup!` de este commit con lo preparado.
    Fixup {
        hash: String,
    },
    /// "Checkout pull request…": trae la rama de este PR y la desprotege.
    PullRequest {
        number: u64,
    },
}

/// Qué hace un archivo elegido en la paleta.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FileIntent {
    /// "Go to file…": su pestaña de árbol en `HEAD`.
    #[default]
    Open,
    /// "File history…": su historial.
    History,
}

/// El texto del campo vacío de la paleta normal.
pub const DEFAULT_PLACEHOLDER: &str = "Type a command, @branch, #repository or /file…";

/// Un resultado que no es un archivo.
#[derive(Debug, Clone)]
pub struct PaletteEntry {
    pub kind: PaletteKind,
    pub icon: Lucide,
    pub label: SharedString,
    pub context: SharedString,
    pub shortcut: Option<SharedString>,
    pub enabled: bool,
    pub action: PaletteAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaletteEvent {
    Run(PaletteAction),
    Dismiss,
}

pub struct CommandPalette {
    input: Entity<InputState>,
    entries: Rc<Vec<PaletteEntry>>,
    /// Lo mismo que `entries`, para ordenar fuera del hilo de la interfaz.
    candidates: Arc<Vec<PaletteCandidate>>,
    files: FileList,
    files_loading: bool,
    file_intent: FileIntent,
    results: Rc<Vec<RankedCandidate>>,
    selected: usize,
    scroll: UniformListScrollHandle,
    /// Consulta y versión de los candidatos de los resultados actuales.
    ranked: Option<(String, u64)>,
    candidates_version: u64,
    ranking: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<PaletteEvent> for CommandPalette {}

impl CommandPalette {
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn new(entries: Vec<PaletteEntry>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::with_options(entries, DEFAULT_PLACEHOLDER, FileIntent::Open, window, cx)
    }

    /// La paleta como selector de un comando ("File history…", "Compare
    /// with current branch…", "Fixup into…"): otro texto en el campo vacío y
    /// qué hace un archivo elegido.
    pub fn with_options(
        entries: Vec<PaletteEntry>,
        placeholder: &str,
        file_intent: FileIntent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let placeholder = placeholder.to_string();
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        let subscriptions = vec![cx.subscribe_in(
            &input,
            window,
            |this: &mut Self, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    this.rerank(cx);
                }
            },
        )];
        let candidates = entries
            .iter()
            .map(|entry| {
                PaletteCandidate::new(entry.kind, entry.label.to_string()).enabled(entry.enabled)
            })
            .collect();
        let mut palette = Self {
            input,
            entries: Rc::new(entries),
            candidates: Arc::new(candidates),
            files: FileList::default(),
            files_loading: false,
            file_intent,
            results: Rc::default(),
            selected: 0,
            scroll: UniformListScrollHandle::new(),
            ranked: None,
            candidates_version: 0,
            ranking: None,
            _subscriptions: subscriptions,
        };
        palette.rerank(cx);
        palette
    }

    pub fn input(&self) -> &Entity<InputState> {
        &self.input
    }

    /// Los archivos del repositorio (llegan después de abrir, de la caché o
    /// de `git ls-files`).
    pub fn set_files(&mut self, files: FileList, loading: bool, cx: &mut Context<Self>) {
        self.files_loading = loading;
        if files == self.files {
            cx.notify();
            return;
        }
        self.files = files;
        self.candidates_version += 1;
        self.rerank(cx);
    }

    pub fn query(&self, cx: &App) -> String {
        self.input.read(cx).value().to_string()
    }

    /// Escribe la consulta (p. ej. el prefijo de "Switch to branch…").
    pub fn set_query(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| {
            input.set_value(text.to_string(), window, cx)
        });
        self.rerank(cx);
    }

    /// Los resultados actuales: etiqueta y si se pueden ejecutar.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn result_labels(&self) -> Vec<(SharedString, bool)> {
        self.results
            .iter()
            .filter_map(|ranked| self.item(ranked.index))
            .map(|item| (item.label, item.enabled))
            .collect()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn selected_index(&self) -> usize {
        self.selected
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn is_ranking(&self) -> bool {
        self.ranking.is_some()
    }

    /// Un resultado como entrada (los archivos se arman al pedirlos).
    fn item(&self, index: usize) -> Option<PaletteEntry> {
        if let Some(entry) = self.entries.get(index) {
            return Some(entry.clone());
        }
        let path = self.files.files.get(index - self.entries.len())?;
        let (icon, context, action) = match self.file_intent {
            FileIntent::Open => (
                Lucide::File,
                "File",
                PaletteAction::File {
                    path: path.clone(),
                    head: self.files.head.clone().unwrap_or_default(),
                },
            ),
            FileIntent::History => (
                Lucide::FileClock,
                "File history",
                PaletteAction::FileHistory { path: path.clone() },
            ),
        };
        Some(PaletteEntry {
            kind: PaletteKind::File,
            icon,
            label: path.clone().into(),
            context: context.into(),
            shortcut: None,
            enabled: self.files.head.is_some(),
            action,
        })
    }

    /// Vuelve a filtrar si cambió la consulta o los candidatos.
    fn rerank(&mut self, cx: &mut Context<Self>) {
        let text = self.query(cx);
        let key = (text.clone(), self.candidates_version);
        if self.ranked.as_ref() == Some(&key) {
            return;
        }
        let candidates = self.candidates.clone();
        let files = self.files.files.clone();
        let files_enabled = self.files.head.is_some();
        let (scope, query) = parse_query(&text);
        let file_count = if scope.includes(PaletteKind::File)
            && !(query.is_empty() && scope == PaletteScope::All)
        {
            files.len()
        } else {
            0
        };

        if candidates.len() + file_count <= SYNC_RANK_LIMIT {
            let results = rank_with_files(&candidates, &files, files_enabled, &text, RESULT_LIMIT);
            self.ranking = None;
            self.apply(key, results, cx);
            return;
        }
        let ranking = cx.background_spawn(async move {
            rank_with_files(&candidates, &files, files_enabled, &text, RESULT_LIMIT)
        });
        self.ranking = Some(cx.spawn(async move |this, cx| {
            let results = ranking.await;
            this.update(cx, |this, cx| {
                this.ranking = None;
                this.apply(key, results, cx);
            })
            .ok();
        }));
    }

    fn apply(&mut self, key: (String, u64), results: Vec<RankedCandidate>, cx: &mut Context<Self>) {
        // Si cambió la consulta, la selección vuelve arriba; si solo llegaron
        // archivos, se queda donde estaba.
        let same_query = self.ranked.as_ref().is_some_and(|(text, _)| *text == key.0);
        self.ranked = Some(key);
        self.results = Rc::new(results);
        if !same_query {
            self.selected = 0;
            self.scroll.scroll_to_item(0, ScrollStrategy::Top);
        }
        self.selected = self.selected.min(self.results.len().saturating_sub(1));
        cx.notify();
    }

    fn select(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.results.is_empty() {
            return;
        }
        self.selected = index.min(self.results.len() - 1);
        self.scroll
            .scroll_to_item(self.selected, ScrollStrategy::Nearest);
        cx.notify();
    }

    fn move_by(&mut self, delta: isize, wrap: bool, cx: &mut Context<Self>) {
        let count = self.results.len();
        if count == 0 {
            return;
        }
        let current = self.selected as isize;
        let next = if wrap {
            (current + delta).rem_euclid(count as isize)
        } else {
            (current + delta).clamp(0, count as isize - 1)
        };
        self.select(next as usize, cx);
    }

    /// Enter o clic: ejecuta el resultado elegido.
    pub fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self
            .results
            .get(self.selected)
            .and_then(|ranked| self.item(ranked.index))
        else {
            return;
        };
        if !item.enabled {
            return;
        }
        match item.action {
            // "Switch to branch…" y compañía: se queda abierta con el prefijo.
            PaletteAction::Command(Command::Scope(scope)) => {
                self.set_query(scope.prefix(), window, cx);
            }
            action => cx.emit(PaletteEvent::Run(action)),
        }
    }

    fn render_rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let palette = *cx.palette();
        let mono = cx.theme().mono_font_family.clone();
        range
            .filter_map(|index| {
                let ranked = self.results.get(index)?;
                let item = self.item(ranked.index)?;
                let selected = index == self.selected;
                let (text, muted) = if selected {
                    (palette.sel_fg, palette.sel_fg_2)
                } else {
                    (palette.text_1, palette.text_3)
                };
                let highlight = HighlightStyle {
                    color: Some(
                        if selected {
                            palette.primary
                        } else {
                            palette.primary_text
                        }
                        .into(),
                    ),
                    font_weight: Some(FontWeight::SEMIBOLD),
                    ..Default::default()
                };
                let label = StyledText::new(item.label.clone()).with_highlights(highlight_ranges(
                    &item.label,
                    &ranked.positions,
                    highlight,
                ));

                Some(
                    div()
                        .id(("palette-row", index))
                        .test_support()
                        .relative()
                        .w_full()
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .h(px(ROW_HEIGHT))
                        .px(px(12.))
                        .cursor_pointer()
                        .text_color(text)
                        .when(selected, |row| row.bg(palette.sel_bg))
                        .when(!selected, |row| {
                            row.hover(move |row| row.bg(palette.bg_hover))
                        })
                        .when(!item.enabled, |row| row.opacity(0.45))
                        .when(selected, |row| {
                            // Señal: la fila elegida va en tinta con la marca naranja.
                            row.child(
                                div()
                                    .absolute()
                                    .left_0()
                                    .top_0()
                                    .bottom_0()
                                    .w(px(2.))
                                    .bg(palette.primary),
                            )
                        })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.select(index, cx);
                            this.confirm(window, cx);
                        }))
                        .child(
                            div().flex_none().text_color(muted).child(
                                mergeon_ui::icon(item.icon)
                                    .size(px(15.))
                                    .tone(IconTone::Current),
                            ),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .max_w(px(380.))
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_size(px(13.))
                                .when(
                                    item.kind == PaletteKind::File
                                        || item.kind == PaletteKind::Branch,
                                    |label| label.font_family(mono.clone()).text_size(px(12.5)),
                                )
                                .child(label),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_size(px(12.))
                                .text_color(muted)
                                .child(item.context.clone()),
                        )
                        .children(item.shortcut.clone().map(mergeon_ui::kbd))
                        .into_any_element(),
                )
            })
            .collect()
    }
}

/// Los offsets (de carácter) que casaron, como rangos de bytes con estilo.
fn highlight_ranges(
    text: &str,
    positions: &[usize],
    style: HighlightStyle,
) -> Vec<(Range<usize>, HighlightStyle)> {
    let mut ranges: Vec<Range<usize>> = Vec::new();
    for &position in positions {
        let Some(character) = text.get(position..).and_then(|rest| rest.chars().next()) else {
            continue;
        };
        let end = position + character.len_utf8();
        match ranges.last_mut() {
            Some(last) if last.end == position => last.end = end,
            _ => ranges.push(position..end),
        }
    }
    ranges.into_iter().map(|range| (range, style)).collect()
}

impl Render for CommandPalette {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let count = self.results.len();
        let list_height = px(ROW_HEIGHT * count.clamp(1, VISIBLE_ROWS) as f32);
        let (scope, _) = parse_query(&self.query(cx));
        let empty_text = if scope == PaletteScope::Files && self.files_loading {
            "Loading files…"
        } else {
            "No matching results"
        };

        let list = if count == 0 {
            div()
                .id("palette-empty")
                .test_support()
                .flex()
                .items_center()
                .justify_center()
                .h(list_height)
                .text_color(palette.text_3)
                .child(empty_text)
                .into_any_element()
        } else {
            uniform_list(
                "palette-results",
                count,
                cx.processor(|this, range, _, cx| this.render_rows(range, cx)),
            )
            .track_scroll(&self.scroll)
            .h(list_height)
            .into_any_element()
        };

        let hint = |key: &'static str, label: &'static str| {
            div()
                .flex()
                .items_center()
                .gap(px(5.))
                .child(mergeon_ui::kbd(key))
                .child(label)
        };

        div()
            .id("command-palette")
            .test_support()
            .key_context("CommandPalette")
            .flex()
            .flex_col()
            .w_full()
            .capture_action(cx.listener(|this, _: &MoveUp, _, cx| {
                cx.stop_propagation();
                this.move_by(-1, true, cx);
            }))
            .capture_action(cx.listener(|this, _: &MoveDown, _, cx| {
                cx.stop_propagation();
                this.move_by(1, true, cx);
            }))
            .capture_action(cx.listener(|this, _: &MovePageUp, _, cx| {
                cx.stop_propagation();
                this.move_by(-(VISIBLE_ROWS as isize), false, cx);
            }))
            .capture_action(cx.listener(|this, _: &MovePageDown, _, cx| {
                cx.stop_propagation();
                this.move_by(VISIBLE_ROWS as isize, false, cx);
            }))
            .capture_action(cx.listener(|this, _: &Enter, window, cx| {
                cx.stop_propagation();
                this.confirm(window, cx);
            }))
            .capture_action(cx.listener(|_, _: &Escape, _, cx| {
                cx.stop_propagation();
                cx.emit(PaletteEvent::Dismiss);
            }))
            .child(
                div()
                    .p(px(10.))
                    .border_b_1()
                    .border_color(palette.border)
                    .child(
                        mergeon_ui::input(&self.input)
                            .prefix(mergeon_ui::icon(Lucide::Search).tone(IconTone::Muted)),
                    ),
            )
            .child(div().py(px(4.)).child(list))
            .child(
                div()
                    .id("palette-hints")
                    .flex()
                    .items_center()
                    .gap(px(14.))
                    .px(px(12.))
                    .py(px(8.))
                    .border_t_1()
                    .border_color(palette.border)
                    .bg(palette.bg_inset)
                    .text_size(px(11.5))
                    .text_color(palette.text_3)
                    .child(hint("↑↓", "navigate"))
                    .child(hint("↵", "run"))
                    .child(hint("Esc", "close"))
                    .child(div().flex_1())
                    .child(hint(">", "commands"))
                    .child(hint("@", "branches"))
                    .child(hint("#", "repos"))
                    .child(hint("/", "files")),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlights_merge_consecutive_letters_and_respect_utf8() {
        let style = HighlightStyle::default();
        let ranges: Vec<Range<usize>> = highlight_ranges("añob", &[0, 1, 4], style)
            .into_iter()
            .map(|(range, _)| range)
            .collect();
        assert_eq!(ranges, [0..3, 4..5]);
    }
}
