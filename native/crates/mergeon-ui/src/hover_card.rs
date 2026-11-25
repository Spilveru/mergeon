//! Tarjeta flotante al pasar el cursor (`src/components/ui/tooltip/HoverCard.vue`)
//! y tooltip de texto.
//!
//! Las dos usan el comportamiento de gpui-kit (`HoverCard`, `Tooltip`) con la
//! superficie flotante de Mergeon.

use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::hover_card::HoverCard as KitHoverCard;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Anchor, AnyElement, AnyView, App, ElementId, IntoElement, ParentElement as _, RenderOnce,
    SharedString, Styled, Window, div, px, rems,
};

use crate::style::{Radius, floating_shadow};
use crate::theme::ActivePaletteExt as _;

/// Alineación horizontal de la tarjeta respecto al disparador (`align`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HoverCardAlign {
    Start,
    #[default]
    Center,
    End,
}

type ContentBuilder = Rc<dyn Fn(&mut Window, &mut App) -> AnyElement>;

/// Tarjeta flotante de Mergeon. Se crea con [`hover_card`].
#[derive(IntoElement)]
pub struct HoverCard {
    id: ElementId,
    align: HoverCardAlign,
    open_delay: Duration,
    close_delay: Duration,
    disabled: bool,
    trigger: Option<AnyElement>,
    content: Option<ContentBuilder>,
}

/// Envuelve un disparador y muestra contenido libre al pasar el cursor.
///
/// ```ignore
/// mergeon_ui::hover_card("author")
///     .trigger(div().child("Ada Lovelace"))
///     .content(|_, _| div().p_2().child("ada@example.com"))
/// ```
pub fn hover_card(id: impl Into<ElementId>) -> HoverCard {
    HoverCard {
        id: id.into(),
        align: HoverCardAlign::default(),
        // Los mismos tiempos del original: abre rápido y deja margen para
        // llevar el cursor del disparador a la tarjeta sin que se cierre.
        open_delay: Duration::from_millis(120),
        close_delay: Duration::from_millis(150),
        disabled: false,
        trigger: None,
        content: None,
    }
}

impl HoverCard {
    /// Slot `trigger`.
    pub fn trigger(mut self, trigger: impl IntoElement) -> Self {
        self.trigger = Some(trigger.into_any_element());
        self
    }

    /// Slot por defecto. Se construye cada vez que la tarjeta se dibuja.
    pub fn content<E: IntoElement>(
        mut self,
        content: impl Fn(&mut Window, &mut App) -> E + 'static,
    ) -> Self {
        self.content = Some(Rc::new(move |window, cx| {
            content(window, cx).into_any_element()
        }));
        self
    }

    pub fn align(mut self, align: HoverCardAlign) -> Self {
        self.align = align;
        self
    }

    pub fn open_delay(mut self, delay: Duration) -> Self {
        self.open_delay = delay;
        self
    }

    pub fn close_delay(mut self, delay: Duration) -> Self {
        self.close_delay = delay;
        self
    }

    /// Deshabilitada solo muestra el disparador.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

impl RenderOnce for HoverCard {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let trigger = self.trigger.unwrap_or_else(|| div().into_any_element());
        let Some(content) = self.content.filter(|_| !self.disabled) else {
            return div()
                .flex()
                .items_center()
                .child(trigger)
                .into_any_element();
        };
        let palette = cx.palette();

        KitHoverCard::new(self.id)
            .anchor(match self.align {
                HoverCardAlign::Start => Anchor::TopLeft,
                HoverCardAlign::Center => Anchor::TopCenter,
                HoverCardAlign::End => Anchor::TopRight,
            })
            .open_delay(self.open_delay)
            .close_delay(self.close_delay)
            .trigger(div().flex().items_center().child(trigger))
            .content(move |_, window, cx| content(window, cx))
            // `.popover.hover-card`: superficie flotante con `padding: 0.25rem`.
            .bg(palette.popover)
            .text_color(palette.popover_foreground)
            .border_1()
            .border_color(palette.floating_border)
            .rounded(Radius::Xl.px(cx))
            .shadow(floating_shadow(cx))
            .p_1()
            .max_w(rems(24.))
            .into_any_element()
    }
}

/// Constructor de tooltip de texto para `.tooltip(..)` de GPUI, con la
/// superficie flotante de Mergeon. Es el equivalente al `title` del original.
///
/// ```ignore
/// div().id("refresh").tooltip(mergeon_ui::tooltip("Refresh"))
/// ```
pub fn tooltip(
    text: impl Into<SharedString>,
) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let text: SharedString = text.into();
    move |window, cx| tooltip_view(text.clone(), None, window, cx)
}

/// Como [`tooltip`], con el atajo de teclado a la derecha.
pub fn tooltip_with_shortcut(
    text: impl Into<SharedString>,
    shortcut: impl Into<SharedString>,
) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let text: SharedString = text.into();
    let shortcut: SharedString = shortcut.into();
    move |window, cx| tooltip_view(text.clone(), Some(shortcut.clone()), window, cx)
}

fn tooltip_view(
    text: SharedString,
    shortcut: Option<SharedString>,
    window: &mut Window,
    cx: &mut App,
) -> AnyView {
    let palette = *cx.palette();
    // Señal: tooltip con radio 2, borde base y `shadow-2`; el atajo en mono.
    let radius = Radius::Xs.px(cx);
    let shadow = floating_shadow(cx);
    let mono = gpui_kit::component::ActiveTheme::theme(cx)
        .mono_font_family
        .clone();
    Tooltip::element(move |_, _| {
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(text.clone())
            .when_some(shortcut.clone(), |this, shortcut| {
                this.child(
                    div()
                        .font_family(mono.clone())
                        .text_size(px(11.))
                        .text_color(palette.text_3)
                        .child(shortcut),
                )
            })
    })
    .bg(palette.popover)
    .text_color(palette.popover_foreground)
    .border_color(palette.floating_border)
    .rounded(radius)
    .shadow(shadow)
    .py(px(5.))
    .px_2()
    .text_size(px(12.))
    .line_height(px(16.))
    .build(window, cx)
}
