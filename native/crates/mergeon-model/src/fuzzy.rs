//! Búsqueda difusa de la paleta de comandos: las letras de la consulta tienen
//! que aparecer en orden dentro del candidato, no seguidas. Gana el
//! alineamiento con más letras seguidas, al principio de palabra (tras `/`,
//! `-`, `_`, `.`, espacio o en un cambio a mayúscula) y dentro del nombre del
//! archivo, como en VS Code y Zed.
//!
//! Los espacios de la consulta separan términos: cada uno tiene que aparecer
//! (en cualquier orden) y la puntuación es la suma.
//!
//! ```
//! use mergeon_model::fuzzy::fuzzy_match;
//! let found = fuzzy_match("swb", "Switch to branch…").unwrap();
//! assert_eq!(found.positions, [0, 1, 10]);
//! assert!(fuzzy_match("xyz", "Fetch").is_none());
//! ```

/// Puntos por letra encontrada.
const SCORE_MATCH: i32 = 16;
/// La letra sigue a la anterior encontrada.
const BONUS_CONSECUTIVE: i32 = 18;
/// Primera letra del candidato.
const BONUS_START: i32 = 20;
/// Tras un separador (`/`, `-`, espacio…).
const BONUS_BOUNDARY: i32 = 16;
/// Mayúscula tras minúscula (`camelCase`).
const BONUS_CAMEL: i32 = 12;
/// Dentro del último tramo de una ruta (el nombre del archivo).
const BONUS_BASENAME: i32 = 6;
/// Misma caja que en la consulta.
const BONUS_CASE: i32 = 1;
/// Penalización por abrir un hueco entre dos letras y por cada letra más.
const PENALTY_GAP_START: i32 = 6;
const PENALTY_GAP_EXTENSION: i32 = 1;
/// Más allá de esto no se mira el candidato (rutas absurdamente largas).
const MAX_CANDIDATE_CHARS: usize = 512;

/// Un candidato que casa con la consulta.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuzzyMatch {
    pub score: i32,
    /// Offsets en bytes (dentro del candidato) de las letras encontradas, en
    /// orden y sin repetir.
    pub positions: Vec<usize>,
}

fn is_separator(character: char) -> bool {
    matches!(
        character,
        '/' | '\\' | '-' | '_' | '.' | ' ' | ':' | '@' | '#' | '(' | '[' | '…'
    )
}

fn same_letter(query: char, candidate: char) -> bool {
    query == candidate || candidate.to_lowercase().eq(query.to_lowercase())
}

/// Busca `query` (sin espacios) dentro de `candidate`. `chars` son los
/// caracteres del candidato con su offset en bytes.
fn match_term(term: &[char], chars: &[(usize, char)], basename_start: usize) -> Option<FuzzyMatch> {
    let n = chars.len();
    let m = term.len();
    if m == 0 {
        return Some(FuzzyMatch {
            score: 0,
            positions: Vec::new(),
        });
    }
    if m > n {
        return None;
    }
    // Filtro rápido: ¿están todas, en orden?
    let mut next = 0;
    for &(_, character) in chars {
        if next < m && same_letter(term[next], character) {
            next += 1;
        }
    }
    if next < m {
        return None;
    }

    let letter_score = |j: usize, query: char| -> i32 {
        let (_, character) = chars[j];
        let mut score = SCORE_MATCH;
        if j == 0 {
            score += BONUS_START;
        } else {
            let previous = chars[j - 1].1;
            if is_separator(previous) {
                score += BONUS_BOUNDARY;
            } else if previous.is_lowercase() && character.is_uppercase() {
                score += BONUS_CAMEL;
            }
        }
        if j >= basename_start {
            score += BONUS_BASENAME;
        }
        if character == query {
            score += BONUS_CASE;
        }
        score
    };

    // `best[i][j]`: la mejor puntuación con la letra `i` de la consulta en
    // la posición `j`; `from[i][j]`, dónde quedó la letra anterior.
    const NONE: i32 = i32::MIN / 2;
    let mut best = vec![NONE; m * n];
    let mut from = vec![usize::MAX; m * n];
    for j in 0..n {
        if same_letter(term[0], chars[j].1) {
            best[j] = letter_score(j, term[0]);
        }
    }
    for i in 1..m {
        let (previous_row, row) = best.split_at_mut(i * n);
        let previous_row = &previous_row[(i - 1) * n..];
        let row = &mut row[..n];
        // Mejor `best[i-1][k] + EXT * k` con `k <= j - 2` (con hueco).
        let mut gap_best = NONE;
        let mut gap_from = usize::MAX;
        for j in 1..n {
            if j >= 2 {
                let k = j - 2;
                if previous_row[k] > NONE {
                    let value = previous_row[k] + PENALTY_GAP_EXTENSION * k as i32;
                    if value > gap_best {
                        gap_best = value;
                        gap_from = k;
                    }
                }
            }
            if !same_letter(term[i], chars[j].1) {
                continue;
            }
            let here = letter_score(j, term[i]);
            let mut candidate = NONE;
            let mut origin = usize::MAX;
            if previous_row[j - 1] > NONE {
                candidate = previous_row[j - 1] + BONUS_CONSECUTIVE;
                origin = j - 1;
            }
            if gap_best > NONE {
                // Hueco de `j - k - 1` letras.
                let with_gap =
                    gap_best - PENALTY_GAP_START - PENALTY_GAP_EXTENSION * (j as i32 - 2);
                if with_gap > candidate {
                    candidate = with_gap;
                    origin = gap_from;
                }
            }
            if candidate > NONE {
                row[j] = candidate + here;
                from[i * n + j] = origin;
            }
        }
    }

    let last = &best[(m - 1) * n..];
    let (end, score) = last
        .iter()
        .enumerate()
        .filter(|(_, score)| **score > NONE)
        .max_by_key(|(j, score)| (**score, std::cmp::Reverse(*j)))?;
    let mut positions = vec![0; m];
    let mut j = end;
    for i in (0..m).rev() {
        positions[i] = chars[j].0;
        if i > 0 {
            j = from[i * n + j];
        }
    }
    Some(FuzzyMatch {
        score: *score,
        positions,
    })
}

