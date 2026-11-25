//! "File history": la lista de revisiones de un archivo siguiendo sus
//! renombres, por páginas de 100. Vive a la izquierda de la pestaña de
//! historial (`HistoricalFileTabView` con fuente `history`); el diff de la
//! revisión elegida lo pinta la pestaña.

use std::rc::Rc;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, ClickEvent, Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight,
    KeyDownEvent, ScrollStrategy, SharedString, Task, UniformListScrollHandle, WeakEntity, Window,
    div, px, relative, rems, uniform_list,
};
use mergeon_app::AppModel;
use mergeon_backend::{FILE_HISTORY_PAGE_SIZE, FileHistoryOptions, FileHistoryPage, FileRevision};
use mergeon_model::format::{format_date_time, format_relative_date};
use mergeon_ui::menu::{MenuItem, MenuTriggerExt as _};
use mergeon_ui::theme::with_alpha;
use mergeon_ui::{ActivePaletteExt as _, BadgeVariant, Lucide, SpinnerColor, SpinnerSize};
use mergeon_view_diff::code_font_family;

use crate::monogram::monogram;

/// Alto de una fila (asunto arriba; autor, fecha y hash abajo).
pub const FILE_HISTORY_ROW_HEIGHT: f32 = 46.;

/// Lo que la lista pide a quien la aloja.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileHistoryListEvent {
    /// Se eligió una revisión (clic, flechas o la primera al cargar).
    Select(FileRevision),
    /// Clic en el hash o "Show commit": ir al commit en el historial.
    ShowCommit(String),
    /// "View file at this revision": el archivo tal como estaba.
    ViewFileAt(FileRevision),
}

/// De qué archivo es la historia.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileHistorySource {
    pub repo_path: String,
    /// Ruta en la revisión de partida.
    pub path: String,
    /// Revisión de partida; `None` es `HEAD`.
    pub revision: Option<String>,
}

/// Una revisión con lo que la fila pinta ya calculado.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryRow {
    pub revision: FileRevision,
    pub relative_date: SharedString,
    /// "renamed from <nombre>" (o "copied from"), si la revisión lo fue.
    pub badge: Option<SharedString>,
}

impl HistoryRow {
    pub fn new(revision: FileRevision) -> Self {
        let relative_date = if revision.date.is_empty() {
            SharedString::default()
        } else {
            compact_relative(&format_relative_date(revision.date.as_str())).into()
        };
        Self {
            badge: revision_badge(&revision),
            relative_date,
            revision,
        }
    }
}

/// "24 minutes ago" → "24 min ago", como la columna de fecha del historial.
fn compact_relative(relative: &str) -> String {
    match relative.strip_suffix(" ago") {
        Some(amount) => format!(
            "{} ago",
            amount
                .replace(" minutes", " min")
                .replace(" minute", " min")
                .replace(" hours", " h")
                .replace(" hour", " h")
                .replace(" days", " d")
                .replace(" day", " d")
        ),
        None => relative.to_string(),
    }
}

/// La insignia de una revisión: de dónde viene si fue un renombre o una
/// copia (con la carpeta solo si cambió), o si lo creó o lo borró.
pub fn revision_badge(revision: &FileRevision) -> Option<SharedString> {
    let file_name = |path: &str| path.rsplit('/').next().unwrap_or(path).to_string();
    let dir = |path: &str| path.rsplit_once('/').map(|(dir, _)| dir.to_string());
    match revision.status.as_str() {
        "R" | "C" => {
            let previous = revision.previous_path.as_deref()?;
            let shown = if dir(previous) == dir(&revision.path) {
                file_name(previous)
            } else {
                previous.to_string()
            };
            let verb = if revision.status == "R" {
                "renamed from"
            } else {
                "copied from"
            };
            Some(format!("{verb} {shown}").into())
        }
        "A" => Some("added".into()),
        "D" => Some("deleted".into()),
        _ => None,
    }
}

