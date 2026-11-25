//! Piezas pequeñas que comparten las vistas de este crate.

use gpui_kit::base::Button as BaseButton;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, ClickEvent, ElementId, InteractiveElement as _, IntoElement, MouseButton,
    ParentElement, RenderOnce, SharedString, StatefulInteractiveElement as _, Styled as _, Window,
};
use mergeon_ui::Radius;
use mergeon_ui::style::focus_ring_overlay;

type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>;
type ButtonStyle = Box<dyn FnOnce(BaseButton, &App) -> BaseButton>;

/// Un `<button>` con estilo propio del original (los de la lista de cambios y
/// del resolvedor no usan la receta de `Button.vue`): el comportamiento es el
/// de un botón —foco, teclado, deshabilitado— y el aspecto lo pone quien lo
/// usa con [`PlainButton::styled`].
#[derive(IntoElement)]
pub(crate) struct PlainButton {
    id: ElementId,
    label: SharedString,
    tooltip: Option<SharedString>,
    disabled: bool,
    disabled_opacity: f32,
    on_click: Option<ClickHandler>,
    style: Option<ButtonStyle>,
    children: Vec<AnyElement>,
}

impl PlainButton {
    pub(crate) fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            label: SharedString::default(),
            tooltip: None,
            disabled: false,
            disabled_opacity: 0.5,
            on_click: None,
            style: None,
            children: Vec::new(),
        }
    }

    /// El nombre del botón. Es también su texto, salvo que lleve hijos (un
    /// ícono, por ejemplo).
    pub(crate) fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = label.into();
        self
    }

    /// El `title` del original.
    pub(crate) fn tooltip(mut self, tooltip: impl Into<SharedString>) -> Self {
        self.tooltip = Some(tooltip.into());
        self
    }

    pub(crate) fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Opacidad con la que se pinta deshabilitado.
    pub(crate) fn disabled_opacity(mut self, opacity: f32) -> Self {
        self.disabled_opacity = opacity;
        self
    }

    pub(crate) fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }

    /// El aspecto: recibe el botón desnudo y lo devuelve vestido.
    pub(crate) fn styled(
        mut self,
        style: impl FnOnce(BaseButton, &App) -> BaseButton + 'static,
    ) -> Self {
        self.style = Some(Box::new(style));
        self
    }
}

impl ParentElement for PlainButton {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for PlainButton {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let disabled = self.disabled;
        let focus = window
            .use_keyed_state(self.id.clone(), cx, |_, cx| cx.focus_handle())
            .read(cx)
            .clone();
        let focused = !disabled && focus.is_focused(window);
        let radius = Radius::Md.px(cx);

        let button = BaseButton::new(self.id)
            .disabled(disabled)
            .track_focus(&focus)
            .accessibility_label(self.label.clone())
            .relative()
            .flex()
            .flex_shrink_0()
            .items_center()
            .justify_center()
            .whitespace_nowrap();
        let button = match self.style {
            Some(style) => style(button, cx),
            None => button,
        };

        button
            .map(|this| {
                if disabled {
                    this.opacity(self.disabled_opacity)
                } else {
                    this.cursor_pointer()
                        .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                }
            })
            .when_some(self.on_click, |this, on_click| {
                this.on_click(move |event, window, cx| on_click(event, window, cx))
            })
            .when_some(self.tooltip, |this, tooltip| {
                this.tooltip(mergeon_ui::tooltip(tooltip))
            })
            .map(|this| {
                if self.children.is_empty() {
                    this.child(self.label)
                } else {
                    this.children(self.children)
                }
            })
            .when(focused, |this| this.child(focus_ring_overlay(radius, cx)))
    }
}
