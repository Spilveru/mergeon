//! Monograma del autor (Señal `.mg`): un cuadrito con sus iniciales en mono,
//! fondo neutro y borde fino. Lo usan la columna Author del historial (16px) y
//! la cabecera del commit (22px).

use gpui_kit::prelude::*;
use gpui_kit::{App, Div, FontWeight, Hsla, div, px};
use mergeon_ui::ActivePaletteExt as _;
use mergeon_ui::theme::with_alpha;
use mergeon_view_diff::code_font_family;

/// Dos letras para un nombre: la inicial de las dos primeras palabras
/// («Josefa Contreras» → `JC`, «Diego F.» → `DF`) o, con una sola palabra, sus
/// dos primeras letras (`diego` → `DI`). Sin letras, `?`.
pub fn initials(name: &str) -> String {
    let words: Vec<&str> = name
        .split(|c: char| c.is_whitespace() || matches!(c, '.' | '-' | '_' | '@'))
        .filter(|word| word.chars().any(char::is_alphanumeric))
        .collect();
    let letters: String = match words.as_slice() {
        [] => return "?".to_string(),
        [word] => word
            .chars()
            .filter(|c| c.is_alphanumeric())
            .take(2)
            .collect(),
        [first, second, ..] => [first, second]
            .iter()
            .filter_map(|word| word.chars().find(|c| c.is_alphanumeric()))
            .collect(),
    };
    letters.to_uppercase()
}

/// El cuadrito de `size` px con las iniciales de `name`. `on_ink`: sobre la
/// fila seleccionada (tinta), con el blanco translúcido de Señal.
pub fn monogram(name: &str, size: f32, font_size: f32, on_ink: bool, cx: &App) -> Div {
    let palette = *cx.palette();
    let (bg, border, fg): (Hsla, Hsla, Hsla) = if on_ink {
        (
            with_alpha(palette.sel_fg, 0.14),
            with_alpha(palette.sel_fg, 0.22),
            palette.sel_fg.into(),
        )
    } else {
        (
            palette.bg_pressed.into(),
            palette.border_hair.into(),
            palette.text_2.into(),
        )
    };
    div()
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .size(px(size))
        .rounded(px(2.))
        .border_1()
        .border_color(border)
        .bg(bg)
        .text_color(fg)
        .font_family(code_font_family(cx))
        .text_size(px(font_size))
        .line_height(px(font_size))
        .font_weight(FontWeight::SEMIBOLD)
        .child(initials(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn takes_two_letters_from_a_name() {
        assert_eq!(initials("Josefa Contreras"), "JC");
        assert_eq!(initials("Diego F."), "DF");
        assert_eq!(initials("ana maría torres"), "AM");
        assert_eq!(initials("diego"), "DI");
        assert_eq!(initials("j"), "J");
        assert_eq!(initials("dependabot[bot]"), "DE");
        assert_eq!(initials("  "), "?");
        assert_eq!(initials(""), "?");
    }
}