/// Id del elemento de la fila de una revisión, para las pruebas.
pub fn file_history_row_id(hash: &str) -> SharedString {
    SharedString::from(format!("file-revision:{hash}"))
}

pub struct FileHistoryList {
    model: Entity<AppModel>,
    source: Option<FileHistorySource>,
    rows: Rc<Vec<HistoryRow>>,
    selected: Option<usize>,
    has_more: bool,
    loading: bool,
    loading_more: bool,
    error: Option<String>,
    request: u64,
    scroll: UniformListScrollHandle,
    focus: FocusHandle,
    load_count: usize,
    _load: Option<Task<()>>,
}

impl EventEmitter<FileHistoryListEvent> for FileHistoryList {}

impl Focusable for FileHistoryList {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl FileHistoryList {
    pub fn new(_: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            model: AppModel::global(cx),
            source: None,
            rows: Rc::default(),
            selected: None,
            has_more: false,
            loading: false,
            loading_more: false,
            error: None,
            request: 0,
            scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle().tab_stop(true),
            load_count: 0,
            _load: None,
        }
    }

    // --- API ------------------------------------------------------------------

    /// El archivo cuya historia se muestra. Si cambia, se vacía y se pide la
    /// primera página; si es el mismo, no hace nada.
    pub fn set_source(&mut self, source: FileHistorySource, cx: &mut Context<Self>) {
        if self.source.as_ref() == Some(&source) {
            return;
        }
        self.source = Some(source);
        self.rows = Rc::default();
        self.selected = None;
        self.has_more = false;
        self.error = None;
        self.load_page(0, FILE_HISTORY_PAGE_SIZE, None, cx);
    }

