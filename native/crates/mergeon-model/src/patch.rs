//! Recortar un diff hasta dejar un solo bloque, para poder preparar una parte de
//! un archivo en vez del archivo entero (`src/lib/patch.js`).
//!
//! Un parche que git acepte necesita dos cosas: la cabecera del archivo —de
//! `diff --git` hasta antes del primer `@@`— y el bloque elegido tal cual, sin
//! tocar sus líneas ni recalcular sus números. Git ya sabe encajarlo.
//!
//! Se trabaja sobre el texto crudo del diff, no sobre las filas ya interpretadas,
//! porque la cabecera es justo lo que aquel descarta.
//!
//! Para unas líneas sueltas ([`extract_lines_patch`]) no basta con recortar:
//! hay que reescribir el bloque (lo no elegido pasa a contexto o desaparece,
//! según hacia dónde se aplique) y recalcular sus números.

use regex::Regex;
use std::sync::LazyLock;

static HUNK_START: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^@@ -[0-9]+(?:,[0-9]+)? \+[0-9]+(?:,[0-9]+)? @@").expect("hunk start regex")
});

struct SplitDiff<'a> {
    header: Vec<&'a str>,
    hunks: Vec<Vec<&'a str>>,
}

fn split_diff(diff: &str) -> SplitDiff<'_> {
    let mut header = Vec::new();
    let mut hunks: Vec<Vec<&str>> = Vec::new();

    for line in diff.split('\n') {
        if HUNK_START.is_match(line) {
            hunks.push(vec![line]);
            continue;
        }

        match hunks.last_mut() {
            Some(current) => current.push(line),
            None => header.push(line),
        }
    }

    // El último elemento del split es el salto final del texto, no una línea.
    if header.last() == Some(&"") && hunks.is_empty() {
        header.pop();
    }

    if let Some(last_hunk) = hunks.last_mut()
        && last_hunk.last() == Some(&"")
    {
        last_hunk.pop();
    }

    SplitDiff { header, hunks }
}

pub fn count_hunks(diff: &str) -> usize {
    split_diff(diff).hunks.len()
}

/// Parche con la cabecera del archivo y solo el bloque `index`. `None` si ese
/// bloque no existe o el diff no trae cabecera.
///
/// `git apply` exige que el parche termine en salto de línea; sin él dice
/// "corrupt patch at line N" y no hay pista de por qué.
pub fn extract_hunk_patch(diff: &str, index: usize) -> Option<String> {
    let SplitDiff { header, hunks } = split_diff(diff);
    let hunk = hunks.get(index)?;

    if header.is_empty() {
        return None;
    }

    let mut lines = header;
    lines.extend(hunk);
    Some(format!("{}\n", lines.join("\n")))
}

/// Hacia dónde se va a aplicar el parche de unas líneas sueltas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PatchDirection {
    /// `git apply` tal cual: preparar líneas del working tree en el índice. El
    /// lado viejo del parche (el índice) tiene que quedar entero: las bajas no
    /// elegidas pasan a contexto y las altas no elegidas se omiten.
    Forward,
    /// `git apply --reverse`: quitar líneas del índice o descartarlas del
    /// working tree. Ahora es el lado nuevo el que tiene que quedar entero: las
    /// altas no elegidas pasan a contexto y las bajas no elegidas se omiten.
    Reverse,
}

/// `@@ -a,b +c,d @@ resto`: los cuatro números y lo que git pone detrás
/// (el nombre de la función).
struct HunkHeader<'a> {
    old_start: u64,
    new_start: u64,
    tail: &'a str,
}

static HUNK_HEADER_PARTS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^@@ -([0-9]+)(?:,[0-9]+)? \+([0-9]+)(?:,[0-9]+)? @@(.*)$")
        .expect("hunk header parts regex")
});

fn parse_hunk_header(line: &str) -> Option<HunkHeader<'_>> {
    let captures = HUNK_HEADER_PARTS.captures(line)?;
    Some(HunkHeader {
        old_start: captures[1].parse().ok()?,
        new_start: captures[2].parse().ok()?,
        tail: captures.get(3).map_or("", |tail| tail.as_str()),
    })
}

/// Dónde cae un bloque en su lado: git escribe la línea anterior cuando el lado
/// no tiene líneas (`-5,0` es "detrás de la 5").
fn side_position(start: u64, count: u64) -> u64 {
    if count == 0 { start + 1 } else { start }
}

fn side_start(position: u64, count: u64) -> u64 {
    if count == 0 {
        position.saturating_sub(1)
    } else {
        position
    }
}

