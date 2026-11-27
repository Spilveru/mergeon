//! Diálogo «Branch Actions» y su «Branch Comparison»
//! (`src/components/repository/BranchActionDialog.vue`): merge, rebase o
//! comparación entre una rama origen y la rama destino.

use std::rc::Rc;

use gpui_kit::base::Button as BaseButton;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::prelude::*;
use gpui_kit::{
    App, BoxShadow, Context, Div, Entity, EventEmitter, SharedString, TestSupportExt as _, Window,
    div, px, relative, rems,
};
use mergeon_app::{AppModel, StructuredOutcome as _};
use mergeon_backend::{
    BranchComparison, ComparedCommit, MergeConflictCheck, MergeOptions, Outcome, PushOptions,
    RebaseOptions,
};
use mergeon_model::dialogs::{ConfirmOptions, PromptOptions};
use mergeon_model::notify::{NotifyOptions, NotifyType};
use mergeon_model::operation_types::OperationResult;
use mergeon_ui::dialog::{self, AppDialog, DialogId, DialogWidth};
use mergeon_ui::theme::with_alpha;
use mergeon_ui::{
    ActivePaletteExt as _, AlertVariant, IconTone, Lucide, MergeonStyled as _, Radius,
    SpinnerColor, SpinnerSize, TextareaState,
};

use crate::menus::BranchAction;

/// Lo que el diálogo le pide a quien lo abrió cuando una acción termina
/// (`emit('action-completed', ...)`).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct BranchActionCompleted {
    /// Resultado del merge o del rebase.
    pub result: OperationResult,
    /// Operación que se registra como última mutación (`merge-conflict`,
    /// `rebase-conflict` o la del resultado).
    pub mutation_operation: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BranchActionDialogEvent {
    /// En caja: el resultado de la operación es mucho mayor que `Close`.
    ActionCompleted(Box<BranchActionCompleted>),
    /// "Browse changes" en Branch Comparison: abrir la comparación navegable
    /// de `head` (el origen) frente a `base` (el destino). El diálogo ya se
    /// cerró.
    BrowseComparison { base: String, head: String },
    /// El diálogo se cerró (`emit('close')`).
    Close,
}

/// Estado del diálogo. Se abre con [`open_branch_action_dialog`].
pub struct BranchActionDialog {
    model: Entity<AppModel>,
    repo_path: String,
    source_branch: String,
    target_branch: String,
    checking_conflicts: bool,
    conflict_check: Option<MergeConflictCheck>,
    show_merge_options: bool,
    selected_action: Option<BranchAction>,
    busy_action: Option<BranchAction>,
    merge_no_ff: bool,
    merge_squash: bool,
    push_after_merge: bool,
    merge_message: Entity<TextareaState>,
    dialog_id: Option<DialogId>,
    comparison: Option<BranchComparison>,
}

impl EventEmitter<BranchActionDialogEvent> for BranchActionDialog {}

/// `result.success || result.outcome === 'noop'`.
fn succeeded_or_noop(success: bool, outcome: Option<Outcome>) -> bool {
    success || outcome == Some(Outcome::Noop)
}

impl BranchActionDialog {
    fn new(
        model: Entity<AppModel>,
        repo_path: &str,
        source_branch: &str,
        target_branch: &str,
        initial_action: Option<BranchAction>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let merge_message = cx.new(|cx| {
            TextareaState::new(window, cx)
                .rows(3)
                .placeholder("Optional merge message")
        });

        let mut this = Self {
            model,
            repo_path: repo_path.to_string(),
            source_branch: source_branch.to_string(),
            target_branch: target_branch.to_string(),
            checking_conflicts: false,
            conflict_check: None,
            show_merge_options: initial_action == Some(BranchAction::Merge),
            selected_action: initial_action,
            busy_action: None,
            merge_no_ff: false,
            merge_squash: false,
            push_after_merge: false,
            merge_message,
            dialog_id: None,
            comparison: None,
        };
        this.run_conflict_check(cx);
        this
    }

    pub fn source_branch(&self) -> &str {
        &self.source_branch
    }

    pub fn target_branch(&self) -> &str {
        &self.target_branch
    }

    /// `dialogTitle`.
    pub fn title(&self) -> String {
        format!(
            "Branch Actions: {} → {}",
            self.source_branch, self.target_branch
        )
    }

    pub fn selected_action(&self) -> Option<BranchAction> {
        self.selected_action
    }

    /// `isBusy`: hay un merge, un rebase o una comparación en curso.
    pub fn is_busy(&self) -> bool {
        self.busy_action.is_some()
    }

    pub fn is_open(&self) -> bool {
        self.dialog_id.is_some()
    }

