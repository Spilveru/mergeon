//! Divisor arrastrable de 8px (`.commit-resizer`, `.changes-resizer`,
//! `.files-resizer`) y la escucha del ratón de toda la ventana mientras dura
//! un arrastre (los `window.addEventListener('mousemove' / 'mouseup')` del
//! original).

use std::rc::Rc;

use gpui_kit::prelude::*;
use gpui_kit::{
    App, Context, Div, ElementId, MouseMoveEvent, MouseUpEvent, Pixels, Point, Stateful,
    WeakEntity, Window, canvas, div, px,
};
use mergeon_ui::ActivePaletteExt as _;
use mergeon_ui::theme::with_alpha;

/// Ancho de los divisores (`RESIZER_WIDTH`).
pub(crate) const RESIZER_WIDTH: f32 = 8.;

/// El divisor. Señal: un filete de 1px sobre el fondo de los paneles, y del
/// color primario al pasar o al arrastrar.
pub(crate) fn resizer(id: impl Into<ElementId>, dragging: bool, cx: &App) -> Stateful<Div> {
    let palette = cx.palette();
    let idle = palette.card;
    let lit = with_alpha(palette.primary, 0.42);

    div()
        .id(id)
        .flex()
        .justify_center()
        .flex_shrink_0()
        .w(px(RESIZER_WIDTH))
        .h_full()
        .cursor_col_resize()
        .map(|this| {
            if dragging {
                this.bg(lit)
            } else {
                this.bg(idle)
            }
        })
        .hover(move |style| style.bg(lit))
        .child(div().w(px(1.)).h_full().bg(palette.border))
}

/// Mientras `active`, entrega a la vista cada movimiento del ratón sobre la
/// ventana y el momento en que se suelta el botón, caiga donde caiga.
pub(crate) fn drag_tracker<V: 'static>(
    active: bool,
    view: WeakEntity<V>,
    on_move: impl Fn(&mut V, Point<Pixels>, &mut Window, &mut Context<V>) + 'static,
    on_release: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |_, _, window, _| {
            if !active {
                return;
            }
            let on_move = Rc::new(on_move);
            let on_release = Rc::new(on_release);
            window.on_mouse_event({
                let view = view.clone();
                move |event: &MouseMoveEvent, phase, window, cx| {
                    if phase.capture() {
                        let position = event.position;
                        let _ = view.update(cx, |view, cx| on_move(view, position, window, cx));
                    }
                }
            });
            window.on_mouse_event(move |_: &MouseUpEvent, phase, window, cx| {
                if phase.capture() {
                    let _ = view.update(cx, |view, cx| on_release(view, window, cx));
                }
            });
        },
    )
    .absolute()
    .size_0()
}
