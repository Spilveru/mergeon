//! "Clean up merged branches…" (sin original): las ramas locales ya
//! integradas en la rama principal, con casilla por rama, para borrarlas en
//! lote.
//!
//! - La lista sale de `AppModel::get_merged_branches` (fuera del hilo de UI) y
//!   sus textos se preparan una vez al llegar; la lista es virtual.
//! - Las integradas (merge o fast-forward) llegan marcadas. Las que parecen
//!   integradas por squash llegan sin marcar ("probably merged (squash)"); las
//!   que tienen commits sin integrar y el upstream borrado ("gone") solo se
//!   pueden marcar con "Force delete…", porque se borran con `-D`.
//! - "Also delete remote branches" (apagada) borra además la rama remota de
//!   las que aún la tienen.
//! - Borrar cierra el diálogo y lanza una sola mutación por la cola
//!   (`flows::cleanup_branches`): aviso "Deleted 7 branches." y entrada en el
//!   log de operaciones.

use std::collections::HashSet;
use std::ops::Range;
use std::rc::Rc;

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, Context, Entity, ListSizingBehavior, SharedString, TestSupportExt as _,
    UniformListScrollHandle, Window, div, px, rems, uniform_list,
};
use mergeon_app::AppModel;
use mergeon_backend::{
    CleanupBranchRequest, CleanupBranchesOptions, CleanupCandidate, MergeState,
    MergedBranchesReport,
};
use mergeon_model::format::format_relative_date;
use mergeon_ui::dialog::{self, AppDialog, DialogId, DialogWidth};
use mergeon_ui::{
    ActivePaletteExt as _, AlertVariant, BadgeSize, BadgeVariant, Lucide, MergeonStyled as _,
    SpinnerColor, SpinnerSize,
};

use super::parts::caption;
use crate::flows::{self, RepoRef};

/// Alto de una fila: nombre y, debajo, el último commit.
const ROW_HEIGHT: f32 = 46.;
/// Alto máximo de la lista antes de hacer scroll.
const LIST_MAX_HEIGHT: f32 = 322.;

/// Lo que se pinta de una candidata, preparado al cargar.
#[derive(Debug, Clone)]
struct Row {
    name: SharedString,
    state: MergeState,
    short_tip: SharedString,
    subject: SharedString,
    date: SharedString,
    upstream_gone: bool,
    /// Tiene una rama remota viva que "Also delete remote branches" borraría.
    has_remote: bool,
    unmerged_commits: u32,
}

impl Row {
    fn new(candidate: &CleanupCandidate) -> Self {
        Self {
            name: candidate.name.clone().into(),
            state: candidate.state,
            short_tip: candidate.short_tip.clone().into(),
            subject: candidate.subject.clone().into(),
            date: format_relative_date(candidate.committed_at.saturating_mul(1000)).into(),
            upstream_gone: candidate.upstream_gone,
            has_remote: candidate.upstream.is_some() && !candidate.upstream_gone,
            unmerged_commits: candidate.unmerged_commits,
        }
    }
}

/// Estado del diálogo.
pub struct CleanupBranchesDialog {
    model: Entity<AppModel>,
    repo: RepoRef,
    report: Option<MergedBranchesReport>,
    rows: Rc<Vec<Row>>,
    /// La base pedida; `None` es la que el repositorio tenga por defecto.
    base: Option<String>,
    loading: bool,
    error: Option<SharedString>,
    selected: HashSet<SharedString>,
    delete_remote: bool,
    force_unmerged: bool,
    /// Una respuesta que llega tarde (se cambió la base) no pisa la última.
    load_token: u64,
    scroll_handle: UniformListScrollHandle,
    dialog_id: Option<DialogId>,
}

impl CleanupBranchesDialog {
    fn new(model: Entity<AppModel>, repo: RepoRef) -> Self {
        Self {
            model,
            repo,
            report: None,
            rows: Rc::default(),
            base: None,
            loading: true,
            error: None,
            selected: HashSet::new(),
            delete_remote: false,
            force_unmerged: false,
            load_token: 0,
            scroll_handle: UniformListScrollHandle::new(),
            dialog_id: None,
        }
    }