    /// Vuelve a leer lo ya cargado (otro commit movió `HEAD`, o "Retry"),
    /// conservando la revisión elegida si sigue ahí.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        if self.source.is_none() {
            return;
        }
        let keep = self
            .selected_revision()
            .map(|revision| revision.hash.clone());
        let count = (self.rows.len() as u32).max(FILE_HISTORY_PAGE_SIZE);
        self.error = None;
        self.load_page(0, count, keep, cx);
    }

    /// La página siguiente ("Load more", o al llegar al final de la lista).
    pub fn load_more(&mut self, cx: &mut Context<Self>) {
        if !self.has_more || self.loading || self.loading_more || self.source.is_none() {
            return;
        }
        let skip = self.rows.len() as u32;
        self.load_page(skip, FILE_HISTORY_PAGE_SIZE, None, cx);
    }

    /// Elige la revisión `index` y avisa.
    pub fn select(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get(index) else {
            return;
        };
        let revision = row.revision.clone();
        if self.selected != Some(index) {
            self.selected = Some(index);
            self.scroll.scroll_to_item(index, ScrollStrategy::Nearest);
            cx.notify();
        }
        cx.emit(FileHistoryListEvent::Select(revision));
    }

    pub fn source(&self) -> Option<&FileHistorySource> {
        self.source.as_ref()
    }

    pub fn rows(&self) -> &[HistoryRow] {
        &self.rows
    }

    pub fn selected_index(&self) -> Option<usize> {
        self.selected
    }

    pub fn selected_revision(&self) -> Option<&FileRevision> {
        self.selected
            .and_then(|index| self.rows.get(index))
            .map(|row| &row.revision)
    }

    pub fn has_more(&self) -> bool {
        self.has_more
    }

    /// Cargando la primera página (o releyendo).
    pub fn is_loading(&self) -> bool {
        self.loading
    }

    pub fn is_loading_more(&self) -> bool {
        self.loading_more
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Lecturas de git hechas (para las pruebas de paginación).
    pub fn load_count(&self) -> usize {
        self.load_count
    }

    // --- interno ----------------------------------------------------------------

    fn load_page(
        &mut self,
        skip: u32,
        max_count: u32,
        keep_selected: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(source) = self.source.clone() else {
            return;
        };
        self.request += 1;
        self.load_count += 1;
        let version = self.request;
        let appending = skip > 0;
        if appending {
            self.loading_more = true;
        } else {
            self.loading = true;
        }
        cx.notify();

        let read = self.model.update(cx, |model, cx| {
            model.get_file_history(
                &source.repo_path,
                &source.path,
                FileHistoryOptions {
                    revision: source.revision.clone(),
                    skip: Some(skip),
                    max_count: Some(max_count),
                },
                cx,
            )
        });
        self._load = Some(cx.spawn(async move |this, cx| {
            // Las fechas relativas y las insignias se calculan una vez, aquí,
            // y no en cada render.
            let page = read.await.map(
                |FileHistoryPage {
                     revisions,
                     has_more,
                 }| {
                    let rows: Vec<HistoryRow> =
                        revisions.into_iter().map(HistoryRow::new).collect();
                    (rows, has_more)
                },
            );
            let _ = this.update(cx, |this, cx| {
                if version != this.request {
                    return;
                }
                this.loading = false;
                this.loading_more = false;
                match page {
                    Ok((rows, has_more)) => {
                        this.has_more = has_more;
                        this.apply_rows(rows, appending, keep_selected, cx);
                    }
                    Err(error) => {
                        this.error = Some(error.message().to_string());
                        this.has_more = false;
                    }
                }
                cx.notify();
            });
        }));
    }

    fn apply_rows(
        &mut self,
        rows: Vec<HistoryRow>,
        appending: bool,
        keep_selected: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if appending {
            let mut all = Vec::with_capacity(self.rows.len() + rows.len());
            all.extend(self.rows.iter().cloned());
            all.extend(rows);
            self.rows = Rc::new(all);
            return;
        }
        self.rows = Rc::new(rows);
        let index = keep_selected
            .and_then(|hash| self.rows.iter().position(|row| row.revision.hash == hash))
            .or_else(|| (!self.rows.is_empty()).then_some(0));
        let previous = self
            .selected_revision()
            .map(|revision| revision.hash.clone());
        self.selected = None;
        if let Some(index) = index {
            // Al releer con la misma revisión elegida, el diff no cambia.
            if previous.as_deref() == Some(self.rows[index].revision.hash.as_str()) {
                self.selected = Some(index);
            } else {
                self.select(index, cx);
            }
        }
    }

    fn handle_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let modifiers = event.keystroke.modifiers;
        if modifiers.control || modifiers.alt || modifiers.platform || modifiers.shift {
            return;
        }
        let last = self.rows.len().checked_sub(1);
        let next = match (event.keystroke.key.as_str(), self.selected, last) {
            (_, _, None) => return,
            ("down", Some(index), Some(last)) => (index + 1).min(last),
            ("down", None, _) => 0,
            ("up", Some(index), _) => index.saturating_sub(1),
            ("up", None, _) => 0,
            ("home", _, _) => 0,
            ("end", _, Some(last)) => last,
            _ => return,
        };
        cx.stop_propagation();
        if Some(next) != self.selected {
            self.select(next, cx);
        }
        // En la última fila, la página siguiente.
        if Some(next) == last {
            self.load_more(cx);
        }
    }

    fn header(&self, cx: &App) -> impl IntoElement {
        let palette = *cx.palette();
        let name = self
            .source
            .as_ref()
            .map(|source| {
                source
                    .path
                    .rsplit('/')
                    .next()
                    .unwrap_or(&source.path)
                    .to_string()
            })
            .unwrap_or_default();
        let path = self
            .source
            .as_ref()
            .map(|source| source.path.clone())
            .unwrap_or_default();
        let count = match (self.rows.len(), self.has_more) {
            (0, _) => String::new(),
            (1, false) => "1 revision".to_string(),
            (count, false) => format!("{count} revisions"),
            (count, true) => format!("{count}+ revisions"),
        };
        div()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .gap(px(1.))
            .py(rems(0.42))
            .px(rems(0.65))
            .border_b_1()
            .border_color(palette.border)
            .bg(palette.accent)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .min_w_0()
                            .text_size(rems(0.75))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(palette.foreground)
                            .child(
                                mergeon_ui::icon(Lucide::FileClock)
                                    .size(px(13.))
                                    .tone(mergeon_ui::IconTone::Muted),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .child(format!("History · {name}")),
                            ),
                    )
                    .child(
                        div()
                            .id("file-history-count")
                            .test_support()
                            .aria_label(SharedString::from(count.clone()))
                            .flex_shrink_0()
                            .text_size(rems(0.75))
                            .text_color(palette.muted_foreground)
                            .child(count),
                    ),
            )
            .child(
                div()
                    .id("file-history-path")
                    .min_w_0()
                    .truncate()
                    .font_family(code_font_family(cx))
                    .text_size(px(11.))
                    .text_color(palette.text_3)
                    .tooltip(mergeon_ui::tooltip(path.clone()))
                    .child(path),
            )
    }

    fn placeholder(text: impl Into<SharedString>, spinner: bool, cx: &App) -> AnyElement {
        div()
            .id("file-history-state")
            .test_support()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_2()
            .p(px(16.))
            .size_full()
            .text_size(rems(0.8125))
            .text_center()
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
}

