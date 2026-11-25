//! Operación de secuencia en curso
//! (`src/components/repository/OperationBanner.vue`): merge, rebase,
//! cherry-pick o revert a medias, con el progreso de sus conflictos y los
//! botones Continue / Skip / Abort.

use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, RenderOnce,
    SharedString, StatefulInteractiveElement as _, Styled as _, TestSupportExt as _, Window, div,
    px, rems,
};
use mergeon_gitcmd::{ConflictState, OperationState};
use mergeon_ui::{ActivePaletteExt as _, IconTone, Lucide, MergeonStyled as _, Radius};

use crate::parts::tint;

/// Lo que se le puede pedir a una operación de secuencia.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SequenceAction {
    Continue,
    Skip,
    Abort,
}

impl SequenceAction {
    /// El nombre que entiende `AppModel::handle_sequence_action`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Continue => "continue",
            Self::Skip => "skip",
            Self::Abort => "abort",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "continue" => Some(Self::Continue),
            "skip" => Some(Self::Skip),
            "abort" => Some(Self::Abort),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Continue => "Continue",
            Self::Skip => "Skip",
            Self::Abort => "Abort",
        }
    }

    fn button_id(self) -> &'static str {
        match self {
            Self::Continue => "sequence-continue",
            Self::Skip => "sequence-skip",
            Self::Abort => "sequence-abort",
        }
    }
}

/// `titleText`: la etiqueta de la operación o, con todos los conflictos
/// resueltos, el aviso de que ya se puede continuar.
pub fn operation_banner_title(
    operation_state: Option<&OperationState>,
    conflict_state: Option<&ConflictState>,
) -> String {
    let Some(operation) = operation_state.filter(|state| state.in_progress) else {
        return String::new();
    };

    if let Some(kind) = operation.kind
        && conflict_state.is_some_and(|conflict| conflict.ready_to_continue)
    {
        return format!(
            "All conflicts resolved. Ready to continue {}.",
            kind.as_str()
        );
    }

    operation.label.clone()
}

/// `detailText`: cuántos conflictos van resueltos mientras queden pendientes.
pub fn operation_banner_detail(
    operation_state: Option<&OperationState>,
    conflict_state: Option<&ConflictState>,
) -> String {
    if !operation_state.is_some_and(|state| state.in_progress) {
        return String::new();
    }

    match conflict_state {
        Some(conflict) if conflict.pending_count > 0 => {
            let total = if conflict.initial_count > 0 {
                conflict.initial_count
            } else {
                conflict.pending_count
            };
            format!("{} of {total} conflicts resolved", conflict.resolved_count)
        }
        _ => String::new(),
    }
}

type ActionHandler = Rc<dyn Fn(SequenceAction, &mut Window, &mut App)>;

/// Banner de la operación de secuencia.
///
/// ```ignore
/// OperationBanner::new(repo.operation_state.clone(), Some(repo.conflict_state.clone()))
///     .compact(true)
///     .busy_action(busy)
///     .on_sequence_action(|action, _, cx| { /* ... */ })
/// ```
#[derive(IntoElement)]
pub struct OperationBanner {
    operation_state: Option<OperationState>,
    conflict_state: Option<ConflictState>,
    show_actions: bool,
    busy_action: Option<SequenceAction>,
    compact: bool,
    on_sequence_action: Option<ActionHandler>,
}

impl OperationBanner {
    pub fn new(
        operation_state: Option<OperationState>,
        conflict_state: Option<ConflictState>,
    ) -> Self {
        Self {
            operation_state,
            conflict_state,
            show_actions: true,
            busy_action: None,
            compact: false,
            on_sequence_action: None,
        }
    }

    /// `showActions` (por defecto sí).
    pub fn show_actions(mut self, show_actions: bool) -> Self {
        self.show_actions = show_actions;
        self
    }

    /// `busyAction`: la acción en curso; su botón se bloquea y dice "Working…".
    pub fn busy_action(mut self, action: Option<SequenceAction>) -> Self {
        self.busy_action = action;
        self
    }

    /// `compact`: a todo el ancho, sin bordes laterales ni radio.
    pub fn compact(mut self, compact: bool) -> Self {
        self.compact = compact;
        self
    }