    pub fn is_open(&self) -> bool {
        self.dialog_id.is_some()
    }

    /// Cierra sin avisar a nadie (lo llama quien lo abrió: otro diálogo del
    /// mismo tipo o un cambio de repositorio).
    pub fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.dialog_id.take() {
            dialog::close_id(window, cx, id);
        }
    }

    pub fn is_loading(&self) -> bool {
        self.loading
    }

    pub fn error(&self) -> Option<&SharedString> {
        self.error.as_ref()
    }

    /// La base contra la que se comparó la lista a la vista.
    pub fn base(&self) -> Option<&str> {
        self.report.as_ref().map(|report| report.base.as_str())
    }

    pub fn candidates(&self) -> &[CleanupCandidate] {
        self.report
            .as_ref()
            .map_or(&[], |report| report.candidates.as_slice())
    }

    /// Las ramas marcadas, en el orden de la lista.
    pub fn selected_names(&self) -> Vec<String> {
        self.rows
            .iter()
            .filter(|row| self.selected.contains(&row.name))
            .map(|row| row.name.to_string())
            .collect()
    }

    pub fn delete_remote(&self) -> bool {
        self.delete_remote
    }

    pub fn force_unmerged(&self) -> bool {
        self.force_unmerged
    }

    fn can_select(&self, row: &Row) -> bool {
        row.state != MergeState::Unmerged || self.force_unmerged
    }

    fn selectable_rows(&self) -> impl Iterator<Item = &Row> {
        self.rows.iter().filter(|row| self.can_select(row))
    }

    /// Marca o desmarca una rama (si se puede marcar).
    pub fn set_selected(&mut self, name: &str, selected: bool, cx: &mut Context<Self>) {
        let Some(row) = self.rows.iter().find(|row| row.name.as_ref() == name) else {
            return;
        };
        if selected && !self.can_select(row) {
            return;
        }
        let name = row.name.clone();
        if selected {
            self.selected.insert(name);
        } else {
            self.selected.remove(&name);
        }
        cx.notify();
    }

    /// "Select all": marca todas las que se pueden marcar, o ninguna si ya
    /// estaban todas.
    pub fn toggle_all(&mut self, cx: &mut Context<Self>) {
        let all: Vec<SharedString> = self.selectable_rows().map(|row| row.name.clone()).collect();
        if all.iter().all(|name| self.selected.contains(name)) {
            self.selected.clear();
        } else {
            self.selected.extend(all);
        }
        cx.notify();
    }

    pub fn set_delete_remote(&mut self, value: bool, cx: &mut Context<Self>) {
        self.delete_remote = value;
        cx.notify();
    }

    /// "Force delete branches with unmerged commits": sin ella, las ramas con
    /// commits sin integrar no se pueden marcar (y se desmarcan).
    pub fn set_force_unmerged(&mut self, value: bool, cx: &mut Context<Self>) {
        self.force_unmerged = value;
        if !value {
            let unmerged: Vec<SharedString> = self
                .rows
                .iter()
                .filter(|row| row.state == MergeState::Unmerged)
                .map(|row| row.name.clone())
                .collect();
            for name in unmerged {
                self.selected.remove(&name);
            }
        }
        cx.notify();
    }

    /// Compara con otra base.
    pub fn set_base(&mut self, base: &str, cx: &mut Context<Self>) {
        if self.base() == Some(base) {
            return;
        }
        self.base = Some(base.to_string());
        self.load(cx);
    }

    /// Hay algo marcado que borrar (lo que mira el botón en cada frame, sin
    /// construir la petición).
    fn has_request(&self) -> bool {
        self.report.is_some()
            && self
                .rows
                .iter()
                .any(|row| self.selected.contains(&row.name))
    }

    /// Lo que se borraría con lo marcado ahora; `None` si no hay nada.
    pub fn request(&self) -> Option<CleanupBranchesOptions> {
        let report = self.report.as_ref()?;
        let branches: Vec<CleanupBranchRequest> = self
            .rows
            .iter()
            .filter(|row| self.selected.contains(&row.name))
            .map(|row| CleanupBranchRequest {
                name: row.name.to_string(),
                force: row.state != MergeState::Merged,
            })
            .collect();
        (!branches.is_empty()).then(|| CleanupBranchesOptions {
            base: report.base.clone(),
            branches,
            delete_remote: self.delete_remote,
            auth_token: None,
        })
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        self.error = None;
        self.load_token += 1;
        let token = self.load_token;
        let base = self.base.clone();
        let read = self.model.update(cx, |model, cx| {
            model.get_merged_branches(&self.repo.path, base, cx)
        });
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = read.await;
            this.update(cx, |this, cx| {
                if this.load_token != token {
                    return;
                }
                this.loading = false;
                match result {
                    Ok(report) => this.apply_report(report),
                    Err(error) => {
                        this.error = Some(error.message().to_string().into());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn apply_report(&mut self, report: MergedBranchesReport) {
        let rows: Vec<Row> = report.candidates.iter().map(Row::new).collect();
        self.selected = report
            .candidates
            .iter()
            .filter(|candidate| candidate.selected_by_default())
            .map(|candidate| SharedString::from(candidate.name.clone()))
            .collect();
        self.base = Some(report.base.clone());
        self.rows = Rc::new(rows);
        self.report = Some(report);
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.dialog_id.take() {
            dialog::close_id(window, cx, id);
        }
    }

    /// "Delete N branches": cierra el diálogo y lanza el borrado.
    pub fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(options) = self.request() else {
            return;
        };
        self.close(window, cx);
        flows::cleanup_branches(&self.model, self.repo.clone(), options, cx).detach();
    }

    fn confirm_label(&self) -> String {
        match self.selected.len() {
            0 => "Delete branches".to_string(),
            1 => "Delete 1 branch".to_string(),
            count => format!("Delete {count} branches"),
        }
    }

    fn render_rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let rows = self.rows.clone();
        rows[range]
            .iter()
            .map(|row| self.render_row(row, cx))
            .collect()
    }

    fn render_row(&self, row: &Row, cx: &mut Context<Self>) -> AnyElement {
        let palette = *cx.palette();
        let mono = cx.theme().mono_font_family.clone();
        let checked = self.selected.contains(&row.name);
        let enabled = self.can_select(row);
        let name = row.name.clone();
        let badge = |label: String, variant: BadgeVariant| {
            div().flex_shrink_0().child(
                mergeon_ui::badge(label)
                    .variant(variant)
                    .size(BadgeSize::Sm),
            )
        };

        div()
            .id(SharedString::from(format!("cleanup-row:{}", row.name)))
            .test_support()
            .w_full()
            .h(px(ROW_HEIGHT))
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(10.))
            .border_b_1()
            .border_color(palette.border_hair)
            .when(enabled, |this| {
                this.cursor_pointer()
                    .hover(|style| style.bg(palette.bg_hover))
                    .on_click(cx.listener({
                        let name = name.clone();
                        move |this, _, _, cx| {
                            let selected = !this.selected.contains(&name);
                            this.set_selected(&name, selected, cx);
                        }
                    }))
            })
            .when(!enabled, |this| this.opacity(0.6))
            .child(
                mergeon_ui::checkbox(SharedString::from(format!("cleanup-check:{}", row.name)))
                    .checked(checked)
                    .disabled(!enabled)
                    .on_change(cx.listener({
                        let name = name.clone();
                        move |this, checked: &bool, _, cx| {
                            // La fila entera también marca: que no lo deshaga.
                            cx.stop_propagation();
                            this.set_selected(&name, *checked, cx);
                        }
                    })),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .min_w_0()
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .font_family(mono.clone())
                                    .text_size(px(13.))
                                    .line_height(px(18.))
                                    .text_color(palette.text_1)
                                    .child(row.name.clone()),
                            )
                            .when(row.state == MergeState::Squashed, |this| {
                                this.child(badge(
                                    "probably merged (squash)".into(),
                                    BadgeVariant::Info,
                                ))
                            })
                            .when(row.state == MergeState::Unmerged, |this| {
                                this.child(badge(
                                    match row.unmerged_commits {
                                        1 => "1 unmerged commit".to_string(),
                                        count => format!("{count} unmerged commits"),
                                    },
                                    BadgeVariant::Warning,
                                ))
                            })
                            .when(row.upstream_gone, |this| {
                                this.child(badge("gone".into(), BadgeVariant::Secondary))
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .min_w_0()
                            .text_size(px(12.))
                            .line_height(px(16.))
                            .text_color(palette.text_3)
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .font_family(mono)
                                    .child(row.short_tip.clone()),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(palette.text_2)
                                    .child(row.subject.clone()),
                            )
                            .child(div().flex_shrink_0().child(row.date.clone())),
                    ),
            )
            .into_any_element()
    }

    fn render_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let palette = *cx.palette();
        let list = uniform_list(
            "cleanup-rows",
            self.rows.len(),
            cx.processor(|this, range, _, cx| this.render_rows(range, cx)),
        )
        .with_sizing_behavior(ListSizingBehavior::Infer)
        .track_scroll(&self.scroll_handle)
        .min_h_0()
        .max_h(px(LIST_MAX_HEIGHT));

        div()
            .id("cleanup-list")
            .test_support()
            .relative()
            .flex()
            .flex_col()
            .border_1()
            .border_color(palette.border_base)
            .radius(mergeon_ui::Radius::Md, cx)
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .max_h(px(LIST_MAX_HEIGHT))
                    .overflow_hidden()
                    .child(list),
            )
            .vertical_scrollbar(&self.scroll_handle)
            .into_any_element()
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let selectable = self.selectable_rows().count();
        let selected = self.selected.len();
        let base = self
            .report
            .as_ref()
            .map(|report| report.base.clone())
            .or_else(|| self.base.clone())
            .unwrap_or_default();
        let options: Vec<String> = self
            .report
            .as_ref()
            .map(|report| report.base_options.clone())
            .unwrap_or_default();

        div()
            .flex()
            .items_center()
            .gap(px(10.))
            .child(
                mergeon_ui::checkbox("cleanup-select-all")
                    .checked(selectable > 0 && selected >= selectable)
                    .mixed(selected > 0 && selected < selectable)
                    .disabled(selectable == 0 || self.loading)
                    .on_change(cx.listener(|this, _: &bool, _, cx| this.toggle_all(cx))),
            )
            .child(
                div()
                    .flex_1()
                    .text_size(px(12.))
                    .text_color(palette.text_2)
                    .child(if self.rows.is_empty() {
                        String::new()
                    } else {
                        format!("{selected} of {} selected", self.rows.len())
                    }),
            )
            .child(caption("Merged into", cx))
            .child(
                mergeon_ui::select("cleanup-base")
                    .items(options)
                    .value(base)
                    .full_width(false)
                    .w(rems(12.))
                    .disabled(self.loading && self.report.is_none())
                    .on_change(cx.listener(|this, value: &SharedString, _, cx| {
                        this.set_base(value, cx);
                    })),
            )
    }
}

