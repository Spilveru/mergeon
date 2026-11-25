//! Filtros de la búsqueda de commits: los campos plegables de "More
//! filters", los chips de los filtros activos, el tooltip de la sintaxis
//! rápida y "Load more".

use gpui_kit::component::Sizable as _;
use gpui_kit::component::input::Input;
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, Context, Entity, SharedString, TestSupportExt as _, Window, div, px, rems,
};
use mergeon_backend::{Commit, CommitSearchOptions};
use mergeon_ui::{ActivePaletteExt as _, IconTone, InputState, Lucide, Radius, TabItem};

use crate::logic::{
    CommitSearchQuery, DatePreset, SEARCH_SYNTAX_HELP, SearchFilterFields, SearchFilterKind,
    merge_search_filters, parse_commit_search_query, remove_search_filters, search_filter_chips,
};
use crate::sidebar::{CommitSearchResult, Sidebar};

/// Resultados por página de la búsqueda.
pub const SEARCH_PAGE_SIZE: u32 = 100;

/// Los campos de "More filters". Los de texto son `InputState`; el preset de
/// fecha y la casilla de regex, valores.
pub(crate) struct SearchFilterInputs {
    pub open: bool,
    pub path: Entity<InputState>,
    pub branch: Entity<InputState>,
    pub after: Entity<InputState>,
    pub before: Entity<InputState>,
    pub code: Entity<InputState>,
    pub date: DatePreset,
    pub code_regex: bool,
}

impl SearchFilterInputs {
    pub fn new(window: &mut Window, cx: &mut App) -> Self {
        let input = |placeholder: &'static str, window: &mut Window, cx: &mut App| {
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
        };
        Self {
            open: false,
            path: input("File or folder, e.g. src/app", window, cx),
            branch: input("All branches", window, cx),
            after: input("YYYY-MM-DD", window, cx),
            before: input("YYYY-MM-DD", window, cx),
            code: input("Text added or removed", window, cx),
            date: DatePreset::AnyTime,
            code_regex: false,
        }
    }

    pub fn inputs(&self) -> [Entity<InputState>; 5] {
        [
            self.path.clone(),
            self.branch.clone(),
            self.after.clone(),
            self.before.clone(),
            self.code.clone(),
        ]
    }

    pub fn fields(&self, cx: &App) -> SearchFilterFields {
        let value = |input: &Entity<InputState>| input.read(cx).value().to_string();
        SearchFilterFields {
            path: value(&self.path),
            branch: value(&self.branch),
            date: self.date,
            after: value(&self.after),
            before: value(&self.before),
            code: value(&self.code),
            code_regex: self.code_regex,
        }
    }
}

/// Los commits de git como resultados de la búsqueda.
pub(crate) fn search_results(commits: Vec<Commit>) -> Vec<CommitSearchResult> {
    commits
        .into_iter()
        .map(|commit| CommitSearchResult {
            hash: commit.hash,
            full_hash: commit.full_hash,
            message: if commit.message.is_empty() {
                "(no message)".to_string()
            } else {
                commit.message
            },
            author: commit.author,
        })
        .collect()
}

fn options_from(query: CommitSearchQuery) -> CommitSearchOptions {
    let some = |value: String| Some(value).filter(|value| !value.is_empty());
    CommitSearchOptions {
        max_count: Some(SEARCH_PAGE_SIZE),
        skip: None,
        text: Some(query.text),
        author: Some(query.author),
        hash: Some(query.hash),
        path: some(query.path),
        branch: some(query.branch),
        since: some(query.after),
        until: some(query.before),
        code: some(query.code),
        code_regex: query.code_regex,
    }
}

/// Un campo de "More filters" con la medida del cuadro de búsqueda (alto 26,
/// texto pequeño): el `TextField` normal es demasiado grande para la barra.
fn compact_input(input: &Entity<InputState>, cx: &App) -> impl IntoElement {
    let palette = cx.palette();
    div()
        .flex()
        .items_center()
        .h(px(26.))
        .px(rems(0.5))
        .border_1()
        .border_color(palette.border)
        .rounded(Radius::Md.px(cx))
        .bg(palette.background)
        .child(
            Input::new(input)
                .small()
                .appearance(false)
                .cleanable(true)
                .px_0()
                .text_size(rems(0.75)),
        )
}

impl Sidebar {
    /// Lo que se buscaría ahora: el cuadro más "More filters". `None` si no
    /// hay nada que buscar.
    pub(crate) fn current_search_options(&self, cx: &App) -> Option<CommitSearchOptions> {
        let query = self.current_search_query(cx);
        (!query.is_empty()).then(|| options_from(query))
    }