    /// `@sequence-action`.
    pub fn on_sequence_action(
        mut self,
        handler: impl Fn(SequenceAction, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_sequence_action = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for OperationBanner {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let palette = *cx.palette();
        let operation = self.operation_state.as_ref();
        let conflict = self.conflict_state.as_ref();
        let title: SharedString = operation_banner_title(operation, conflict).into();
        let detail = operation_banner_detail(operation, conflict);
        let in_progress = operation.is_some_and(|state| state.in_progress);

        let actions: Vec<SequenceAction> = operation
            .filter(|_| self.show_actions && in_progress)
            .map(|state| {
                [
                    (state.can_continue, SequenceAction::Continue),
                    (state.can_skip, SequenceAction::Skip),
                    (state.can_abort, SequenceAction::Abort),
                ]
                .into_iter()
                .filter_map(|(allowed, action)| allowed.then_some(action))
                .collect()
            })
            .unwrap_or_default();
        let busy_action = self.busy_action;
        let handler = self.on_sequence_action;

        div()
            .id("operation-banner")
            .test_support()
            .aria_label(title.clone())
            .flex()
            .items_center()
            .justify_between()
            .gap(rems(0.6))
            .border_color(palette.border)
            .bg(palette.card)
            .map(|this| {
                if self.compact {
                    this.border_t_1().border_b_1().py(rems(0.42)).px(rems(0.55))
                } else {
                    this.border_1()
                        .radius(Radius::Xl, cx)
                        .py(rems(0.5))
                        .px(rems(0.65))
                }
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(rems(0.5))
                    .min_w_0()
                    .child(
                        div()
                            .flex()
                            .flex_shrink_0()
                            .items_center()
                            .justify_center()
                            .size(rems(1.55))
                            .border_1()
                            .border_color(tint(palette.primary, 0.24))
                            .radius(Radius::Lg, cx)
                            .bg(tint(palette.primary, 0.08))
                            .child(
                                mergeon_ui::icon(Lucide::GitBranch)
                                    .size(px(14.))
                                    .tone(IconTone::Active),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .min_w_0()
                            .gap(rems(0.08))
                            .text_size(rems(0.75))
                            .child(
                                div()
                                    .truncate()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(palette.foreground)
                                    .child(title),
                            )
                            .when(!detail.is_empty(), |this| {
                                this.child(
                                    div()
                                        .truncate()
                                        .text_color(palette.muted_foreground)
                                        .child(detail),
                                )
                            }),
                    ),
            )
            .when(!actions.is_empty(), |this| {
                this.child(div().flex().flex_shrink_0().gap(rems(0.3)).children(
                    actions.into_iter().map(|action| {
                        let busy = busy_action == Some(action);
                        let handler = handler.clone();
                        mergeon_ui::button(action.button_id())
                            .sm()
                            .map(|this| match action {
                                SequenceAction::Continue => this,
                                SequenceAction::Skip => this.outline(),
                                SequenceAction::Abort => this.destructive(),
                            })
                            .disabled(busy)
                            .label(if busy { "Working…" } else { action.label() })
                            .on_click(move |_, window, cx| {
                                if let Some(handler) = &handler {
                                    handler(action, window, cx);
                                }
                            })
                    }),
                ))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mergeon_gitcmd::OperationType;

    fn operation(kind: OperationType, label: &str) -> OperationState {
        OperationState {
            in_progress: true,
            kind: Some(kind),
            label: label.to_string(),
            can_continue: true,
            can_abort: true,
            can_skip: kind == OperationType::Rebase,
        }
    }

    /// `OperationBanner.test.js`: "renders action buttons for a rebase
    /// operation" (los textos).
    #[test]
    fn shows_the_operation_label_and_the_conflict_progress() {
        let operation = operation(OperationType::Rebase, "Rebase in progress");
        let conflict = ConflictState {
            in_conflict: true,
            operation_type: Some(OperationType::Rebase),
            initial_count: 5,
            pending_count: 3,
            resolved_count: 2,
            pending_files: vec!["file.txt".to_string()],
            ready_to_continue: false,
        };

        assert_eq!(
            operation_banner_title(Some(&operation), Some(&conflict)),
            "Rebase in progress"
        );
        assert_eq!(
            operation_banner_detail(Some(&operation), Some(&conflict)),
            "2 of 5 conflicts resolved"
        );
    }

    /// "shows ready to continue text when there are no pending conflicts".
    #[test]
    fn announces_that_the_operation_can_continue_once_conflicts_are_resolved() {
        let operation = operation(OperationType::Merge, "Merge in progress");
        let conflict = ConflictState {
            in_conflict: true,
            operation_type: Some(OperationType::Merge),
            initial_count: 2,
            pending_count: 0,
            resolved_count: 2,
            pending_files: Vec::new(),
            ready_to_continue: true,
        };

        assert_eq!(
            operation_banner_title(Some(&operation), Some(&conflict)),
            "All conflicts resolved. Ready to continue merge."
        );
        assert_eq!(
            operation_banner_detail(Some(&operation), Some(&conflict)),
            ""
        );
    }

    #[test]
    fn the_total_falls_back_to_the_pending_count() {
        let operation = operation(OperationType::CherryPick, "Cherry-pick in progress");
        let conflict = ConflictState {
            in_conflict: true,
            pending_count: 2,
            ..Default::default()
        };

        assert_eq!(
            operation_banner_detail(Some(&operation), Some(&conflict)),
            "0 of 2 conflicts resolved"
        );
    }

    #[test]
    fn says_nothing_without_an_operation_in_progress() {
        let idle = OperationState::default();
        assert_eq!(operation_banner_title(None, None), "");
        assert_eq!(operation_banner_title(Some(&idle), None), "");
        assert_eq!(operation_banner_detail(Some(&idle), None), "");
    }

    #[test]
    fn sequence_actions_round_trip_through_their_names() {
        for action in [
            SequenceAction::Continue,
            SequenceAction::Skip,
            SequenceAction::Abort,
        ] {
            assert_eq!(SequenceAction::parse(action.as_str()), Some(action));
        }
        assert_eq!(SequenceAction::parse("retry"), None);
    }
}