    pub fn is_checking_conflicts(&self) -> bool {
        self.checking_conflicts
    }

    pub fn conflict_check(&self) -> Option<&MergeConflictCheck> {
        self.conflict_check.as_ref()
    }

    /// La última comparación cargada con «Compare».
    pub fn comparison(&self) -> Option<&BranchComparison> {
        self.comparison.as_ref()
    }

    pub fn merge_message_input(&self) -> &Entity<TextareaState> {
        &self.merge_message
    }

    pub fn set_push_after_merge(&mut self, checked: bool, cx: &mut Context<Self>) {
        self.push_after_merge = checked;
        cx.notify();
    }

    pub fn set_merge_no_ff(&mut self, checked: bool, cx: &mut Context<Self>) {
        self.merge_no_ff = checked;
        cx.notify();
    }

    pub fn set_merge_squash(&mut self, checked: bool, cx: &mut Context<Self>) {
        self.merge_squash = checked;
        cx.notify();
    }

    /// `runConflictCheck`: predice si integrar las dos ramas dará conflictos.
    fn run_conflict_check(&mut self, cx: &mut Context<Self>) {
        if self.repo_path.is_empty()
            || self.source_branch.is_empty()
            || self.target_branch.is_empty()
        {
            return;
        }
        self.checking_conflicts = true;
        self.conflict_check = None;
        let check = self.model.update(cx, |model, cx| {
            model.check_merge_conflicts(
                &self.repo_path,
                &self.source_branch,
                &self.target_branch,
                cx,
            )
        });
        cx.spawn(async move |this, cx| {
            let result = check.await;
            this.update(cx, |this, cx| {
                this.conflict_check = result.ok();
                this.checking_conflicts = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn set_busy(&mut self, action: Option<BranchAction>, cx: &mut Context<Self>) {
        self.busy_action = action;
        cx.notify();
    }

    /// `emitActionCompleted`.
    fn emit_action_completed(
        &mut self,
        result: OperationResult,
        mutation_operation: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let mutation_operation = mutation_operation
            .map(str::to_string)
            .or_else(|| result.operation.clone().filter(|name| !name.is_empty()));
        cx.emit(BranchActionDialogEvent::ActionCompleted(Box::new(
            BranchActionCompleted {
                result,
                mutation_operation,
            },
        )));
    }

    /// `resetDialogState` + `emit('close')`.
    fn close_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.selected_action = None;
        self.show_merge_options = false;
        self.merge_no_ff = false;
        self.merge_squash = false;
        self.push_after_merge = false;
        if let Some(id) = self.dialog_id.take() {
            dialog::close_id(window, cx, id);
        }
        cx.emit(BranchActionDialogEvent::Close);
        cx.notify();
    }

    /// `handleClose`: no se cierra con una acción en curso.
    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_busy() {
            return;
        }
        self.close_dialog(window, cx);
    }

    /// Cierra sin avisar a nadie (cambió el repositorio activo).
    pub fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.dialog_id.take() {
            dialog::close_id(window, cx, id);
        }
    }

    /// La capa de diálogos ya quitó el diálogo (X, Escape o clic fuera). Con una
    /// acción en curso el original no dejaba cerrarlo: se vuelve a mostrar.
    fn dismissed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dialog_id = None;
        if self.is_busy() {
            let view = cx.entity();
            window.defer(cx, move |window, cx| Self::present(&view, window, cx));
            return;
        }
        self.close_dialog(window, cx);
    }

    /// El push posterior al merge no puede asumir que el remoto se llama
    /// `origin`: se prefiere `origin` si existe y, si no, el primero.
    async fn resolve_push_remote(
        model: &Entity<AppModel>,
        repo_path: &str,
        cx: &mut gpui_kit::AsyncWindowContext,
    ) -> String {
        let remotes = model
            .update(cx, |model, cx| model.get_remotes(repo_path, cx))
            .await;
        match remotes {
            Ok(remotes) => remotes
                .iter()
                .find(|remote| remote.name == "origin")
                .or_else(|| remotes.first())
                .map(|remote| remote.name.clone())
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| "origin".to_string()),
            Err(_) => "origin".to_string(),
        }
    }

