//! Casilla (`src/components/ui/checkbox/Checkbox.vue`, `checkboxControlVariants`).
//!
//! El comportamiento (alternar, foco, Espacio, rol accesible) es el del
//! `Checkbox` de `gpui_kit::base`. El de `gpui_kit::component` queda cerca pero
//! no iguala la receta: línea de 17.5px en vez de 20, otro trazo para el check y
//! otro atenuado al deshabilitar.

use std::rc::Rc;

use gpui_kit::base::{Checkbox as BaseCheckbox, CheckboxIndicator, CheckboxState, StateStyle};
use gpui_kit::component::{Disableable, Icon, Sizable as _, Size, StyledExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, ElementId, InteractiveElement as _, IntoElement, MouseButton, ParentElement,
    RenderOnce, SharedString, StyleRefinement, Styled, Window, div, px,
};

use crate::menu::CHECK_SVG;
use crate::style::{DISABLED_OPACITY, MergeonStyled as _, Radius, focus_ring_overlay};
use crate::theme::ActivePaletteExt as _;

type ChangeHandler = Rc<dyn Fn(&bool, &mut Window, &mut App)>;

/// Casilla de Mergeon. Se crea con [`checkbox`].
#[derive(IntoElement)]
pub struct Checkbox {
    id: ElementId,
    checked: bool,
    mixed: bool,
    on_ink: bool,
    disabled: bool,
    label: Option<SharedString>,
    children: Vec<AnyElement>,
    on_change: Option<ChangeHandler>,
    style: StyleRefinement,
}

/// Casilla controlada: se le pasa `checked` y avisa del valor pedido con
/// `on_change`.
///
/// ```ignore
/// mergeon_ui::checkbox("fetch-all")
///     .label("Fetch all remotes")
///     .checked(self.fetch_all)
///     .on_change(cx.listener(|this, checked: &bool, _, cx| {
///         this.fetch_all = *checked;
///         cx.notify();
///     }))
/// ```
pub fn checkbox(id: impl Into<ElementId>) -> Checkbox {
    Checkbox {
        id: id.into(),
        checked: false,
        mixed: false,
        on_ink: false,
        disabled: false,
        label: None,
        children: Vec::new(),
        on_change: None,
        style: StyleRefinement::default(),
    }
}

impl Checkbox {
    /// Texto a la derecha de la casilla (el slot por defecto del original).
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }

    /// Estado mixto (`indeterminate`): ni todo ni nada. Se pinta como marcada
    /// con una raya en vez del check; el clic pide `!checked`, como siempre.
    pub fn mixed(mut self, mixed: bool) -> Self {
        self.mixed = mixed;
        self
    }

    /// Sobre una fila seleccionada (fondo tinta): borde claro y sin fondo
    /// propio mientras no está marcada.
    pub fn on_ink(mut self, on_ink: bool) -> Self {
        self.on_ink = on_ink;
        self
    }

    /// Recibe el valor pedido (`update:modelValue`); la vista decide si lo aplica.
    pub fn on_change(mut self, handler: impl Fn(&bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Rc::new(handler));
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Alias de [`Self::on_change`], con el nombre que usa gpui-kit.
    pub fn on_click(self, handler: impl Fn(&bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_change(handler)
    }
}

impl Disableable for Checkbox {
    fn disabled(self, disabled: bool) -> Self {
        Checkbox::disabled(self, disabled)
    }
}

/// Contenido libre en lugar de (o además de) la etiqueta.
impl ParentElement for Checkbox {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl Styled for Checkbox {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for Checkbox {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let palette = *cx.palette();
        let radius = Radius::Xs.px(cx);
        let checked = self.checked;
        let state = if checked {
            CheckboxState::Checked
        } else if self.mixed {
            CheckboxState::Indeterminate
        } else {
            CheckboxState::Unchecked
        };
        let on_ink = self.on_ink;
        let focus_handle = window
            .use_keyed_state(self.id.clone(), cx, |_, cx| cx.focus_handle())
            .read(cx)
            .clone();
        // El clic no mueve el foco: con foco, llegó del teclado.
        let focused = !self.disabled && focus_handle.is_focused(window);
        let on_change = self.on_change;

        BaseCheckbox::new(self.id)
            .state(state)
            .disabled(self.disabled)
            .track_focus(&focus_handle)
            .when_some(self.label.clone(), |this, label| {
                this.accessibility_label(label)
            })
            .when_some(on_change, |this, on_change| {
                this.on_change(move |_, _, window, cx| on_change(&!checked, window, cx))
            })
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap_2()
            .tw_text_sm()
            .text_color(palette.foreground)
            .when(!self.disabled, |this| {
                this.cursor_pointer()
                    .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
            })
            .when(self.disabled, |this| this.opacity(DISABLED_OPACITY))
            .refine_style(&self.style)
            .child(
                CheckboxIndicator::new()
                    .state(state)
                    .relative()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .justify_center()
                    // Señal: 14px, radio 2, borde `border-control`; marcado en
                    // tinta (no en naranja: en Cambios hay muchos marcados y el
                    // naranja dejaría de ser señal).
                    .size(px(14.))
                    .rounded(radius)
                    .border_1()
                    .map(|this| {
                        if on_ink {
                            this.border_color(palette.sel_fg_2)
                                .bg(gpui_kit::transparent_black())
                        } else {
                            this.border_color(palette.border_control)
                                .bg(palette.bg_base)
                        }
                    })
                    .styles(|styles| {
                        let filled = |style: StateStyle| {
                            style
                                .border_color(if on_ink {
                                    palette.sel_fg_2
                                } else {
                                    palette.border_ink
                                })
                                .bg(palette.border_ink)
                                .shadow_none()
                        };
                        styles.checked(filled).indeterminate(filled)
                    })
                    .when(self.mixed && !checked, |this| {
                        this.child(div().w(px(8.)).h(px(2.)).bg(palette.bg_base))
                    })
                    .when(checked, |this| {
                        this.child(
                            Icon::default()
                                .data(CHECK_SVG)
                                .with_size(Size::Size(px(11.)))
                                .text_color(palette.bg_base),
                        )
                    })
                    .when(focused, |this| this.child(focus_ring_overlay(radius, cx))),
            )
            .when_some(self.label, |this, label| this.child(div().child(label)))
            .children(self.children)
    }
}
