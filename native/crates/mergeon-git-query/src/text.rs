//! Utilidades de texto con la semántica de JavaScript de la que depende el
//! código original (`trim`, `parseInt`, orden de claves de un objeto).

/// Espacios que quita `String.prototype.trim` (WhiteSpace + LineTerminator).
pub(crate) fn is_js_whitespace(character: char) -> bool {
    matches!(
        character,
        '\u{9}'..='\u{d}'
            | ' '
            | '\u{a0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200a}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202f}'
            | '\u{205f}'
            | '\u{3000}'
            | '\u{feff}'
    )
}

pub(crate) fn js_trim(text: &str) -> &str {
    text.trim_matches(is_js_whitespace)
}

/// `parseInt(text, 10)`: espacios, signo y dígitos iniciales. `None` es NaN.
pub(crate) fn parse_int(text: &str) -> Option<i64> {
    let text = text.trim_start_matches(is_js_whitespace);
    let (negative, digits) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text.strip_prefix('+').unwrap_or(text)),
    };
    let length = digits.bytes().take_while(u8::is_ascii_digit).count();
    if length == 0 {
        return None;
    }
    let value = digits[..length].parse::<i64>().unwrap_or(i64::MAX);
    Some(if negative { -value } else { value })
}

/// `parseInt(text, 10) || 0` para contadores que no pueden ser negativos.
pub(crate) fn parse_count(text: &str) -> u64 {
    parse_int(text)
        .and_then(|value| u64::try_from(value).ok())
        .unwrap_or(0)
}

/// Primeros `units` code units UTF-16 de `text` (`String.prototype.substring`),
/// sin partir un carácter.
pub(crate) fn utf16_prefix(text: &str, units: usize) -> &str {
    let mut seen = 0;
    for (offset, character) in text.char_indices() {
        seen += character.len_utf16();
        if seen > units {
            return &text[..offset];
        }
    }
    text
}

fn array_index(key: &str) -> Option<u32> {
    let canonical = key == "0" || (!key.starts_with('0') && !key.is_empty());
    if !canonical || !key.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    key.parse::<u32>().ok().filter(|index| *index < u32::MAX)
}

/// Deja `items` en el orden en que JavaScript enumera las claves de un objeto:
/// primero las que son índices enteros, en orden numérico, y después el resto
/// en orden de inserción. simple-git acumula ramas y remotos en objetos planos.
pub(crate) fn sort_like_object_keys<T>(items: &mut [T], key: impl Fn(&T) -> &str) {
    items.sort_by_key(|item| match array_index(key(item)) {
        Some(index) => (0, index),
        None => (1, 0),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_like_javascript() {
        assert_eq!(js_trim(" \t\r\n x y \u{feff}\u{a0}"), "x y");
        // Los separadores de registro no son espacio: no se pierden campos vacíos.
        assert_eq!(js_trim("\u{1f}a\u{1f}\u{1e}"), "\u{1f}a\u{1f}\u{1e}");
    }

    #[test]
    fn parses_integers_like_parse_int() {
        assert_eq!(parse_int("42"), Some(42));
        assert_eq!(parse_int("  7 files"), Some(7));
        assert_eq!(parse_int("-3"), Some(-3));
        assert_eq!(parse_int("-"), None);
        assert_eq!(parse_int(""), None);
        assert_eq!(parse_count("-"), 0);
        assert_eq!(parse_count("12\n"), 12);
    }

    #[test]
    fn takes_a_utf16_prefix() {
        assert_eq!(utf16_prefix("abcdefghij", 7), "abcdefg");
        assert_eq!(utf16_prefix("abc", 7), "abc");
        assert_eq!(utf16_prefix("a😀b", 2), "a");
    }

    #[test]
    fn orders_keys_like_a_javascript_object() {
        let mut names = vec!["main", "10", "feature", "2", "007"];
        sort_like_object_keys(&mut names, |name| name);
        assert_eq!(names, ["2", "10", "main", "feature", "007"]);
    }
}