/// El menú de una revisión.
fn revision_menu(view: &WeakEntity<FileHistoryList>, revision: &FileRevision) -> Vec<MenuItem> {
    let emit = |event: FileHistoryListEvent| {
        let view = view.clone();
        move |_: &mut Window, cx: &mut App| {
            let _ = view.update(cx, |_, cx| cx.emit(event.clone()));
        }
    };
    let hash = revision.hash.clone();
    vec![
        MenuItem::new("Show commit in history")
            .icon(Lucide::GitCommitHorizontal)
            .on_click(emit(FileHistoryListEvent::ShowCommit(
                revision.hash.clone(),
            ))),
        MenuItem::new("View file at this revision")
            .icon(Lucide::FileCodeCorner)
            // Borrado en esta revisión: no hay archivo que ver.
            .disabled(revision.status == "D")
            .on_click(emit(FileHistoryListEvent::ViewFileAt(revision.clone()))),
        MenuItem::separator(),
        MenuItem::new("Copy commit hash")
            .icon(Lucide::Copy)
            .on_click(move |_, cx| {
                cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(hash.clone()));
            }),
    ]
}

fn revision_row(
    index: usize,
    row: &HistoryRow,
    selected: bool,
    this: &WeakEntity<FileHistoryList>,
    focus: &FocusHandle,
    cx: &App,
) -> AnyElement {
    let focus = focus.clone();
    let palette = *cx.palette();
    let revision = &row.revision;
    let mono = code_font_family(cx);
    let (text, quiet, dim) = if selected {
        (palette.sel_fg, palette.sel_fg_2, palette.sel_fg_2)
    } else {
        (palette.text_1, palette.text_2, palette.text_3)
    };
    let select_to = this.clone();
    let commit_to = this.clone();
    let menu_to = this.clone();
    let menu_revision = revision.clone();
    let hash = revision.hash.clone();
    let tooltip = format!(
        "{}\n{} <{}> · {}",
        revision.subject,
        revision.author,
        revision.email,
        format_date_time(revision.date.as_str())
    );

    div()
        .id(file_history_row_id(&revision.hash))
        .test_support()
        .aria_label(SharedString::from(revision.subject.clone()))
        .aria_selected(selected)
        .relative()
        .flex()
        .flex_col()
        .justify_center()
        .gap(px(3.))
        .flex_shrink_0()
        .w_full()
        .h(px(FILE_HISTORY_ROW_HEIGHT))
        .px(rems(0.65))
        .border_b_1()
        .border_color(palette.border_hair)
        .cursor_pointer()
        .text_color(text)
        .when(!selected, |row| {
            row.hover(|style| style.bg(palette.bg_hover))
        })
        .when(selected, |row| {
            row.bg(palette.sel_bg).child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .bottom_0()
                    .w(px(2.))
                    .bg(palette.primary),
            )
        })
        .on_click(move |event: &ClickEvent, window, cx| {
            if !event.standard_click() {
                return;
            }
            // Tras un clic, las flechas siguen desde ahí.
            window.focus(&focus, cx);
            let _ = select_to.update(cx, |list, cx| list.select(index, cx));
        })
        .tooltip(mergeon_ui::tooltip(tooltip))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .min_w_0()
                .child(
                    div()
                        .flex_1()
                        .min_w(px(56.))
                        .truncate()
                        .text_size(px(12.5))
                        .font_weight(FontWeight::MEDIUM)
                        .child(revision.subject.clone()),
                )
                .children(row.badge.clone().map(|badge| {
                    let id = SharedString::from(format!("revision-badge:{}", revision.hash));
                    let shell = div()
                        .id(id)
                        .test_support()
                        .aria_label(badge.clone())
                        .flex_shrink_1()
                        .min_w_0()
                        .max_w(relative(0.62));
                    match revision.status.as_str() {
                        "A" | "D" => shell.child(
                            mergeon_ui::badge(badge)
                                .variant(if revision.status == "A" {
                                    BadgeVariant::Success
                                } else {
                                    BadgeVariant::Destructive
                                })
                                .on_ink(selected),
                        ),
                        // "renamed from …" puede ser largo: la pill de etiqueta
                        // de Señal, recortada con puntos suspensivos.
                        _ => shell
                            .h(px(16.))
                            .px(px(5.))
                            .rounded(px(2.))
                            .bg(if selected {
                                with_alpha(palette.sel_fg, 0.14)
                            } else {
                                palette.bg_pressed.into()
                            })
                            .text_color(if selected {
                                palette.sel_fg_2
                            } else {
                                palette.text_2
                            })
                            .text_size(px(10.5))
                            .line_height(px(16.))
                            .truncate()
                            .tooltip(mergeon_ui::tooltip(badge.clone()))
                            .child(badge),
                    }
                })),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .min_w_0()
                .text_size(px(11.5))
                .child(monogram(&revision.author, 16., 8.5, selected, cx))
                .child(
                    div()
                        .min_w_0()
                        .flex_shrink_1()
                        .truncate()
                        .text_color(quiet)
                        .child(revision.author.clone()),
                )
                .child(div().flex_shrink_0().text_color(dim).child("·"))
                .child(
                    div()
                        .flex_shrink_0()
                        .text_color(dim)
                        .child(row.relative_date.clone()),
                )
                .child(div().flex_1())
                .child(
                    div()
                        .id(SharedString::from(format!(
                            "revision-hash:{}",
                            revision.hash
                        )))
                        .test_support()
                        .aria_label(SharedString::from(revision.short_hash.clone()))
                        .flex_shrink_0()
                        .font_family(mono)
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(if selected {
                            palette.sel_fg
                        } else {
                            palette.primary_text
                        })
                        .hover(|style| style.underline())
                        .tooltip(mergeon_ui::tooltip("Show this commit in the history"))
                        .on_click(move |_, _, cx| {
                            cx.stop_propagation();
                            let hash = hash.clone();
                            let _ = commit_to.update(cx, |_, cx| {
                                cx.emit(FileHistoryListEvent::ShowCommit(hash));
                            });
                        })
                        .child(revision.short_hash.clone()),
                ),
        )
        .context_menu_items(move |_, _| revision_menu(&menu_to, &menu_revision))
        .into_any_element()
}

