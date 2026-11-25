//! Comparación navegable de dos ramas o revisiones: cabecera (A, B, modo,
//! ahead/behind), commits de cada lado, archivos cambiados con sus cifras y el
//! diff del archivo elegido. Reutiliza [`CommitChangesTab`] (lista + visor).
//!
//! Las dos revisiones se fijan a su SHA al abrir: si una rama se mueve, la
//! comparación no cambia en silencio; "Refresh" las vuelve a resolver.

use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, ClickEvent, Context, Entity, EventEmitter, FontWeight, SharedString,
    Subscription, UniformListScrollHandle, WeakEntity, Window, div, px, relative, rems,
    uniform_list,
};
use mergeon_app::AppModel;
use mergeon_backend::{
    CompareLimits, CompareMode, ComparedCommit, ComparedFile, DiffOptions, FileChange,
    RevisionComparison,
};
use mergeon_ui::dialog::{AppDialog, DialogWidth};
use mergeon_ui::{
    ActivePaletteExt as _, IconTone, Lucide, Radius, SpinnerColor, SpinnerSize, TabItem,
};
use mergeon_view_diff::{code_font_family, is_image_path};
use mergeon_view_sidebar::ComparisonRequest;

use crate::changes_tab::{CommitChangesTab, CommitChangesTabEvent};

/// Alto de una fila de la lista de commits (cabeceras incluidas).
const COMMIT_ROW_HEIGHT: f32 = 28.;
/// Ancho de la columna de commits.
const COMMITS_PANE_WIDTH: f32 = 300.;

/// Lo que la comparación pide a quien la abrió.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompareViewEvent {
    /// Doble clic en un commit: mostrarlo en el historial.
    ShowCommit { full_hash: String },
}

/// Una fila de la columna de commits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompareCommitRow {
    /// "On <rama> · N".
    Header(SharedString),
    Commit(ComparedCommit),
    /// "and N more commits".
    More(SharedString),
}

/// Frase que describe el modo, como la lee el usuario.
pub fn compare_mode_description(mode: CompareMode, base: &str, head: &str) -> String {
    match mode {
        CompareMode::SinceDivergence => {
            format!("Changes on {head} since it diverged from {base}")
        }
        CompareMode::Direct => format!("Direct diff {base} \u{2192} {head}"),
    }
}

fn more_label(omitted: usize, noun: &str) -> Option<SharedString> {
    match omitted {
        0 => None,
        1 => Some(format!("and 1 more {noun}").into()),
        count => Some(format!("and {count} more {noun}s").into()),
    }
}

/// Las filas de la columna de commits: primero los de B (lo que se revisa),
/// luego los de A que B no tiene.
pub fn compare_commit_rows(
    comparison: &RevisionComparison,
    base: &str,
    head: &str,
) -> Vec<CompareCommitRow> {
    let mut rows = Vec::new();
    let mut side = |label: String, commits: &[ComparedCommit], omitted: usize| {
        rows.push(CompareCommitRow::Header(label.into()));
        rows.extend(commits.iter().cloned().map(CompareCommitRow::Commit));
        if let Some(more) = more_label(omitted, "commit") {
            rows.push(CompareCommitRow::More(more));
        }
    };
    side(
        format!("On {head} \u{00b7} {}", comparison.ahead),
        &comparison.ahead_commits,
        comparison.ahead_omitted(),
    );
    if comparison.behind > 0 {
        side(
            format!("Only on {base} \u{00b7} {}", comparison.behind),
            &comparison.behind_commits,
            comparison.behind_omitted(),
        );
    }
    rows
}

fn file_change(file: &ComparedFile) -> FileChange {
    FileChange {
        path: file.path.clone(),
        status: file.status.clone(),
        additions: file.additions,
        deletions: file.deletions,
    }
}

