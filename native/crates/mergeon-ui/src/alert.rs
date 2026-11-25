//! Aviso en línea (`src/components/ui/alert/Alert.vue`, `alertVariants`).

use gpui_kit::component::{Icon, Sizable as _, Size, StyledExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, FontWeight, Hsla, IntoElement, ParentElement, RenderOnce, SharedString,
    StyleRefinement, Styled, Window, div, px, relative, rems, transparent_black,
};

use crate::Lucide;
use crate::style::{MergeonStyled as _, Radius};
use crate::theme::{ActivePaletteExt as _, with_alpha};

/// Estados de `Alert.vue` (`status`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AlertVariant {
    #[default]
    Default,
    /// En Vue también `accent`.
    Info,
    Success,
    Warning,
    /// En Vue `danger` / `destructive`.
    Error,
}

impl AlertVariant {
    /// Traduce el `status` de `Alert.vue`; lo desconocido es `Default`.
    pub fn parse(status: &str) -> Self {
        match status {
            "accent" | "info" => Self::Info,
            "success" => Self::Success,
            "warning" => Self::Warning,
            "danger" | "destructive" => Self::Error,
            _ => Self::Default,
        }
    }

    fn default_icon(self) -> Lucide {
        match self {
            Self::Default | Self::Info => Lucide::Info,
            Self::Success => Lucide::CircleCheck,
            Self::Warning => Lucide::TriangleAlert,
            Self::Error => Lucide::CircleAlert,
        }
    }
}

/// Aviso de Mergeon. Se crea con [`alert`].
#[derive(IntoElement)]
pub struct Alert {
    variant: AlertVariant,
    title: Option<SharedString>,
    description: Option<SharedString>,
    /// `None`: ícono por defecto del estado; `Some(None)`: sin ícono.
    icon: Option<Option<Icon>>,
    children: Vec<AnyElement>,
    actions: Vec<AnyElement>,
    style: StyleRefinement,
}

/// Crea un aviso con el estado dado.
///
/// ```ignore
/// mergeon_ui::alert(AlertVariant::Warning)
///     .title("Merge conflicts expected")
///     .description("Integrating these branches will conflict in 2 file(s).")
/// ```
pub fn alert(variant: AlertVariant) -> Alert {
    Alert {
        variant,
        title: None,
        description: None,
        icon: None,
        children: Vec::new(),
        actions: Vec::new(),
        style: StyleRefinement::default(),
    }
}

impl Alert {
    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Sustituye el ícono del estado.
    pub fn icon(mut self, icon: impl Into<Icon>) -> Self {
        self.icon = Some(Some(icon.into()));
        self
    }

    /// Oculta el ícono (`:icon="null"` en Vue).
    pub fn no_icon(mut self) -> Self {
        self.icon = Some(None);
        self
    }

    /// Slot `actions`: se coloca a la derecha del contenido.
    pub fn action(mut self, action: impl IntoElement) -> Self {
        self.actions.push(action.into_any_element());
        self
    }

    /// Borde, fondo, texto e ícono del estado en el tema activo.
    fn colors(&self, cx: &App) -> (Hsla, Hsla, Hsla, Hsla) {
        // Señal: fondo `*-soft`, borde del color de estado y el ícono en
        // `*-text`; el texto sigue en tinta.
        let palette = cx.palette();
        let tinted = |color: gpui_kit::Rgba, soft: gpui_kit::Rgba, text: gpui_kit::Rgba| {
            (
                with_alpha(color, 0.36),
                soft.into(),
                palette.text_1.into(),
                text.into(),
            )
        };
        match self.variant {
            AlertVariant::Default => (
                palette.border_base.into(),
                transparent_black(),
                palette.text_1.into(),
                palette.text_3.into(),
            ),
            AlertVariant::Info => tinted(palette.info, palette.info_soft, palette.info_foreground),
            AlertVariant::Success => tinted(
                palette.success,
                palette.success_soft,
                palette.success_foreground,
            ),
            AlertVariant::Warning => tinted(
                palette.warning,
                palette.warning_soft,
                palette.warning_foreground,
            ),
            AlertVariant::Error => {
                let (border, bg, _, icon) = tinted(
                    palette.destructive,
                    palette.destructive_soft,
                    palette.destructive_foreground,
                );
                (border, bg, palette.destructive_foreground.into(), icon)
            }
        }
    }
}

/// El slot por defecto de `Alert.vue` (contenido libre bajo el título).
impl ParentElement for Alert {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl Styled for Alert {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for Alert {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let (border, bg, fg, icon_color) = self.colors(cx);
        let icon = match self.icon {
            None => Some(Icon::new(self.variant.default_icon())),
            Some(icon) => icon,
        };
        let has_body = self.description.is_some() || !self.children.is_empty();

        div()
            .flex()
            .items_start()
            .gap(rems(0.625))
            .radius(Radius::Md, cx)
            .border_1()
            .border_color(border)
            .bg(bg)
            .px(rems(0.875))
            .py_3()
            .text_color(fg)
            .tw_text_sm()
            .refine_style(&self.style)
            .when_some(icon, |this, icon| {
                this.child(
                    div()
                        .flex_shrink_0()
                        .mt(rems(0.125))
                        .child(icon.with_size(Size::Size(px(16.))).text_color(icon_color)),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(rems(0.125))
                    .when_some(self.title, |this, title| {
                        this.child(
                            div()
                                .font_weight(FontWeight::MEDIUM)
                                .line_height(relative(1.375))
                                .child(title),
                        )
                    })
                    .when(has_body, |this| {
                        this.child(
                            div()
                                .line_height(relative(1.625))
                                .opacity(0.8)
                                .when_some(self.description, |this, text| this.child(text))
                                .children(self.children),
                        )
                    }),
            )
            .children(self.actions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_vue_status_names() {
        assert_eq!(AlertVariant::parse("accent"), AlertVariant::Info);
        assert_eq!(AlertVariant::parse("danger"), AlertVariant::Error);
        assert_eq!(AlertVariant::parse("nope"), AlertVariant::Default);
    }
}
