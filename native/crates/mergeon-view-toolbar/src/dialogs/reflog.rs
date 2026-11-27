//! Diálogo Reflog (`src/components/repository/ReflogDialog.vue`): por dónde ha
//! pasado HEAD, para recuperar trabajo tras un reset o un rebase que salió mal.

use std::ops::Range;
use std::rc::Rc;

use gpui_kit::base::Button as BaseButton;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ListSizingBehavior, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, TestSupportExt as _, UniformListScrollHandle, Window, div, px, rems, uniform_list,
};
use mergeon_app::{AppModel, SyncRequest};
use mergeon_backend::{ReflogEntry, ReflogOptions};
use mergeon_model::dialogs::{ConfirmOptions, PromptOptions};
use mergeon_model::format::format_relative_date;
use mergeon_ui::dialog::{self, AppDialog, DialogId, DialogWidth};
use mergeon_ui::{
    ActivePaletteExt as _, IconTone, Lucide, MergeonStyled as _, Palette, Radius, SpinnerColor,
    SpinnerSize,
};

use super::{is_open, scroll_list};
use crate::parts::{mono_font, plain_icon, tint};

const TITLE: &str = "Reflog";

/// Entradas que se piden (`{ maxCount: 100 }`).
const MAX_ENTRIES: u32 = 100;

/// Hueco entre filas (`gap: 0.15rem`).
const ROW_GAP: f32 = 0.15;

/// Una entrada con su fecha ya escrita: se formatea al recibirla, no en cada
/// frame.
struct ReflogRow {
    entry: ReflogEntry,
    date: SharedString,
}

/// Contenido del diálogo.
pub(crate) struct ReflogDialog {
    model: Entity<AppModel>,
    entries: Rc<[ReflogRow]>,
    loading: bool,
    dialog_id: Option<DialogId>,
    scroll_handle: UniformListScrollHandle,
    /// La lista no cabe y hace scroll (lo que se vio en el último frame).
    overflowing: bool,
}

