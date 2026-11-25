//! Indicador de actividad del chip central de la barra
//! (`src/components/layout/OperationIndicator.vue`): un spinner con el número
//! de operaciones en curso y, al pasar el cursor, el detalle (operación de
//! secuencia y cola de mutaciones con sus botones de cancelar).

use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, RenderOnce,
    SharedString, StatefulInteractiveElement as _, Styled as _, TestSupportExt as _, Window, div,
    px, relative, rems,
};
use mergeon_gitcmd::{ConflictState, OperationState};
use mergeon_model::repositories::MutationQueue;
use mergeon_ui::{ActivePaletteExt as _, HoverCardAlign, Lucide, MergeonStyled as _, Radius};

use crate::mutation_queue_banner::{MutationQueueBanner, queue_operation_count};

type CancelHandler = Rc<dyn Fn(&str, &mut Window, &mut App)>;

/// Lo que el indicador deriva de sus propiedades (los `computed` del original).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct OperationActivity {
    /// Hay una operación de secuencia a medias (`hasSequence`).
    pub has_sequence: bool,
    /// `sequenceLabel`.
    pub sequence_label: String,
    /// `sequenceHint`: vacío si la operación no está en conflicto.
    pub sequence_hint: String,
    /// `queueCount`: operaciones de la cola, activa incluida.
    pub queue_count: usize,
}

impl OperationActivity {
    pub fn new(
        operation_state: Option<&OperationState>,
        conflict_state: Option<&ConflictState>,
        queue: &MutationQueue,
    ) -> Self {
        let has_sequence = operation_state.is_some_and(|state| state.in_progress);
        let sequence_label = operation_state
            .map(|state| state.label.as_str())
            .filter(|label| !label.is_empty())
            .unwrap_or("Git operation")
            .to_string();
        let sequence_hint = match conflict_state.filter(|conflict| conflict.in_conflict) {
            Some(conflict) if conflict.pending_count > 0 => format!(
                "{} conflict{} to resolve",
                conflict.pending_count,
                if conflict.pending_count == 1 { "" } else { "s" }
            ),
            Some(_) => "All conflicts resolved".to_string(),
            None => String::new(),
        };

        Self {
            has_sequence,
            sequence_label,
            sequence_hint,
            queue_count: queue_operation_count(queue),
        }
    }

    /// `hasQueue`.
    pub fn has_queue(&self) -> bool {
        self.queue_count > 0
    }

    /// `totalCount`: la cola más la operación de secuencia.
    pub fn total_count(&self) -> usize {
        self.queue_count + usize::from(self.has_sequence)
    }

    /// `hasActivity`: sin actividad el indicador no se dibuja.
    pub fn has_activity(&self) -> bool {
        self.total_count() > 0
    }

    /// `summaryLabel`: etiqueta accesible, para cuando no se abre el popover.
    pub fn summary_label(&self) -> String {
        let mut parts = Vec::new();
        if self.has_sequence {
            parts.push(self.sequence_label.clone());
        }
        if self.has_queue() {
            parts.push(format!(
                "{} git operation{} active or queued",
                self.queue_count,
                if self.queue_count == 1 { "" } else { "s" }
            ));
        }
        parts.join(" · ")
    }
}

/// Indicador de actividad. Sin operaciones no dibuja nada.
#[derive(IntoElement)]
pub struct OperationIndicator {
    operation_state: Option<OperationState>,
    conflict_state: Option<ConflictState>,
    queue: MutationQueue,
    on_cancel: Option<CancelHandler>,
}

impl OperationIndicator {
    pub fn new(
        operation_state: Option<OperationState>,
        conflict_state: Option<ConflictState>,
        queue: MutationQueue,
    ) -> Self {
        Self {
            operation_state,
            conflict_state,
            queue,
            on_cancel: None,
        }
    }

