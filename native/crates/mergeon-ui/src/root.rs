//! Capa de Mergeon sobre la raíz de cada ventana.
//!
//! `gpui_kit::open_window` monta `gpui_kit::base::Root`; este plugin se
//! registra en [`crate::init`] y aporta dos cosas a todas las ventanas:
//!
//! - el texto base de la app (Señal: Geist 13px con línea de 18px), tanto
//!   para las vistas como para diálogos, menús y toasts;
//! - el anfitrión de toasts ([`crate::toast`]), por encima de los diálogos.

use gpui_kit::base::{Root, RootPlugin};
use gpui_kit::{
    App, AppContext as _, Context, Div, Entity, IntoElement, ParentElement as _, Refineable as _,
    Render, Stateful, StyleRefinement, Styled as _, Window, deferred, div, relative, rems,
};

use crate::dialog::DialogStack;
use crate::toast::ToastHost;

/// Por encima de los diálogos de `gpui_kit::base` (10 + capa) y por debajo de
/// sus popups (100): un menú abierto sigue tapando a un toast.
const TOAST_PRIORITY: usize = 90;

pub(crate) struct MergeonLayer {
    pub(crate) dialogs: DialogStack,
    toasts: Entity<ToastHost>,
}

impl MergeonLayer {
    fn new(_: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            dialogs: DialogStack::default(),
            toasts: cx.new(ToastHost::new),
        }
    }

    /// La capa de la ventana, si la ventana se abrió con `gpui_kit::open_window`
    /// después de [`crate::init`].
    pub(crate) fn get(window: &Window, cx: &App) -> Option<Entity<Self>> {
        window.root::<Root>()??.read(cx).plugin::<Self>()
    }

    pub(crate) fn toasts(window: &Window, cx: &App) -> Option<Entity<ToastHost>> {
        Self::get(window, cx).map(|layer| layer.read(cx).toasts.clone())
    }
}

impl RootPlugin for MergeonLayer {
    fn style(&self, surface: &mut Stateful<Div>, _: &mut Window, _: &mut App) {
        surface.style().refine(
            &StyleRefinement::default()
                .text_size(rems(13. / 16.))
                .line_height(relative(18. / 13.)),
        );
    }
}

impl Render for MergeonLayer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .absolute()
            .inset_0()
            .children(self.dialogs.render(window, cx))
            // Los toasts van por encima de los diálogos (que se pintan
            // diferidos), como el `z-index: 9999` del original.
            .child(deferred(self.toasts.clone()).with_priority(TOAST_PRIORITY))
    }
}

pub(crate) fn init(cx: &mut App) {
    Root::register_plugin::<MergeonLayer>(cx, MergeonLayer::new);
}
