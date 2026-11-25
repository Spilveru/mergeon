//! Piezas de presentación compartidas por las vistas de este crate.

use std::collections::HashMap;

use gpui_kit::base::Button as BaseButton;
use gpui_kit::component::{ActiveTheme as _, Icon, IconNamed, Sizable as _, Size};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyEntity, App, BoxShadow, Div, ElementId, Entity, Global, InteractiveElement as _,
    MouseButton, ParentElement as _, Pixels, SharedString, Styled as _, Window, black, div, font,
    px, relative,
};
use mergeon_ui::dialog::{self, DialogId};
use mergeon_ui::style::focus_ring_overlay;
use mergeon_ui::theme::with_alpha;
use mergeon_ui::{ActivePaletteExt as _, AppIcon, MergeonStyled as _, Radius};

/// Un ícono de Lucide tal cual lo pinta `lucide-vue-next` (trazo 2), para los
/// sitios donde el original no pasa por `AppIcon`.
pub(crate) fn lucide(icon: impl IconNamed, size: Pixels) -> Icon {
    Icon::new(icon).with_size(Size::Size(size)).flex_shrink_0()
}

// El `Github` de Lucide (el gato de trazo que usa la app). El catálogo de
// íconos de gpui-kit trae en su lugar la marca rellena de GitHub.
macro_rules! github_svg {
    ($stroke:literal) => {
        concat!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"24\" height=\"24\" ",
            "viewBox=\"0 0 24 24\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"",
            $stroke,
            "\" stroke-linecap=\"round\" stroke-linejoin=\"round\">",
            "<path d=\"M15 22v-4a4.8 4.8 0 0 0-1-3.5c3 0 6-2 6-5.5.08-1.25-.27-2.48-1-3.5.28-1.15",
            ".28-2.35 0-3.5 0 0-1 0-3 1.5-2.64-.5-5.36-.5-8 0C6 2 5 2 5 2c-.3 1.15-.3 2.35 0 3.5A5.403 ",
            "5.403 0 0 0 4 9c0 3.5 3 5.5 6 5.5-.39.49-.68 1.05-.85 1.65-.17.6-.22 1.23-.15 1.85v4\"/>",
            "<path d=\"M9 18c-4.51 2-5-2-7-2\"/></svg>"
        )
    };
}

/// `<Github :size>` de `lucide-vue-next` (trazo 2).
pub(crate) fn github_icon(size: Pixels) -> Icon {
    Icon::default()
        .data(github_svg!("2").as_bytes())
        .with_size(Size::Size(size))
        .flex_shrink_0()
}

/// `<AppIcon :icon="Github">` (trazo 1.8).
pub(crate) fn github_app_icon() -> AppIcon {
    AppIcon::from_icon(Icon::default().data(github_svg!("1.8").as_bytes()))
}

/// `rounded-lg border border-border bg-card/70 p-3`: las tarjetas del selector
/// de repositorios.
pub(crate) fn card_section(cx: &App) -> Div {
    let palette = cx.palette();
    div()
        .flex_shrink_0()
        .radius(Radius::Lg, cx)
        .border_1()
        .border_color(palette.border)
        .bg(with_alpha(palette.card, 0.7))
        .p_3()
}

/// `shadow-sm` de Tailwind.
pub(crate) fn shadow_sm() -> Vec<BoxShadow> {
    vec![
        BoxShadow::new(px(0.), px(1.), black().opacity(0.1)).blur_radius(px(1.5)),
        BoxShadow::new(px(0.), px(1.), black().opacity(0.1))
            .blur_radius(px(1.))
            .spread_radius(px(-1.)),
    ]
}

