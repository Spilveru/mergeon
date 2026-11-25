//! Semántica de cadenas de JavaScript que el original da por sentada.
//!
//! `String.prototype.trim`, `\s` y `.length` no coinciden con sus parientes de
//! Rust: el conjunto de espacios es otro y las longitudes se cuentan en
//! unidades UTF-16.

/// `\s` de una expresión regular de JavaScript (y lo que recorta `trim`).
pub(crate) fn is_js_whitespace(character: char) -> bool {
    matches!(
        character,
        '\t' | '\n' | '\u{000b}' | '\u{000c}' | '\r' | ' ' | '\u{00a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}'
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

pub(crate) fn js_trim_end(text: &str) -> &str {
    text.trim_end_matches(is_js_whitespace)
}

/// `text.split(/\s+/).filter(Boolean)`.
pub(crate) fn js_split_whitespace(text: &str) -> impl Iterator<Item = &str> {
    text.split(is_js_whitespace).filter(|part| !part.is_empty())
}

/// `text.replace(/\s+/g, ' ')`.
pub(crate) fn collapse_js_whitespace(text: &str) -> String {
    let mut collapsed = String::with_capacity(text.len());
    let mut in_whitespace = false;
    for character in text.chars() {
        if is_js_whitespace(character) {
            if !in_whitespace {
                collapsed.push(' ');
            }
            in_whitespace = true;
        } else {
            collapsed.push(character);
            in_whitespace = false;
        }
    }
    collapsed
}

/// `text.length`.
pub(crate) fn utf16_len(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

/// `text.slice(0, units)`, sin partir un carácter fuera del plano básico (JS
/// dejaría medio par subrogado; aquí ese carácter queda fuera).
pub(crate) fn utf16_prefix(text: &str, units: usize) -> &str {
    let mut used = 0;
    for (offset, character) in text.char_indices() {
        used += character.len_utf16();
        if used > units {
            return &text[..offset];
        }
    }
    text
}

/// `Boolean(value)` para un valor leído de JSON.
pub(crate) fn js_truthy(value: &serde_json::Value) -> bool {
    use serde_json::Value;

    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number
            .as_f64()
            .is_some_and(|number| number != 0.0 && !number.is_nan()),
        Value::String(text) => !text.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// `Math.round(value)`: los empates van hacia arriba.
pub(crate) fn js_round(value: f64) -> f64 {
    (value + 0.5).floor()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_what_javascript_trims() {
        assert_eq!(js_trim("\u{feff}\u{a0} a b \t\n"), "a b");
        assert_eq!(js_trim_end("a \u{2028}"), "a");
        // U+0085 es espacio para Rust, no para JavaScript.
        assert_eq!(js_trim("\u{85}a"), "\u{85}a");
    }

    #[test]
    fn collapses_runs_of_whitespace() {
        assert_eq!(collapse_js_whitespace("a \n\t b\u{a0}c"), "a b c");
        assert_eq!(
            js_split_whitespace("  a  b\tc ").collect::<Vec<_>>(),
            ["a", "b", "c"]
        );
    }

    #[test]
    fn follows_javascript_truthiness_and_rounding() {
        use serde_json::json;

        for falsy in [json!(null), json!(false), json!(0), json!("")] {
            assert!(!js_truthy(&falsy), "{falsy}");
        }
        for truthy in [json!(true), json!(1), json!("0"), json!([]), json!({})] {
            assert!(js_truthy(&truthy), "{truthy}");
        }
        assert_eq!(js_round(2.5), 3.0);
        assert_eq!(js_round(-2.5), -2.0);
        assert_eq!(js_round(2.4), 2.0);
    }

    #[test]
    fn measures_in_utf16_units() {
        assert_eq!(utf16_len("añ😀"), 4);
        assert_eq!(utf16_prefix("añ😀b", 3), "añ");
        assert_eq!(utf16_prefix("añ😀b", 4), "añ😀");
        assert_eq!(utf16_prefix("abc", 10), "abc");
    }
}
