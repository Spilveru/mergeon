//! Lógica de la paleta de comandos (sin interfaz): qué busca cada prefijo y
//! cómo se ordenan los resultados.
//!
//! - `>` solo comandos, `@` solo ramas, `#` solo repositorios, `/` solo
//!   archivos ("Go to file…").
//! - Sin prefijo, todo mezclado. Con la consulta vacía se ven los comandos,
//!   los repositorios y las ramas (los archivos solo aparecen al escribir:
//!   son demasiados para listarlos sin filtrar).
//! - Con texto, gana la mejor puntuación difusa ([`crate::fuzzy`]) más un
//!   pequeño empujón por tipo (comando > repositorio > rama > archivo), para
//!   que "fetch" encuentre antes el comando que un archivo `fetch.rs`.
//! - Lo deshabilitado va siempre detrás de lo que se puede ejecutar.
//!
//! [`rank`] no toca la interfaz y trabaja sobre un `&[PaletteCandidate]`:
//! la vista lo llama fuera del render (en segundo plano si hay muchos
//! candidatos) y guarda el resultado por consulta.

use crate::fuzzy::fuzzy_match;

/// Qué es cada resultado.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PaletteKind {
    Command,
    Repository,
    Branch,
    File,
}

impl PaletteKind {
    /// Empujón en la puntuación con texto.
    fn boost(self) -> i32 {
        match self {
            Self::Command => 24,
            Self::Repository => 16,
            Self::Branch => 8,
            Self::File => 0,
        }
    }
}

/// A qué se limita la búsqueda.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PaletteScope {
    All,
    Commands,
    Branches,
    Repositories,
    Files,
}

impl PaletteScope {
    pub fn includes(self, kind: PaletteKind) -> bool {
        match self {
            Self::All => true,
            Self::Commands => kind == PaletteKind::Command,
            Self::Branches => kind == PaletteKind::Branch,
            Self::Repositories => kind == PaletteKind::Repository,
            Self::Files => kind == PaletteKind::File,
        }
    }

    /// El prefijo que la elige (`""` para todo).
    pub fn prefix(self) -> &'static str {
        match self {
            Self::All => "",
            Self::Commands => ">",
            Self::Branches => "@",
            Self::Repositories => "#",
            Self::Files => "/",
        }
    }
}

/// Separa el prefijo de la consulta: `"@ma"` → (ramas, `"ma"`).
pub fn parse_query(text: &str) -> (PaletteScope, &str) {
    let trimmed = text.trim_start();
    let scope = match trimmed.chars().next() {
        Some('>') => PaletteScope::Commands,
        Some('@') => PaletteScope::Branches,
        Some('#') => PaletteScope::Repositories,
        Some('/') => PaletteScope::Files,
        _ => return (PaletteScope::All, text.trim()),
    };
    (scope, trimmed[1..].trim())
}

/// Algo que se puede buscar en la paleta.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaletteCandidate {
    pub kind: PaletteKind,
    /// Lo que se busca y se pinta (con las letras encontradas resaltadas).
    pub label: String,
    pub enabled: bool,
}

impl PaletteCandidate {
    pub fn new(kind: PaletteKind, label: impl Into<String>) -> Self {
        Self {
            kind,
            label: label.into(),
            enabled: true,
        }
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
}

/// Un resultado: qué candidato (por posición) y qué letras casaron.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RankedCandidate {
    pub index: usize,
    pub score: i32,
    /// Offsets en bytes de `label` que casaron.
    pub positions: Vec<usize>,
}

/// Los candidatos que casan con `text` (con su prefijo), ordenados, como
/// mucho `limit`.
pub fn rank(candidates: &[PaletteCandidate], text: &str, limit: usize) -> Vec<RankedCandidate> {
    rank_with_files(candidates, &[], true, text, limit)
}

