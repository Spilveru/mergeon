//! Etiqueta compacta (`src/components/ui/badge/Badge.vue`, `badgeVariants`) y
//! la receta del estado de un archivo (`src/lib/ui/statusBadge.js`).

use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Size, StyledExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, FontWeight, Hsla, IntoElement, ParentElement, RenderOnce, SharedString,
    StyleRefinement, Styled, Window, div, px, relative, rems, transparent_black, white,
};

use crate::style::Radius;
use crate::theme::{ActivePaletteExt as _, with_alpha};

/// Variantes de `Badge.vue`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BadgeVariant {
    /// `bg-accent text-foreground`
    #[default]
    Default,
    /// `bg-primary`. En Vue también `accent`.
    Primary,
    Secondary,
    Outline,
    Info,
    Success,
    Warning,
    /// En Vue también `danger`.
    Destructive,
}

impl BadgeVariant {
    /// Traduce los nombres que acepta `Badge.vue`; lo desconocido es `Default`.
    pub fn parse(name: &str) -> Self {
        match name {
            "primary" | "accent" => Self::Primary,
            "secondary" => Self::Secondary,
            "outline" => Self::Outline,
            "info" => Self::Info,
            "success" => Self::Success,
            "warning" => Self::Warning,
            "danger" | "destructive" => Self::Destructive,
            _ => Self::Default,
        }
    }
}

/// Tamaños de `badgeVariants`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BadgeSize {
    /// `h-4`, 10px
    Sm,
    /// `h-4.5`, 12px
    #[default]
    Md,
    /// `h-5.5`, 14px
    Lg,
}

/// Badge de Mergeon. Se crea con [`badge`] o [`status_badge`].
#[derive(IntoElement)]
pub struct Badge {
    variant: BadgeVariant,
    size: BadgeSize,
    solid: bool,
    square: bool,
    on_ink: bool,
    icon: Option<Icon>,
    children: Vec<AnyElement>,
    style: StyleRefinement,
}

/// Crea un badge con la etiqueta dada.
///
/// ```ignore
/// mergeon_ui::badge("3").variant(BadgeVariant::Info)
/// mergeon_ui::badge("Private").variant(BadgeVariant::Secondary)
/// ```
pub fn badge(label: impl Into<SharedString>) -> Badge {
    Badge::new().child(label.into())
}

impl Badge {
    /// Badge sin contenido; se le añaden hijos con `.child(..)`.
    pub fn new() -> Self {
        Self {
            variant: BadgeVariant::default(),
            size: BadgeSize::default(),
            solid: false,
            square: false,
            on_ink: false,
            icon: None,
            children: Vec::new(),
            style: StyleRefinement::default(),
        }
    }

    pub fn variant(mut self, variant: BadgeVariant) -> Self {
        self.variant = variant;
        self
    }

    pub fn size(mut self, size: BadgeSize) -> Self {
        self.size = size;
        self
    }

    /// Fuerza el relleno sólido en las variantes que por defecto son tenues.
    pub fn solid(mut self, solid: bool) -> Self {
        self.solid = solid;
        self
    }

    /// `shape="square"`: cuadrado de 16px para un solo carácter de estado.
    pub fn square(mut self) -> Self {
        self.square = true;
        self
    }

    /// El badge va sobre una fila seleccionada en tinta: el estado de archivo
    /// pasa a `sel-fg-2` para leerse sobre el fondo invertido.
    pub fn on_ink(mut self, on_ink: bool) -> Self {
        self.on_ink = on_ink;
        self
    }