/// Puntúa `candidate` contra `query`. `None` si alguna letra (o algún
/// término) no aparece. Una consulta vacía casa con todo con puntuación 0.
pub fn fuzzy_match(query: &str, candidate: &str) -> Option<FuzzyMatch> {
    let chars: Vec<(usize, char)> = candidate.char_indices().take(MAX_CANDIDATE_CHARS).collect();
    // El nombre del archivo: lo que va tras la última barra.
    let basename_start = chars
        .iter()
        .rposition(|(_, character)| *character == '/' || *character == '\\')
        .map_or(0, |position| position + 1);

    let mut total = FuzzyMatch {
        score: 0,
        positions: Vec::new(),
    };
    for term in query.split_whitespace() {
        let term: Vec<char> = term.chars().collect();
        let found = match_term(&term, &chars, basename_start)?;
        total.score += found.score;
        total.positions.extend(found.positions);
    }
    total.positions.sort_unstable();
    total.positions.dedup();
    // Entre dos iguales, el más corto.
    total.score -= (chars.len() / 8) as i32;
    Some(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn positions(query: &str, candidate: &str) -> Vec<usize> {
        fuzzy_match(query, candidate)
            .map(|found| found.positions)
            .unwrap_or_default()
    }

    fn score(query: &str, candidate: &str) -> i32 {
        fuzzy_match(query, candidate).map_or(i32::MIN, |found| found.score)
    }

    #[test]
    fn letters_must_appear_in_order() {
        assert!(fuzzy_match("ftch", "Fetch").is_some());
        assert!(fuzzy_match("hctef", "Fetch").is_none());
        assert!(fuzzy_match("fetchh", "Fetch").is_none());
        assert!(fuzzy_match("", "Fetch").is_some_and(|found| found.positions.is_empty()));
    }

    #[test]
    fn ignores_case_but_prefers_the_same_case() {
        assert_eq!(positions("PUSH", "push"), [0, 1, 2, 3]);
        assert!(score("Push", "Push") > score("push", "Push"));
    }

    #[test]
    fn prefers_word_starts_and_consecutive_letters() {
        // "sb" en "Switch to branch": la s y la b de las palabras, no la del medio.
        assert_eq!(positions("sb", "Switch to branch"), [0, 10]);
        // Seguidas ganan a sueltas.
        assert!(score("log", "Show operation log") > score("log", "Load other grid"));
        // camelCase
        assert_eq!(positions("fcv", "FileContentViewer"), [0, 4, 11]);
    }

    #[test]
    fn prefers_matches_in_the_file_name() {
        assert!(score("main", "src/main.rs") > score("main", "main/src/lib.rs"));
        assert_eq!(positions("lib", "src/lib.rs"), [4, 5, 6]);
    }

    #[test]
    fn spaces_split_terms_that_can_match_in_any_order() {
        assert!(fuzzy_match("branch switch", "Switch to branch…").is_some());
        assert!(fuzzy_match("branch zzz", "Switch to branch…").is_none());
        assert_eq!(positions("to sw", "Switch to branch"), [0, 1, 7, 8]);
    }

    #[test]
    fn positions_are_byte_offsets() {
        // "ñ" ocupa dos bytes.
        assert_eq!(positions("ña", "año"), Vec::<usize>::new());
        assert_eq!(positions("ño", "año"), [1, 3]);
        assert_eq!(positions("é", "café"), [3]);
    }

    #[test]
    fn shorter_candidates_win_ties() {
        assert!(score("fetch", "Fetch") > score("fetch", "Fetch all remotes and prune them now"));
    }
}