/// Como [`rank`], con los archivos aparte (no se copian a candidatos: un
/// repositorio puede tener cientos de miles). En el resultado, un índice
/// `candidates.len() + n` es el archivo `files[n]`.
pub fn rank_with_files(
    candidates: &[PaletteCandidate],
    files: &[String],
    files_enabled: bool,
    text: &str,
    limit: usize,
) -> Vec<RankedCandidate> {
    let (scope, query) = parse_query(text);
    let empty = query.is_empty();
    // Sin texto ni prefijo, los archivos no se listan.
    let list_files = scope.includes(PaletteKind::File) && !(empty && scope == PaletteScope::All);

    let all = candidates
        .iter()
        .map(|candidate| (candidate.kind, candidate.label.as_str(), candidate.enabled))
        .chain(
            files
                .iter()
                .take(if list_files { usize::MAX } else { 0 })
                .map(|file| (PaletteKind::File, file.as_str(), files_enabled)),
        );

    let mut ranked: Vec<(bool, i32, usize, Vec<usize>)> = all
        .enumerate()
        .filter(|(_, (kind, _, _))| scope.includes(*kind))
        .filter(|(_, (kind, _, _))| {
            !(empty && scope == PaletteScope::All && *kind == PaletteKind::File)
        })
        .filter_map(|(index, (kind, label, enabled))| {
            if empty {
                // Orden del registro, agrupado por tipo.
                let group = 3 - kind as i32;
                return Some((enabled, group, index, Vec::new()));
            }
            let found = fuzzy_match(query, label)?;
            Some((enabled, found.score + kind.boost(), index, found.positions))
        })
        .collect();

    let order = |a: &(bool, i32, usize, Vec<usize>), b: &(bool, i32, usize, Vec<usize>)| {
        b.0.cmp(&a.0).then(b.1.cmp(&a.1)).then(a.2.cmp(&b.2))
    };
    if ranked.len() > limit && limit > 0 {
        ranked.select_nth_unstable_by(limit - 1, order);
        ranked.truncate(limit);
    }
    ranked.sort_unstable_by(order);
    ranked.truncate(limit);
    ranked
        .into_iter()
        .map(|(_, score, index, positions)| RankedCandidate {
            index,
            score,
            positions,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn candidates() -> Vec<PaletteCandidate> {
        vec![
            PaletteCandidate::new(PaletteKind::Command, "Fetch"),
            PaletteCandidate::new(PaletteKind::Command, "Push").enabled(false),
            PaletteCandidate::new(PaletteKind::Command, "Pull"),
            PaletteCandidate::new(PaletteKind::Repository, "mergeon"),
            PaletteCandidate::new(PaletteKind::Branch, "main"),
            PaletteCandidate::new(PaletteKind::Branch, "feature/push-hooks"),
            PaletteCandidate::new(PaletteKind::File, "src/fetch.rs"),
            PaletteCandidate::new(PaletteKind::File, "README.md"),
        ]
    }

    fn labels(text: &str) -> Vec<String> {
        let all = candidates();
        rank(&all, text, 100)
            .into_iter()
            .map(|ranked| all[ranked.index].label.clone())
            .collect()
    }

    #[test]
    fn parses_prefixes() {
        assert_eq!(parse_query("> fetch"), (PaletteScope::Commands, "fetch"));
        assert_eq!(parse_query("@ma"), (PaletteScope::Branches, "ma"));
        assert_eq!(parse_query("#"), (PaletteScope::Repositories, ""));
        assert_eq!(parse_query("  main "), (PaletteScope::All, "main"));
        assert_eq!(PaletteScope::Branches.prefix(), "@");
    }

    #[test]
    fn an_empty_query_lists_everything_but_files_with_disabled_last() {
        assert_eq!(
            labels(""),
            [
                "Fetch",
                "Pull",
                "mergeon",
                "main",
                "feature/push-hooks",
                "Push"
            ]
        );
        assert_eq!(labels(">"), ["Fetch", "Pull", "Push"]);
        assert_eq!(labels("@"), ["main", "feature/push-hooks"]);
        assert_eq!(labels("#"), ["mergeon"]);
        assert_eq!(labels("/"), ["src/fetch.rs", "README.md"]);
        assert_eq!(labels("/fetch"), ["src/fetch.rs"]);
    }

    #[test]
    fn commands_win_over_files_with_the_same_text() {
        assert_eq!(labels("fetch"), ["Fetch", "src/fetch.rs"]);
        assert_eq!(labels("@fetch"), Vec::<String>::new());
        // Lo deshabilitado, detrás aunque case mejor.
        assert_eq!(labels("push"), ["feature/push-hooks", "Push"]);
    }

    #[test]
    fn returns_the_matched_positions_and_respects_the_limit() {
        let all = candidates();
        let ranked = rank(&all, "rdm", 100);
        assert_eq!(all[ranked[0].index].label, "README.md");
        assert_eq!(ranked[0].positions, [0, 3, 4]);
        assert_eq!(rank(&all, "", 2).len(), 2);
    }

    /// 5.000 ramas y 20.000 archivos: una consulta tiene que quedar muy por
    /// debajo de lo que se nota al escribir (la vista además la hace fuera
    /// del render y en segundo plano). El tope es holgado para el perfil de
    /// depuración de los tests.
    #[test]
    fn ranks_thousands_of_branches_and_files_quickly() {
        let mut all = candidates();
        for index in 0..5_000 {
            all.push(PaletteCandidate::new(
                PaletteKind::Branch,
                format!("team-{}/feature-{index}", index % 37),
            ));
        }
        for index in 0..20_000 {
            all.push(PaletteCandidate::new(
                PaletteKind::File,
                format!(
                    "src/module_{}/sub_{}/file_{index}.rs",
                    index % 50,
                    index % 7
                ),
            ));
        }
        let files: Vec<String> = (0..20_000)
            .map(|index| format!("lib/area_{}/{index}_view.rs", index % 40))
            .collect();
        let started = Instant::now();
        let separate = rank_with_files(&all, &files, true, "area7 view", 500);
        assert!(!separate.is_empty());
        assert!(separate.iter().all(|ranked| ranked.index >= all.len()));
        assert!(started.elapsed().as_millis() < 1500);
        let started = Instant::now();
        let ranked = rank(&all, "mod3 file12", 500);
        let elapsed = started.elapsed();
        assert!(!ranked.is_empty() && ranked.len() <= 500);
        assert!(elapsed.as_millis() < 1500, "{elapsed:?}");
        let started = Instant::now();
        let branches = rank(&all, "@feat 42", 500);
        assert!(!branches.is_empty());
        assert!(started.elapsed().as_millis() < 1500);
    }
}