    /// Ícono inicial de 12px.
    pub fn icon(mut self, icon: impl Into<Icon>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    /// Fondo, texto y borde de la variante en el tema activo (badges de Señal:
    /// suaves con fondo `*-soft` y texto `*-text`, sólido de acento con tinta y
    /// outline con `border-control`).
    fn colors(&self, cx: &App) -> (Hsla, Hsla, Hsla) {
        let palette = cx.palette();
        let clear = transparent_black();
        let primary = (
            palette.primary.into(),
            palette.primary_foreground.into(),
            clear,
        );

        // Estado de archivo: letra en mono dentro de un cuadrado con borde del
        // mismo color (M neutro, A verde, D rojo, R/C azul, conflicto ámbar).
        if self.square {
            if self.on_ink {
                let fg: Hsla = palette.sel_fg_2.into();
                return (clear, fg, fg);
            }
            let fg: Hsla = match (self.variant, self.solid) {
                (BadgeVariant::Success, _) => palette.success_foreground.into(),
                (BadgeVariant::Destructive, true) => palette.destructive_foreground.into(),
                (BadgeVariant::Destructive, false) => palette.warning_foreground.into(),
                (BadgeVariant::Default | BadgeVariant::Info, _) => palette.info_foreground.into(),
                _ => palette.text_2.into(),
            };
            return (clear, fg, fg);
        }

        // `SOLID_MAP` de Badge.vue: `default` y `primary` sólidos son el primario.
        if self.solid {
            match self.variant {
                BadgeVariant::Default | BadgeVariant::Primary => return primary,
                BadgeVariant::Success => return (palette.success.into(), white(), clear),
                // El relleno de aviso es claro: tinta encima.
                BadgeVariant::Warning => {
                    return (
                        palette.warning.into(),
                        palette.primary_foreground.into(),
                        clear,
                    );
                }
                BadgeVariant::Destructive => return (palette.destructive.into(), white(), clear),
                _ => {}
            }
        }

        match self.variant {
            BadgeVariant::Default => (palette.bg_pressed.into(), palette.text_2.into(), clear),
            BadgeVariant::Primary => (
                palette.primary_soft.into(),
                palette.primary_text.into(),
                palette.primary_soft_border.into(),
            ),
            BadgeVariant::Secondary => (palette.bg_hover.into(), palette.text_1.into(), clear),
            BadgeVariant::Outline => (clear, palette.text_2.into(), palette.border_control.into()),
            BadgeVariant::Info => (
                palette.info_soft.into(),
                palette.info_foreground.into(),
                clear,
            ),
            BadgeVariant::Success => (
                palette.success_soft.into(),
                palette.success_foreground.into(),
                clear,
            ),
            BadgeVariant::Warning => (
                palette.warning_soft.into(),
                palette.warning_foreground.into(),
                clear,
            ),
            BadgeVariant::Destructive => (
                palette.destructive_soft.into(),
                palette.destructive_foreground.into(),
                clear,
            ),
        }
    }
}

impl Default for Badge {
    fn default() -> Self {
        Self::new()
    }
}

impl ParentElement for Badge {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl Styled for Badge {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for Badge {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let (bg, fg, border) = self.colors(cx);
        // Alto, padding horizontal (incluye el borde de 1px), cuerpo y radio.
        let (height, padding, text, radius) = match self.size {
            BadgeSize::Sm => (16., 4., 10.5, Radius::Xs),
            BadgeSize::Md => (18., 5., 11.5, Radius::Xs),
            BadgeSize::Lg => (20., 7., 12., Radius::Xs),
        };

        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .justify_center()
            .gap_1()
            .h(rems(height / 16.))
            .min_w(rems(height / 16.))
            .px(rems(padding / 16.))
            .rounded(radius.px(cx))
            .border_1()
            .border_color(border)
            .bg(bg)
            .text_color(fg)
            .text_size(rems(text / 16.))
            .line_height(relative(1.))
            .font_weight(FontWeight::MEDIUM)
            .whitespace_nowrap()
            .when(self.square, |this| {
                this.size_4()
                    .min_w_4()
                    .p_0()
                    .rounded(Radius::Xs.px(cx))
                    .font_family(cx.theme().mono_font_family.clone())
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_size(rems(10. / 16.))
            })
            .refine_style(&self.style)
            .when_some(self.icon, |this, icon| {
                this.child(
                    icon.with_size(Size::Size(px(12.)))
                        .text_color(with_alpha(fg, 0.8)),
                )
            })
            .children(self.children)
    }
}

/// Variante y relleno del badge de un estado de archivo (`statusBadge` de
/// `statusBadge.js`).
///
/// Cubre los códigos de un commit (`M`, `A`, `D`, `R`, `C`) y los del working
/// tree (`?`, `??`, `U`). Sin seguir se pinta como añadido; el conflicto es lo
/// único que no va sólido, porque no es un cambio sino algo que resolver.
pub fn status_badge_style(status: &str) -> (BadgeVariant, bool) {
    let variant = match status {
        "M" => BadgeVariant::Warning,
        "A" | "?" | "??" => BadgeVariant::Success,
        "D" | "U" => BadgeVariant::Destructive,
        "R" | "C" => BadgeVariant::Default,
        _ => BadgeVariant::Secondary,
    };
    let solid = matches!(status, "M" | "A" | "D" | "?" | "??");
    (variant, solid)
}

/// Carácter que se muestra para un estado (`statusLabel` de `statusBadge.js`).
pub fn status_label(status: &str) -> SharedString {
    match status {
        "A" | "?" | "??" => "+".into(),
        "U" => "!".into(),
        other => SharedString::from(other.to_string()),
    }
}

/// Badge cuadrado del estado de un archivo: la única receta con la que las
/// listas de archivos pintan un estado.
///
/// ```ignore
/// mergeon_ui::status_badge(&file.status) // "M", "A", "D", "R", "??", "U"...
/// ```
pub fn status_badge(status: &str) -> Badge {
    let (variant, solid) = status_badge_style(status);
    badge(status_label(status))
        .variant(variant)
        .solid(solid)
        .square()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Port de `src/lib/ui/statusBadge.test.js`.

    #[test]
    fn gives_modified_added_and_deleted_their_own_solid_colour() {
        assert_eq!(status_badge_style("M"), (BadgeVariant::Warning, true));
        assert_eq!(status_badge_style("A"), (BadgeVariant::Success, true));
        assert_eq!(status_badge_style("D"), (BadgeVariant::Destructive, true));
    }

    /// Para quien mira, un archivo sin seguir es un archivo nuevo.
    #[test]
    fn shows_an_untracked_file_as_added() {
        assert_eq!(status_badge_style("?"), status_badge_style("A"));
        assert_eq!(status_badge_style("??"), status_badge_style("A"));
        assert_eq!(status_label("??"), "+");
    }

    /// Un conflicto no es un cambio, es algo que resolver: no va sólido.
    #[test]
    fn sets_a_conflict_apart_from_a_deletion() {
        assert_eq!(status_badge_style("U"), (BadgeVariant::Destructive, false));
        assert_eq!(status_label("U"), "!");
    }

    #[test]
    fn falls_back_instead_of_leaving_a_hole_for_an_unknown_code() {
        assert_eq!(status_badge_style("X"), (BadgeVariant::Secondary, false));
        assert_eq!(status_label("X"), "X");
    }

    #[test]
    fn status_badge_is_square_and_carries_the_recipe() {
        let badge = status_badge("M");
        assert!(badge.square);
        assert!(badge.solid);
        assert_eq!(badge.variant, BadgeVariant::Warning);
    }

    #[test]
    fn parses_the_vue_variant_names() {
        assert_eq!(BadgeVariant::parse("accent"), BadgeVariant::Primary);
        assert_eq!(BadgeVariant::parse("danger"), BadgeVariant::Destructive);
        assert_eq!(BadgeVariant::parse("whatever"), BadgeVariant::Default);
    }
}