    /// `@cancel`: recibe el `operationId` de la operación a cancelar.
    pub fn on_cancel(mut self, handler: impl Fn(&str, &mut Window, &mut App) + 'static) -> Self {
        self.on_cancel = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for OperationIndicator {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let activity = OperationActivity::new(
            self.operation_state.as_ref(),
            self.conflict_state.as_ref(),
            &self.queue,
        );
        if !activity.has_activity() {
            return div().into_any_element();
        }

        let palette = *cx.palette();
        let (background, color) = if activity.has_sequence {
            (
                gpui_kit::Hsla::from(palette.warning_soft),
                palette.warning_foreground,
            )
        } else {
            (
                gpui_kit::Hsla::from(palette.primary_soft),
                palette.primary_text,
            )
        };
        let total = activity.total_count();

        let trigger = div()
            .id("operation-indicator")
            .test_support()
            .aria_label(SharedString::from(activity.summary_label()))
            .flex()
            .items_center()
            .gap(rems(0.3))
            .h(px(20.))
            .px(rems(0.4))
            .radius(Radius::Sm, cx)
            .bg(background)
            .text_color(color)
            .text_size(rems(0.6875))
            .font_weight(FontWeight::SEMIBOLD)
            .cursor_default()
            .child(
                mergeon_ui::icon(Lucide::LoaderCircle)
                    .size(px(12.))
                    .spin(true),
            )
            .when(total > 1, |this| {
                this.child(div().line_height(relative(1.)).child(total.to_string()))
            });

        let queue = self.queue;
        let on_cancel = self.on_cancel;
        mergeon_ui::hover_card("operation-indicator-card")
            .align(HoverCardAlign::Center)
            .trigger(trigger)
            .content(move |_, cx| {
                let palette = *cx.palette();
                div()
                    .id("operation-popover")
                    .test_support()
                    .flex()
                    .flex_col()
                    .min_w(rems(16.))
                    .max_w(rems(24.))
                    .when(activity.has_sequence, |this| {
                        this.child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(rems(0.15))
                                .py(rems(0.55))
                                .px(rems(0.65))
                                .child(
                                    div()
                                        .text_size(rems(0.8125))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(palette.foreground)
                                        .child(activity.sequence_label.clone()),
                                )
                                .when(!activity.sequence_hint.is_empty(), |this| {
                                    this.child(
                                        div()
                                            .text_size(rems(0.75))
                                            .text_color(palette.muted_foreground)
                                            .child(activity.sequence_hint.clone()),
                                    )
                                }),
                        )
                    })
                    .when(activity.has_queue(), |this| {
                        // Dentro del popover el banner ya no necesita su propio marco.
                        let on_cancel = on_cancel.clone();
                        this.child(
                            MutationQueueBanner::new(queue.clone())
                                .framed(false)
                                .separated(activity.has_sequence)
                                .on_cancel(move |operation_id, window, cx| {
                                    if let Some(on_cancel) = &on_cancel {
                                        on_cancel(operation_id, window, cx);
                                    }
                                }),
                        )
                    })
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mergeon_gitcmd::OperationType;
    use mergeon_model::operation_types::QueueOperation;

    fn merge() -> OperationState {
        OperationState {
            in_progress: true,
            kind: Some(OperationType::Merge),
            label: "Merge in progress".to_string(),
            can_continue: true,
            can_abort: true,
            can_skip: false,
        }
    }

    fn queue(queued: usize) -> MutationQueue {
        MutationQueue {
            active: Some(QueueOperation::default()),
            queued: vec![QueueOperation::default(); queued],
        }
    }

    #[test]
    fn stays_hidden_without_activity() {
        let activity = OperationActivity::new(None, None, &MutationQueue::default());
        assert!(!activity.has_activity());
        assert_eq!(activity.summary_label(), "");
    }

    #[test]
    fn counts_the_sequence_operation_together_with_the_queue() {
        let conflict = ConflictState {
            in_conflict: true,
            pending_count: 2,
            ..Default::default()
        };
        let activity = OperationActivity::new(Some(&merge()), Some(&conflict), &queue(1));

        assert!(activity.has_sequence);
        assert_eq!(activity.total_count(), 3);
        assert_eq!(activity.sequence_hint, "2 conflicts to resolve");
        assert_eq!(
            activity.summary_label(),
            "Merge in progress · 2 git operations active or queued"
        );
    }

    #[test]
    fn words_the_hint_and_the_summary_in_singular() {
        let conflict = ConflictState {
            in_conflict: true,
            pending_count: 1,
            ..Default::default()
        };
        let activity = OperationActivity::new(Some(&merge()), Some(&conflict), &queue(0));

        assert_eq!(activity.sequence_hint, "1 conflict to resolve");
        assert_eq!(
            activity.summary_label(),
            "Merge in progress · 1 git operation active or queued"
        );
    }

    #[test]
    fn a_conflict_without_pending_files_is_resolved() {
        let conflict = ConflictState {
            in_conflict: true,
            ..Default::default()
        };
        let activity =
            OperationActivity::new(Some(&merge()), Some(&conflict), &MutationQueue::default());

        assert_eq!(activity.sequence_hint, "All conflicts resolved");
        assert_eq!(activity.total_count(), 1);
    }

    #[test]
    fn an_unlabelled_operation_reads_as_a_git_operation() {
        let state = OperationState {
            in_progress: true,
            ..Default::default()
        };
        let activity = OperationActivity::new(Some(&state), None, &MutationQueue::default());
        assert_eq!(activity.sequence_label, "Git operation");
    }
}
