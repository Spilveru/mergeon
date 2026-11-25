//! Franja de estado bajo la barra
//! (`src/components/repository/OperationStatusSlot.vue`).
//!
//! La cola de mutaciones vive en el indicador de la barra; aquí solo queda la
//! operación de secuencia (rebase/merge/cherry-pick/revert), que es accionable
//! y debe verse. Sin operación no se dibuja nada: la franja desaparece en
//! lugar de reservar altura.

use gpui_kit::{
    Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _, Render, Styled as _,
    Subscription, TestSupportExt as _, Window, div,
};
use mergeon_app::AppModel;
use mergeon_gitcmd::{ConflictState, OperationState};
use mergeon_ui::ActivePaletteExt as _;

use crate::operation_banner::{OperationBanner, SequenceAction};

/// Franja con la operación de secuencia del repositorio activo. Sus botones
/// llaman a `AppModel::handle_sequence_action`.
pub struct OperationStatusSlot {
    model: Entity<AppModel>,
    /// Lo último que se pintó (o se iba a pintar) del modelo.
    shown: Option<ShownState>,
    renders: usize,
    _observer: Subscription,
}

/// Lo que la franja enseña del modelo. Solo se repinta cuando cambia: el
/// modelo avisa por cualquier cosa (avisos, progreso, otros repositorios...)
/// y, sin operación de secuencia en curso, la franja no dibuja nada. Se pinta
/// desde aquí, sin leer el modelo al pintar.
#[derive(Debug, Clone, PartialEq)]
struct ShownState {
    repo_id: String,
    operation_state: OperationState,
    conflict_state: ConflictState,
    busy_action: Option<SequenceAction>,
}

impl ShownState {
    /// `None` cuando no hay nada que enseñar (sin repositorio activo o sin
    /// operación de secuencia en curso).
    fn read(model: &AppModel) -> Option<Self> {
        let repository = model.active_repository()?;
        let operation_state = repository
            .operation_state
            .clone()
            .filter(|state| state.in_progress)?;
        Some(Self {
            repo_id: repository.id.clone(),
            operation_state,
            conflict_state: repository.conflict_state.clone(),
            busy_action: model.busy_sequence_action().and_then(SequenceAction::parse),
        })
    }
}

impl OperationStatusSlot {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let model = AppModel::global(cx);
        let observer = cx.observe(&model, |this: &mut Self, _, cx| this.sync_shown(cx));
        let shown = ShownState::read(model.read(cx));
        // El observador empieza a escuchar al final de este ciclo: un aviso
        // del modelo que ya estuviera en cola no le llegaría.
        let this = cx.weak_entity();
        cx.defer(move |cx| {
            this.update(cx, |this, cx| this.sync_shown(cx)).ok();
        });
        Self {
            model,
            shown,
            renders: 0,
            _observer: observer,
        }
    }

    /// Veces que se pintó (tests).
    #[doc(hidden)]
    pub fn render_count(&self) -> usize {
        self.renders
    }

    /// Recoge lo que la franja enseña del modelo y repinta si cambió.
    fn sync_shown(&mut self, cx: &mut Context<Self>) {
        let state = ShownState::read(self.model.read(cx));
        if state != self.shown {
            self.shown = state;
            cx.notify();
        }
    }

    fn run(&mut self, action: SequenceAction, cx: &mut Context<Self>) {
        self.model
            .update(cx, |model, cx| {
                model.handle_sequence_action(action.as_str(), cx)
            })
            .detach();
    }
}

impl Render for OperationStatusSlot {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        // Se pinta lo leído al avisar el modelo, sin leerlo aquí: la ventana
        // repinta con cada aviso de una entidad leída al pintar.
        let Some(ShownState {
            operation_state,
            conflict_state,
            busy_action,
            ..
        }) = self.shown.clone()
        else {
            return div().into_any_element();
        };
        let palette = *cx.palette();
        let this = cx.entity();

        div()
            .id("operation-status-slot")
            .test_support()
            .flex_shrink_0()
            .bg(palette.card)
            .border_b_1()
            .border_color(palette.border)
            .child(
                OperationBanner::new(Some(operation_state), Some(conflict_state))
                    .busy_action(busy_action)
                    .compact(true)
                    .on_sequence_action(move |action, _, cx| {
                        this.update(cx, |this, cx| this.run(action, cx));
                    }),
            )
            .into_any_element()
    }
}
