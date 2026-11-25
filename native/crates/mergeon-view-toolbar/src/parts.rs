//! Piezas pequeñas que comparten la barra, los banners y los diálogos.

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{App, Div, Hsla, ParentElement as _, SharedString, Styled as _, div, px};
use mergeon_ui::theme::with_alpha;
use mergeon_ui::{ActivePaletteExt as _, AppIcon, Lucide};

/// Ícono dentro de un `Button` cuyo contenido es libre (`<Button><AppIcon/> Texto</Button>`).
///
/// La receta del botón manda sobre lo que pida el ícono: lo deja en 16px
/// (`[&_svg]:size-4`), al 80 % (`[&_svg]:opacity-80`) y lo hace invadir 2px a
/// cada lado de su hueco (`[&_svg]:-mx-0.5`). El color sí es el del `AppIcon`
/// (`text-foreground`), porque su clase `text-*` lo saca de la regla que tiñe
/// los íconos de `outline` y `ghost`.
pub(crate) fn button_icon(icon: Lucide, cx: &App) -> Div {
    let color = with_alpha(cx.palette().foreground, 0.8);
    div()
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .h(px(16.))
        .w(px(12.))
        .child(mergeon_ui::icon(icon).size(px(16.)).text_color(color))
}

/// Ícono suelto de un botón de solo ícono (`.action-btn-icon`, `.reflog__action`).
pub(crate) fn plain_icon(icon: Lucide, size: f32) -> AppIcon {
    mergeon_ui::icon(icon).size(px(size))
}

/// `color-mix(in srgb, var(--token) NN%, transparent)`.
pub(crate) fn tint(color: impl Into<Hsla>, amount: f32) -> Hsla {
    with_alpha(color, amount)
}

/// Familia monoespaciada del tema (`var(--font-mono)`).
pub(crate) fn mono_font(cx: &App) -> SharedString {
    cx.theme().mono_font_family.clone()
}