/// Parche con solo las líneas elegidas de un diff de **un** archivo.
///
/// `selected` son posiciones en las líneas de
/// [`crate::diff::parse_unified_diff`] (las que pinta el visor: cabeceras `@@`,
/// altas, bajas, contexto y notas). Solo cuentan las altas y las bajas; lo
/// demás se ignora. Las cabeceras `@@ -a,b +c,d @@` se recalculan y los bloques
/// sin ninguna línea elegida desaparecen.
///
/// Si el recorte deja contenido en un lado que la cabecera del archivo daba por
/// inexistente (preparar parte de un borrado, quitar del stage parte de un
/// archivo nuevo), el parche pasa a ser de modificación: si no, git exigiría
/// crear o borrar el archivo entero.
///
/// `None` si el diff no trae cabecera de archivo, trae más de un archivo o no
/// hay ninguna alta ni baja elegida.
pub fn extract_lines_patch(
    diff: &str,
    selected: &[usize],
    direction: PatchDirection,
) -> Option<String> {
    let SplitDiff { header, hunks } = split_diff(diff);
    if header.is_empty() || hunks.is_empty() {
        return None;
    }
    // `split_diff` mete en el último bloque lo que venga detrás; un segundo
    // archivo no se puede recortar con estas posiciones.
    if hunks
        .iter()
        .flatten()
        .any(|line| line.starts_with("diff --git "))
    {
        return None;
    }

    let mut chosen = selected.to_vec();
    chosen.sort_unstable();
    let is_chosen = |index: usize| chosen.binary_search(&index).is_ok();
    let forward = direction == PatchDirection::Forward;

    let mut body: Vec<String> = Vec::new();
    let mut delta: i64 = 0;
    let (mut old_total, mut new_total) = (0u64, 0u64);
    // Posición en `parse_unified_diff`: cada cabecera `@@` y cada línea de
    // bloque cuentan una.
    let mut index = 0usize;

    for hunk in &hunks {
        let header_line = hunk.first()?;
        let parsed = parse_hunk_header(header_line)?;
        index += 1;

        let mut lines: Vec<String> = Vec::new();
        let mut changed = false;
        // La nota `\ No newline at end of file` habla de la línea anterior: va
        // o no va con ella.
        let mut previous_kept = false;

        for line in &hunk[1..] {
            let position = index;
            index += 1;
            let chosen = is_chosen(position);
            match line.as_bytes().first() {
                Some(b'+') => {
                    if chosen {
                        lines.push((*line).to_string());
                        changed = true;
                        previous_kept = true;
                    } else if forward {
                        previous_kept = false;
                    } else {
                        lines.push(format!(" {}", &line[1..]));
                        previous_kept = true;
                    }
                }
                Some(b'-') => {
                    if chosen {
                        lines.push((*line).to_string());
                        changed = true;
                        previous_kept = true;
                    } else if forward {
                        lines.push(format!(" {}", &line[1..]));
                        previous_kept = true;
                    } else {
                        previous_kept = false;
                    }
                }
                Some(b'\\') => {
                    if previous_kept {
                        lines.push((*line).to_string());
                    }
                }
                _ => {
                    lines.push((*line).to_string());
                    previous_kept = true;
                }
            }
        }

        if !changed {
            continue;
        }
        if forward {
            keep_unterminated_line_last(&mut lines);
        }
        let (old_count, new_count) = side_counts(&lines);

        // El lado que se respeta entero conserva su número; el otro se corre
        // con lo que suman o restan los bloques anteriores de este parche.
        let (old_start, new_start) = if forward {
            let old_position = side_position(parsed.old_start, old_count);
            let new_position = (old_position as i64 + delta).max(0) as u64;
            (parsed.old_start, side_start(new_position, new_count))
        } else {
            let new_position = side_position(parsed.new_start, new_count);
            let old_position = (new_position as i64 - delta).max(0) as u64;
            (side_start(old_position, old_count), parsed.new_start)
        };
        delta += new_count as i64 - old_count as i64;
        old_total += old_count;
        new_total += new_count;

        body.push(format!(
            "@@ -{old_start},{old_count} +{new_start},{new_count} @@{}",
            parsed.tail
        ));
        body.extend(lines);
    }

    if body.is_empty() {
        return None;
    }

    let header = file_header_for(&header, old_total, new_total);
    let mut patch = header.join("\n");
    patch.push('\n');
    patch.push_str(&body.join("\n"));
    patch.push('\n');
    Some(patch)
}

/// Líneas de cada lado de un bloque: el contexto cuenta en los dos.
fn side_counts(lines: &[String]) -> (u64, u64) {
    lines
        .iter()
        .fold((0, 0), |(old, new), line| match line.as_bytes().first() {
            Some(b'+') => (old, new + 1),
            Some(b'-') => (old + 1, new),
            Some(b'\\') => (old, new),
            _ => (old + 1, new + 1),
        })
}

