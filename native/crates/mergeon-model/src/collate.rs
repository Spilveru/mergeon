//! Orden de nombres para humanos.
//!
//! El original ordena con `String.prototype.localeCompare`, que usa la
//! colación de ICU del sistema. Aquí no hay ICU: se aproxima su orden raíz para
//! lo que aparece en nombres de archivo. Espacio, luego puntuación, luego
//! dígitos y al final letras; las letras sin distinguir mayúsculas ni acentos,
//! que solo desempatan (primero sin acento, primero minúscula).
//!
//! Es la misma aproximación que usa `mergeon-git-query` para el árbol de un
//! commit; se repite aquí porque aquel módulo es privado de su crate.

use std::cmp::Ordering;

// Orden de la puntuación ASCII en la colación raíz de CLDR.
const PUNCTUATION: &str = "_-,;:!?.'\"()[]{}@*/\\&#%`^+<=>|~$";

const GROUP_SPACE: u8 = 0;
const GROUP_PUNCTUATION: u8 = 1;
const GROUP_DIGIT: u8 = 2;
const GROUP_LATIN: u8 = 3;
const GROUP_OTHER_LETTER: u8 = 4;

/// Letra base y marca diacrítica (en el orden secundario de la colación raíz).
fn unaccent(character: char) -> Option<(char, u8)> {
    Some(match character {
        'á' => ('a', 1),
        'à' => ('a', 2),
        'â' => ('a', 3),
        'å' => ('a', 4),
        'ä' => ('a', 5),
        'ã' => ('a', 6),
        'ç' => ('c', 7),
        'é' => ('e', 1),
        'è' => ('e', 2),
        'ê' => ('e', 3),
        'ë' => ('e', 5),
        'í' => ('i', 1),
        'ì' => ('i', 2),
        'î' => ('i', 3),
        'ï' => ('i', 5),
        'ñ' => ('n', 6),
        'ó' => ('o', 1),
        'ò' => ('o', 2),
        'ô' => ('o', 3),
        'ö' => ('o', 5),
        'õ' => ('o', 6),
        'ú' => ('u', 1),
        'ù' => ('u', 2),
        'û' => ('u', 3),
        'ü' => ('u', 5),
        'ý' => ('y', 1),
        'ÿ' => ('y', 5),
        _ => return None,
    })
}

#[derive(Default, PartialEq, Eq, PartialOrd, Ord)]
struct SortKey {
    primary: Vec<(u8, u32)>,
    accents: Vec<u8>,
    cases: Vec<bool>,
}

fn sort_key(text: &str) -> SortKey {
    let mut key = SortKey::default();

    for character in text.chars() {
        let lower = character.to_lowercase().next().unwrap_or(character);
        let (base, accent) = unaccent(lower).unwrap_or((lower, 0));

        let primary = if base.is_ascii_lowercase() {
            (GROUP_LATIN, base as u32)
        } else if base.is_ascii_digit() {
            (GROUP_DIGIT, base as u32)
        } else if let Some(position) = PUNCTUATION.find(base) {
            (GROUP_PUNCTUATION, position as u32)
        } else if base.is_ascii() {
            (GROUP_SPACE, base as u32)
        } else if base.is_alphabetic() {
            (GROUP_OTHER_LETTER, base as u32)
        } else {
            (GROUP_PUNCTUATION, 0x100 + base as u32)
        };

        key.primary.push(primary);
        key.accents.push(accent);
        key.cases.push(character != lower);
    }

    key
}

/// Aproximación de `left.localeCompare(right)`.
pub fn locale_compare(left: &str, right: &str) -> Ordering {
    sort_key(left).cmp(&sort_key(right))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sorted(mut names: Vec<&str>) -> Vec<&str> {
        names.sort_by(|left, right| locale_compare(left, right));
        names
    }

    // Orden de referencia sacado de `localeCompare` en el Electron de la app
    // (ICU 78, locale en-US).
    #[test]
    fn matches_the_order_icu_gives_for_file_names() {
        assert_eq!(
            sorted(vec![
                "utils", "alpha.js", "zeta.js", "Beta.js", "_x", ".git", "10.txt", "2.txt", "ñu",
                "nu", "a-b", "a_b", "a b", "ab", "A", "a", "á", "Z", "#x", "~x", "$x", "(x", "+x",
                "=x", "é",
            ]),
            [
                "_x", ".git", "(x", "#x", "+x", "=x", "~x", "$x", "10.txt", "2.txt", "a", "A", "á",
                "a b", "a_b", "a-b", "ab", "alpha.js", "Beta.js", "é", "nu", "ñu", "utils", "Z",
                "zeta.js",
            ]
        );
    }

    #[test]
    fn accents_and_case_only_break_ties() {
        assert_eq!(
            sorted(vec!["zeta", "ano", "Año", "año", "anz", "ANO"]),
            ["ano", "ANO", "año", "Año", "anz", "zeta"]
        );
        assert_eq!(locale_compare("año", "año"), Ordering::Equal);
    }

    #[test]
    fn a_prefix_sorts_before_the_longer_name() {
        assert_eq!(sorted(vec!["app.js", "app"]), ["app", "app.js"]);
    }
}
