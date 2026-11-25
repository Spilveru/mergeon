//! Tipografía y colores del código: lo que en la app Electron fijaban
//! `--font-mono` y los `<style scoped>` de los visores.

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{App, Font, FontStyle, FontWeight, Hsla, SharedString, TextRun, font};
use mergeon_ui::ActivePaletteExt as _;

use crate::highlight::{SyntaxColors, Token, TokenKind};

/// Familia monoespaciada de los visores: Geist Mono, embebida en mergeon-ui
/// (la `mono_font.family` del tema «Señal»).
pub fn code_font_family(cx: &App) -> SharedString {
    cx.theme().mono_font_family.clone()
}

pub(crate) fn code_font(cx: &App) -> Font {
    font(code_font_family(cx))
}

/// Colores de los visores de código para el tema activo.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct CodeColors {
    /// `color-mix(--foreground 96%, transparent)`.
    pub text: Hsla,
    pub foreground: Hsla,
    pub muted_foreground: Hsla,
    pub border: Hsla,
    /// `:hover` de una fila.
    pub hover: Hsla,
    /// Medianil fijo: opaco, porque el código pasa por debajo.
    pub gutter: Hsla,
    /// Números de línea.
    pub gutter_text: Hsla,
    /// Medianil de la vista partida (`--accent`).
    pub split_gutter: Hsla,
    pub addition: Hsla,
    pub addition_guide: Hsla,
    pub addition_mark: Hsla,
    /// Medianil de una línea añadida (`diff-add-gutter`) y sus números.
    pub addition_gutter: Hsla,
    pub addition_sign: Hsla,
    pub deletion: Hsla,
    pub deletion_guide: Hsla,
    pub deletion_mark: Hsla,
    pub deletion_gutter: Hsla,
    pub deletion_sign: Hsla,
    pub hunk: Hsla,
    /// Texto de la cabecera de hunk (`diff-hunk-text`).
    pub hunk_text: Hsla,
    /// Hueco de la vista partida.
    pub empty: Hsla,
    pub primary: Hsla,
    pub secondary: Hsla,
    pub secondary_foreground: Hsla,
    pub card: Hsla,
    /// Medianil de una línea elegida (tinta), números de sus altas y bajas
    /// encima y su marca naranja.
    pub selection: Hsla,
    pub selection_add: Hsla,
    pub selection_del: Hsla,
    pub selection_mark: Hsla,
    pub syntax: SyntaxColors,
}

impl CodeColors {
    pub(crate) fn new(cx: &App) -> Self {
        // Señal: el diff es la única gran zona de color (fondos de línea,
        // medianil algo más saturado, resaltado por palabra) y el resto es tinta.
        let palette = cx.palette();
        Self {
            text: palette.text_1.into(),
            foreground: palette.foreground.into(),
            muted_foreground: palette.muted_foreground.into(),
            border: palette.border.into(),
            hover: palette.bg_hover.into(),
            gutter: palette.card.into(),
            gutter_text: palette.text_3.into(),
            split_gutter: palette.card.into(),
            addition: palette.diff_add_bg.into(),
            addition_guide: palette.diff_add_sign.into(),
            addition_mark: palette.diff_add_word.into(),
            addition_gutter: palette.diff_add_gutter.into(),
            addition_sign: palette.diff_add_sign.into(),
            deletion: palette.diff_del_bg.into(),
            deletion_guide: palette.diff_del_sign.into(),
            deletion_mark: palette.diff_del_word.into(),
            deletion_gutter: palette.diff_del_gutter.into(),
            deletion_sign: palette.diff_del_sign.into(),
            hunk: palette.diff_hunk_bg.into(),
            hunk_text: palette.diff_hunk_text.into(),
            empty: palette.bg_inset.into(),
            primary: palette.primary.into(),
            secondary: palette.secondary.into(),
            secondary_foreground: palette.secondary_foreground.into(),
            card: palette.card.into(),
            selection: palette.sel_bg.into(),
            selection_add: palette.sel_add.into(),
            selection_del: palette.sel_del.into(),
            selection_mark: palette.primary.into(),
            syntax: SyntaxColors::new(cx),
        }
    }
}

fn run(len: usize, font: Font, color: Hsla) -> TextRun {
    TextRun {
        len,
        font,
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    }
}

/// Un único tramo de color para todo el texto.
pub(crate) fn plain_runs(len: usize, font: &Font, color: Hsla) -> Vec<TextRun> {
    if len == 0 {
        return Vec::new();
    }
    vec![run(len, font.clone(), color)]
}

/// Los tramos de color de una línea: el texto base y, encima, los trozos de
/// sintaxis (`tokens` ordenados y sin solaparse, en offsets de `len`).
pub(crate) fn syntax_runs(
    len: usize,
    tokens: &[Token],
    font: &Font,
    base: Hsla,
    colors: &SyntaxColors,
) -> Vec<TextRun> {
    let mut runs = Vec::with_capacity(tokens.len() * 2 + 1);
    let mut cursor = 0;
    for (range, kind) in tokens {
        if range.start < cursor || range.end > len || range.is_empty() {
            continue;
        }
        if range.start > cursor {
            runs.push(run(range.start - cursor, font.clone(), base));
        }
        let mut token_font = font.clone();
        if kind.is_italic() {
            token_font.style = FontStyle::Italic;
        }
        // Señal: sintaxis monocroma, las palabras clave en 600.
        if *kind == TokenKind::Keyword {
            token_font.weight = FontWeight::SEMIBOLD;
        }
        runs.push(run(range.len(), token_font, colors.color(*kind)));
        cursor = range.end;
    }
    if cursor < len {
        runs.push(run(len - cursor, font.clone(), base));
    }
    runs
}

pub(crate) fn italic(font: &Font) -> Font {
    Font {
        style: FontStyle::Italic,
        ..font.clone()
    }
}

pub(crate) fn medium(font: &Font) -> Font {
    Font {
        weight: FontWeight::MEDIUM,
        ..font.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::highlight::TokenKind;
    use gpui_kit::rgb;

    #[test]
    fn colour_runs_cover_the_whole_line() {
        let colors = SyntaxColors {
            keyword: rgb(0x000001).into(),
            string: rgb(0x000002).into(),
            number: rgb(0x000003).into(),
            title: rgb(0x000004).into(),
            variable: rgb(0x000005).into(),
            comment: rgb(0x000006).into(),
            deletion: rgb(0x000007).into(),
            addition: rgb(0x000008).into(),
        };
        let base: Hsla = rgb(0xffffff).into();
        let font = font("monospace");
        let tokens = [(0..5, TokenKind::Keyword), (12..15, TokenKind::Comment)];

        let runs = syntax_runs(15, &tokens, &font, base, &colors);

        assert_eq!(
            runs.iter().map(|run| run.len).collect::<Vec<_>>(),
            [5, 7, 3]
        );
        assert_eq!(runs[0].color, colors.keyword);
        assert_eq!(runs[1].color, base);
        assert_eq!(runs[2].font.style, FontStyle::Italic);
        assert!(syntax_runs(0, &[], &font, base, &colors).is_empty());
        assert_eq!(plain_runs(4, &font, base).len(), 1);
    }
}