/// Un `<button>` propio del original: activable con el teclado y con el anillo
/// de foco de la app cuando el foco le llega por teclado. El clic no mueve el
/// foco, así que un botón con foco lo recibió del teclado (`:focus-visible`).
pub(crate) fn plain_button(
    id: impl Into<ElementId>,
    radius: Pixels,
    window: &mut Window,
    cx: &mut App,
) -> BaseButton {
    let id = id.into();
    let focus_handle = window
        .use_keyed_state(id.clone(), cx, |_, cx| cx.focus_handle())
        .read(cx)
        .clone();
    let focused = focus_handle.is_focused(window);
    BaseButton::new(id)
        .track_focus(&focus_handle)
        .relative()
        .rounded(radius)
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
        .when(focused, |this| this.child(focus_ring_overlay(radius, cx)))
}

/// Ancho de `n` caracteres `0` del texto de la interfaz (la unidad `ch` de
/// CSS) a `font_size`.
pub(crate) fn ch(n: f32, font_size: Pixels, window: &Window, cx: &App) -> Pixels {
    let text_system = window.text_system();
    let font_id = text_system.resolve_font(&font(cx.theme().font_family.clone()));
    text_system
        .ch_advance(font_id, font_size)
        .unwrap_or(font_size * 0.6)
        * n
}

/// Un `AppIcon` como hijo de un botón: la receta del botón lo pinta a 16px
/// (`[&_svg:not([class*='size-'])]:size-4`), al 80% de opacidad y con `-mx-0.5`,
/// así que ocupa 4px menos de lo que mide.
pub(crate) fn button_app_icon(icon: AppIcon) -> Div {
    div()
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .h_4()
        .w_3()
        .child(icon.size(px(16.)).opacity(0.8))
}

/// `.picker-label`: como [`field_label`], con el interlineado que hereda del
/// cuerpo del diálogo.
pub(crate) fn picker_label(label: &'static str, cx: &App) -> Div {
    field_label(label, cx).line_height(relative(20. / 14.))
}

/// `text-xs font-medium text-muted-foreground`: el rótulo de un campo.
pub(crate) fn field_label(label: &'static str, cx: &App) -> Div {
    div()
        .tw_text_xs()
        .tw_font_medium()
        .text_color(cx.palette().muted_foreground)
        .child(label)
}

/// Lo que en `App.vue` eran `showAccountDialog`, `showRepoPicker`... y el
/// estado que los componentes conservaban entre una apertura y la siguiente.
#[derive(Default)]
pub(crate) struct HomeDialogs {
    open: HashMap<&'static str, (DialogId, Option<AnyEntity>)>,
    /// `showTokenForm` del diálogo de cuenta: sobrevive a cerrarlo.
    pub show_token_form: bool,
}

impl Global for HomeDialogs {}

impl HomeDialogs {
    pub(crate) fn get(cx: &mut App) -> &mut HomeDialogs {
        cx.default_global::<HomeDialogs>()
    }

    /// El diálogo de `title` que este crate ya tiene abierto en la ventana.
    pub(crate) fn open_dialog(
        title: &'static str,
        window: &Window,
        cx: &mut App,
    ) -> Option<(DialogId, Option<AnyEntity>)> {
        let entry = Self::get(cx).open.get(title).cloned()?;
        let still_open = dialog::open_titles(window, cx)
            .iter()
            .any(|open| open.as_ref() == title);
        if !still_open {
            Self::get(cx).open.remove(title);
            return None;
        }
        Some(entry)
    }

    /// La vista del diálogo de `title`, si está abierto.
    pub(crate) fn open_view<V: 'static>(
        title: &'static str,
        window: &Window,
        cx: &mut App,
    ) -> Option<Entity<V>> {
        Self::open_dialog(title, window, cx)?
            .1
            .and_then(|view| view.downcast::<V>().ok())
    }

    pub(crate) fn register(
        title: &'static str,
        id: DialogId,
        view: Option<AnyEntity>,
        cx: &mut App,
    ) {
        Self::get(cx).open.insert(title, (id, view));
    }

    pub(crate) fn forget(title: &'static str, cx: &mut App) {
        Self::get(cx).open.remove(title);
    }
}

/// Texto de una línea que no se parte ni encoge.
pub(crate) fn nowrap(text: impl Into<SharedString>) -> Div {
    div().flex_shrink_0().whitespace_nowrap().child(text.into())
}
