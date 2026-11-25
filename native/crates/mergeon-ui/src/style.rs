//! Escalas compartidas del sistema de diseño: radios, sombras y superficies.
//!
//! Tema «Señal»: radios de 0 a 6px, filetes de 1px como estructura y sombras
//! solo en lo que flota (`shadow-1` botones, `shadow-2` menús/toasts/popovers,
//! `shadow-3` diálogos).

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{
    App, BoxShadow, Div, FontWeight, Hsla, ParentElement as _, Pixels, Styled, black, div, px,
    relative,
};

use crate::theme::ActivePaletteExt as _;

/// Escala tipográfica de la interfaz de Señal, en píxeles enteros (sin los
/// 11,5 / 12,8 heredados de los `rem` del original): lo que cabe en una fila
/// de 24–32px sin medios píxeles que emborronen el texto.
pub mod text {
    use gpui_kit::{Pixels, px};

    /// Pastillas mono pequeñas, marcas de tiempo.
    pub const MICRO: Pixels = px(10.);
    /// Texto secundario: contexto, carpetas, contadores, barra de estado.
    pub const CAPTION: Pixels = px(11.);
    /// Pestañas, migas y botones compactos.
    pub const UI: Pixels = px(12.);
    /// Texto de controles y filas de lista.
    pub const BODY: Pixels = px(13.);
    /// Títulos de panel.
    pub const TITLE: Pixels = px(14.);

    /// La escala en orden, para comprobarla.
    pub const SCALE: [Pixels; 5] = [MICRO, CAPTION, UI, BODY, TITLE];
}

/// Escala de radios de Mergeon (Señal: 0–6px).
///
/// Sale de `cx.theme().radius` (4px, `--r-2`), así que cambia con el tema. Los
/// nombres son los heredados de Tailwind; los valores, los de Señal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Radius {
    /// `--r-1` (2px): checkbox, pills de ref, badges, resaltado por palabra.
    Xs,
    /// `--r-1` (2px).
    Sm,
    /// `--r-2` (4px): botones pequeños, segmentados.
    Md,
    /// `--r-2` (4px): botones, inputs, selects.
    Lg,
    /// `--r-3` (6px): menús, diálogos, toasts (el máximo).
    Xl,
    /// `--r-3` (6px).
    Xxl,
}

impl Radius {
    /// Radio en píxeles para el tema activo.
    pub fn px(self, cx: &App) -> Pixels {
        let base = cx.theme().radius;
        match self {
            Self::Xs | Self::Sm => (base - px(2.)).max(px(0.)),
            Self::Md | Self::Lg => base,
            Self::Xl | Self::Xxl => base + px(2.),
        }
    }
}

/// Utilidades de estilo de Mergeon sobre cualquier elemento `Styled`.
pub trait MergeonStyled: Styled + Sized {
    /// Redondea con la escala de radios de Mergeon (`rounded-sm` → [`Radius::Sm`]).
    fn radius(self, radius: Radius, cx: &App) -> Self {
        self.rounded(radius.px(cx))
    }

    /// Texto `text-xs` de Tailwind: 12px con línea de 16px.
    fn tw_text_xs(self) -> Self {
        self.text_xs().line_height(relative(4. / 3.))
    }

    /// Texto `text-sm` de Tailwind: 14px con línea de 20px (el texto base de la app).
    fn tw_text_sm(self) -> Self {
        self.text_sm().line_height(relative(20. / 14.))
    }

    /// Texto `text-base` de Tailwind: 16px con línea de 24px.
    fn tw_text_base(self) -> Self {
        self.text_base().line_height(relative(1.5))
    }

    /// Texto `text-lg` de Tailwind: 18px con línea de 28px.
    fn tw_text_lg(self) -> Self {
        self.text_lg().line_height(relative(28. / 18.))
    }

    /// `font-medium` (peso 500).
    fn tw_font_medium(self) -> Self {
        self.font_weight(FontWeight::MEDIUM)
    }

    /// `shadow-1` de Señal: un píxel inferior, el relieve de botones.
    fn shadow_control(self) -> Self {
        self.shadow(control_shadow(black().opacity(0.04)))
    }

    /// Superficie flotante de Señal: `bg-raised`, borde `border-base`, radio
    /// `--r-3` (6px) y `shadow-2`.
    ///
    /// Es la misma para menús, selects, hover cards y toasts.
    fn floating_surface(self, cx: &App) -> Self {
        let palette = cx.palette();
        self.bg(palette.popover)
            .text_color(palette.popover_foreground)
            .border_1()
            .border_color(palette.floating_border)
            .rounded(Radius::Xl.px(cx))
            .shadow(floating_shadow(cx))
    }
}