impl Render for FileHistoryList {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let shell = div()
            .id("file-history-list")
            .test_support()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::handle_key_down))
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .min_w_0()
            .overflow_hidden()
            .border_r_1()
            .border_color(palette.border)
            .bg(palette.card)
            .line_height(relative(1.3))
            .child(self.header(cx));

        if self.loading && self.rows.is_empty() {
            return shell.child(Self::placeholder("Loading history…", true, cx));
        }
        if let Some(error) = self.error.clone()
            && self.rows.is_empty()
        {
            return shell.child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_2()
                    .child(Self::placeholder(error, false, cx))
                    .child(
                        mergeon_ui::button("file-history-retry")
                            .outline()
                            .sm()
                            .label("Retry")
                            .on_click(cx.listener(|list, _, _, cx| list.reload(cx))),
                    ),
            );
        }
        if self.rows.is_empty() {
            return shell.child(
                div().flex_1().child(
                    mergeon_ui::empty_state("No history yet")
                        .icon(Lucide::FileClock)
                        .description("No commit has touched this file."),
                ),
            );
        }

        let rows = self.rows.clone();
        let selected = self.selected;
        let has_more = self.has_more;
        let loading_more = self.loading_more;
        let this = cx.entity().downgrade();
        let focus = self.focus.clone();
        // Una fila más al final para "Load more" mientras queden revisiones.
        let count = rows.len() + usize::from(has_more);
        let list = uniform_list("file-history-rows", count, move |range, _, cx| {
            // Al asomar el final, la página siguiente.
            if has_more && !loading_more && range.end >= rows.len() {
                let this = this.clone();
                cx.defer(move |cx| {
                    let _ = this.update(cx, |list, cx| list.load_more(cx));
                });
            }
            range
                .map(|index| match rows.get(index) {
                    Some(row) => {
                        revision_row(index, row, selected == Some(index), &this, &focus, cx)
                    }
                    None => load_more_row(loading_more, &this, cx),
                })
                .collect()
        })
        .track_scroll(&self.scroll)
        .size_full();

        shell.child(
            div()
                .relative()
                .flex_1()
                .min_h_0()
                .child(list)
                .vertical_scrollbar(&self.scroll),
        )
    }
}