/// La comparación. Se abre con [`open_compare_view`] o se monta donde se
/// quiera con [`CompareView::new`].
pub struct CompareView {
    model: Entity<AppModel>,
    repo_path: String,
    /// Lo que eligió el usuario (ramas, tags, SHAs): se muestra y se vuelve a
    /// resolver con "Refresh".
    base_label: String,
    head_label: String,
    mode: CompareMode,
    limits: CompareLimits,
    comparison: Option<RevisionComparison>,
    loading: bool,
    files_loading: bool,
    error: Option<String>,
    load_token: u64,
    commit_rows: Rc<Vec<CompareCommitRow>>,
    commit_scroll: UniformListScrollHandle,
    files_by_path: HashMap<String, ComparedFile>,
    selected: Option<ComparedFile>,
    diff_token: u64,
    /// Preferencias con las que se pidió el diff a la vista.
    ignore_whitespace: bool,
    whole_file: bool,
    changes: Entity<CommitChangesTab>,
    /// Lecturas de git que hizo la vista (tests).
    reads: usize,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<CompareViewEvent> for CompareView {}

impl CompareView {
    pub fn new(request: &ComparisonRequest, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let model = AppModel::global(cx);
        let changes = cx.new(|cx| CommitChangesTab::new(window, cx));
        let preferences = *model.read(cx).diff_preferences();
        changes.update(cx, |tab, cx| {
            tab.set_diff_preferences(
                preferences.diff_view_mode,
                preferences.ignore_whitespace,
                cx,
            );
            tab.set_whole_file(preferences.whole_file, cx);
        });
        let subscriptions = vec![
            cx.subscribe(
                &changes,
                |this, _, event: &CommitChangesTabEvent, cx| match event {
                    CommitChangesTabEvent::SelectFile(file)
                    | CommitChangesTabEvent::OpenFile(file) => {
                        let path = file.path.clone();
                        this.select_file(&path, cx);
                    }
                    CommitChangesTabEvent::ToggleIgnoreWhitespace => {
                        this.model
                            .update(cx, |model, cx| model.toggle_ignore_whitespace(cx).detach());
                    }
                    CommitChangesTabEvent::ToggleWholeFile => {
                        this.model
                            .update(cx, |model, cx| model.toggle_whole_file(cx).detach());
                    }
                },
            ),
            // Solo interesa el cambio de preferencias del diff.
            cx.observe(&model, |this, model, cx| {
                let preferences = *model.read(cx).diff_preferences();
                this.changes.update(cx, |tab, cx| {
                    tab.set_diff_preferences(
                        preferences.diff_view_mode,
                        preferences.ignore_whitespace,
                        cx,
                    );
                    tab.set_whole_file(preferences.whole_file, cx);
                });
                if preferences.ignore_whitespace != this.ignore_whitespace
                    || preferences.whole_file != this.whole_file
                {
                    this.load_diff(cx);
                }
            }),
        ];

        let mut this = Self {
            model,
            repo_path: request.repo_path.clone(),
            base_label: request.base.clone(),
            head_label: request.head.clone(),
            mode: request.mode,
            limits: CompareLimits::default(),
            comparison: None,
            loading: false,
            files_loading: false,
            error: None,
            load_token: 0,
            commit_rows: Rc::new(Vec::new()),
            commit_scroll: UniformListScrollHandle::new(),
            files_by_path: HashMap::new(),
            selected: None,
            diff_token: 0,
            ignore_whitespace: preferences.ignore_whitespace,
            whole_file: preferences.whole_file,
            changes,
            reads: 0,
            _subscriptions: subscriptions,
        };
        let (base, head) = (this.base_label.clone(), this.head_label.clone());
        this.load(base, head, cx);
        this
    }

    // --- lectura ----------------------------------------------------------------

    pub fn base_label(&self) -> &str {
        &self.base_label
    }

    pub fn head_label(&self) -> &str {
        &self.head_label
    }

    pub fn mode(&self) -> CompareMode {
        self.mode
    }

    pub fn comparison(&self) -> Option<&RevisionComparison> {
        self.comparison.as_ref()
    }