impl Render for CleanupBranchesDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let base = self.base().unwrap_or("the main branch").to_string();
        let has_unmerged = self
            .rows
            .iter()
            .any(|row| row.state == MergeState::Unmerged);
        let remote_count = self
            .rows
            .iter()
            .filter(|row| row.has_remote && self.selected.contains(&row.name))
            .count();
        let forced = self
            .rows
            .iter()
            .filter(|row| row.state != MergeState::Merged && self.selected.contains(&row.name))
            .count();

        let content = if self.loading && self.report.is_none() {
            div()
                .id("cleanup-loading")
                .test_support()
                .flex()
                .items_center()
                .gap(rems(0.45))
                .py(rems(1.))
                .text_size(rems(0.8125))
                .text_color(palette.text_2)
                .child(
                    mergeon_ui::spinner()
                        .size(SpinnerSize::Sm)
                        .color(SpinnerColor::Primary),
                )
                .child("Looking for merged branches…")
                .into_any_element()
        } else if let Some(error) = self.error.clone() {
            mergeon_ui::alert(AlertVariant::Error)
                .title("Could not list the branches")
                .description(error)
                .into_any_element()
        } else if self.rows.is_empty() {
            mergeon_ui::empty_state("No merged branches")
                .description(format!(
                    "Every local branch has commits that are not in {base}."
                ))
                .icon(Lucide::BrushCleaning)
                .into_any_element()
        } else {
            self.render_list(cx)
        };

        div()
            .id("cleanup-branches")
            .test_support()
            .flex()
            .flex_col()
            .gap(rems(0.75))
            .child(self.render_header(cx))
            .child(content)
            .when(!self.rows.is_empty(), |this| {
                this.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(rems(0.5))
                        .child(
                            mergeon_ui::checkbox("cleanup-delete-remote")
                                .label(if remote_count > 0 {
                                    format!("Also delete remote branches ({remote_count})")
                                } else {
                                    "Also delete remote branches".to_string()
                                })
                                .checked(self.delete_remote)
                                .on_change(cx.listener(|this, checked: &bool, _, cx| {
                                    this.set_delete_remote(*checked, cx)
                                })),
                        )
                        .when(has_unmerged, |this| {
                            this.child(
                                mergeon_ui::checkbox("cleanup-force")
                                    .label("Allow deleting branches with unmerged commits")
                                    .checked(self.force_unmerged)
                                    .on_change(cx.listener(|this, checked: &bool, _, cx| {
                                        this.set_force_unmerged(*checked, cx)
                                    })),
                            )
                        }),
                )
            })
            .when(forced > 0, |this| {
                this.child(mergeon_ui::alert(AlertVariant::Warning).description(format!(
                    "{forced} selected {} commits that are not in {base}: {} deleted with git branch -D.",
                    if forced == 1 { "branch has" } else { "branches have" },
                    if forced == 1 { "it will be" } else { "they will be" },
                )))
            })
            .when(
                self.report
                    .as_ref()
                    .is_some_and(|report| report.squash_check_truncated),
                |this| {
                    this.child(caption(
                        "Squash-merge detection only checked the most recent branches.",
                        cx,
                    ))
                },
            )
    }
}