/// Una línea sin salto final solo puede ser la última del archivo. Al preparar
/// líneas añadidas detrás de ella (el caso típico: escribir al final de un
/// archivo que no terminaba en salto de línea), la baja no elegida que se pasó a
/// contexto dejaría esa línea en medio: se vuelve a dar de baja y se añade con
/// su salto, salvo que ya venga esa misma alta entre las elegidas.
fn keep_unterminated_line_last(lines: &mut Vec<String>) {
    let Some(at) = lines.iter().enumerate().position(|(index, line)| {
        line.starts_with(' ')
            && lines
                .get(index + 1)
                .is_some_and(|next| next.starts_with('\\'))
    }) else {
        return;
    };
    if !lines[at + 2..].iter().any(|line| line.starts_with('+')) {
        return;
    }

    let content = lines[at][1..].to_string();
    lines[at] = format!("-{content}");
    let replaced = lines[at + 2..]
        .iter()
        .any(|line| line.strip_prefix('+') == Some(content.as_str()));
    if !replaced {
        lines.insert(at + 2, format!("+{content}"));
    }
}

/// La cabecera del archivo, pasada a modificación si el recorte deja líneas en
/// el lado que era `/dev/null`.
fn file_header_for(header: &[&str], old_total: u64, new_total: u64) -> Vec<String> {
    let creates = header.contains(&"--- /dev/null");
    let deletes = header.contains(&"+++ /dev/null");
    let becomes_modification = (creates && old_total > 0) || (deletes && new_total > 0);
    if !becomes_modification {
        return header.iter().map(|line| (*line).to_string()).collect();
    }

    let path_of = |prefix: &str| {
        header
            .iter()
            .find_map(|line| line.strip_prefix(prefix))
            .map(str::to_string)
    };
    let new_path = path_of("+++ b/");
    let old_path = path_of("--- a/");

    header
        .iter()
        .filter(|line| {
            !line.starts_with("new file mode ")
                && !line.starts_with("deleted file mode ")
                && !line.starts_with("index ")
        })
        .map(|line| match *line {
            "--- /dev/null" => match &new_path {
                Some(path) => format!("--- a/{path}"),
                None => (*line).to_string(),
            },
            "+++ /dev/null" => match &old_path {
                Some(path) => format!("+++ b/{path}"),
                None => (*line).to_string(),
            },
            other => other.to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: [&str; 4] = [
        "diff --git a/src/app.js b/src/app.js",
        "index 4f77f5a..b3eda6e 100644",
        "--- a/src/app.js",
        "+++ b/src/app.js",
    ];

    fn with_header(lines: &[&str]) -> String {
        HEADER
            .iter()
            .chain(lines)
            .copied()
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn diff() -> String {
        with_header(&[
            "@@ -1,3 +1,3 @@",
            " const a = 1",
            "-const b = 2",
            "+const b = 3",
            "@@ -20,3 +20,3 @@",
            " const x = 1",
            "-const y = 2",
            "+const y = 3",
            "",
        ])
    }

    #[test]
    fn counts_the_hunks() {
        assert_eq!(count_hunks(&diff()), 2);
        assert_eq!(count_hunks(""), 0);
    }

    // Sin la cabecera del archivo, git no sabe a qué aplicar el parche.
    #[test]
    fn keeps_the_file_header() {
        let patch = extract_hunk_patch(&diff(), 0).unwrap();

        for line in HEADER {
            assert!(patch.contains(line), "missing {line}");
        }
    }

    #[test]
    fn takes_only_the_hunk_that_was_asked_for() {
        let patch = extract_hunk_patch(&diff(), 1).unwrap();

        assert!(patch.contains("@@ -20,3 +20,3 @@"));
        assert!(!patch.contains("@@ -1,3 +1,3 @@"));
        assert!(patch.contains("-const y = 2"));
        assert!(!patch.contains("-const b = 2"));
    }

    #[test]
    fn leaves_the_hunk_exactly_as_git_wrote_it() {
        assert_eq!(
            extract_hunk_patch(&diff(), 0),
            Some(with_header(&[
                "@@ -1,3 +1,3 @@",
                " const a = 1",
                "-const b = 2",
                "+const b = 3",
                "",
            ]))
        );
    }

    // Sin salto final, `git apply` dice "corrupt patch" y no explica por qué.
    #[test]
    fn ends_in_a_newline() {
        assert!(extract_hunk_patch(&diff(), 1).unwrap().ends_with('\n'));
        assert!(!extract_hunk_patch(&diff(), 1).unwrap().ends_with("\n\n"));
    }

    #[test]
    fn carries_the_no_newline_note_which_belongs_to_its_hunk() {
        let diff = with_header(&[
            "@@ -1,1 +1,1 @@",
            "-última sin salto",
            "\\ No newline at end of file",
            "+última con salto",
            "",
        ]);

        assert!(
            extract_hunk_patch(&diff, 0)
                .unwrap()
                .contains("\\ No newline at end of file")
        );
    }

    #[test]
    fn has_nothing_to_give_for_a_hunk_that_is_not_there() {
        assert_eq!(extract_hunk_patch(&diff(), 5), None);
    }

    #[test]
    fn has_nothing_to_give_without_a_diff() {
        assert_eq!(extract_hunk_patch("", 0), None);
    }

    // Un diff sin cabecera no se puede aplicar, así que es mejor no fabricarlo.
    #[test]
    fn refuses_a_diff_that_has_no_file_header() {
        assert_eq!(extract_hunk_patch("@@ -1,1 +1,1 @@\n-a\n+b\n", 0), None);
    }

    // El parche de un bloque tiene que aplicarse de verdad con `git apply`.
    #[test]
    fn git_accepts_the_patch_of_a_single_hunk() {
        use mergeon_gitcmd::Git;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();
        let git = Git::new(path).with_config([
            "user.name=Test",
            "user.email=test@test.local",
            "commit.gpgsign=false",
        ]);
        git.raw(["init", "-b", "main"]).unwrap();

        let original: Vec<String> = (1..=40).map(|number| format!("line {number}")).collect();
        std::fs::write(path.join("file.txt"), format!("{}\n", original.join("\n"))).unwrap();
        git.raw(["add", "--all"]).unwrap();
        git.raw(["commit", "-m", "base"]).unwrap();

        let mut edited = original.clone();
        edited[1] = "line two".to_string();
        edited[34] = "line thirty-five".to_string();
        std::fs::write(path.join("file.txt"), format!("{}\n", edited.join("\n"))).unwrap();

        let diff = git.raw(["diff", "--", "file.txt"]).unwrap();
        assert_eq!(count_hunks(&diff), 2);

        let patch = extract_hunk_patch(&diff, 1).unwrap();
        std::fs::write(path.join("hunk.patch"), &patch).unwrap();
        git.raw(["apply", "--cached", "hunk.patch"]).unwrap();

        let staged = git.raw(["diff", "--cached", "--", "file.txt"]).unwrap();
        assert!(staged.contains("+line thirty-five"));
        assert!(!staged.contains("+line two"));
    }
}

#[cfg(test)]
mod line_tests {
    use super::*;
    use crate::diff::{DiffLineType, parse_unified_diff};
    use mergeon_gitcmd::Git;
    use std::path::Path;

    const HEADER: &str = "diff --git a/f.txt b/f.txt\n\
index 1111111..2222222 100644\n\
--- a/f.txt\n\
+++ b/f.txt\n";

    /// Un bloque mezclado: contexto, dos bajas, tres altas, contexto, una baja
    /// y contexto.
    fn mixed() -> String {
        format!(
            "{HEADER}@@ -10,6 +10,6 @@ fn main\n ctx1\n-old1\n-old2\n+new1\n+new2\n+new3\n ctx2\n-old3\n ctx3\n"
        )
    }

    /// Dos bloques: el primero suma una línea, así que el segundo empieza una
    /// más abajo en el lado nuevo.
    fn two_hunks() -> String {
        format!("{HEADER}@@ -1,3 +1,4 @@\n a\n+b1\n+b2\n c\n-d\n@@ -20,2 +21,3 @@\n x\n+y\n z\n")
    }

    /// Posición en `parse_unified_diff` de la línea `+texto` / `-texto`.
    fn at(diff: &str, line: &str) -> usize {
        let (kind, content) = match line.split_at(1) {
            ("+", content) => (DiffLineType::Add, content),
            ("-", content) => (DiffLineType::Del, content),
            _ => panic!("solo altas y bajas: {line}"),
        };
        parse_unified_diff(diff)
            .iter()
            .position(|parsed| parsed.kind == kind && parsed.content == content)
            .unwrap_or_else(|| panic!("no está {line}"))
    }

    fn pick(diff: &str, lines: &[&str]) -> Vec<usize> {
        lines.iter().map(|line| at(diff, line)).collect()
    }

    fn stage(diff: &str, lines: &[&str]) -> Option<String> {
        extract_lines_patch(diff, &pick(diff, lines), PatchDirection::Forward)
    }

    fn unstage(diff: &str, lines: &[&str]) -> Option<String> {
        extract_lines_patch(diff, &pick(diff, lines), PatchDirection::Reverse)
    }

    fn body(patch: &str) -> &str {
        patch
            .strip_prefix(HEADER)
            .unwrap_or_else(|| panic!("sin la cabecera del archivo:\n{patch}"))
    }

    // --- Preparar (Forward) -------------------------------------------------

    #[test]
    fn stages_one_addition_from_the_middle_of_a_hunk() {
        let patch = stage(&mixed(), &["+new2"]).unwrap();
        assert_eq!(
            body(&patch),
            "@@ -10,6 +10,7 @@ fn main\n ctx1\n old1\n old2\n+new2\n ctx2\n old3\n ctx3\n"
        );
    }

    #[test]
    fn stages_one_deletion_from_the_start_of_a_hunk() {
        let patch = stage(&mixed(), &["-old1"]).unwrap();
        assert_eq!(
            body(&patch),
            "@@ -10,6 +10,5 @@ fn main\n ctx1\n-old1\n old2\n ctx2\n old3\n ctx3\n"
        );
    }

    #[test]
    fn stages_the_deletion_at_the_end_of_a_hunk() {
        let patch = stage(&mixed(), &["-old3"]).unwrap();
        assert_eq!(
            body(&patch),
            "@@ -10,6 +10,5 @@ fn main\n ctx1\n old1\n old2\n ctx2\n-old3\n ctx3\n"
        );
    }

    #[test]
    fn stages_a_replacement_made_of_a_deletion_and_an_addition() {
        let patch = stage(&mixed(), &["-old1", "+new1"]).unwrap();
        assert_eq!(
            body(&patch),
            "@@ -10,6 +10,6 @@ fn main\n ctx1\n-old1\n old2\n+new1\n ctx2\n old3\n ctx3\n"
        );
    }

    #[test]
    fn stages_only_additions() {
        let patch = stage(&mixed(), &["+new1", "+new2", "+new3"]).unwrap();
        assert_eq!(
            body(&patch),
            "@@ -10,6 +10,9 @@ fn main\n ctx1\n old1\n old2\n+new1\n+new2\n+new3\n ctx2\n old3\n ctx3\n"
        );
    }

    #[test]
    fn stages_only_deletions() {
        let patch = stage(&mixed(), &["-old1", "-old2", "-old3"]).unwrap();
        assert_eq!(
            body(&patch),
            "@@ -10,6 +10,3 @@ fn main\n ctx1\n-old1\n-old2\n ctx2\n-old3\n ctx3\n"
        );
    }

    #[test]
    fn selecting_every_change_gives_the_whole_hunk_back() {
        let all = ["-old1", "-old2", "+new1", "+new2", "+new3", "-old3"];
        assert_eq!(stage(&mixed(), &all), extract_hunk_patch(&mixed(), 0));
        assert_eq!(unstage(&mixed(), &all), extract_hunk_patch(&mixed(), 0));
    }

    #[test]
    fn the_order_of_the_selection_does_not_matter() {
        assert_eq!(
            stage(&mixed(), &["+new3", "-old1"]),
            stage(&mixed(), &["-old1", "+new3"])
        );
    }

    #[test]
    fn a_hunk_without_chosen_lines_is_left_out_and_does_not_shift_the_next() {
        let patch = stage(&two_hunks(), &["+y"]).unwrap();
        assert_eq!(body(&patch), "@@ -20,2 +20,3 @@\n x\n+y\n z\n");
    }

    #[test]
    fn several_zones_shift_the_new_side_by_what_the_previous_ones_add() {
        let patch = stage(&two_hunks(), &["+b1", "+y"]).unwrap();
        assert_eq!(
            body(&patch),
            "@@ -1,3 +1,4 @@\n a\n+b1\n c\n d\n@@ -20,2 +21,3 @@\n x\n+y\n z\n"
        );

        let patch = stage(&two_hunks(), &["-d", "+y"]).unwrap();
        assert_eq!(
            body(&patch),
            "@@ -1,3 +1,2 @@\n a\n c\n-d\n@@ -20,2 +19,3 @@\n x\n+y\n z\n"
        );
    }

    #[test]
    fn keeps_what_git_wrote_after_the_hunk_numbers() {
        let patch = stage(&mixed(), &["+new1"]).unwrap();
        assert!(body(&patch).starts_with("@@ -10,6 +10,7 @@ fn main\n"));
    }

    // --- Quitar del stage o descartar (Reverse) -----------------------------

    #[test]
    fn unstages_one_addition_keeping_the_others_as_context() {
        let patch = unstage(&mixed(), &["+new2"]).unwrap();
        assert_eq!(
            body(&patch),
            "@@ -10,5 +10,6 @@ fn main\n ctx1\n new1\n+new2\n new3\n ctx2\n ctx3\n"
        );
    }

    #[test]
    fn unstages_one_deletion_dropping_the_others() {
        let patch = unstage(&mixed(), &["-old3"]).unwrap();
        assert_eq!(
            body(&patch),
            "@@ -10,7 +10,6 @@ fn main\n ctx1\n new1\n new2\n new3\n ctx2\n-old3\n ctx3\n"
        );
    }

    #[test]
    fn unstages_a_replacement_at_the_start_of_the_hunk() {
        let patch = unstage(&mixed(), &["-old1", "+new1"]).unwrap();
        assert_eq!(
            body(&patch),
            "@@ -10,6 +10,6 @@ fn main\n ctx1\n-old1\n+new1\n new2\n new3\n ctx2\n ctx3\n"
        );
    }

    #[test]
    fn several_reverse_zones_shift_the_old_side() {
        let patch = unstage(&two_hunks(), &["+b1", "+y"]).unwrap();
        assert_eq!(
            body(&patch),
            "@@ -1,3 +1,4 @@\n a\n+b1\n b2\n c\n@@ -20,2 +21,3 @@\n x\n+y\n z\n"
        );

        // Sin el primer bloque, el lado viejo del segundo sigue donde está hoy.
        let patch = unstage(&two_hunks(), &["+y"]).unwrap();
        assert_eq!(body(&patch), "@@ -21,2 +21,3 @@\n x\n+y\n z\n");
    }

    // --- Sin salto final ----------------------------------------------------

    fn unterminated() -> String {
        format!(
            "{HEADER}@@ -1,2 +1,4 @@\n first\n-last\n\\ No newline at end of file\n+last\n+added1\n+added2\n"
        )
    }

    #[test]
    fn staging_lines_after_an_unterminated_line_gives_it_its_newline() {
        let patch = stage(&unterminated(), &["+added1"]).unwrap();
        assert_eq!(
            body(&patch),
            "@@ -1,2 +1,3 @@\n first\n-last\n\\ No newline at end of file\n+last\n+added1\n"
        );

        // Si la línea con salto también se eligió, no se repite.
        let patch = stage(&unterminated(), &["+last", "+added1"]).unwrap();
        assert_eq!(
            body(&patch),
            "@@ -1,2 +1,3 @@\n first\n-last\n\\ No newline at end of file\n+last\n+added1\n"
        );
    }

    #[test]
    fn the_note_goes_with_its_line() {
        // La baja elegida se lleva su nota.
        let patch = stage(&unterminated(), &["-last"]).unwrap();
        assert_eq!(
            body(&patch),
            "@@ -1,2 +1,1 @@\n first\n-last\n\\ No newline at end of file\n"
        );

        // Al descartar las altas, la baja no elegida se omite con su nota.
        let patch = unstage(&unterminated(), &["+added1", "+added2"]).unwrap();
        assert_eq!(
            body(&patch),
            "@@ -1,2 +1,4 @@\n first\n last\n+added1\n+added2\n"
        );

        // Una nota detrás de una alta no elegida desaparece al preparar.
        let diff = format!("{HEADER}@@ -1,1 +1,1 @@\n-a\n+b\n\\ No newline at end of file\n");
        let patch = stage(&diff, &["-a"]).unwrap();
        assert_eq!(body(&patch), "@@ -1,1 +0,0 @@\n-a\n");
        // Y se queda si la alta pasa a contexto.
        let patch = unstage(&diff, &["-a"]).unwrap();
        assert_eq!(
            body(&patch),
            "@@ -1,2 +1,1 @@\n-a\n b\n\\ No newline at end of file\n"
        );
    }

    // --- Archivos nuevos y borrados -----------------------------------------

    const NEW_FILE: &str = "diff --git a/n.txt b/n.txt\n\
new file mode 100644\n\
index 0000000..3333333\n\
--- /dev/null\n\
+++ b/n.txt\n\
@@ -0,0 +1,3 @@\n+one\n+two\n+three\n";

    const DELETED_FILE: &str = "diff --git a/d.txt b/d.txt\n\
deleted file mode 100644\n\
index 3333333..0000000\n\
--- a/d.txt\n\
+++ /dev/null\n\
@@ -1,3 +0,0 @@\n-one\n-two\n-three\n";

    #[test]
    fn a_new_file_keeps_its_header_while_nothing_is_left_on_the_old_side() {
        let patch = stage(NEW_FILE, &["+one", "+two"]).unwrap();
        assert_eq!(
            patch,
            "diff --git a/n.txt b/n.txt\nnew file mode 100644\nindex 0000000..3333333\n--- /dev/null\n+++ b/n.txt\n@@ -0,0 +1,2 @@\n+one\n+two\n"
        );
    }

    #[test]
    fn unstaging_part_of_a_new_file_turns_the_patch_into_a_modification() {
        let patch = unstage(NEW_FILE, &["+two"]).unwrap();
        assert_eq!(
            patch,
            "diff --git a/n.txt b/n.txt\n--- a/n.txt\n+++ b/n.txt\n@@ -1,2 +1,3 @@\n one\n+two\n three\n"
        );
    }

    #[test]
    fn staging_part_of_a_deletion_turns_the_patch_into_a_modification() {
        let patch = stage(DELETED_FILE, &["-two"]).unwrap();
        assert_eq!(
            patch,
            "diff --git a/d.txt b/d.txt\n--- a/d.txt\n+++ b/d.txt\n@@ -1,3 +1,2 @@\n one\n-two\n three\n"
        );
        // Entero sigue siendo un borrado.
        assert_eq!(
            stage(DELETED_FILE, &["-one", "-two", "-three"]),
            Some(DELETED_FILE.to_string())
        );
    }

    // --- Nada que hacer -----------------------------------------------------

    #[test]
    fn has_nothing_to_give_without_a_chosen_change() {
        assert_eq!(
            extract_lines_patch(&mixed(), &[], PatchDirection::Forward),
            None
        );
        // Cabecera (0) y contexto (1) no son cambios.
        assert_eq!(
            extract_lines_patch(&mixed(), &[0, 1, 9], PatchDirection::Forward),
            None
        );
        assert_eq!(
            extract_lines_patch(&mixed(), &[500], PatchDirection::Reverse),
            None
        );
    }

    #[test]
    fn refuses_diffs_it_cannot_cut() {
        assert_eq!(
            extract_lines_patch("@@ -1,1 +1,1 @@\n-a\n+b\n", &[1], PatchDirection::Forward),
            None
        );
        assert_eq!(extract_lines_patch("", &[1], PatchDirection::Forward), None);
        let two_files = format!("{}{}", mixed(), mixed());
        assert_eq!(
            extract_lines_patch(&two_files, &[2], PatchDirection::Forward),
            None
        );
    }

    // --- Con git de verdad --------------------------------------------------

    struct Repo {
        _dir: tempfile::TempDir,
        git: Git,
    }

    impl Repo {
        fn new(files: &[(&str, &str)]) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let git = Git::new(dir.path()).with_config([
                "user.name=Test",
                "user.email=test@test.local",
                "commit.gpgsign=false",
                "core.autocrlf=false",
            ]);
            git.raw(["init", "-b", "main"]).unwrap();
            let repo = Self { _dir: dir, git };
            for (path, content) in files {
                repo.write(path, content);
            }
            repo.git.raw(["add", "--all"]).unwrap();
            repo.git.raw(["commit", "-m", "base"]).unwrap();
            repo
        }

        fn path(&self) -> &Path {
            self._dir.path()
        }

        fn write(&self, path: &str, content: &str) {
            std::fs::write(self.path().join(path), content).unwrap();
        }

        fn read(&self, path: &str) -> String {
            std::fs::read_to_string(self.path().join(path)).unwrap()
        }

        fn diff(&self, cached: bool, path: &str) -> String {
            if cached {
                self.git.raw(["diff", "--cached", "--", path]).unwrap()
            } else {
                self.git.raw(["diff", "--", path]).unwrap()
            }
        }

        fn index(&self, path: &str) -> String {
            self.git.raw(["show", &format!(":{path}")]).unwrap()
        }

        /// `git apply --check` y, si pasa, `git apply` con las mismas opciones.
        fn apply(&self, patch: &str, options: &[&str]) {
            let file = self.path().join("lines.patch");
            std::fs::write(&file, patch).unwrap();
            let file = file.to_string_lossy().into_owned();
            let mut check = vec!["apply", "--check"];
            check.extend(options);
            check.push(&file);
            self.git
                .raw(check)
                .unwrap_or_else(|error| panic!("{error:?}\n{patch}"));
            let mut apply = vec!["apply"];
            apply.extend(options);
            apply.push(&file);
            self.git.raw(apply).unwrap();
            std::fs::remove_file(self.path().join("lines.patch")).unwrap();
        }
    }

    const BASE: &str = "uno\ndos\ntres\ncuatro\ncinco\n";
    const EDITED: &str = "uno\nDOS\nDOS bis\ntres\ncinco\nseis\n";

    #[test]
    fn git_stages_two_lines_of_a_mixed_hunk() {
        let repo = Repo::new(&[("f.txt", BASE)]);
        repo.write("f.txt", EDITED);
        let diff = repo.diff(false, "f.txt");

        let patch = stage(&diff, &["-dos", "+DOS"]).unwrap();
        repo.apply(&patch, &["--cached"]);

        assert_eq!(repo.index("f.txt"), "uno\nDOS\ntres\ncuatro\ncinco\n");
        assert_eq!(repo.read("f.txt"), EDITED);
    }

    #[test]
    fn git_unstages_one_line_and_keeps_the_rest_staged() {
        let repo = Repo::new(&[("f.txt", BASE)]);
        repo.write("f.txt", EDITED);
        repo.git.raw(["add", "f.txt"]).unwrap();
        let diff = repo.diff(true, "f.txt");

        let patch = unstage(&diff, &["+DOS bis", "-cuatro"]).unwrap();
        repo.apply(&patch, &["--cached", "--reverse"]);

        assert_eq!(repo.index("f.txt"), "uno\nDOS\ntres\ncuatro\ncinco\nseis\n");
        assert_eq!(repo.read("f.txt"), EDITED);
    }

    #[test]
    fn git_discards_one_line_from_the_working_tree() {
        let repo = Repo::new(&[("f.txt", BASE)]);
        repo.write("f.txt", EDITED);
        let diff = repo.diff(false, "f.txt");

        let patch = unstage(&diff, &["+seis"]).unwrap();
        repo.apply(&patch, &["--reverse"]);

        assert_eq!(repo.read("f.txt"), "uno\nDOS\nDOS bis\ntres\ncinco\n");
        assert_eq!(repo.index("f.txt"), BASE);
    }

    #[test]
    fn git_takes_lines_from_several_hunks_at_once() {
        let base: Vec<String> = (1..=30).map(|n| format!("line {n}")).collect();
        let repo = Repo::new(&[("f.txt", &format!("{}\n", base.join("\n")))]);
        let mut edited = base.clone();
        edited.insert(2, "extra a".to_string());
        edited.insert(3, "extra b".to_string());
        edited[25] = "changed 24".to_string();
        repo.write("f.txt", &format!("{}\n", edited.join("\n")));
        let diff = repo.diff(false, "f.txt");
        assert_eq!(count_hunks(&diff), 2);

        let patch = stage(&diff, &["+extra b", "-line 24", "+changed 24"]).unwrap();
        repo.apply(&patch, &["--cached"]);

        let mut expected = base.clone();
        expected.insert(2, "extra b".to_string());
        expected[24] = "changed 24".to_string();
        assert_eq!(repo.index("f.txt"), format!("{}\n", expected.join("\n")));

        // Y deshacerlo línea a línea desde el índice, también entre bloques.
        let staged = repo.diff(true, "f.txt");
        let patch = unstage(&staged, &["+extra b", "+changed 24", "-line 24"]).unwrap();
        repo.apply(&patch, &["--cached", "--reverse"]);
        assert_eq!(repo.index("f.txt"), format!("{}\n", base.join("\n")));
    }

    // "Whole file" (`--unified=999999999`): un único bloque con todo el
    // archivo como contexto. Elegir líneas sueltas ahí también se aplica.
    #[test]
    fn git_takes_lines_from_a_whole_file_diff() {
        let base: Vec<String> = (1..=30).map(|n| format!("line {n}")).collect();
        let repo = Repo::new(&[("f.txt", &format!("{}\n", base.join("\n")))]);
        let mut edited = base.clone();
        edited[1] = "line two".to_string();
        edited.insert(20, "extra".to_string());
        repo.write("f.txt", &format!("{}\n", edited.join("\n")));
        let diff = repo
            .git
            .raw(["diff", "--unified=999999999", "--", "f.txt"])
            .unwrap();
        assert_eq!(count_hunks(&diff), 1);

        let patch = stage(&diff, &["+extra"]).unwrap();
        repo.apply(&patch, &["--cached"]);
        let mut expected = base.clone();
        expected.insert(20, "extra".to_string());
        assert_eq!(repo.index("f.txt"), format!("{}\n", expected.join("\n")));

        // Descartar del working tree la otra, desde el diff entero también.
        let diff = repo
            .git
            .raw(["diff", "--unified=999999999", "--", "f.txt"])
            .unwrap();
        let patch = unstage(&diff, &["-line 2", "+line two"]).unwrap();
        repo.apply(&patch, &["--reverse"]);
        assert_eq!(repo.read("f.txt"), format!("{}\n", expected.join("\n")));

        // Y quitar del índice lo preparado.
        let staged = repo
            .git
            .raw(["diff", "--cached", "--unified=999999999", "--", "f.txt"])
            .unwrap();
        let patch = unstage(&staged, &["+extra"]).unwrap();
        repo.apply(&patch, &["--cached", "--reverse"]);
        assert_eq!(repo.index("f.txt"), format!("{}\n", base.join("\n")));
    }

    #[test]
    fn git_stages_a_line_appended_after_an_unterminated_last_line() {
        let repo = Repo::new(&[("f.txt", "first\nlast")]);
        repo.write("f.txt", "first\nlast\nadded1\nadded2\n");
        let diff = repo.diff(false, "f.txt");
        assert!(diff.contains("\\ No newline at end of file"));

        let patch = stage(&diff, &["+added1"]).unwrap();
        repo.apply(&patch, &["--cached"]);
        assert_eq!(repo.index("f.txt"), "first\nlast\nadded1\n");

        // Descartar la otra deja el working tree igual al índice.
        let diff = repo.diff(false, "f.txt");
        let patch = unstage(&diff, &["+added2"]).unwrap();
        repo.apply(&patch, &["--reverse"]);
        assert_eq!(repo.read("f.txt"), "first\nlast\nadded1\n");
    }

    #[test]
    fn git_unstages_part_of_a_new_file_and_stages_part_of_a_deletion() {
        let repo = Repo::new(&[("d.txt", "one\ntwo\nthree\n")]);
        repo.write("n.txt", "one\ntwo\nthree\n");
        repo.git.raw(["add", "n.txt"]).unwrap();
        let diff = repo.diff(true, "n.txt");
        let patch = unstage(&diff, &["+two"]).unwrap();
        repo.apply(&patch, &["--cached", "--reverse"]);
        assert_eq!(repo.index("n.txt"), "one\nthree\n");

        std::fs::remove_file(repo.path().join("d.txt")).unwrap();
        let diff = repo.diff(false, "d.txt");
        let patch = stage(&diff, &["-two"]).unwrap();
        repo.apply(&patch, &["--cached"]);
        assert_eq!(repo.index("d.txt"), "one\nthree\n");
    }

    #[test]
    fn git_keeps_windows_line_endings() {
        let repo = Repo::new(&[("w.txt", "uno\r\ndos\r\ntres\r\n")]);
        repo.write("w.txt", "uno\r\nDOS\r\ntres\r\ncuatro\r\n");
        let diff = repo.diff(false, "w.txt");

        let patch = stage(&diff, &["+cuatro\r"]).unwrap();
        repo.apply(&patch, &["--cached"]);
        assert_eq!(repo.index("w.txt"), "uno\r\ndos\r\ntres\r\ncuatro\r\n");
    }
}
