//! Indicador de carga (`src/components/ui/spinner/Spinner.vue`).

use std::time::Duration;

use gpui_kit::component::{Icon, Sizable as _, Size};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Animation, AnimationExt as _, App, Hsla, IntoElement, RenderOnce, Styled as _, Transformation,
    Window, percentage, px,
};

use crate::theme::ActivePaletteExt as _;

/// El mismo trazo del original: aro tenue y un cuarto de arco encima. El aro
/// lleva `stroke-opacity` 0.125 y no el 0.25 del original porque GPUI corrige
/// la gamma de las máscaras monocromas: con 0.25 se ve al 40 %.
const SPINNER_SVG: &[u8] = include_bytes!("../assets/icons/spinner.svg");

/// Tamaños de `spinnerVariants`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SpinnerSize {
    /// 14px
    Sm,
    /// 16px
    #[default]
    Md,
    /// 20px
    Lg,
    /// 24px
    Xl,
}

impl SpinnerSize {
    fn px(self) -> f32 {
        match self {
            Self::Sm => 14.,
            Self::Md => 16.,
            Self::Lg => 20.,
            Self::Xl => 24.,
        }
    }
}

/// Colores de `spinnerVariants`. `Current` hereda el color del texto.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SpinnerColor {
    #[default]
    Current,
    /// `accent` y `primary` en Vue.
    Primary,
    Danger,
    Success,
    Warning,
}

/// Spinner de Mergeon. Se crea con [`spinner`].
#[derive(IntoElement)]
pub struct Spinner {
    size: SpinnerSize,
    color: SpinnerColor,
    color_value: Option<Hsla>,
}

/// Crea un spinner mediano que hereda el color del texto.
///
/// ```ignore
/// mergeon_ui::spinner().size(SpinnerSize::Lg).color(SpinnerColor::Primary)
/// ```
pub fn spinner() -> Spinner {
    Spinner::new()
}

impl Spinner {
    pub fn new() -> Self {
        Self {
            size: SpinnerSize::default(),
            color: SpinnerColor::default(),
            color_value: None,
        }
    }

    pub fn size(mut self, size: SpinnerSize) -> Self {
        self.size = size;
        self
    }

    pub fn color(mut self, color: SpinnerColor) -> Self {
        self.color = color;
        self
    }

    /// Color explícito, para componentes que ya resolvieron el suyo.
    pub(crate) fn color_value(mut self, color: Hsla) -> Self {
        self.color_value = Some(color);
        self
    }
}

impl Default for Spinner {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderOnce for Spinner {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let palette = cx.palette();
        let color = self.color_value.or(match self.color {
            SpinnerColor::Current => None,
            SpinnerColor::Primary => Some(palette.primary.into()),
            SpinnerColor::Danger => Some(palette.destructive.into()),
            SpinnerColor::Success => Some(palette.success.into()),
            SpinnerColor::Warning => Some(palette.warning.into()),
        });

        Icon::default()
            .data(SPINNER_SVG)
            .with_size(Size::Size(px(self.size.px())))
            .flex_shrink_0()
            .when_some(color, |this, color| this.text_color(color))
            // `animate-spin`: una vuelta por segundo, lineal.
            .with_animation(
                "spin",
                Animation::new(Duration::from_secs(1)).repeat(),
                |this, delta| this.transform(Transformation::rotate(percentage(delta))),
            )
    }
}
