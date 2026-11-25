//! Texto de una línea tal como se pinta: tabuladores expandidos y cortes de
//! línea de la vista partida.
//!
//! GPUI no conoce las paradas de tabulación, así que una línea con tabuladores
//! se pinta con espacios hasta la siguiente parada (cada 8 columnas, el
//! `tab-size` por defecto del navegador). Los rangos del modelo —lo que cambió
//! dentro de la línea, los trozos de sintaxis— vienen en offsets del texto
//! original, y lo que se copia tiene que ser el texto original: de ahí las
//! conversiones en los dos sentidos.

use std::ops::Range;

/// `tab-size` por defecto de Chromium.
pub(crate) const TAB_SIZE: usize = 8;

/// El texto con los tabuladores convertidos en espacios, o `None` si no tiene
/// ninguno y vale tal cual.
pub(crate) fn expand_tabs(source: &str) -> Option<String> {
    if !source.contains('\t') {
        return None;
    }

    let mut display = String::with_capacity(source.len() + TAB_SIZE);
    let mut column = 0;
    for character in source.chars() {
        if character == '\t' {
            let width = TAB_SIZE - column % TAB_SIZE;
            display.extend(std::iter::repeat_n(' ', width));
            column += width;
        } else {
            display.push(character);
            column += 1;
        }
    }
    Some(display)
}

/// Columnas que ocupa el texto una vez expandidos los tabuladores.
pub(crate) fn display_columns(source: &str) -> usize {
    let mut column = 0;
    for character in source.chars() {
        column += if character == '\t' {
            TAB_SIZE - column % TAB_SIZE
        } else {
            1
        };
    }
    column
}

/// Offset en el texto pintado que corresponde a un offset del original.
pub(crate) fn display_offset(source: &str, offset: usize) -> usize {
    if !source.contains('\t') {
        return offset.min(source.len());
    }

    let mut column = 0;
    let mut display = 0;
    for (at, character) in source.char_indices() {
        if at >= offset {
            return display;
        }
        if character == '\t' {
            let width = TAB_SIZE - column % TAB_SIZE;
            column += width;
            display += width;
        } else {
            column += 1;
            display += character.len_utf8();
        }
    }
    display
}

/// Trozo del original que corresponde a un rango del texto pintado. Un
/// tabulador seleccionado a medias se copia entero.
pub(crate) fn source_range(source: &str, display: Range<usize>) -> Range<usize> {
    if !source.contains('\t') {
        let end = display.end.min(source.len());
        return display.start.min(end)..end;
    }

    let mut column = 0;
    let mut shown = 0;
    let mut start = None;
    let mut end = source.len();
    for (at, character) in source.char_indices() {
        let width = if character == '\t' {
            TAB_SIZE - column % TAB_SIZE
        } else {
            character.len_utf8()
        };
        column += if character == '\t' { width } else { 1 };

        if start.is_none() && shown + width > display.start {
            start = Some(at);
        }
        if shown >= display.end {
            end = at;
            break;
        }
        shown += width;
    }
    let start = start.unwrap_or(source.len()).min(end);
    start..end
}