/// Abre el diálogo sobre `repo` y carga sus ramas integradas.
pub fn open_cleanup_branches_dialog(
    model: &Entity<AppModel>,
    repo_id: &str,
    repo_path: &str,
    window: &mut Window,
    cx: &mut App,
) -> Entity<CleanupBranchesDialog> {
    let repo = RepoRef {
        id: repo_id.to_string(),
        path: repo_path.to_string(),
    };
    let view = cx.new(|_| CleanupBranchesDialog::new(model.clone(), repo));

    let id = AppDialog::new()
        .title("Clean up merged branches")
        .description(
            "Local branches already merged into the main branch. The current branch, \
             the base and protected branches (main, develop, release/*…) are never listed.",
        )
        .width(DialogWidth::Xl2)
        .body({
            let view = view.clone();
            move |_, _| view.clone()
        })
        .footer({
            let view = view.clone();
            move |_, cx| {
                let dialog = view.read(cx);
                let label = dialog.confirm_label();
                let disabled = !dialog.has_request() || dialog.loading;
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        mergeon_ui::button("cleanup-cancel")
                            .ghost()
                            .sm()
                            .label("Cancel")
                            .on_click({
                                let view = view.clone();
                                move |_, window, cx| {
                                    view.update(cx, |view, cx| view.close(window, cx));
                                }
                            }),
                    )
                    .child(
                        mergeon_ui::button("cleanup-submit")
                            .destructive()
                            .sm()
                            .icon(Lucide::Trash)
                            .label(label)
                            .disabled(disabled)
                            .on_click({
                                let view = view.clone();
                                move |_, window, cx| {
                                    view.update(cx, |view, cx| view.submit(window, cx));
                                }
                            }),
                    )
            }
        })
        .on_close({
            let view = view.clone();
            move |_, cx| view.update(cx, |view, _| view.dialog_id = None)
        })
        .open(window, cx);

    view.update(cx, |view, cx| {
        view.dialog_id = Some(id);
        view.load(cx);
    });
    view
}