fn load_more_row(loading: bool, this: &WeakEntity<FileHistoryList>, cx: &App) -> AnyElement {
    let this = this.clone();
    div()
        .flex()
        .items_center()
        .justify_center()
        .gap_2()
        .h(px(FILE_HISTORY_ROW_HEIGHT))
        .text_size(rems(0.75))
        .text_color(cx.palette().muted_foreground)
        .child(
            mergeon_ui::button("file-history-load-more")
                .ghost()
                .sm()
                .label(if loading { "Loading…" } else { "Load more" })
                .loading(loading)
                .on_click(move |_, _, cx| {
                    let _ = this.update(cx, |list, cx| list.load_more(cx));
                }),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn revision(status: &str, path: &str, previous: Option<&str>) -> FileRevision {
        FileRevision {
            status: status.into(),
            path: path.into(),
            previous_path: previous.map(str::to_string),
            ..FileRevision::default()
        }
    }

    #[test]
    fn badges_say_where_a_renamed_file_came_from() {
        assert_eq!(
            revision_badge(&revision("R", "src/new.rs", Some("src/old.rs"))).as_deref(),
            Some("renamed from old.rs")
        );
        // Otra carpeta: la ruta entera.
        assert_eq!(
            revision_badge(&revision("R", "lib/new.rs", Some("src/old.rs"))).as_deref(),
            Some("renamed from src/old.rs")
        );
        assert_eq!(
            revision_badge(&revision("C", "b.rs", Some("a.rs"))).as_deref(),
            Some("copied from a.rs")
        );
        assert_eq!(
            revision_badge(&revision("A", "a", None)).as_deref(),
            Some("added")
        );
        assert_eq!(
            revision_badge(&revision("D", "a", None)).as_deref(),
            Some("deleted")
        );
        assert_eq!(revision_badge(&revision("M", "a", None)), None);
        assert_eq!(revision_badge(&revision("", "a", None)), None);
    }
}