    pub fn is_loading(&self) -> bool {
        self.loading || self.files_loading
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn selected_file(&self) -> Option<&ComparedFile> {
        self.selected.as_ref()
    }

    pub fn commit_rows(&self) -> &[CompareCommitRow] {
        &self.commit_rows
    }

    pub fn changes_tab(&self) -> &Entity<CommitChangesTab> {
        &self.changes
    }

    #[doc(hidden)]
    pub fn reads(&self) -> usize {
        self.reads
    }

    /// Cambia los topes (tests; por defecto, los de `CompareLimits`). Hay que
    /// llamarlo antes de `refresh`.
    pub fn set_limits(&mut self, limits: CompareLimits) {
        self.limits = limits;
    }

    // --- acciones ---------------------------------------------------------------

    /// Otro modo sobre las mismas revisiones fijadas: solo se releen los
    /// archivos.
    pub fn set_mode(&mut self, mode: CompareMode, cx: &mut Context<Self>) {
        if mode == self.mode {
            return;
        }
        self.mode = mode;
        let Some(revisions) = self.comparison.as_ref().map(|c| c.revisions.clone()) else {
            // Aún no hay comparación (o falló): se vuelve a intentar entera.
            self.refresh(cx);
            return;
        };
        self.load_token += 1;
        let token = self.load_token;
        self.files_loading = true;
        self.reads += 1;
        let read = self.model.update(cx, |model, cx| {
            model.get_comparison_files(&self.repo_path, &revisions, mode, self.limits.files, cx)
        });
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = read.await;
            this.update(cx, |this, cx| {
                if token != this.load_token {
                    return;
                }
                this.files_loading = false;
                match result {
                    Ok(files) => {
                        if let Some(comparison) = this.comparison.as_mut() {
                            comparison.files = files;
                            comparison.mode = mode;
                        }
                        this.error = None;
                        this.publish_files(cx);
                    }
                    Err(error) => this.error = Some(error.message().to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Vuelve a resolver A y B (por si las ramas se movieron).
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let (base, head) = (self.base_label.clone(), self.head_label.clone());
        self.load(base, head, cx);
    }

    /// Intercambia A y B, sobre los mismos SHA fijados.
    pub fn swap(&mut self, cx: &mut Context<Self>) {
        std::mem::swap(&mut self.base_label, &mut self.head_label);
        match self.comparison.as_ref().map(|c| c.revisions.clone()) {
            Some(revisions) => self.load(revisions.head, revisions.base, cx),
            None => self.refresh(cx),
        }
    }

    /// Elige un archivo de la lista y pide su diff.
    pub fn select_file(&mut self, path: &str, cx: &mut Context<Self>) {
        let Some(file) = self.files_by_path.get(path).cloned() else {
            return;
        };
        if self.selected.as_ref() == Some(&file) {
            return;
        }
        self.selected = Some(file);
        self.changes
            .update(cx, |tab, cx| tab.set_selected_path(Some(path), cx));
        self.load_diff(cx);
        cx.notify();
    }

    /// Doble clic en un commit.
    pub fn show_commit(&mut self, full_hash: &str, cx: &mut Context<Self>) {
        cx.emit(CompareViewEvent::ShowCommit {
            full_hash: full_hash.to_string(),
        });
    }

    // --- interno ----------------------------------------------------------------

    fn load(&mut self, base: String, head: String, cx: &mut Context<Self>) {
        self.load_token += 1;
        let token = self.load_token;
        self.loading = true;
        self.files_loading = false;
        self.reads += 1;
        let mode = self.mode;
        let read = self.model.update(cx, |model, cx| {
            model.compare_revisions(&self.repo_path, &base, &head, mode, self.limits, cx)
        });
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = read.await;
            this.update(cx, |this, cx| {
                if token != this.load_token {
                    return;
                }
                this.loading = false;
                match result {
                    Ok(comparison) => {
                        this.error = None;
                        this.commit_rows = Rc::new(compare_commit_rows(
                            &comparison,
                            &this.base_label,
                            &this.head_label,
                        ));
                        this.comparison = Some(comparison);
                        this.publish_files(cx);
                    }
                    Err(error) => {
                        this.error = Some(error.message().to_string());
                        this.comparison = None;
                        this.commit_rows = Rc::new(Vec::new());
                        this.publish_files(cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Pasa los archivos a la lista y conserva el elegido si sigue ahí (si
    /// no, el primero).
    fn publish_files(&mut self, cx: &mut Context<Self>) {
        let files = self
            .comparison
            .as_ref()
            .map(|comparison| comparison.files.clone())
            .unwrap_or_default();
        self.files_by_path = files
            .files
            .iter()
            .map(|file| (file.path.clone(), file.clone()))
            .collect();
        let changes: Vec<FileChange> = files.files.iter().map(file_change).collect();
        let note = more_label(files.omitted(), "file")
            .map(|more| SharedString::from(format!("{more} not listed")));

        let keep = self
            .selected
            .as_ref()
            .and_then(|selected| self.files_by_path.get(&selected.path))
            .cloned();
        let first = || {
            let mut sorted: Vec<&ComparedFile> = files.files.iter().collect();
            sorted.sort_by(|a, b| mergeon_model::collate::locale_compare(&a.path, &b.path));
            sorted.first().map(|file| (*file).clone())
        };
        self.selected = keep.or_else(first);
        let selected_path = self.selected.as_ref().map(|file| file.path.clone());
        self.changes.update(cx, |tab, cx| {
            tab.set_commit_files(&changes, cx);
            tab.set_files_note(note, cx);
            tab.set_selected_path(selected_path.as_deref(), cx);
        });
        // El contenido de un archivo con la misma ruta puede haber cambiado
        // (otro modo, Refresh): el diff se pide siempre.
        self.load_diff(cx);
    }

    fn load_diff(&mut self, cx: &mut Context<Self>) {
        self.diff_token += 1;
        let token = self.diff_token;
        let preferences = *self.model.read(cx).diff_preferences();
        let (ignore_whitespace, whole_file) =
            (preferences.ignore_whitespace, preferences.whole_file);
        self.ignore_whitespace = ignore_whitespace;
        self.whole_file = whole_file;
        let (Some(file), Some(comparison)) = (self.selected.clone(), self.comparison.as_ref())
        else {
            self.changes.update(cx, |tab, cx| {
                tab.set_loading_diff(false, cx);
                tab.set_diff("", false, 0, cx);
            });
            return;
        };
        let revisions = comparison.revisions.clone();
        let mode = comparison.mode;
        if is_image_path(&file.path)
            && let Some(from) = revisions.diff_from(mode)
        {
            let request = mergeon_backend::ImagePairRequest::Range {
                from: from.to_string(),
                to: revisions.head.clone(),
                path: file.path.clone(),
                old_path: file.old_path.clone(),
            };
            let viewer = self.changes.read(cx).image_viewer().clone();
            let task = crate::images::load_image_pair(
                &self.model,
                &viewer,
                &self.repo_path,
                &file.path,
                request,
                cx,
            );
            cx.spawn(async move |this, cx| {
                let images = task.await;
                this.update(cx, |this, cx| {
                    if token == this.diff_token {
                        crate::images::show(&viewer, images, cx);
                    }
                })
                .ok();
            })
            .detach();
        }
        self.reads += 1;
        let read = self.model.update(cx, |model, cx| {
            model.get_comparison_diff(
                &self.repo_path,
                &revisions,
                mode,
                &file,
                DiffOptions {
                    ignore_whitespace,
                    whole_file,
                },
                cx,
            )
        });
        self.changes
            .update(cx, |tab, cx| tab.set_loading_diff(true, cx));
        cx.spawn(async move |this, cx| {
            let payload = read.await;
            this.update(cx, |this, cx| {
                if token != this.diff_token {
                    return;
                }
                this.changes.update(cx, |tab, cx| {
                    match payload {
                        Ok(payload) => {
                            tab.set_whole_file_fell_back(payload.whole_file_fell_back, cx);
                            tab.set_diff(payload.diff, payload.too_large, payload.size, cx);
                        }
                        Err(error) => {
                            log::warn!("comparison diff: {}", error.message());
                            tab.set_whole_file_fell_back(false, cx);
                            tab.set_diff("", false, 0, cx);
                        }
                    }
                    tab.set_loading_diff(false, cx);
                });
            })
            .ok();
        })
        .detach();
    }

    // --- pintado ----------------------------------------------------------------

    fn revision_chip(
        id: &'static str,
        label: &str,
        sha: Option<&str>,
        cx: &App,
    ) -> impl IntoElement {
        let palette = cx.palette();
        let mono = code_font_family(cx);
        div()
            .id(id)
            .test_support()
            .flex()
            .items_center()
            .gap(px(6.))
            .min_w_0()
            .h(px(24.))
            .px(px(8.))
            .border_1()
            .border_color(palette.border_control)
            .rounded(Radius::Md.px(cx))
            .bg(palette.bg_raised)
            .child(
                mergeon_ui::icon(Lucide::GitBranch)
                    .size(px(12.))
                    .tone(IconTone::Muted),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .font_family(mono.clone())
                    .text_size(px(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(palette.text_1)
                    .child(label.to_string()),
            )
            .when_some(sha, |chip, sha| {
                chip.child(
                    div()
                        .flex_shrink_0()
                        .font_family(mono)
                        .text_size(px(11.))
                        .text_color(palette.text_3)
                        .child(sha.chars().take(7).collect::<String>()),
                )
            })
            .tooltip(mergeon_ui::tooltip(match sha {
                Some(sha) => format!("{label} \u{00b7} {sha}"),
                None => label.to_string(),
            }))
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let mono = code_font_family(cx);
        let revisions = self.comparison.as_ref().map(|c| &c.revisions);
        let (ahead, behind) = self
            .comparison
            .as_ref()
            .map_or((None, None), |c| (Some(c.ahead), Some(c.behind)));
        let files = self.comparison.as_ref().map(|c| &c.files);
        let current = self.mode;

        let counter = |id: &'static str, icon: Lucide, value: Option<usize>, tip: String| {
            div()
                .id(id)
                .test_support()
                .flex()
                .items_center()
                .gap(px(3.))
                .font_family(mono.clone())
                .text_size(px(12.))
                .text_color(palette.text_2)
                .child(mergeon_ui::icon(icon).size(px(12.)).tone(IconTone::Muted))
                .child(value.map_or("\u{2013}".to_string(), |value| value.to_string()))
                .tooltip(mergeon_ui::tooltip(tip))
        };

        div()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .gap(px(6.))
            .pb(px(10.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .min_w_0()
                    .child(Self::revision_chip(
                        "compare-base",
                        &self.base_label,
                        revisions.map(|r| r.base.as_str()),
                        cx,
                    ))
                    .child(
                        mergeon_ui::icon(match current {
                            CompareMode::SinceDivergence => Lucide::Ellipsis,
                            CompareMode::Direct => Lucide::ArrowRight,
                        })
                        .size(px(14.))
                        .tone(IconTone::Muted),
                    )
                    .child(Self::revision_chip(
                        "compare-head",
                        &self.head_label,
                        revisions.map(|r| r.head.as_str()),
                        cx,
                    ))
                    .child(
                        mergeon_ui::button("compare-swap")
                            .ghost()
                            .xs()
                            .icon(Lucide::ArrowLeftRight)
                            .tooltip("Swap A and B")
                            .on_click(cx.listener(|this, _, _, cx| this.swap(cx))),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .ml(px(4.))
                            .child(counter(
                                "compare-ahead",
                                Lucide::ArrowUp,
                                ahead,
                                format!(
                                    "Commits on {} not on {}",
                                    self.head_label, self.base_label
                                ),
                            ))
                            .child(counter(
                                "compare-behind",
                                Lucide::ArrowDown,
                                behind,
                                format!(
                                    "Commits on {} not on {}",
                                    self.base_label, self.head_label
                                ),
                            )),
                    )
                    .child(div().flex_1())
                    .child({
                        mergeon_ui::tabs("compare-mode")
                            .item(TabItem::new("since", "Since diverged").icon(Lucide::GitFork))
                            .item(TabItem::new("direct", "Direct").icon(Lucide::ArrowRight))
                            .selected(match current {
                                CompareMode::SinceDivergence => "since",
                                CompareMode::Direct => "direct",
                            })
                            .on_change(cx.listener(|this, key: &SharedString, _, cx| {
                                let mode = if key.as_ref() == "direct" {
                                    CompareMode::Direct
                                } else {
                                    CompareMode::SinceDivergence
                                };
                                this.set_mode(mode, cx);
                            }))
                    })
                    .child(
                        mergeon_ui::button("compare-refresh")
                            .outline()
                            .xs()
                            .icon(Lucide::RefreshCw)
                            .label("Refresh")
                            .loading(self.is_loading())
                            .tooltip("Resolve both branches again and reload")
                            .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .min_w_0()
                    .text_size(px(12.))
                    .child(
                        div()
                            .id("compare-description")
                            .test_support()
                            .min_w_0()
                            .truncate()
                            .text_color(palette.text_2)
                            .child(compare_mode_description(
                                current,
                                &self.base_label,
                                &self.head_label,
                            )),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .font_family(mono.clone())
                            .text_color(palette.text_3)
                            .child(current.notation(&self.base_label, &self.head_label)),
                    )
                    .child(div().flex_1())
                    .when_some(files, |row, files| {
                        row.child(
                            div()
                                .id("compare-totals")
                                .test_support()
                                .flex()
                                .flex_shrink_0()
                                .gap(px(8.))
                                .font_family(mono.clone())
                                .child(div().text_color(palette.text_2).child(format!(
                                    "{} {}",
                                    files.total,
                                    if files.total == 1 { "file" } else { "files" }
                                )))
                                .child(
                                    div()
                                        .text_color(palette.success_foreground)
                                        .child(format!("+{}", files.insertions)),
                                )
                                .child(
                                    div()
                                        .text_color(palette.destructive_foreground)
                                        .child(format!("\u{2212}{}", files.deletions)),
                                ),
                        )
                    }),
            )
    }

    fn render_commits(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let rows = self.commit_rows.clone();
        let this: WeakEntity<Self> = cx.entity().downgrade();
        let list = uniform_list("compare-commits-list", rows.len(), move |range, _, cx| {
            rows[range.start..range.end.min(rows.len())]
                .iter()
                .map(|row| commit_row_element(row, &this, cx))
                .collect()
        })
        .track_scroll(&self.commit_scroll)
        .size_full();

        div()
            .id("compare-commits")
            .test_support()
            .relative()
            .flex_shrink_0()
            .w(px(COMMITS_PANE_WIDTH))
            .h_full()
            .border_r_1()
            .border_color(palette.border)
            .bg(palette.card)
            .child(list)
            .vertical_scrollbar(&self.commit_scroll)
    }

    fn placeholder(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let palette = *cx.palette();
        if self.loading && self.comparison.is_none() {
            return Some(
                div()
                    .flex()
                    .size_full()
                    .items_center()
                    .justify_center()
                    .gap(px(8.))
                    .text_color(palette.text_2)
                    .child(
                        mergeon_ui::spinner()
                            .size(SpinnerSize::Sm)
                            .color(SpinnerColor::Primary),
                    )
                    .child("Comparing...")
                    .into_any_element(),
            );
        }
        if let Some(error) = &self.error {
            return Some(
                div()
                    .id("compare-error")
                    .test_support()
                    .flex()
                    .size_full()
                    .items_center()
                    .justify_center()
                    .child(
                        mergeon_ui::empty_state("Unable to compare")
                            .icon(Lucide::TriangleAlert)
                            .description(error.clone()),
                    )
                    .into_any_element(),
            );
        }
        let comparison = self.comparison.as_ref()?;
        if comparison.mode == CompareMode::SinceDivergence && !comparison.has_common_history() {
            return Some(
                div()
                    .id("compare-no-common-history")
                    .test_support()
                    .flex()
                    .flex_col()
                    .size_full()
                    .items_center()
                    .justify_center()
                    .gap(px(12.))
                    .child(
                        mergeon_ui::empty_state("No common history")
                            .icon(Lucide::GitFork)
                            .description(format!(
                                "{} and {} never diverged from a shared commit, so there are no \"changes since\" to show. The direct diff compares their contents.",
                                self.head_label, self.base_label
                            )),
                    )
                    .child(
                        mergeon_ui::button("compare-show-direct")
                            .outline()
                            .sm()
                            .label("Show direct diff")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.set_mode(CompareMode::Direct, cx)
                            })),
                    )
                    .into_any_element(),
            );
        }
        if comparison.files.total == 0 && !self.files_loading {
            return Some(
                div()
                    .id("compare-no-changes")
                    .test_support()
                    .flex()
                    .size_full()
                    .items_center()
                    .justify_center()
                    .child(
                        mergeon_ui::empty_state("No differences")
                            .icon(Lucide::Check)
                            .description(match comparison.mode {
                                CompareMode::SinceDivergence => format!(
                                    "{} has no file changes since it diverged from {}.",
                                    self.head_label, self.base_label
                                ),
                                CompareMode::Direct => format!(
                                    "{} and {} have the same files.",
                                    self.base_label, self.head_label
                                ),
                            }),
                    )
                    .into_any_element(),
            );
        }
        None
    }
}

/// Una fila de la columna de commits. Fuera de `render`: la pinta la lista
/// virtual.
fn commit_row_element(
    row: &CompareCommitRow,
    this: &WeakEntity<CompareView>,
    cx: &App,
) -> AnyElement {
    let palette = cx.palette();
    let base = div()
        .flex()
        .items_center()
        .w_full()
        .h(px(COMMIT_ROW_HEIGHT))
        .px(px(10.))
        .gap(px(8.))
        .min_w_0();
    match row {
        CompareCommitRow::Header(label) => base
            .pt(px(6.))
            .text_size(px(11.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(palette.text_3)
            .child(div().min_w_0().truncate().child(label.clone()))
            .into_any_element(),
        CompareCommitRow::More(label) => base
            .id("compare-commits-more")
            .test_support()
            .text_size(px(12.))
            .text_color(palette.text_3)
            .child(label.clone())
            .into_any_element(),
        CompareCommitRow::Commit(commit) => {
            let full_hash = commit.full_hash.clone();
            let this = this.clone();
            base.id(SharedString::from(format!(
                "compare-commit:{}",
                commit.full_hash
            )))
            .test_support()
            .cursor_pointer()
            .hover(|style| style.bg(palette.bg_hover))
            .on_click(move |event: &ClickEvent, _, cx| {
                if event.click_count() >= 2 {
                    this.update(cx, |view, cx| view.show_commit(&full_hash, cx))
                        .ok();
                }
            })
            .tooltip(mergeon_ui::tooltip(format!(
                "{}\n{} \u{00b7} {}\nDouble-click to show it in the history",
                commit.message, commit.author, commit.hash
            )))
            .child(
                div()
                    .flex_shrink_0()
                    .font_family(code_font_family(cx))
                    .text_size(px(11.))
                    .text_color(palette.text_3)
                    .child(commit.hash.clone()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(12.))
                    .text_color(palette.text_1)
                    .child(commit.message.clone()),
            )
            .into_any_element()
        }
    }
}

impl Render for CompareView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let body = match self.placeholder(cx) {
            Some(placeholder) => placeholder,
            None => self.changes.clone().into_any_element(),
        };
        div()
            .id("compare-view")
            .test_support()
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .line_height(relative(1.5))
            .text_color(palette.text_1)
            .child(self.render_header(cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .border_1()
                    .border_color(palette.border)
                    .rounded(rems(0.375))
                    .overflow_hidden()
                    .child(self.render_commits(cx))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .bg(palette.card)
                            .child(body),
                    ),
            )
    }
}

/// Abre la comparación en un diálogo que ocupa casi toda la ventana y
/// devuelve la vista (el diálogo la conserva mientras está abierto).
pub fn open_compare_view(
    request: &ComparisonRequest,
    window: &mut Window,
    cx: &mut App,
) -> Entity<CompareView> {
    let view = cx.new(|cx| CompareView::new(request, window, cx));
    let body = view.clone();
    AppDialog::new()
        .title("Compare")
        // Tan ancho como deje la ventana (el diálogo se ciñe a ella).
        .width(DialogWidth::Rems(400.))
        .body(move |window, _| {
            // Alto fijo: la lista de commits y la de archivos son virtuales y
            // necesitan un alto que llenar. Se descuentan el margen del fondo,
            // el título y los rellenos del cuerpo.
            let height = (window.viewport_size().height - px(120.)).max(px(240.));
            div().h(height).child(body.clone())
        })
        .open(window, cx);
    view
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(message: &str) -> ComparedCommit {
        ComparedCommit {
            hash: "abc1234".into(),
            full_hash: "abc1234".repeat(5) + "abcde",
            message: message.into(),
            author: "Ana".into(),
            date: "2024-01-01T00:00:00Z".into(),
        }
    }

    #[test]
    fn lists_the_head_side_first_and_says_what_it_left_out() {
        let comparison = RevisionComparison {
            ahead: 3,
            behind: 1,
            ahead_commits: vec![commit("b2"), commit("b1")],
            behind_commits: vec![commit("a1")],
            ..Default::default()
        };
        let labels: Vec<String> = compare_commit_rows(&comparison, "main", "feature")
            .into_iter()
            .map(|row| match row {
                CompareCommitRow::Header(label) | CompareCommitRow::More(label) => {
                    label.to_string()
                }
                CompareCommitRow::Commit(commit) => commit.message,
            })
            .collect();
        assert_eq!(
            labels,
            [
                "On feature \u{00b7} 3",
                "b2",
                "b1",
                "and 1 more commit",
                "Only on main \u{00b7} 1",
                "a1",
            ]
        );
    }

    #[test]
    fn describes_each_mode() {
        assert_eq!(
            compare_mode_description(CompareMode::SinceDivergence, "main", "feature"),
            "Changes on feature since it diverged from main"
        );
        assert_eq!(
            compare_mode_description(CompareMode::Direct, "main", "feature"),
            "Direct diff main \u{2192} feature"
        );
        assert_eq!(more_label(2, "file").as_deref(), Some("and 2 more files"));
        assert_eq!(more_label(0, "file"), None);
    }
}