/// Dónde se parte una línea que no cabe en `wrap_width`, como
/// `white-space: pre-wrap; overflow-wrap: break-word`: se corta tras un
/// espacio si lo hay, y por donde toque si una palabra sola no cabe. Los
/// espacios del final de un renglón se quedan colgando en él.
///
/// Devuelve los offsets (en bytes de `text`) donde empieza cada renglón a
/// partir del segundo.
pub(crate) fn wrap_offsets(
    text: &str,
    wrap_width: f32,
    mut char_width: impl FnMut(char) -> f32,
) -> Vec<usize> {
    let mut breaks = Vec::new();
    let mut line_start = 0;
    let mut width = 0.;
    // Última oportunidad de corte del renglón: offset y ancho acumulado hasta ella.
    let mut candidate: Option<(usize, f32)> = None;

    for (at, character) in text.char_indices() {
        let advance = char_width(character);
        if character == ' ' {
            width += advance;
            candidate = Some((at + 1, width));
            continue;
        }

        if width + advance > wrap_width && at > line_start {
            match candidate {
                Some((offset, width_before)) if offset > line_start => {
                    breaks.push(offset);
                    line_start = offset;
                    width -= width_before;
                }
                _ => {
                    breaks.push(at);
                    line_start = at;
                    width = 0.;
                }
            }
            candidate = None;

            // La palabra arrastrada al renglón nuevo puede no caber tampoco.
            if width + advance > wrap_width && at > line_start {
                breaks.push(at);
                line_start = at;
                width = 0.;
            }
        }
        width += advance;
    }

    breaks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wrap(text: &str, columns: usize) -> Vec<&str> {
        let breaks = wrap_offsets(text, columns as f32, |_| 1.);
        let mut lines = Vec::new();
        let mut start = 0;
        for offset in breaks {
            lines.push(&text[start..offset]);
            start = offset;
        }
        lines.push(&text[start..]);
        lines
    }

    #[test]
    fn expands_tabs_to_the_next_stop() {
        assert_eq!(expand_tabs("sin tabuladores"), None);
        assert_eq!(expand_tabs("\tx").as_deref(), Some("        x"));
        assert_eq!(expand_tabs("ab\tx").as_deref(), Some("ab      x"));
        assert_eq!(
            expand_tabs("12345678\tx").as_deref(),
            Some("12345678        x")
        );
        assert_eq!(display_columns("ab\tx"), 9);
        assert_eq!(display_columns("ñandú"), 5);
    }

    #[test]
    fn maps_offsets_between_the_source_and_what_is_painted() {
        let source = "\tfoo\tbar";
        let display = expand_tabs(source).unwrap();
        assert_eq!(display, "        foo     bar");

        assert_eq!(display_offset(source, 0), 0);
        assert_eq!(display_offset(source, 1), 8);
        assert_eq!(display_offset(source, 4), 11);
        assert_eq!(display_offset(source, 5), 16);
        assert_eq!(display_offset(source, source.len()), display.len());
        assert_eq!(display_offset("ñu", 2), 2);

        assert_eq!(&source[source_range(source, 8..11)], "foo");
        assert_eq!(&source[source_range(source, 0..display.len())], source);
        // Medio tabulador seleccionado se copia entero.
        assert_eq!(&source[source_range(source, 4..11)], "\tfoo");
        assert_eq!(&source[source_range(source, 9..19)], "oo\tbar");
        assert_eq!(&source[source_range(source, 19..19)], "");
        assert_eq!(&"plano"[source_range("plano", 1..99)], "lano");
    }

    #[test]
    fn keeps_a_line_that_fits_whole() {
        assert_eq!(wrap("const a = 1", 20), ["const a = 1"]);
        assert_eq!(wrap("", 20), [""]);
    }

    #[test]
    fn breaks_after_a_space() {
        assert_eq!(wrap("const total = 1", 8), ["const ", "total = ", "1"]);
    }

    // Un espacio al final de renglón cuelga: no empuja la palabra al siguiente.
    #[test]
    fn lets_trailing_spaces_hang() {
        assert_eq!(wrap("abcd efgh", 4), ["abcd ", "efgh"]);
    }

    #[test]
    fn breaks_a_word_that_does_not_fit_on_its_own() {
        assert_eq!(wrap("abcdefghij", 4), ["abcd", "efgh", "ij"]);
        assert_eq!(wrap("ab cdefghijkl", 5), ["ab ", "cdefg", "hijkl"]);
    }

    #[test]
    fn never_produces_an_empty_piece() {
        for text in [
            "a",
            "aaaa aaaa",
            "    indent(long_argument_name)",
            "ñañañaña ñu",
        ] {
            for columns in 1..12 {
                let lines = wrap(text, columns);
                assert_eq!(lines.concat(), text);
                assert!(lines.iter().all(|line| !line.is_empty()), "{lines:?}");
            }
        }
    }
}
