//! Buscar texto en las líneas de un visor (diff o archivo): la consulta con
//! sus opciones (mayúsculas, expresión regular) y las coincidencias por
//! línea. Sin interfaz: el visor lo llama fuera del render, en segundo plano
//! si el documento es grande.

use std::ops::Range;

use regex::{Regex, RegexBuilder};

/// Más coincidencias que esto no se cuentan ("10000+"): resaltarlas todas
/// no ayuda a nadie y una regex como `.` en un archivo enorme daría millones.
pub const MAX_MATCHES: usize = 10_000;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct SearchQuery {
    pub text: String,
    /// Distingue mayúsculas de minúsculas ("Aa").
    pub case_sensitive: bool,
    /// `text` es una expresión regular (".*").
    pub regex: bool,
}

impl SearchQuery {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Self::default()
        }
    }

    pub fn case_sensitive(mut self, on: bool) -> Self {
        self.case_sensitive = on;
        self
    }

    pub fn regex(mut self, on: bool) -> Self {
        self.regex = on;
        self
    }

    /// La expresión que busca: `Ok(None)` con la consulta vacía, `Err` con
    /// una regex que no compila.
    pub fn compile(&self) -> Result<Option<Regex>, String> {
        if self.text.is_empty() {
            return Ok(None);
        }
        let pattern = if self.regex {
            self.text.clone()
        } else {
            regex::escape(&self.text)
        };
        RegexBuilder::new(&pattern)
            .case_insensitive(!self.case_sensitive)
            .size_limit(1 << 20)
            .build()
            .map(Some)
            .map_err(|error| error.to_string())
    }
}

/// Una coincidencia: fila del visor, columna (en la vista partida, 0 es el
/// lado viejo y 1 el nuevo) y bytes de esa línea.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextMatch {
    pub row: usize,
    pub column: usize,
    pub range: Range<usize>,
}

/// Todas las coincidencias, en orden de lectura.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SearchResults {
    pub matches: Vec<TextMatch>,
    /// Había más de [`MAX_MATCHES`].
    pub truncated: bool,
}

impl SearchResults {
    /// Las coincidencias de una fila (las del rango que ocupa en `matches`).
    pub fn in_row(&self, row: usize) -> Range<usize> {
        let start = self.matches.partition_point(|found| found.row < row);
        let end = start + self.matches[start..].partition_point(|found| found.row == row);
        start..end
    }

    /// La primera coincidencia en la fila `row` o después (para empezar
    /// cerca de lo que se ve). `None` si no hay ninguna.
    pub fn first_from(&self, row: usize) -> Option<usize> {
        if self.matches.is_empty() {
            return None;
        }
        let index = self.matches.partition_point(|found| found.row < row);
        Some(if index < self.matches.len() { index } else { 0 })
    }

    /// "3 / 17", "10000+" si se cortó.
    pub fn counter(&self, current: Option<usize>) -> String {
        let total = if self.truncated {
            format!("{}+", self.matches.len())
        } else {
            self.matches.len().to_string()
        };
        match current {
            Some(current) if !self.matches.is_empty() => format!("{} / {total}", current + 1),
            _ => format!("0 / {total}"),
        }
    }
}

/// Busca `regex` en cada línea: `(fila, columna, texto)`. Las coincidencias
/// vacías (`^`, `a*`) no cuentan.
pub fn search_lines<'a>(
    lines: impl IntoIterator<Item = (usize, usize, &'a str)>,
    regex: &Regex,
) -> SearchResults {
    let mut results = SearchResults::default();
    for (row, column, text) in lines {
        for found in regex.find_iter(text) {
            if found.start() == found.end() {
                continue;
            }
            if results.matches.len() == MAX_MATCHES {
                results.truncated = true;
                return results;
            }
            results.matches.push(TextMatch {
                row,
                column,
                range: found.range(),
            });
        }
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    fn search(query: SearchQuery, lines: &[&str]) -> SearchResults {
        let regex = query.compile().unwrap().unwrap();
        search_lines(
            lines.iter().enumerate().map(|(row, line)| (row, 0, *line)),
            &regex,
        )
    }

    #[test]
    fn finds_plain_text_ignoring_case_by_default() {
        let results = search(SearchQuery::new("foo"), &["Foo foo", "bar", "xFOOx"]);
        let found: Vec<(usize, Range<usize>)> = results
            .matches
            .iter()
            .map(|found| (found.row, found.range.clone()))
            .collect();
        assert_eq!(found, [(0, 0..3), (0, 4..7), (2, 1..4)]);
        assert_eq!(results.counter(Some(1)), "2 / 3");
    }

    #[test]
    fn case_sensitive_and_regex_options() {
        assert_eq!(
            search(SearchQuery::new("Foo").case_sensitive(true), &["Foo foo"])
                .matches
                .len(),
            1
        );
        // Sin regex, los metacaracteres son texto.
        assert_eq!(
            search(SearchQuery::new("a.c"), &["abc a.c"]).matches.len(),
            1
        );
        assert_eq!(
            search(SearchQuery::new("a.c").regex(true), &["abc a.c"])
                .matches
                .len(),
            2
        );
        assert!(SearchQuery::new("(").regex(true).compile().is_err());
        assert!(matches!(SearchQuery::new("").compile(), Ok(None)));
    }

    #[test]
    fn skips_empty_matches_and_caps_the_count() {
        assert!(
            search(SearchQuery::new("x*").regex(true), &["abc"])
                .matches
                .is_empty()
        );
        let line = "a".repeat(MAX_MATCHES + 5);
        let results = search(SearchQuery::new("a"), &[&line]);
        assert!(results.truncated);
        assert_eq!(results.matches.len(), MAX_MATCHES);
        assert_eq!(results.counter(Some(0)), format!("1 / {MAX_MATCHES}+"));
    }

    #[test]
    fn finds_the_matches_of_a_row() {
        let results = search(SearchQuery::new("x"), &["x", "", "xx", "x"]);
        assert_eq!(results.in_row(0), 0..1);
        assert_eq!(results.in_row(1), 1..1);
        assert_eq!(results.in_row(2), 1..3);
        assert_eq!(results.first_from(1), Some(1));
        assert_eq!(results.first_from(9), Some(0));
        assert_eq!(SearchResults::default().counter(None), "0 / 0");
    }
}
