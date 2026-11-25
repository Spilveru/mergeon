//! Tema de Mergeon sobre gpui-kit.
//!
//! Tema «Señal»: papel blanco y tinta neutra, filetes de 1px y un único color,
//! el naranja señal. Los tokens (claro y oscuro) los genera
//! `native/tools/gen-theme.py`. El tema de gpui-kit cubre lo que pintan sus
//! componentes; [`Palette`] lleva los tokens propios (superficies, selección en
//! tinta, carriles del grafo, diff...) y los alias heredados de shadcn.

mod palette;

use gpui_kit::component::scroll::ScrollbarMode;
use gpui_kit::component::{Theme, ThemeMode, ThemeRegistry};
use gpui_kit::{App, Global, Hsla, WindowAppearance};

pub use palette::Palette;

const THEME_JSON: &str = include_str!("../../assets/themes/mergeon.json");
const LIGHT_THEME: &str = "Mergeon Light";
const DARK_THEME: &str = "Mergeon Dark";

/// Preferencia de tema del usuario (`settings.theme` en la configuración).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemePreference {
    #[default]
    Light,
    Dark,
    System,
}

impl ThemePreference {
    pub fn parse(value: &str) -> Self {
        match value {
            "dark" => Self::Dark,
            "system" => Self::System,
            _ => Self::Light,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
            Self::System => "system",
        }
    }
}

struct ActivePalette(Palette);

impl Global for ActivePalette {}

/// Acceso a los tokens propios desde cualquier contexto: `cx.palette().card`.
pub trait ActivePaletteExt {
    fn palette(&self) -> &Palette;
}

impl ActivePaletteExt for App {
    fn palette(&self) -> &Palette {
        &self.global::<ActivePalette>().0
    }
}

pub(crate) fn init(cx: &mut App) {
    if let Err(error) = ThemeRegistry::global_mut(cx).load_themes_from_str(THEME_JSON) {
        log::error!("tema de Mergeon inválido: {error}");
    }
    let registry = ThemeRegistry::global(cx);
    let light = registry.themes().get(LIGHT_THEME).cloned();
    let dark = registry.themes().get(DARK_THEME).cloned();
    {
        let theme = Theme::global_mut(cx);
        if let Some(light) = light {
            theme.light_theme = light;
        }
        if let Some(dark) = dark {
            theme.dark_theme = dark;
        }
    }
    cx.set_global(ActivePalette(palette::LIGHT()));
    apply(ThemePreference::Light, cx);
}

/// Aplica la preferencia de tema a toda la app.
pub fn apply(preference: ThemePreference, cx: &mut App) {
    let mode = match preference {
        ThemePreference::Light => ThemeMode::Light,
        ThemePreference::Dark => ThemeMode::Dark,
        ThemePreference::System => {
            if matches!(
                cx.window_appearance(),
                WindowAppearance::Dark | WindowAppearance::VibrantDark
            ) {
                ThemeMode::Dark
            } else {
                ThemeMode::Light
            }
        }
    };
    cx.set_global(ActivePalette(if mode.is_dark() {
        palette::DARK()
    } else {
        palette::LIGHT()
    }));
    Theme::change(mode, None, cx);
    // Las barras de `::-webkit-scrollbar` del original se ven siempre que hay
    // desborde, no solo al pasar el cursor o al hacer scroll.
    Theme::set_scrollbar_mode(ScrollbarMode::Always, cx);
}

/// Color con la opacidad multiplicada (equivale a `bg-color/NN` de Tailwind).
pub fn with_alpha(color: impl Into<Hsla>, alpha: f32) -> Hsla {
    let mut color = color.into();
    color.a *= alpha;
    color
}
