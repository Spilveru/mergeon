//! Colores de la vista de commits: los que el modelo entrega como texto CSS
//! (`oklch(...)`, `var(--primary)`, `var(--lane-N)` de los carriles de Señal).

use gpui_kit::{App, Hsla, Rgba};
use mergeon_model::commit_view_utils::{ColorSpec, parse_css_color};
use mergeon_ui::ActivePaletteExt as _;

/// Color real de un texto CSS de los que produce `commit_view_utils`
/// (carriles del grafo, ramas, tokens del tema). Lo que no se entiende cae
/// en `--primary`, como el `var(--primary)` de respaldo del original.
pub(crate) fn css_color(color: &str, cx: &App) -> Hsla {
    let palette = cx.palette();
    match parse_css_color(color) {
        Some(ColorSpec::ThemeToken(token)) => match token.as_str() {
            lane if lane.starts_with("lane-") => lane["lane-".len()..]
                .parse::<usize>()
                .ok()
                .and_then(|index| palette.lanes.get(index))
                .map_or(palette.primary.into(), |color| (*color).into()),
            "warning" => palette.warning.into(),
            "success" => palette.success.into(),
            "destructive" => palette.destructive.into(),
            "foreground" => palette.foreground.into(),
            "muted-foreground" => palette.muted_foreground.into(),
            _ => palette.primary.into(),
        },
        Some(spec) => match spec.to_srgb() {
            Some([r, g, b]) => Rgba { r, g, b, a: 1. }.into(),
            None => palette.primary.into(),
        },
        None => palette.primary.into(),
    }
}