impl ReflogDialog {
    fn new(cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            model: AppModel::global(cx),
            entries: Rc::from([]),
            loading: false,
            dialog_id: None,
            scroll_handle: UniformListScrollHandle::new(),
            overflowing: false,
        };
        this.load_reflog(cx);
        this
    }

    /// `loadReflog`.
    fn load_reflog(&mut self, cx: &mut Context<Self>) {
        let Some(repo_path) = self
            .model
            .read(cx)
            .active_repository()
            .map(|repository| repository.path.clone())
        else {
            self.entries = Rc::from([]);
            return;
        };

        self.loading = true;
        let read = self.model.update(cx, |model, cx| {
            model.get_reflog(
                &repo_path,
                ReflogOptions {
                    max_count: Some(MAX_ENTRIES),
                },
                cx,
            )
        });
        cx.spawn(async move |this, cx| {
            let result = read.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(entries) => {
                        this.entries = entries
                            .into_iter()
                            .map(|entry| ReflogRow {
                                date: format_relative_date(&entry.date).into(),
                                entry,
                            })
                            .collect();
                    }
                    Err(error) => {
                        this.entries = Rc::from([]);
                        let message = error.message().to_string();
                        this.model.update(cx, |model, cx| {
                            model.notify_error(
                                if message.is_empty() {
                                    "Failed to read the reflog"
                                } else {
                                    &message
                                },
                                cx,
                            );
                        });
                    }
                }
                this.loading = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn active_repository(&self, cx: &App) -> Option<(String, String)> {
        self.model
            .read(cx)
            .active_repository()
            .map(|repository| (repository.id.clone(), repository.path.clone()))
    }

    /// `handleCreateBranch`: pide un nombre y crea la rama en esa entrada.
    fn create_branch(&mut self, entry: &ReflogEntry, cx: &mut Context<Self>) {
        let Some((repo_id, repo_path)) = self.active_repository(cx) else {
            return;
        };
        let model = self.model.clone();
        let full_hash = entry.full_hash.clone();
        let prompt = model.update(cx, |model, cx| {
            model.prompt(
                PromptOptions {
                    title: Some(format!("Create branch at {}", entry.hash)),
                    description: Some(entry.subject.clone()),
                    confirm_text: Some("Create".to_string()),
                    cancel_text: Some("Cancel".to_string()),
                    placeholder: Some("recovered-work".to_string()),
                    ..Default::default()
                },
                cx,
            )
        });

        cx.spawn(async move |_, cx| {
            let answer = prompt.await;
            let branch_name = answer
                .value
                .as_deref()
                .map(str::trim)
                .unwrap_or_default()
                .to_string();
            if !answer.confirmed || branch_name.is_empty() {
                return;
            }

            let result = model
                .update(cx, |model, cx| {
                    model.create_branch(&repo_path, &branch_name, Some(&full_hash), false, cx)
                })
                .await;
            if result.success() {
                model
                    .update(cx, |model, cx| {
                        model.sync_repository_read_model(
                            repo_id.as_str(),
                            SyncRequest::new()
                                .branches(true)
                                .operation_or(result.operation(), "create-branch"),
                            cx,
                        )
                    })
                    .await;
            }
        })
        .detach();
    }

    /// `handleResetHere`: tras confirmar, deja la rama actual en esa entrada
    /// (reset `--hard`) y cierra el diálogo.
    fn reset_here(&mut self, entry: &ReflogEntry, window: &mut Window, cx: &mut Context<Self>) {
        let Some((repo_id, repo_path)) = self.active_repository(cx) else {
            return;
        };
        let model = self.model.clone();
        let full_hash = entry.full_hash.clone();
        let confirmation = model.update(cx, |model, cx| {
            model.confirm(
                ConfirmOptions {
                    title: Some("Reset here".to_string()),
                    description: Some(format!(
                        "Hard-reset the current branch to {} ({})? Uncommitted changes will be lost.",
                        entry.hash, entry.selector
                    )),
                    confirm_text: Some("Reset".to_string()),
                    cancel_text: Some("Cancel".to_string()),
                    variant: Some("destructive".to_string()),
                    ..Default::default()
                },
                cx,
            )
        });

        cx.spawn_in(window, async move |this, cx| {
            if !confirmation.await {
                return;
            }

            let result = model
                .update(cx, |model, cx| {
                    model.reset(&repo_path, &full_hash, "hard", cx)
                })
                .await;
            model.update(cx, |model, cx| {
                model.apply_repository_operation_result(repo_id.as_str(), &result, cx);
            });

            if result.success() {
                // 'external-head-change' fuerza la recarga del historial vía el watcher.
                model
                    .update(cx, |model, cx| {
                        model.sync_repository_read_model(
                            repo_id.as_str(),
                            SyncRequest::new()
                                .status(true)
                                .branches(true)
                                .changes_count(true)
                                .operation("external-head-change"),
                            cx,
                        )
                    })
                    .await;
                this.update_in(cx, |this, window, cx| this.close(window, cx))
                    .ok();
            }
        })
        .detach();
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.dialog_id.take() {
            dialog::close_id(window, cx, id);
        }
    }

    /// `.reflog__action`: botón de 1.35rem que se aviva al pasar el cursor.
    fn action_button(
        id: SharedString,
        title: &'static str,
        icon: Lucide,
        tone: IconTone,
        danger: bool,
        palette: &Palette,
        cx: &App,
    ) -> BaseButton {
        let hover = if danger {
            tint(palette.destructive, 0.12)
        } else {
            palette.sidebar_row_hover.into()
        };
        BaseButton::new(id)
            .key_context(mergeon_ui::BUTTON_KEY_CONTEXT)
            .accessibility_label(title)
            .flex()
            .flex_shrink_0()
            .items_center()
            .justify_center()
            .size(rems(1.35))
            .rounded(Radius::Md.px(cx))
            .opacity(0.7)
            .cursor_pointer()
            .hover(move |style| style.opacity(1.).bg(hover))
            .tooltip(mergeon_ui::tooltip(title))
            .child(plain_icon(icon, 13.).tone(tone))
    }

    /// Las filas `range`, cada una con el hueco que la separa de la
    /// siguiente.
    fn render_rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let palette = *cx.palette();
        let entries = self.entries.clone();
        let last = entries.len().saturating_sub(1);
        range
            .filter_map(|ix| {
                let row = entries.get(ix)?;
                Some(
                    div()
                        .when(ix < last, |this| this.pb(rems(ROW_GAP)))
                        .child(self.render_row(&row.entry, row.date.clone(), &palette, cx))
                        .into_any_element(),
                )
            })
            .collect()
    }

    fn render_row(
        &self,
        entry: &ReflogEntry,
        date: SharedString,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let mono = mono_font(cx);
        let hover = palette.sidebar_row_hover;
        let subject: SharedString = entry.subject.clone().into();
        let key = |prefix: &str| SharedString::from(format!("{prefix}-{}", entry.selector));

        div()
            .id(key("reflog-row"))
            .test_support()
            .aria_label(SharedString::from(format!(
                "{} {} {}",
                entry.selector, entry.hash, entry.subject
            )))
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(rems(0.6))
            .py(rems(0.3))
            .px(rems(0.45))
            .radius(Radius::Md, cx)
            .hover(move |style| style.bg(hover))
            .child(
                div()
                    .w(px(84.))
                    .flex_shrink_0()
                    .font_family(mono.clone())
                    .text_size(rems(0.71))
                    .text_color(palette.primary_text)
                    .child(entry.selector.clone()),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .font_family(mono)
                    .text_size(rems(0.73))
                    .text_color(palette.muted_foreground)
                    .child(entry.hash.clone()),
            )
            .child(
                div()
                    .id(key("reflog-subject"))
                    .tooltip(mergeon_ui::tooltip(subject.clone()))
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(rems(0.8125))
                    .text_color(palette.foreground)
                    .child(subject),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(rems(0.71))
                    .text_color(palette.muted_foreground)
                    .child(date),
            )
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .gap(rems(0.2))
                    .child(
                        Self::action_button(
                            key("reflog-branch"),
                            "Create branch here",
                            Lucide::GitBranchPlus,
                            IconTone::Muted,
                            false,
                            palette,
                            cx,
                        )
                        .on_click(cx.listener({
                            let entry = entry.clone();
                            move |this, _, _, cx| this.create_branch(&entry, cx)
                        })),
                    )
                    .child(
                        Self::action_button(
                            key("reflog-reset"),
                            "Reset current branch here (hard)",
                            Lucide::RotateCcw,
                            IconTone::Danger,
                            true,
                            palette,
                            cx,
                        )
                        .on_click(cx.listener({
                            let entry = entry.clone();
                            move |this, _, window, cx| this.reset_here(&entry, window, cx)
                        })),
                    ),
            )
    }
}

