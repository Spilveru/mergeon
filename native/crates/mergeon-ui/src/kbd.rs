//! Tecla o atajo (`src/components/ui/kbd/Kbd.vue`, `kbdClass`).

use gpui_kit::component::{ActiveTheme as _, StyledExt as _};
use gpui_kit::{
    App, BoxShadow, FontWeight, IntoElement, ParentElement as _, RenderOnce, SharedString,
    StyleRefinement, Styled, Window, div, px, relative,
};

use crate::theme::ActivePaletteExt as _;

/// Atajo de teclado. Se crea con [`kbd`].
#[derive(IntoElement)]
pub struct Kbd {
    label: SharedString,
    style: StyleRefinement,
}

/// Muestra un atajo tal como se escribe en el original (`"Ctrl+K"`, `"⌘P"`).
///
/// ```ignore
/// mergeon_ui::kbd("Ctrl+Shift+P")
/// ```
pub fn kbd(label: impl Into<SharedString>) -> Kbd {
    Kbd {
        label: label.into(),
        style: StyleRefinement::default(),
    }
}

impl Styled for Kbd {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for Kbd {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let palette = cx.palette();
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .justify_center()
            // Señal: mono 10,5, alto 18, borde base con bisel inferior de 1px.
            .h(px(18.))
            .min_w(px(18.))
            .gap_1()
            .px(px(4.))
            .rounded(px(3.))
            .border_1()
            .border_color(palette.border_base)
            .shadow(vec![
                BoxShadow::new(px(0.), px(-1.), palette.border_base.into()).inset(),
            ])
            .bg(palette.bg_base)
            .text_color(palette.text_2)
            .font_family(cx.theme().mono_font_family.clone())
            .text_size(px(10.5))
            .line_height(relative(1.))
            .font_weight(FontWeight::MEDIUM)
            .whitespace_nowrap()
            .refine_style(&self.style)
            .child(self.label)
    }
}