impl<T: Styled + Sized> MergeonStyled for T {}

// CSS define el `blur` de una sombra como el doble de la desviación estándar de
// la gaussiana; el shader de GPUI toma ese campo como la desviación misma. Por
// eso todos los desenfoques de este módulo son la mitad del valor del CSS.

/// `shadow-1` de Señal (`0 1px 0`) con el color indicado.
pub fn control_shadow(color: Hsla) -> Vec<BoxShadow> {
    vec![BoxShadow::new(px(0.), px(1.), color)]
}

/// `shadow-2` de Señal (menú, toast, popover):
/// `0 1px 2px /.06, 0 8px 24px -6px /.16` (claro) y
/// `0 0 0 1px /.4, 0 10px 28px -6px /.6` (oscuro: el anillo negro recorta el
/// borde contra fondos casi negros).
pub fn floating_shadow(cx: &App) -> Vec<BoxShadow> {
    let shadow = cx.palette().shadow_floating.into();
    if cx.theme().mode.is_dark() {
        vec![
            BoxShadow::new(px(0.), px(0.), black().opacity(0.4)).spread_radius(px(1.)),
            BoxShadow::new(px(0.), px(10.), shadow)
                .blur_radius(px(14.))
                .spread_radius(px(-6.)),
        ]
    } else {
        vec![
            BoxShadow::new(px(0.), px(1.), black().opacity(0.06)).blur_radius(px(1.)),
            BoxShadow::new(px(0.), px(8.), shadow)
                .blur_radius(px(12.))
                .spread_radius(px(-6.)),
        ]
    }
}

/// `shadow-3` de Señal (diálogo modal):
/// `0 2px 4px /.06, 0 24px 48px -12px /.28` (claro) y
/// `0 0 0 1px /.5, 0 28px 56px -12px /.75` (oscuro).
pub fn dialog_shadow(cx: &App) -> Vec<BoxShadow> {
    let shadow = cx.palette().shadow_dialog.into();
    if cx.theme().mode.is_dark() {
        vec![
            BoxShadow::new(px(0.), px(0.), black().opacity(0.5)).spread_radius(px(1.)),
            BoxShadow::new(px(0.), px(28.), shadow)
                .blur_radius(px(28.))
                .spread_radius(px(-12.)),
        ]
    } else {
        vec![
            BoxShadow::new(px(0.), px(2.), black().opacity(0.06)).blur_radius(px(2.)),
            BoxShadow::new(px(0.), px(24.), shadow)
                .blur_radius(px(24.))
                .spread_radius(px(-12.)),
        ]
    }
}

/// Anillo de foco de teclado de botones, tabs y radios (Señal: contorno de
/// 2px en `--focus` con separación), como hijo absoluto de un control con borde
/// de 1px.
///
/// Es un marco y no una sombra porque GPUI pinta las sombras también debajo del
/// elemento: con un fondo translúcido (`outline` en oscuro) el anillo teñiría
/// todo el control.
pub fn focus_ring_overlay(radius: Pixels, cx: &App) -> Div {
    // 1px de separación + 2px de anillo, medidos desde fuera del borde.
    let inset = -px(4.);
    div()
        .absolute()
        .top(inset)
        .left(inset)
        .right(inset)
        .bottom(inset)
        .rounded(radius + px(3.))
        .border_2()
        .border_color(cx.palette().ring)
}

/// Foco de inputs y selects (Señal: borde `border-ink` + contorno naranja de
/// 2px) para superponer a un control ya dibujado.
pub fn input_focus_overlay(radius: Pixels, cx: &App) -> Div {
    let palette = cx.palette();
    let halo = -px(3.);
    div()
        .absolute()
        .inset_0()
        .child(
            div()
                .absolute()
                .top(halo)
                .left(halo)
                .right(halo)
                .bottom(halo)
                .rounded(radius + px(2.))
                .border_2()
                .border_color(palette.ring),
        )
        .child(
            div()
                .absolute()
                .inset_0()
                .rounded(radius)
                .border_1()
                .border_color(palette.border_ink),
        )
}

/// `disabled:opacity-64` de las recetas.
pub const DISABLED_OPACITY: f32 = 0.64;

#[cfg(test)]
mod tests {
    use super::text;

    #[test]
    fn the_type_scale_grows_one_whole_pixel_at_a_time() {
        let sizes: Vec<f32> = text::SCALE.iter().map(|size| f32::from(*size)).collect();
        assert_eq!(sizes, [10., 11., 12., 13., 14.]);
        assert!(sizes.iter().all(|size| size.fract() == 0.));
    }
}
