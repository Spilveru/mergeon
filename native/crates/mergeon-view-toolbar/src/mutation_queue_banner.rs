//! Cola de mutaciones de un repositorio
//! (`src/components/repository/MutationQueueBanner.vue`): la operación en curso
//! y las que esperan, cada una con su botón para cancelarla o quitarla.

use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, RenderOnce,
    SharedString, StatefulInteractiveElement as _, Styled as _, TestSupportExt as _, Window, div,
    px, rems,
};
use mergeon_model::operation_labels::operation_label;
use mergeon_model::operation_types::{OperationPhase, QueueOperation};
use mergeon_model::repositories::MutationQueue;
use mergeon_ui::{ActivePaletteExt as _, IconTone, Lucide, MergeonStyled as _, Radius};

use crate::parts::{button_icon, tint};

type CancelHandler = Rc<dyn Fn(&str, &mut Window, &mut App)>;

/// `operationCount`: la activa más las encoladas.
pub fn queue_operation_count(queue: &MutationQueue) -> usize {
    usize::from(queue.active.is_some()) + queue.queued.len()
}

/// `phaseLabel`: lo que se lee junto a la operación en curso.
pub fn queue_phase_label(phase: OperationPhase) -> &'static str {
    if phase == OperationPhase::Cancelling {
        "Stopping"
    } else {
        "Running"
    }
}

/// Cola de mutaciones. Sin operaciones no dibuja nada.
///
/// ```ignore
/// MutationQueueBanner::new(queue).on_cancel(|operation_id, _, cx| {
///     AppModel::global(cx).update(cx, |model, cx| {
///         model.handle_cancel_operation(operation_id, cx).detach();
///     });
/// })
/// ```
#[derive(IntoElement)]
pub struct MutationQueueBanner {
    queue: MutationQueue,
    framed: bool,
    separated: bool,
    on_cancel: Option<CancelHandler>,
}

impl MutationQueueBanner {
    pub fn new(queue: MutationQueue) -> Self {
        Self {
            queue,
            framed: true,
            separated: false,
            on_cancel: None,
        }
    }

    /// Con su propio marco (fondo `--card` y borde inferior), como va suelto
    /// en una página. Dentro del popover del indicador va sin él.
    pub fn framed(mut self, framed: bool) -> Self {
        self.framed = framed;
        self
    }

    /// Línea de separación arriba, cuando en el popover le precede la
    /// operación de secuencia.
    pub fn separated(mut self, separated: bool) -> Self {
        self.separated = separated;
        self
    }

    /// `@cancel`: recibe el `operationId` de la operación a cancelar o quitar.
    pub fn on_cancel(mut self, handler: impl Fn(&str, &mut Window, &mut App) + 'static) -> Self {
        self.on_cancel = Some(Rc::new(handler));
        self
    }
}

/// Botón de una fila: los de `Button.vue` en `sm`, encogidos a 24px por
/// `.queue-row :deep(button)`.
fn row_button(
    id: SharedString,
    label: &'static str,
    danger: bool,
    operation_id: Option<String>,
    on_cancel: Option<CancelHandler>,
    cx: &App,
) -> mergeon_ui::Button {
    mergeon_ui::button(id)
        .sm()
        .map(|this| {
            if danger {
                this.destructive_outline()
            } else {
                this.ghost()
            }
        })
        .h(px(24.))
        .px(rems(0.45))
        .gap(rems(0.25))
        .text_size(rems(0.6875))
        .child(button_icon(Lucide::X, cx))
        .child(div().child(label))
        .on_click(move |_, window, cx| {
            if let (Some(on_cancel), Some(operation_id)) = (&on_cancel, &operation_id) {
                on_cancel(operation_id, window, cx);
            }
        })
}

impl RenderOnce for MutationQueueBanner {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let count = queue_operation_count(&self.queue);
        if count == 0 {
            return div().into_any_element();
        }
        let palette = *cx.palette();
        let on_cancel = self.on_cancel;

        let operation = |status: SharedString, operation: &QueueOperation| {
            div()
                .flex()
                .items_center()
                .gap(rems(0.4))
                .min_w_0()
                .child(
                    div()
                        .flex_shrink_0()
                        .text_size(rems(0.6875))
                        .text_color(palette.muted_foreground)
                        .child(status),
                )
                .child(
                    div()
                        .truncate()
                        .text_size(rems(0.75))
                        .font_weight(FontWeight::MEDIUM)
                        .child(operation_label(&operation.operation)),
                )
        };
        let row = || {
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(rems(0.65))
        };
        let key = |prefix: &str, operation: &QueueOperation| {
            SharedString::from(format!(
                "{prefix}-{}",
                operation.operation_id.as_deref().unwrap_or_default()
            ))
        };
        // Fila de una operación, con lo que dice como etiqueta accesible.
        let operation_row = |status: SharedString, operation: &QueueOperation| {
            row()
                .id(key("queue-row", operation))
                .test_support()
                .aria_label(SharedString::from(format!(
                    "{status} {}",
                    operation_label(&operation.operation)
                )))
        };

        div()
            .id("mutation-queue")
            .test_support()
            .flex()
            .flex_col()
            .gap(rems(0.28))
            .py(rems(0.45))
            .px(rems(0.65))
            .when(self.framed, |this| {
                this.border_b_1()
                    .border_color(palette.border)
                    .bg(palette.card)
            })
            .when(self.separated, |this| {
                this.border_t_1().border_color(palette.border)
            })
            .child(
                row()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(rems(0.4))
                            .min_w_0()
                            .text_size(rems(0.75))
                            .child(
                                mergeon_ui::icon(Lucide::LoaderCircle)
                                    .size(px(14.))
                                    .tone(IconTone::Active)
                                    .spin(true),
                            )
                            .child(div().font_weight(FontWeight::BOLD).child("Git operations")),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(rems(0.6875))
                            .text_color(palette.muted_foreground)
                            .child(format!("{count} active or queued")),
                    ),
            )
            .when_some(self.queue.active.as_ref(), |this, active| {
                let status: SharedString = queue_phase_label(active.phase).into();
                this.child(
                    operation_row(status.clone(), active)
                        .py(rems(0.2))
                        .px(rems(0.35))
                        .radius(Radius::Md, cx)
                        .bg(tint(palette.primary, 0.07))
                        .child(operation(status, active))
                        .when(active.cancellable, |this| {
                            this.child(row_button(
                                key("queue-cancel", active),
                                "Cancel",
                                true,
                                active.operation_id.clone(),
                                on_cancel.clone(),
                                cx,
                            ))
                        }),
                )
            })
            .children(self.queue.queued.iter().map(|queued| {
                let status: SharedString = format!("Queued #{}", queued.position).into();
                operation_row(status.clone(), queued)
                    .child(operation(status, queued))
                    .child(row_button(
                        key("queue-remove", queued),
                        "Remove",
                        false,
                        queued.operation_id.clone(),
                        on_cancel.clone(),
                        cx,
                    ))
            }))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_the_active_operation_and_the_queued_ones() {
        let mut queue = MutationQueue::default();
        assert_eq!(queue_operation_count(&queue), 0);

        queue.active = Some(QueueOperation::default());
        queue.queued = vec![QueueOperation::default(), QueueOperation::default()];
        assert_eq!(queue_operation_count(&queue), 3);
    }

    #[test]
    fn a_cancelling_operation_reads_as_stopping() {
        assert_eq!(queue_phase_label(OperationPhase::Running), "Running");
        assert_eq!(queue_phase_label(OperationPhase::Cancelling), "Stopping");
    }
}