    fn current_search_query(&self, cx: &App) -> CommitSearchQuery {
        self.current_search_query_with(&self.search_filters.fields(cx), cx)
    }

    /// La query actual con unos campos ya leídos.
    pub(crate) fn current_search_query_with(
        &self,
        fields: &SearchFilterFields,
        cx: &App,
    ) -> CommitSearchQuery {
        merge_search_filters(
            parse_commit_search_query(&self.search_query.read(cx).value()),
            fields,
        )
    }

    /// Los valores de "More filters".
    pub fn search_filter_fields(&self, cx: &App) -> SearchFilterFields {
        self.search_filters.fields(cx)
    }

    /// Las opciones con las que se pidieron los resultados a la vista.
    pub fn search_options(&self) -> Option<&CommitSearchOptions> {
        self.search_options.as_ref()
    }

    /// El fallo de la última búsqueda, si lo hubo.
    pub fn search_error(&self) -> Option<&str> {
        self.search_error.as_deref()
    }

    /// El fallo del último "Load more" (los resultados siguen a la vista).
    pub fn search_load_more_error(&self) -> Option<&str> {
        self.search_more_error.as_deref()
    }

    /// La última página vino llena: "Load more" trae la siguiente.
    pub fn search_has_more(&self) -> bool {
        self.search_has_more
    }

    pub fn is_search_filters_open(&self) -> bool {
        self.search_filters.open
    }

    pub fn toggle_search_filters(&mut self, cx: &mut Context<Self>) {
        self.search_filters.open = !self.search_filters.open;
        cx.notify();
    }

    /// Rellena "More filters" (como si lo hiciera el usuario) y busca sin
    /// esperar a la pausa de tecleo.
    pub fn set_search_filters(
        &mut self,
        fields: SearchFilterFields,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let inputs = &self.search_filters;
        for (input, value) in [
            (&inputs.path, fields.path),
            (&inputs.branch, fields.branch),
            (&inputs.after, fields.after),
            (&inputs.before, fields.before),
            (&inputs.code, fields.code),
        ] {
            input.update(cx, |state, cx| state.set_value(value, window, cx));
        }
        self.search_filters.date = fields.date;
        self.search_filters.code_regex = fields.code_regex;
        self.run_commit_search(cx).detach();
    }

    pub(crate) fn reset_search_filters(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for input in self.search_filters.inputs() {
            input.update(cx, |state, cx| state.set_value("", window, cx));
        }
        self.search_filters.date = DatePreset::AnyTime;
        self.search_filters.code_regex = false;
    }

    pub fn set_search_date_preset(&mut self, preset: DatePreset, cx: &mut Context<Self>) {
        if self.search_filters.date != preset {
            self.search_filters.date = preset;
            self.run_commit_search(cx).detach();
        }
    }

    pub fn set_search_code_regex(&mut self, regex: bool, cx: &mut Context<Self>) {
        if self.search_filters.code_regex != regex {
            self.search_filters.code_regex = regex;
            self.run_commit_search(cx).detach();
        }
    }

    /// Quita un filtro (la × de su chip): del cuadro y de "More filters".
    pub fn clear_search_filter(
        &mut self,
        kind: SearchFilterKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let query = self.search_query.read(cx).value().to_string();
        let without = remove_search_filters(&query, kind.keys());
        if without != query.split_whitespace().collect::<Vec<_>>().join(" ") {
            self.search_query
                .update(cx, |state, cx| state.set_value(without, window, cx));
        }
        let inputs = &self.search_filters;
        let cleared: Vec<Entity<InputState>> = match kind {
            SearchFilterKind::Path => vec![inputs.path.clone()],
            SearchFilterKind::Branch => vec![inputs.branch.clone()],
            SearchFilterKind::Date => vec![inputs.after.clone(), inputs.before.clone()],
            SearchFilterKind::Code => vec![inputs.code.clone()],
        };
        for input in cleared {
            input.update(cx, |state, cx| state.set_value("", window, cx));
        }
        match kind {
            SearchFilterKind::Date => self.search_filters.date = DatePreset::AnyTime,
            SearchFilterKind::Code => self.search_filters.code_regex = false,
            _ => {}
        }
        self.run_commit_search(cx).detach();
    }