    /// `handleMerge`.
    pub fn merge(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_busy() {
            return;
        }
        self.selected_action = Some(BranchAction::Merge);
        cx.notify();

        let model = self.model.clone();
        let repo_path = self.repo_path.clone();
        let source = self.source_branch.clone();
        let target = self.target_branch.clone();
        // Como en el original, el texto de la confirmación usa la casilla tal
        // como está ahora y el push se decide al confirmar.
        let push_at_prompt = self.push_after_merge;

        cx.spawn_in(window, async move |this, cx| {
            let confirmed = model
                .update(cx, |model, cx| {
                    model.confirm(
                        ConfirmOptions {
                            title: Some("Confirm merge".to_string()),
                            description: Some(format!(
                                "This will switch to '{target}' and merge '{source}' into it{}",
                                if push_at_prompt {
                                    ", then push the result."
                                } else {
                                    "."
                                }
                            )),
                            confirm_text: Some(
                                if push_at_prompt {
                                    "Merge & Push"
                                } else {
                                    "Merge"
                                }
                                .to_string(),
                            ),
                            cancel_text: Some("Cancel".to_string()),
                            ..Default::default()
                        },
                        cx,
                    )
                })
                .await;
            if !confirmed {
                return;
            }

            let Ok((should_push, options)) = this.update(cx, |this, cx| {
                this.set_busy(Some(BranchAction::Merge), cx);
                let message = this.merge_message.read(cx).value().to_string();
                (
                    this.push_after_merge,
                    MergeOptions {
                        target_branch: Some(this.target_branch.clone()),
                        no_fast_forward: this.merge_no_ff,
                        squash: this.merge_squash,
                        message: (!message.is_empty()).then_some(message),
                    },
                )
            }) else {
                return;
            };

            let result = model
                .update(cx, |model, cx| {
                    model.merge(&repo_path, &source, options, should_push, cx)
                })
                .await;
            let summary = result.to_operation_result();

            if succeeded_or_noop(result.success(), result.outcome()) {
                if should_push {
                    let push_remote = Self::resolve_push_remote(&model, &repo_path, cx).await;
                    let push = |remote: String, cx: &mut gpui_kit::AsyncWindowContext| {
                        model.update(cx, |model, cx| {
                            let options = PushOptions {
                                remote,
                                auth_token: model.git_auth_token(),
                                ..PushOptions::default()
                            };
                            model.push(&repo_path, options, true, cx)
                        })
                    };
                    let mut push_result = push(push_remote, cx).await;

                    if push_result.code() == "REMOTE_NOT_FOUND" {
                        let remote_result =
                            Self::prompt_for_origin_remote(&model, &repo_path, cx).await;
                        match remote_result {
                            Some(remote_result) if remote_result.success() => {
                                push_result = push("origin".to_string(), cx).await;
                            }
                            Some(remote_result) => push_result = remote_result,
                            None => {}
                        }
                    }

                    let pushed = succeeded_or_noop(push_result.success(), push_result.outcome());
                    let details = push_result.message().to_string();
                    model.update(cx, |model, cx| {
                        if pushed {
                            model.notify_success("Merge and push completed successfully", cx);
                        } else {
                            model.notify(
                                NotifyType::Error,
                                "Merge completed, but push failed.",
                                NotifyOptions {
                                    details: Some(details),
                                    ..Default::default()
                                },
                                cx,
                            );
                        }
                    });
                }

                this.update_in(cx, |this, window, cx| {
                    this.busy_action = None;
                    this.emit_action_completed(summary, None, cx);
                    this.close_dialog(window, cx);
                })
                .ok();
                return;
            }

            let conflicted = result.code() == "CONFLICT";
            if should_push {
                // Con push pedido el merge fue silencioso: el fallo se avisa aquí.
                let message = if result.message().is_empty() {
                    "Merge failed.".to_string()
                } else {
                    result.message().to_string()
                };
                model.update(cx, |model, cx| {
                    if conflicted {
                        model.notify_warning(&message, cx);
                    } else {
                        model.notify_error(&message, cx);
                    }
                });
            }

            this.update_in(cx, |this, window, cx| {
                this.set_busy(None, cx);
                if conflicted {
                    this.emit_action_completed(summary, Some("merge-conflict"), cx);
                    this.close_dialog(window, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// `promptForOriginRemote` del diálogo: pide la URL de `origin` y lo añade
    /// sin avisos. `None` si se cancela.
    async fn prompt_for_origin_remote(
        model: &Entity<AppModel>,
        repo_path: &str,
        cx: &mut gpui_kit::AsyncWindowContext,
    ) -> Option<mergeon_app::GitMutation> {
        let answer = model
            .update(cx, |model, cx| {
                model.prompt(
                    PromptOptions {
                        title: Some("Configure remote".to_string()),
                        description: Some(
                            "No remote repository is configured. Enter the origin URL.".to_string(),
                        ),
                        confirm_text: Some("Add remote".to_string()),
                        cancel_text: Some("Cancel".to_string()),
                        placeholder: Some("https://github.com/username/repository.git".to_string()),
                        pattern: Some("^https?://.+".to_string()),
                        input_error_message: Some("Please enter a valid https URL.".to_string()),
                        ..Default::default()
                    },
                    cx,
                )
            })
            .await;
        let value = answer
            .value
            .filter(|value| answer.confirmed && !value.is_empty())?;

        let clean_url = crate::logic::normalize_remote_url(&value);
        Some(
            model
                .update(cx, |model, cx| {
                    model.add_remote(repo_path, "origin", &clean_url, true, cx)
                })
                .await,
        )
    }

    /// `handleRebase`.
    pub fn rebase(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_busy() {
            return;
        }
        self.selected_action = Some(BranchAction::Rebase);
        cx.notify();

        let model = self.model.clone();
        let repo_path = self.repo_path.clone();
        let source = self.source_branch.clone();
        let target = self.target_branch.clone();

        cx.spawn_in(window, async move |this, cx| {
            let confirmed = model
                .update(cx, |model, cx| {
                    model.confirm(
                        ConfirmOptions {
                            title: Some("Confirm rebase".to_string()),
                            description: Some(format!(
                                "This will rebase '{target}' onto '{source}'."
                            )),
                            confirm_text: Some("Rebase".to_string()),
                            cancel_text: Some("Cancel".to_string()),
                            ..Default::default()
                        },
                        cx,
                    )
                })
                .await;
            if !confirmed {
                return;
            }

            if this
                .update(cx, |this, cx| this.set_busy(Some(BranchAction::Rebase), cx))
                .is_err()
            {
                return;
            }

            let result = model
                .update(cx, |model, cx| {
                    // Se rebasa la rama destino (no la actual): si no está
                    // activa, la operación cambia antes a ella.
                    model.rebase(
                        &repo_path,
                        &source,
                        RebaseOptions {
                            target_branch: Some(target.clone()),
                            ..Default::default()
                        },
                        cx,
                    )
                })
                .await;
            let summary = result.to_operation_result();
            let completed = succeeded_or_noop(result.success(), result.outcome());
            let conflicted = result.code() == "CONFLICT";

            this.update_in(cx, |this, window, cx| {
                this.set_busy(None, cx);
                if completed {
                    this.emit_action_completed(summary, None, cx);
                    this.close_dialog(window, cx);
                } else if conflicted {
                    this.emit_action_completed(summary, Some("rebase-conflict"), cx);
                    this.close_dialog(window, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// `handleCompare`: carga la comparación y abre «Branch Comparison».
    pub fn compare(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_busy() {
            return;
        }
        self.selected_action = Some(BranchAction::Compare);
        self.set_busy(Some(BranchAction::Compare), cx);

        let model = self.model.clone();
        let compare = model.update(cx, |model, cx| {
            model.compare_branches(
                &self.repo_path,
                &self.target_branch,
                &self.source_branch,
                cx,
            )
        });

        cx.spawn_in(window, async move |this, cx| {
            let result = compare.await;
            match result {
                Ok(comparison) => {
                    this.update_in(cx, |this, window, cx| {
                        this.set_busy(None, cx);
                        this.comparison = Some(comparison.clone());
                        let dialog = cx.entity().downgrade();
                        open_comparison_dialog(
                            &this.source_branch,
                            &this.target_branch,
                            comparison,
                            Some(Rc::new(move |window: &mut Window, cx: &mut App| {
                                dialog
                                    .update(cx, |this, cx| this.browse_comparison(window, cx))
                                    .ok();
                            })),
                            window,
                            cx,
                        );
                    })
                    .ok();
                }
                Err(error) => {
                    model.update(cx, |model, cx| {
                        model.notify_error_with(
                            "Unable to compare these branches right now.",
                            &error,
                            cx,
                        );
                    });
                    this.update(cx, |this, cx| this.set_busy(None, cx)).ok();
                }
            }
        })
        .detach();
    }

    /// "Browse changes": cierra Branch Comparison y Branch Actions y pide la
    /// comparación navegable (los cambios del origen desde que se separó del
    /// destino).
    pub fn browse_comparison(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_busy() {
            return;
        }
        dialog::close(window, cx);
        cx.emit(BranchActionDialogEvent::BrowseComparison {
            base: self.target_branch.clone(),
            head: self.source_branch.clone(),
        });
        self.close_dialog(window, cx);
    }

    fn run(&mut self, action: BranchAction, window: &mut Window, cx: &mut Context<Self>) {
        match action {
            BranchAction::Merge => self.merge(window, cx),
            BranchAction::Rebase => self.rebase(window, cx),
            BranchAction::Compare => self.compare(window, cx),
        }
    }

    fn render_branches(&self, cx: &App) -> impl IntoElement {
        let palette = cx.palette();
        // Nombres largos se recortan en vez de desbordar la tarjeta.
        let chip = |label: &str, target: bool| {
            div()
                .min_w_0()
                .max_w_full()
                .truncate()
                .radius(Radius::Lg, cx)
                .border_1()
                .px(rems(0.625))
                .py(rems(0.375))
                .tw_text_sm()
                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                .text_color(palette.foreground)
                .map(|this| {
                    if target {
                        this.border_color(with_alpha(palette.primary, 0.3))
                            .bg(with_alpha(palette.primary, 0.1))
                    } else {
                        this.border_color(palette.border).bg(palette.background)
                    }
                })
                .child(label.to_string())
        };
        let column = |label: &'static str, content: Div| {
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .items_center()
                .gap_1()
                .child(
                    div()
                        .tw_text_xs()
                        .text_color(palette.muted_foreground)
                        .child(label),
                )
                .child(content)
        };

        section_card(cx).child(
            div()
                .flex()
                .items_center()
                .justify_center()
                .gap_3()
                .child(column("Source", chip(&self.source_branch, false)))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_shrink_0()
                        .gap_1()
                        // El "." invisible que alinea la flecha con las fichas.
                        .child(div().tw_text_xs().invisible().child("."))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_center()
                                .size_8()
                                .radius(Radius::Lg, cx)
                                .border_1()
                                .border_color(with_alpha(palette.primary, 0.25))
                                .bg(with_alpha(palette.primary, 0.1))
                                .child(
                                    mergeon_ui::icon(Lucide::ArrowRight)
                                        .size(px(14.))
                                        .tone(IconTone::Active),
                                ),
                        ),
                )
                .child(column("Target", chip(&self.target_branch, true))),
        )
    }

    fn render_conflict_check(&self, cx: &App) -> Option<gpui_kit::AnyElement> {
        if self.checking_conflicts {
            return Some(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_1()
                    .tw_text_xs()
                    .text_color(cx.palette().muted_foreground)
                    .child(
                        mergeon_ui::spinner()
                            .size(SpinnerSize::Sm)
                            .color(SpinnerColor::Current),
                    )
                    .child("Checking for conflicts…")
                    .into_any_element(),
            );
        }

        let check = self.conflict_check.as_ref()?;
        Some(if check.has_conflicts {
            let count = if check.conflicted_files.is_empty() {
                "one or more".to_string()
            } else {
                check.conflicted_files.len().to_string()
            };
            mergeon_ui::alert(AlertVariant::Warning)
                .title("Merge conflicts expected")
                .description(format!(
                    "Integrating these branches will conflict in {count} file(s) — you'll need to resolve them."
                ))
                .into_any_element()
        } else {
            mergeon_ui::alert(AlertVariant::Success)
                .title("No conflicts")
                .description("These branches integrate cleanly.")
                .into_any_element()
        })
    }

    fn render_card(&self, action: BranchAction, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let busy = self.is_busy();
        let selected = self.selected_action == Some(action);
        let running = self.busy_action == Some(action);
        let (id, icon, title, description, status) = match action {
            BranchAction::Merge => (
                "branch-action-merge",
                Lucide::GitMerge,
                "Merge",
                format!("Merge {} into {}.", self.source_branch, self.target_branch),
                "Merging…",
            ),
            BranchAction::Rebase => (
                "branch-action-rebase",
                Lucide::Workflow,
                "Rebase",
                format!(
                    "Replay {} on top of {}.",
                    self.target_branch, self.source_branch
                ),
                "Rebasing…",
            ),
            BranchAction::Compare => (
                "branch-action-compare",
                Lucide::SquareSplitVertical,
                "Compare",
                "Inspect commit and file differences before acting.".to_string(),
                "Loading comparison…",
            ),
        };

        let inset = |alpha: f32| {
            vec![
                BoxShadow::new(px(0.), px(0.), with_alpha(palette.primary, alpha))
                    .spread_radius(px(1.))
                    .inset(),
            ]
        };

        BaseButton::new(id)
            .key_context(mergeon_ui::BUTTON_KEY_CONTEXT)
            .accessibility_label(title)
            .disabled(busy)
            .relative()
            .flex()
            .flex_col()
            // Son `<button>`: el contenido queda centrado en vertical cuando la
            // tarjeta es más alta que él (las tres miden lo que la más alta).
            .justify_center()
            .flex_1()
            .min_w_0()
            .items_start()
            .border_1()
            .radius(Radius::Xxl, cx)
            .p(rems(0.8))
            .map(|this| {
                if selected {
                    this.border_color(with_alpha(palette.primary, 0.44))
                        .bg(with_alpha(palette.primary, 0.12))
                        .shadow(inset(0.16))
                } else {
                    this.border_color(palette.border)
                        .bg(with_alpha(palette.card, 0.78))
                        .when(action == BranchAction::Merge, |this| {
                            this.shadow(inset(0.08))
                        })
                        .when(!busy, |this| {
                            this.hover(|style| {
                                style
                                    .border_color(with_alpha(palette.primary, 0.4))
                                    .bg(palette.accent)
                            })
                        })
                }
            })
            .map(|this| {
                if busy {
                    this.opacity(0.72).cursor_not_allowed()
                } else {
                    this.cursor_pointer()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.run(action, window, cx);
                        }))
                }
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(rems(1.9))
                    .radius(Radius::Xl, cx)
                    .border_1()
                    .border_color(palette.border)
                    .bg(with_alpha(palette.background, 0.82))
                    .child(mergeon_ui::icon(icon).size(px(20.)).tone(IconTone::Active)),
            )
            .child(
                div()
                    .mt(rems(0.55))
                    .tw_text_sm()
                    .tw_font_medium()
                    .text_color(palette.foreground)
                    .child(title),
            )
            .child(
                div()
                    .w_full()
                    .mt(rems(0.28))
                    .text_size(rems(0.8125))
                    .line_height(relative(1.4))
                    .text_color(palette.muted_foreground)
                    .child(description),
            )
            .when(running, |this| {
                this.child(
                    div()
                        .mt(rems(0.38))
                        .tw_text_xs()
                        .tw_font_medium()
                        .text_color(palette.primary_text)
                        .child(status),
                )
            })
            .when(action == BranchAction::Merge, |this| {
                this.child(
                    // `@click.stop`: marcar la casilla no lanza el merge.
                    div()
                        .id("branch-action-push-option")
                        .w_full()
                        .mt(rems(0.55))
                        .on_click(|_, _, cx| cx.stop_propagation())
                        .child(
                            // La etiqueta va como hijo para que pueda partirse
                            // en dos líneas dentro de la tarjeta.
                            mergeon_ui::checkbox("branch-action-push")
                                .w_full()
                                .child(div().flex_1().min_w_0().child("Push after merge"))
                                .checked(self.push_after_merge)
                                .disabled(busy)
                                .on_change(cx.listener(|this, checked: &bool, _, cx| {
                                    this.set_push_after_merge(*checked, cx);
                                })),
                        ),
                )
            })
    }

    fn render_merge_options(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let busy = self.is_busy();

        section_card(cx)
            .child(
                div()
                    .mb_2()
                    .tw_text_xs()
                    .tw_font_medium()
                    .text_color(palette.muted_foreground)
                    .child("Merge options"),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        // Las dos casillas van en línea, una tras otra, con aire
                        // entre ellas (y entre filas si no caben).
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap_x(rems(1.25))
                            .gap_y(rems(0.5))
                            .child(
                                mergeon_ui::checkbox("merge-no-ff")
                                    .label("No fast-forward (--no-ff)")
                                    .checked(self.merge_no_ff)
                                    .disabled(busy)
                                    .on_change(cx.listener(|this, checked: &bool, _, cx| {
                                        this.set_merge_no_ff(*checked, cx);
                                    })),
                            )
                            .child(
                                mergeon_ui::checkbox("merge-squash")
                                    .label("Squash commits (--squash)")
                                    .checked(self.merge_squash)
                                    .disabled(busy)
                                    .on_change(cx.listener(|this, checked: &bool, _, cx| {
                                        this.set_merge_squash(*checked, cx);
                                    })),
                            ),
                    )
                    .when(self.merge_squash, |this| {
                        this.child(
                            div()
                                .radius(Radius::Xl, cx)
                                .border_1()
                                .border_color(with_alpha(palette.warning, 0.3))
                                // El original pide `bg-warning-soft` y
                                // `text-warning-soft-foreground`, pero el tema no
                                // define esas utilidades: en la app el aviso sale
                                // sin fondo y con el color de texto normal.
                                .px_3()
                                .py(rems(0.625))
                                .tw_text_xs()
                                .text_color(palette.foreground)
                                .child(
                                    "Squash does not create a commit automatically. You will need to commit manually after the merge.",
                                ),
                        )
                    })
                    .child(
                        mergeon_ui::textarea(&self.merge_message)
                            .rows(3)
                            .disabled(busy),
                    ),
            )
    }

    /// Pone el diálogo en pantalla sobre la entidad ya creada.
    fn present(view: &Entity<Self>, window: &mut Window, cx: &mut App) {
        let title = view.read(cx).title();
        let id = AppDialog::new()
            .title(title)
            .description(
                "Choose how to integrate the source branch into the current target branch.",
            )
            .width(DialogWidth::Lg)
            .body({
                let view = view.clone();
                move |_, _| view.clone()
            })
            .footer({
                let view = view.clone();
                move |_, cx| {
                    let busy = view.read(cx).is_busy();
                    div().flex().justify_end().child(
                        mergeon_ui::button("branch-action-cancel")
                            .ghost()
                            .sm()
                            .disabled(busy)
                            .label("Cancel")
                            .on_click({
                                let view = view.clone();
                                move |_, window, cx| {
                                    view.update(cx, |view, cx| view.close(window, cx));
                                }
                            }),
                    )
                }
            })
            .on_close({
                let view = view.clone();
                move |window, cx| view.update(cx, |view, cx| view.dismissed(window, cx))
            })
            .open(window, cx);
        view.update(cx, |view, _| view.dialog_id = Some(id));
    }
}

/// `rounded-xl border border-border bg-card/70 p-2.5`.
fn section_card(cx: &App) -> Div {
    let palette = cx.palette();
    div()
        .radius(Radius::Xl, cx)
        .border_1()
        .border_color(palette.border)
        .bg(with_alpha(palette.card, 0.7))
        .p(rems(0.625))
}

impl Render for BranchActionDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap(rems(0.625))
            .child(self.render_branches(cx))
            .children(self.render_conflict_check(cx))
            .child(
                div()
                    .flex()
                    .items_stretch()
                    .gap(rems(0.625))
                    .child(self.render_card(BranchAction::Merge, cx))
                    .child(self.render_card(BranchAction::Rebase, cx))
                    .child(self.render_card(BranchAction::Compare, cx)),
            )
            .when(self.show_merge_options, |this| {
                this.child(self.render_merge_options(cx))
            })
    }
}

/// Abre «Branch Actions» entre `source_branch` y `target_branch`, con la
/// acción de `initial_action` preseleccionada (y, si es Merge, sus opciones a
/// la vista). Quien llama se suscribe al [`BranchActionDialogEvent`] de la
/// entidad devuelta.
pub fn open_branch_action_dialog(
    model: &Entity<AppModel>,
    repo_path: &str,
    source_branch: &str,
    target_branch: &str,
    initial_action: Option<BranchAction>,
    window: &mut Window,
    cx: &mut App,
) -> Entity<BranchActionDialog> {
    let view = cx.new(|cx| {
        BranchActionDialog::new(
            model.clone(),
            repo_path,
            source_branch,
            target_branch,
            initial_action,
            window,
            cx,
        )
    });
    BranchActionDialog::present(&view, window, cx);
    view
}

// ---------------------------------------------------------------------------
// Branch Comparison
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum StatTone {
    Neutral,
    Success,
    Danger,
}

fn stat_card(value: String, label: &'static str, tone: StatTone, cx: &App) -> impl IntoElement {
    let palette = cx.palette();
    div()
        .flex_1()
        .min_w_0()
        .radius(Radius::Xxl, cx)
        .border_1()
        .p(rems(0.8))
        .map(|this| match tone {
            StatTone::Neutral => this.border_color(palette.border).bg(palette.card),
            StatTone::Success => this
                .border_color(with_alpha(palette.success, 0.35))
                .bg(with_alpha(palette.success, 0.08)),
            StatTone::Danger => this
                .border_color(with_alpha(palette.destructive, 0.35))
                .bg(with_alpha(palette.destructive, 0.08)),
        })
        .child(
            div()
                .text_size(rems(1.125))
                .tw_font_medium()
                .text_color(palette.foreground)
                .child(value),
        )
        .child(
            div()
                .mt(rems(0.2))
                .tw_text_xs()
                .text_color(palette.muted_foreground)
                .child(label),
        )
}

/// Línea bajo una lista de commits cortada: la comparación lista como mucho
/// `COMPARE_COMMIT_LIMIT` por lado, pero cuenta todos.
fn more_commits_label(omitted: usize) -> Option<String> {
    match omitted {
        0 => None,
        1 => Some("and 1 more commit".to_string()),
        omitted => Some(format!("and {omitted} more commits")),
    }
}

fn commits_section(
    title: String,
    commits: &[ComparedCommit],
    omitted: usize,
    more_id: &'static str,
    cx: &App,
) -> impl IntoElement {
    let palette = cx.palette();
    let mono = cx.theme().mono_font_family.clone();
    div()
        .flex()
        .flex_col()
        .gap_3()
        .child(
            div()
                .tw_text_sm()
                .tw_font_medium()
                .text_color(palette.muted_foreground)
                .child(title),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(rems(0.55))
                .children(commits.iter().map(|commit| {
                    div()
                        .flex()
                        .items_center()
                        .gap(rems(0.8))
                        .border_1()
                        .border_color(palette.border)
                        .radius(Radius::Xxl, cx)
                        .px(rems(0.85))
                        .py(rems(0.7))
                        .bg(with_alpha(palette.background, 0.7))
                        .child(
                            div()
                                .w(px(88.))
                                .flex_shrink_0()
                                .font_family(mono.clone())
                                .text_color(palette.primary_text)
                                .child(commit.hash.clone()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_color(palette.foreground)
                                .child(commit.message.clone()),
                        )
                        .child(
                            div()
                                .w(px(180.))
                                .flex_shrink_0()
                                .text_right()
                                .text_size(rems(0.8125))
                                .text_color(palette.muted_foreground)
                                .child(commit.author.clone()),
                        )
                })),
        )
        .when_some(more_commits_label(omitted), |this, label| {
            this.child(
                div()
                    .id(more_id)
                    .test_support()
                    .px(rems(0.85))
                    .tw_text_sm()
                    .text_color(palette.muted_foreground)
                    .child(label),
            )
        })
}

/// Abre «Branch Comparison» con una comparación ya calculada.
pub fn open_branch_comparison_dialog(
    source_branch: &str,
    target_branch: &str,
    comparison: BranchComparison,
    window: &mut Window,
    cx: &mut App,
) -> DialogId {
    open_comparison_dialog(source_branch, target_branch, comparison, None, window, cx)
}

type BrowseHandler = Rc<dyn Fn(&mut Window, &mut App)>;

/// «Branch Comparison»; con `on_browse`, el pie ofrece "Browse changes".
fn open_comparison_dialog(
    source_branch: &str,
    target_branch: &str,
    comparison: BranchComparison,
    on_browse: Option<BrowseHandler>,
    window: &mut Window,
    cx: &mut App,
) -> DialogId {
    let source: SharedString = source_branch.to_string().into();
    let target: SharedString = target_branch.to_string().into();

    AppDialog::new()
        .title("Branch Comparison")
        .description(format!("Differences between {source} and {target}."))
        .width(DialogWidth::Xl3)
        .body(move |_, cx| {
            div()
                .flex()
                .flex_col()
                .gap_6()
                .child(
                    div()
                        .flex()
                        .gap_3()
                        .child(stat_card(
                            comparison.ahead.to_string(),
                            "Ahead",
                            StatTone::Neutral,
                            cx,
                        ))
                        .child(stat_card(
                            comparison.behind.to_string(),
                            "Behind",
                            StatTone::Neutral,
                            cx,
                        ))
                        .child(stat_card(
                            comparison.files_changed.to_string(),
                            "Files changed",
                            StatTone::Neutral,
                            cx,
                        ))
                        .child(stat_card(
                            format!("+{}", comparison.insertions),
                            "Insertions",
                            StatTone::Success,
                            cx,
                        ))
                        .child(stat_card(
                            format!("-{}", comparison.deletions),
                            "Deletions",
                            StatTone::Danger,
                            cx,
                        )),
                )
                .when(!comparison.ahead_commits.is_empty(), |this| {
                    this.child(commits_section(
                        format!("Commits in {source} not in {target}"),
                        &comparison.ahead_commits,
                        comparison.ahead_omitted(),
                        "branch-comparison-ahead-more",
                        cx,
                    ))
                })
                .when(!comparison.behind_commits.is_empty(), |this| {
                    this.child(commits_section(
                        format!("Commits in {target} not in {source}"),
                        &comparison.behind_commits,
                        comparison.behind_omitted(),
                        "branch-comparison-behind-more",
                        cx,
                    ))
                })
        })
        .footer(move |_, _| {
            div()
                .flex()
                .justify_end()
                .gap_2()
                .child(
                    mergeon_ui::button("branch-comparison-close")
                        .ghost()
                        .sm()
                        .label("Close")
                        .on_click(|_, window, cx| {
                            dialog::close(window, cx);
                        }),
                )
                .when_some(on_browse.clone(), |footer, on_browse| {
                    footer.child(
                        mergeon_ui::button("branch-comparison-browse")
                            .sm()
                            .icon(Lucide::GitCompareArrows)
                            .label("Browse changes")
                            .tooltip(
                                "Open the full comparison with the changed files and their diffs",
                            )
                            .on_click(move |_, window, cx| on_browse(window, cx)),
                    )
                })
        })
        .open(window, cx)
}

#[cfg(test)]
mod tests {
    use super::more_commits_label;

    #[test]
    fn says_how_many_commits_the_comparison_left_out() {
        assert_eq!(more_commits_label(0), None);
        assert_eq!(more_commits_label(1).as_deref(), Some("and 1 more commit"));
        assert_eq!(
            more_commits_label(4_800).as_deref(),
            Some("and 4800 more commits")
        );
    }
}
