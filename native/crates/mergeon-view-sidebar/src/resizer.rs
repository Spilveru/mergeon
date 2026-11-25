//! El divisor entre el sidebar y la vista de commits (`.sidebar-resizer` de
//! `RepositoryWorkspace.vue` + `useSidebarWidth`): arrastrar cambia el ancho,
//! las flechas lo ajustan de 16 en 16 y el doble clic vuelve al inicial. El
//! ancho vive en `AppModel` y se guarda al soltar.

use gpui_kit::prelude::*;
use gpui_kit::{
    App, Entity, KeyDownEvent, MouseButton, MouseMoveEvent, MouseUpEvent, Pixels,
    TestSupportExt as _, Window, canvas, div, px,
};
use mergeon_ui::ActivePaletteExt as _;
use mergeon_ui::theme::with_alpha;

use crate::sidebar::Sidebar;

const GROUP: &str = "sidebar-resizer";
/// Paso de las flechas, en píxeles (`nudgeSidebar(±16)`).
const NUDGE: f64 = 16.0;

/// Divisor del sidebar. Va justo después de él, en la misma fila:
///
/// ```ignore
/// div().flex().child(sidebar.clone()).child(SidebarResizer::new(&sidebar)).child(commits)
/// ```
#[derive(IntoElement)]
pub struct SidebarResizer {
    sidebar: Entity<Sidebar>,
}

impl SidebarResizer {
    pub fn new(sidebar: &Entity<Sidebar>) -> Self {
        Self {
            sidebar: sidebar.clone(),
        }
    }
}

/// El sidebar pinta su ancho y el estado del arrastre desde lo que recogió
/// del modelo, al avisar este. Tras un cambio del divisor se le pone al día
/// en el acto: el frame siguiente puede pintarse antes de que llegue el aviso
/// (y el arrastre registra sus manejadores al pintarse).
fn sync_sidebar(sidebar: &Entity<Sidebar>, cx: &mut App) {
    sidebar.update(cx, |sidebar, cx| {
        if sidebar.sync_render_key(cx) {
            cx.notify();
        }
    });
}

fn as_f64(pixels: Pixels) -> f64 {
    f64::from(f32::from(pixels))
}

/// Borde izquierdo del sidebar y ancho del contenedor que comparte con la
/// vista de commits.
fn geometry(sidebar: &Entity<Sidebar>, window: &Window, cx: &App) -> (f64, f64) {
    let sidebar = sidebar.read(cx);
    let origin_x = sidebar.bounds.get().origin.x;
    let container_width = sidebar
        .container_width
        .unwrap_or(window.viewport_size().width - origin_x);
    (as_f64(origin_x), as_f64(container_width))
}

impl RenderOnce for SidebarResizer {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        // Lo que pinta sale de lo que el sidebar ya recogió del modelo: pintar
        // el divisor no lee `AppModel`.
        let (model, resizing) = {
            let sidebar = self.sidebar.read(cx);
            (sidebar.model.clone(), sidebar.render_key().resizing)
        };
        let highlight = with_alpha(cx.palette().primary, 0.42);
        let focus_handle = window
            .use_keyed_state("sidebar-resizer-focus", cx, |_, cx| {
                cx.focus_handle().tab_stop(true)
            })
            .read(cx)
            .clone();
        let focused = focus_handle.is_focused(window);

        div()
            .id("sidebar-resizer")
            .group(GROUP)
            .track_focus(&focus_handle)
            .relative()
            .flex()
            .justify_end()
            .flex_shrink_0()
            .h_full()
            // La franja visible mide 4px y pisa 1px del borde del sidebar
            // (`margin-left: -1px`); los 3px de más a su izquierda son el
            // margen para acertarle (`::before { inset: 0 -3px }`).
            .w(px(7.))
            .ml(px(-4.))
            .cursor_col_resize()
            .tooltip(mergeon_ui::tooltip(
                "Drag to resize · double-click to reset",
            ))
            .on_mouse_down(MouseButton::Left, {
                let model = model.clone();
                let sidebar = self.sidebar.clone();
                move |event, window, cx| {
                    // `@mousedown.prevent`: no selecciona texto al arrastrar.
                    window.prevent_default();
                    if event.click_count >= 2 {
                        model
                            .update(cx, |model, cx| model.reset_sidebar_width(cx))
                            .detach();
                    } else {
                        model.update(cx, |model, cx| model.start_sidebar_resize(cx));
                    }
                    sync_sidebar(&sidebar, cx);
                }
            })
            .on_key_down({
                let model = model.clone();
                let sidebar = self.sidebar.clone();
                move |event: &KeyDownEvent, window, cx| {
                    let delta = match event.keystroke.key.as_str() {
                        "left" => -NUDGE,
                        "right" => NUDGE,
                        _ => return,
                    };
                    cx.stop_propagation();
                    let (_, container_width) = geometry(&sidebar, window, cx);
                    model
                        .update(cx, |model, cx| {
                            model.nudge_sidebar_width(delta, container_width, cx)
                        })
                        .detach();
                    sync_sidebar(&sidebar, cx);
                }
            })
            .child(
                div()
                    .w(px(4.))
                    .h_full()
                    .when(resizing || focused, |strip| strip.bg(highlight))
                    .group_hover(GROUP, |style| style.bg(highlight)),
            )
            .child(
                // Mientras dura el arrastre el puntero se sigue en toda la
                // ventana, no solo sobre el divisor.
                canvas(|_, _, _| (), {
                    let sidebar = self.sidebar.clone();
                    move |_, _, window, _| {
                        if !resizing {
                            return;
                        }
                        window.on_mouse_event({
                            let model = model.clone();
                            let sidebar = sidebar.clone();
                            move |event: &MouseMoveEvent, phase, window, cx| {
                                if !phase.bubble() {
                                    return;
                                }
                                let (origin_x, container_width) = geometry(&sidebar, window, cx);
                                // El ancho cambia sin avisar al modelo (sus
                                // observadores no tienen nada que hacer a
                                // cada píxel): basta con repintar la ventana.
                                let changed = model.update(cx, |model, _| {
                                    model.drag_sidebar_resize(
                                        as_f64(event.position.x),
                                        origin_x,
                                        container_width,
                                    )
                                });
                                if changed {
                                    sync_sidebar(&sidebar, cx);
                                    window.refresh();
                                }
                            }
                        });
                        window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                            if phase.bubble() {
                                model
                                    .update(cx, |model, cx| model.stop_sidebar_resize(cx))
                                    .detach();
                                sync_sidebar(&sidebar, cx);
                            }
                        });
                    }
                })
                .absolute()
                .size_full(),
            )
            .test_support()
    }
}