    /// "Load more": la página siguiente de la misma búsqueda.
    pub fn load_more_search_results(&mut self, cx: &mut Context<Self>) {
        if !self.search_has_more || self.search_loading || self.search_loading_more {
            return;
        }
        let (Some(repo), Some(options)) = (self.active_repo(cx), self.search_options.clone())
        else {
            return;
        };
        let token = self.search_token;
        let options = CommitSearchOptions {
            skip: Some(self.search_results.len() as u32),
            ..options
        };
        self.search_loading_more = true;
        self.search_more_error = None;
        cx.notify();
        let search = self.model.update(cx, |model, cx| {
            model.search_commits(&repo.path, options, cx)
        });
        cx.spawn(async move |this, cx| {
            let result = search.await;
            this.update(cx, |this, cx| {
                // Otra búsqueda empezó mientras tanto: esta página ya no vale.
                if token != this.search_token {
                    return;
                }
                this.search_loading_more = false;
                match result {
                    Ok(commits) => {
                        this.search_has_more = commits.len() >= SEARCH_PAGE_SIZE as usize;
                        this.append_search_results(search_results(commits));
                    }
                    // Lo ya cargado sigue a la vista; el fallo va bajo la lista
                    // y "Load more" queda para reintentar.
                    Err(error) => {
                        this.search_more_error = Some(if error.message().is_empty() {
                            "Failed to load more commits".to_string()
                        } else {
                            error.message().to_string()
                        });
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // --- pintado ----------------------------------------------------------------

    /// "Search Commits", la ayuda de la sintaxis y el botón de los filtros.
    pub(crate) fn render_search_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let open = self.search_filters.open;
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(rems(0.3))
            .child(
                div()
                    .flex_1()
                    .text_size(rems(0.8125))
                    .line_height(gpui_kit::relative(1.5))
                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                    .text_color(palette.muted_foreground)
                    .child("Search Commits"),
            )
            .child(
                div()
                    .id("commit-search-help")
                    .test_support()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(20.))
                    .rounded(Radius::Md.px(cx))
                    .hover(|style| style.bg(palette.sidebar_row_hover))
                    .child(
                        mergeon_ui::icon(Lucide::CircleQuestionMark)
                            .size(px(13.))
                            .tone(IconTone::Muted),
                    )
                    .tooltip(mergeon_ui::tooltip(SEARCH_SYNTAX_HELP)),
            )
            .child(
                mergeon_ui::button("commit-search-filters")
                    .ghost()
                    .xs()
                    .icon(Lucide::ListFilter)
                    .label("Filters")
                    .trailing_icon(if open {
                        Lucide::ChevronUp
                    } else {
                        Lucide::ChevronDown
                    })
                    .selected(open)
                    .tooltip(if open {
                        "Hide the filters"
                    } else {
                        "Filter by path, branch, date or changed code"
                    })
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_search_filters(cx))),
            )
    }

    /// Los chips de los filtros activos, con su ×.
    pub(crate) fn render_search_chips(
        &self,
        query: &CommitSearchQuery,
        fields: &SearchFilterFields,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let chips = search_filter_chips(query, fields);
        if chips.is_empty() {
            return None;
        }
        let palette = *cx.palette();
        Some(
            div()
                .id("commit-search-chips")
                .test_support()
                .flex()
                .flex_shrink_0()
                .flex_wrap()
                .gap(px(4.))
                .children(chips.into_iter().map(|(kind, label)| {
                    let icon = match kind {
                        SearchFilterKind::Path => Lucide::Folder,
                        SearchFilterKind::Branch => Lucide::GitBranch,
                        SearchFilterKind::Date => Lucide::Calendar,
                        SearchFilterKind::Code => Lucide::Code,
                    };
                    div()
                        .id(SharedString::from(format!("search-chip:{}", kind.id())))
                        .test_support()
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .max_w_full()
                        .h(px(20.))
                        .pl(px(6.))
                        .pr(px(2.))
                        .rounded(Radius::Sm.px(cx))
                        .border_1()
                        .border_color(palette.border_control)
                        .bg(palette.bg_raised)
                        .text_size(px(11.))
                        .text_color(palette.text_1)
                        .child(mergeon_ui::icon(icon).size(px(11.)).tone(IconTone::Muted))
                        .child(div().min_w_0().truncate().child(label))
                        .child(
                            div()
                                .id(SharedString::from(format!(
                                    "search-chip-clear:{}",
                                    kind.id()
                                )))
                                .test_support()
                                .flex()
                                .items_center()
                                .justify_center()
                                .size(px(16.))
                                .rounded(Radius::Sm.px(cx))
                                .cursor_pointer()
                                .hover(|style| style.bg(palette.bg_hover))
                                .child(
                                    mergeon_ui::icon(Lucide::X)
                                        .size(px(10.))
                                        .tone(IconTone::Muted),
                                )
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.clear_search_filter(kind, window, cx);
                                })),
                        )
                }))
                .into_any_element(),
        )
    }