impl Render for ReflogDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        // `.reflog__state`
        let state = || {
            div()
                .id("reflog-state")
                .flex()
                .items_center()
                .gap(rems(0.45))
                .py(rems(1.))
                .px(rems(0.25))
                .text_size(rems(0.8125))
        };

        let content = if self.loading {
            state()
                .test_support()
                .aria_label("Loading reflog…")
                .text_color(palette.foreground)
                .child(
                    mergeon_ui::spinner()
                        .size(SpinnerSize::Sm)
                        .color(SpinnerColor::Primary),
                )
                .child("Loading reflog…")
                .into_any_element()
        } else if self.entries.is_empty() {
            state()
                .test_support()
                .aria_label("No reflog entries.")
                .text_color(palette.muted_foreground)
                .child("No reflog entries.")
                .into_any_element()
        } else {
            let list = uniform_list(
                "reflog-rows",
                self.entries.len(),
                cx.processor(|this, range, _, cx| this.render_rows(range, cx)),
            )
            .with_sizing_behavior(ListSizingBehavior::Infer)
            .track_scroll(&self.scroll_handle);
            let this = cx.entity();
            scroll_list(
                "reflog-list",
                list,
                &self.scroll_handle,
                460.,
                self.overflowing,
                move |overflowing, window, cx| {
                    window.defer(cx, move |_, cx| {
                        this.update(cx, |this, cx| {
                            this.overflowing = overflowing;
                            cx.notify();
                        });
                    });
                },
            )
            .into_any_element()
        };

        div()
            .id("reflog-content")
            .test_support()
            .flex()
            .flex_col()
            .min_h(px(200.))
            .child(content)
    }
}

/// Abre el reflog del repositorio activo. Devuelve `None` si ya estaba
/// abierto.
pub fn open_reflog(window: &mut Window, cx: &mut App) -> Option<DialogId> {
    if is_open(TITLE, window, cx) {
        return None;
    }

    let content = cx.new(ReflogDialog::new);
    let id = AppDialog::new()
        .title(TITLE)
        .description("Where HEAD has been. Restore work after a reset or rebase gone wrong.")
        .width(DialogWidth::Xl3)
        .body({
            let content = content.clone();
            move |_, _| content.clone()
        })
        .open(window, cx);
    content.update(cx, |content, _| content.dialog_id = Some(id));
    Some(id)
}
