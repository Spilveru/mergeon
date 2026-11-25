//! Ícono con tono (`src/components/ui/icon/AppIcon.vue`).
//!
//! El catálogo es el de Lucide que ya trae gpui-kit ([`crate::Lucide`]). El
//! original dibuja con trazo fino (Señal: 1,5px); [`crate::Assets`] sirve los mismos
//! SVG con ese trazo.

use std::time::Duration;

use gpui_kit::component::{Icon, IconNamed, Sizable as _, Size};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Animation, AnimationExt as _, App, Hsla, IntoElement, Pixels, Refineable as _, RenderOnce,
    StyleRefinement, Styled, Transformation, Window, percentage, px,
};

use crate::assets::thin_icon_path;
use crate::theme::ActivePaletteExt as _;

/// Tonos de `AppIcon.vue`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IconTone {
    /// `text-foreground`
    #[default]
    Default,
    /// `text-muted-foreground`
    Muted,
    /// `text-primary`
    Active,
    /// `text-destructive`
    Danger,
    /// `text-emerald-400`
    Success,
    /// Hereda el color del texto que lo rodea (el `tone` vacío del original
    /// cuando una clase externa decide el color).
    Current,
}

impl IconTone {
    /// Color del tono en el tema activo; `None` para [`IconTone::Current`].
    pub fn color(self, cx: &App) -> Option<Hsla> {
        let palette = cx.palette();
        match self {
            Self::Default => Some(palette.text_2.into()),
            Self::Muted => Some(palette.muted_foreground.into()),
            Self::Active => Some(palette.primary_text.into()),
            Self::Danger => Some(palette.destructive_foreground.into()),
            Self::Success => Some(palette.success_foreground.into()),
            Self::Current => None,
        }
    }
}

/// Ícono de Mergeon. Se crea con [`icon`].
#[derive(Clone, IntoElement)]
pub struct AppIcon {
    icon: Icon,
    size: Pixels,
    tone: IconTone,
    spin: bool,
    style: StyleRefinement,
}

/// Crea un ícono del catálogo de 16px, con el tono por defecto
/// (`text-foreground`) y el trazo fino de Señal (1,5px).
///
/// El trazo fino lo sirve [`crate::Assets`]: la app tiene que arrancar con
/// `.with_assets(mergeon_ui::Assets)`.
///
/// ```ignore
/// mergeon_ui::icon(Lucide::GitBranch).size(px(18.)).tone(IconTone::Muted)
/// ```
pub fn icon(icon: impl IconNamed) -> AppIcon {
    AppIcon::new(icon)
}

impl AppIcon {
    pub fn new(icon: impl IconNamed) -> Self {
        let path = icon.path();
        let path = thin_icon_path(&path).unwrap_or(path);
        Self::from_icon(Icon::default().path(path))
    }

    /// Ícono a partir de un `Icon` de gpui-kit ya armado (un SVG propio, por
    /// ejemplo). Conserva el trazo que traiga.
    pub fn from_icon(icon: impl Into<Icon>) -> Self {
        Self {
            icon: icon.into(),
            size: px(16.),
            tone: IconTone::default(),
            spin: false,
            style: StyleRefinement::default(),
        }
    }

    /// Lado del ícono en píxeles (`size` del original; 16 por defecto).
    pub fn size(mut self, size: impl Into<Pixels>) -> Self {
        self.size = size.into();
        self
    }

    pub fn tone(mut self, tone: IconTone) -> Self {
        self.tone = tone;
        self
    }

    /// `class="animate-spin"`: gira una vuelta por segundo.
    pub fn spin(mut self, spin: bool) -> Self {
        self.spin = spin;
        self
    }

    /// Lado configurado, en píxeles.
    pub fn icon_size(&self) -> Pixels {
        self.size
    }

    /// Tono configurado.
    pub fn icon_tone(&self) -> IconTone {
        self.tone
    }

    /// Si el ícono gira.
    pub fn is_spinning(&self) -> bool {
        self.spin
    }

    /// El `Icon` de gpui-kit equivalente (tamaño, tono y estilos aplicados),
    /// para las APIs de gpui-kit que piden uno.
    pub fn into_icon(self, cx: &App) -> Icon {
        let mut icon = self
            .icon
            .with_size(Size::Size(self.size))
            .flex_shrink_0()
            .when_some(self.tone.color(cx), |this, color| this.text_color(color));
        // Un color puesto desde fuera gana al tono, como una clase `text-*` externa.
        let external_color = self.style.text.color;
        icon.style().refine(&self.style);
        icon.when_some(external_color, |this, color| this.text_color(color))
    }
}

/// Los estilos externos (márgenes, color, opacidad) llegan al ícono dibujado,
/// igual que las clases que `AppIcon.vue` reenvía al `<svg>`.
impl Styled for AppIcon {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for AppIcon {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let spin = self.spin;
        let icon = self.into_icon(cx);

        if spin {
            icon.with_animation(
                "spin",
                Animation::new(Duration::from_secs(1)).repeat(),
                |this, delta| this.transform(Transformation::rotate(percentage(delta))),
            )
            .into_any_element()
        } else {
            icon.into_any_element()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Lucide;

    /// `AppIcon.test.js`: lo que se le pone desde fuera llega al ícono dibujado.
    #[test]
    fn external_styling_reaches_the_icon() {
        let icon = icon(Lucide::LoaderCircle).spin(true).opacity(0.5);

        assert!(icon.is_spinning());
        assert_eq!(icon.style.opacity, Some(0.5));
    }

    #[test]
    fn defaults_match_the_vue_props() {
        let icon = icon(Lucide::GitBranch);

        assert_eq!(icon.icon_size(), px(16.));
        assert_eq!(icon.icon_tone(), IconTone::Default);
        assert!(!icon.is_spinning());
    }
}