    /// "More filters": ruta, rama, fechas y código.
    pub(crate) fn render_search_fields(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let inputs = &self.search_filters;
        let label = |text: &'static str| {
            div()
                .text_size(px(11.))
                .font_weight(gpui_kit::FontWeight::MEDIUM)
                .text_color(palette.text_3)
                .child(text)
        };
        let field = |id: &'static str, text: &'static str, input: &Entity<InputState>, cx: &App| {
            div()
                .id(id)
                .test_support()
                .flex()
                .flex_col()
                .gap(px(3.))
                .child(label(text))
                .child(compact_input(input, cx))
        };
        let path = field("search-field-path", "Path", &inputs.path, cx);
        let branch = field("search-field-branch", "Branch", &inputs.branch, cx);
        let after = field("search-field-after", "After", &inputs.after, cx);
        let before = field("search-field-before", "Before", &inputs.before, cx);
        let code = field("search-field-code", "Code contains", &inputs.code, cx);
        let preset = self.search_filters.date;
        let regex = self.search_filters.code_regex;

        div()
            .id("commit-search-fields")
            .test_support()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .gap(px(8.))
            .p(px(8.))
            .rounded(Radius::Lg.px(cx))
            .border_1()
            .border_color(palette.border)
            .bg(palette.bg_inset)
            .child(path)
            .child(branch)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .child(label("Date"))
                    .child(
                        mergeon_ui::tabs("search-date-preset")
                            .full_width(true)
                            .item(TabItem::new(DatePreset::AnyTime.key(), "Any"))
                            .item(TabItem::new(DatePreset::Last7Days.key(), "7d"))
                            .item(TabItem::new(DatePreset::Last30Days.key(), "30d"))
                            .item(TabItem::new(DatePreset::Custom.key(), "Custom"))
                            .selected(preset.key())
                            .on_change(cx.listener(|this, key: &SharedString, _, cx| {
                                this.set_search_date_preset(DatePreset::from_key(key), cx);
                            })),
                    )
                    .when(preset == DatePreset::Custom, |this| {
                        this.child(
                            div()
                                .flex()
                                .gap(px(6.))
                                .child(div().flex_1().min_w_0().child(after))
                                .child(div().flex_1().min_w_0().child(before)),
                        )
                    }),
            )
            .child(code)
            .child(
                mergeon_ui::checkbox("search-code-regex")
                    .label("Regular expression")
                    .checked(regex)
                    .on_change(cx.listener(|this, checked: &bool, _, cx| {
                        this.set_search_code_regex(*checked, cx);
                    })),
            )
    }

    /// "Load more" bajo los resultados, si la última página vino llena.
    pub(crate) fn render_load_more(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.search_has_more {
            return None;
        }
        let palette = *cx.palette();
        Some(
            div()
                .flex()
                .flex_col()
                .flex_shrink_0()
                .gap(px(4.))
                .when_some(self.search_more_error.clone(), |this, error| {
                    this.child(
                        div()
                            .id("commit-search-load-more-error")
                            .test_support()
                            .text_size(rems(0.75))
                            .line_height(gpui_kit::relative(1.4))
                            .text_color(palette.destructive_foreground)
                            .child(error),
                    )
                })
                .child(
                    mergeon_ui::button("commit-search-load-more")
                        .ghost()
                        .xs()
                        .full_width(true)
                        .loading(self.search_loading_more)
                        .label(if self.search_more_error.is_some() {
                            "Try again".to_string()
                        } else {
                            format!("Load {SEARCH_PAGE_SIZE} more")
                        })
                        .on_click(cx.listener(|this, _, _, cx| this.load_more_search_results(cx))),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_the_backend_options_from_the_merged_query() {
        let query = merge_search_filters(
            parse_commit_search_query("fix branch:main code:parse_header"),
            &SearchFilterFields {
                path: "src".into(),
                date: DatePreset::Last30Days,
                ..Default::default()
            },
        );
        let options = options_from(query);
        assert_eq!(options.max_count, Some(SEARCH_PAGE_SIZE));
        assert_eq!(options.text.as_deref(), Some("fix"));
        assert_eq!(options.branch.as_deref(), Some("main"));
        assert_eq!(options.path.as_deref(), Some("src"));
        assert_eq!(options.since.as_deref(), Some("30 days ago"));
        assert_eq!(options.until, None);
        assert_eq!(options.code.as_deref(), Some("parse_header"));
        assert!(!options.code_regex);
    }
}
