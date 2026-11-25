//! Lectura de un diff unificado (`src/lib/diff.js`).
//!
//! Dos reglas, y las dos vienen de errores reales:
//!
//! 1. Todo lo que va antes del primer `@@` es metadatos, no contenido. `git show`
//!    antepone el commit entero —hash, autor, fecha y mensaje— y sin esta regla
//!    esas líneas se numeraban y se pintaban como si fueran código del archivo.
//!
//! 2. Dentro de un hunk manda el primer carácter y nada más. Filtrar por
//!    prefijos como `--- ` en cualquier posición borra líneas de verdad: quitar
//!    `-- comentario` (SQL, Lua, Haskell) produce `--- comentario`, que no es una
//!    cabecera sino un borrado, y desaparecía del diff sin avisar.
//!
//! `\ No newline at end of file` es una nota de git sobre la línea anterior, no
//! una línea del archivo: se marca aparte para que no consuma numeración y
//! desplace todo lo que viene después.
//!
//! Diferencia con el original: los rangos intralínea (`inline`) son offsets en
//! bytes UTF-8 de `content` (siempre en frontera de carácter), listos para
//! `&content[from..to]`. En JavaScript eran índices UTF-16; coinciden mientras
//! la línea sea ASCII.

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

static HUNK_HEADER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^@@ -([0-9]+)(?:,[0-9]+)? \+([0-9]+)(?:,[0-9]+)? @@").expect("hunk header regex")
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiffLineType {
    Hunk,
    Add,
    Del,
    Ctx,
    Note,
}

impl DiffLineType {
    pub fn as_str(self) -> &'static str {
        match self {
            DiffLineType::Hunk => "hunk",
            DiffLineType::Add => "add",
            DiffLineType::Del => "del",
            DiffLineType::Ctx => "ctx",
            DiffLineType::Note => "note",
        }
    }

    /// Altas y borrados: lo que cuenta como cambio.
    pub fn is_change(self) -> bool {
        matches!(self, DiffLineType::Add | DiffLineType::Del)
    }
}

/// Trozo `[from, to)` de una línea, en bytes de su `content`.
pub type InlineRange = [usize; 2];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffLine {
    #[serde(rename = "type")]
    pub kind: DiffLineType,
    pub content: String,
    /// Número en la versión anterior (`del` y `ctx`).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub old_num: Option<u32>,
    /// Número en la versión nueva (`add` y `ctx`).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_num: Option<u32>,
    /// Trozo que cambió respecto de su pareja; lo pone [`mark_inline_changes`].
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub inline: Option<InlineRange>,
}

impl DiffLine {
    fn new(kind: DiffLineType, content: &str) -> Self {
        Self {
            kind,
            content: content.to_string(),
            old_num: None,
            new_num: None,
            inline: None,
        }
    }
}

fn line_number(digits: &str) -> u32 {
    digits.parse().unwrap_or(u32::MAX)
}

/// `line.slice(count)` contando caracteres.
fn skip_chars(line: &str, count: usize) -> &str {
    match line.char_indices().nth(count) {
        Some((offset, _)) => &line[offset..],
        None => "",
    }
}

pub fn parse_unified_diff(diff: &str) -> Vec<DiffLine> {
    if diff.is_empty() {
        return Vec::new();
    }

    let mut raw_lines: Vec<&str> = diff.split('\n').collect();
    // El texto termina en salto de línea, así que el último elemento sobra. Una
    // línea de contexto vacía llega como un espacio, no vacía, así que ninguna
    // línea real se pierde por esto.
    if raw_lines.last() == Some(&"") {
        raw_lines.pop();
    }

    let mut lines = Vec::new();
    let mut old_number: u32 = 0;
    let mut new_number: u32 = 0;
    let mut inside_hunk = false;

    for line in raw_lines {
        if let Some(hunk) = HUNK_HEADER.captures(line) {
            inside_hunk = true;
            old_number = line_number(&hunk[1]);
            new_number = line_number(&hunk[2]);
            lines.push(DiffLine::new(DiffLineType::Hunk, line));
            continue;
        }

        // Un archivo nuevo dentro del mismo diff vuelve a empezar por su cabecera.
        if line.starts_with("diff --git ") {
            inside_hunk = false;
            continue;
        }

        if !inside_hunk {
            continue;
        }

        if let Some(content) = line.strip_prefix('+') {
            lines.push(DiffLine {
                new_num: Some(new_number),
                ..DiffLine::new(DiffLineType::Add, content)
            });
            new_number = new_number.saturating_add(1);
            continue;
        }

        if let Some(content) = line.strip_prefix('-') {
            lines.push(DiffLine {
                old_num: Some(old_number),
                ..DiffLine::new(DiffLineType::Del, content)
            });
            old_number = old_number.saturating_add(1);
            continue;
        }

        if line.starts_with('\\') {
            lines.push(DiffLine::new(DiffLineType::Note, skip_chars(line, 2)));
            continue;
        }

        lines.push(DiffLine {
            old_num: Some(old_number),
            new_num: Some(new_number),
            ..DiffLine::new(DiffLineType::Ctx, line.strip_prefix(' ').unwrap_or(line))
        });
        old_number = old_number.saturating_add(1);
        new_number = new_number.saturating_add(1);
    }

    lines
}

// Por debajo de esto las dos líneas no se parecen: marcar un trozo comun sería
// ruido, y es más honesto dejar la línea entera en rojo o en verde.
const MIN_SIMILARITY: f64 = 0.25;

// Y por encima, el cambio abarca casi toda la línea: resaltarlo tampoco dice nada.
const MAX_CHANGED_SHARE: f64 = 0.8;

/// `/[\w$]/.test(text[index])`. Fuera de la línea el original lee `undefined`,
/// que la expresión regular convierte en el texto "undefined" y da por bueno:
/// el borde de la línea cuenta como carácter de palabra. Se conserva porque
/// decide hasta dónde se estira el resaltado cuando una versión es prefijo o
/// sufijo de la otra.
fn is_word_character(text: &[char], index: Option<usize>) -> bool {
    match index.and_then(|index| text.get(index)) {
        Some(character) => character.is_ascii_alphanumeric() || matches!(character, '_' | '$'),
        None => true,
    }
}

/// No cortar por la mitad de una palabra: se busca su principio.
fn back_to_word_start(text: &[char], index: usize) -> usize {
    let mut position = index;
    while position > 0
        && is_word_character(text, Some(position - 1))
        && is_word_character(text, Some(position))
    {
        position -= 1;
    }

    position
}

fn forward_to_word_end(text: &[char], index: usize) -> usize {
    let mut position = index;
    while position < text.len()
        && is_word_character(text, Some(position))
        && is_word_character(text, position.checked_sub(1))
    {
        position += 1;
    }

    position
}

fn byte_offset(text: &str, char_index: usize) -> usize {
    text.char_indices()
        .nth(char_index)
        .map_or(text.len(), |(offset, _)| offset)
}

/// Rangos que cambiaron en cada versión de una línea.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct InlineRanges {
    pub before: InlineRange,
    pub after: InlineRange,
}

/// Qué trozo cambió entre dos versiones de una línea, recortando el principio y
/// el final que comparten. Es lo que separa "algo cambió aquí" de "cambió este
/// identificador", y cubre la edición típica sin necesidad de un LCS completo.
///
/// La comparación va carácter a carácter (el original, unidad UTF-16 a unidad).
pub fn compute_inline_ranges(before: &str, after: &str) -> Option<InlineRanges> {
    if before.is_empty() || after.is_empty() || before == after {
        return None;
    }

    let before_chars: Vec<char> = before.chars().collect();
    let after_chars: Vec<char> = after.chars().collect();
    let shortest = before_chars.len().min(after_chars.len());

    let mut start = 0;
    while start < shortest && before_chars[start] == after_chars[start] {
        start += 1;
    }

    let mut before_end = before_chars.len();
    let mut after_end = after_chars.len();
    while before_end > start
        && after_end > start
        && before_chars[before_end - 1] == after_chars[after_end - 1]
    {
        before_end -= 1;
        after_end -= 1;
    }

    let common = start + (before_chars.len() - before_end);
    let longest = before_chars.len().max(after_chars.len());
    if (common as f64) / (longest as f64) < MIN_SIMILARITY {
        return None;
    }

    let from =
        back_to_word_start(&before_chars, start).min(back_to_word_start(&after_chars, start));
    let before_to = forward_to_word_end(&before_chars, before_end);
    let after_to = forward_to_word_end(&after_chars, after_end);

    // Con el borde contando como palabra, `from` nunca queda más allá de los
    // finales; la resta saturada solo protege el invariante.
    let changed = before_to.max(after_to).saturating_sub(from);
    if (changed as f64) / (longest as f64) > MAX_CHANGED_SHARE {
        return None;
    }

    Some(InlineRanges {
        before: [byte_offset(before, from), byte_offset(before, before_to)],
        after: [byte_offset(after, from), byte_offset(after, after_to)],
    })
}

/// Marca en cada línea el trozo que cambió. Los borrados y las altas seguidos se
/// emparejan por posición, que es como se leen: la primera línea quitada frente a
/// la primera puesta.
pub fn mark_inline_changes(lines: &[DiffLine]) -> Vec<DiffLine> {
    let mut marked = lines.to_vec();

    let mut index = 0;
    while index < marked.len() {
        if marked[index].kind != DiffLineType::Del {
            index += 1;
            continue;
        }

        let removed_start = index;
        while index < marked.len() && marked[index].kind == DiffLineType::Del {
            index += 1;
        }
        let removed_count = index - removed_start;

        let added_start = index;
        while index < marked.len() && marked[index].kind == DiffLineType::Add {
            index += 1;
        }
        let added_count = index - added_start;

        for pair in 0..removed_count.min(added_count) {
            let ranges = compute_inline_ranges(
                &marked[removed_start + pair].content,
                &marked[added_start + pair].content,
            );
            let Some(ranges) = ranges else {
                continue;
            };

            marked[removed_start + pair].inline = Some(ranges.before);
            marked[added_start + pair].inline = Some(ranges.after);
        }
    }

    marked
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InlineSegment {
    pub text: String,
    pub changed: bool,
}

/// Parte un texto en trozos según el rango que cambió. Sin rango (o con uno que
/// no cae dentro del texto) devuelve un único trozo sin marcar.
pub fn inline_segments(content: &str, inline: Option<InlineRange>) -> Vec<InlineSegment> {
    let whole = || {
        vec![InlineSegment {
            text: content.to_string(),
            changed: false,
        }]
    };

    let Some([from, to]) = inline else {
        return whole();
    };
    let (Some(head), Some(middle), Some(tail)) = (
        content.get(..from),
        content.get(from..to),
        content.get(to..),
    ) else {
        return whole();
    };

    [(head, false), (middle, true), (tail, false)]
        .into_iter()
        .filter(|(text, _)| !text.is_empty())
        .map(|(text, changed)| InlineSegment {
            text: text.to_string(),
            changed,
        })
        .collect()
}

/// Parte una línea en trozos según lo que cambió, para pintarlo.
pub fn to_inline_segments(line: &DiffLine) -> Vec<InlineSegment> {
    inline_segments(&line.content, line.inline)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SplitCellType {
    /// Hueco: el otro lado tiene una línea que este no.
    Empty,
    Hunk,
    Add,
    Del,
    Ctx,
    Note,
}

impl SplitCellType {
    pub fn as_str(self) -> &'static str {
        match self {
            SplitCellType::Empty => "empty",
            SplitCellType::Hunk => "hunk",
            SplitCellType::Add => "add",
            SplitCellType::Del => "del",
            SplitCellType::Ctx => "ctx",
            SplitCellType::Note => "note",
        }
    }

    pub fn is_change(self) -> bool {
        matches!(self, SplitCellType::Add | SplitCellType::Del)
    }
}

impl From<DiffLineType> for SplitCellType {
    fn from(kind: DiffLineType) -> Self {
        match kind {
            DiffLineType::Hunk => SplitCellType::Hunk,
            DiffLineType::Add => SplitCellType::Add,
            DiffLineType::Del => SplitCellType::Del,
            DiffLineType::Ctx => SplitCellType::Ctx,
            DiffLineType::Note => SplitCellType::Note,
        }
    }
}

/// Un lado de una fila de la vista partida.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SplitCell {
    #[serde(rename = "type")]
    pub kind: SplitCellType,
    /// Vacío en los huecos (`empty`), que en el original no llevan `content`.
    #[serde(default)]
    pub content: String,
    /// Número de línea en su lado; `None` en huecos, hunks y notas.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub num: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub inline: Option<InlineRange>,
}

impl SplitCell {
    pub fn empty() -> Self {
        Self {
            kind: SplitCellType::Empty,
            content: String::new(),
            num: None,
            inline: None,
        }
    }

    pub fn inline_segments(&self) -> Vec<InlineSegment> {
        inline_segments(&self.content, self.inline)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SplitRow {
    pub left: SplitCell,
    pub right: SplitCell,
}

fn flush_split_rows(
    rows: &mut Vec<SplitRow>,
    removed: &mut Vec<SplitCell>,
    added: &mut Vec<SplitCell>,
) {
    let total = removed.len().max(added.len());
    let mut removed = std::mem::take(removed).into_iter();
    let mut added = std::mem::take(added).into_iter();
    for _ in 0..total {
        rows.push(SplitRow {
            left: removed.next().unwrap_or_else(SplitCell::empty),
            right: added.next().unwrap_or_else(SplitCell::empty),
        });
    }
}

/// Empareja borrados con altas para la vista partida. Se acumulan en dos búferes
/// y se vuelcan juntos, de modo que una línea modificada quede a la misma altura
/// en los dos lados en vez de escalonada.
pub fn to_split_rows(lines: &[DiffLine]) -> Vec<SplitRow> {
    let mut rows = Vec::new();
    let mut removed = Vec::new();
    let mut added = Vec::new();

    for line in lines {
        match line.kind {
            DiffLineType::Del => {
                removed.push(SplitCell {
                    kind: SplitCellType::Del,
                    content: line.content.clone(),
                    num: line.old_num,
                    inline: line.inline,
                });
                continue;
            }
            DiffLineType::Add => {
                added.push(SplitCell {
                    kind: SplitCellType::Add,
                    content: line.content.clone(),
                    num: line.new_num,
                    inline: line.inline,
                });
                continue;
            }
            _ => {}
        }

        flush_split_rows(&mut rows, &mut removed, &mut added);

        if line.kind == DiffLineType::Ctx {
            let side = |num| SplitCell {
                kind: SplitCellType::Ctx,
                content: line.content.clone(),
                num,
                inline: None,
            };
            rows.push(SplitRow {
                left: side(line.old_num),
                right: side(line.new_num),
            });
            continue;
        }

        // Los hunks y las notas hablan de las dos versiones a la vez.
        let side = SplitCell {
            kind: line.kind.into(),
            content: line.content.clone(),
            num: None,
            inline: None,
        };
        rows.push(SplitRow {
            left: side.clone(),
            right: side,
        });
    }

    flush_split_rows(&mut rows, &mut removed, &mut added);

    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str = "diff --git a/src/app.js b/src/app.js\nindex 4f77f5a..b3eda6e 100644\n--- a/src/app.js\n+++ b/src/app.js";

    fn diff_with(hunk_lines: &[&str]) -> String {
        format!("{HEADER}\n{}\n", hunk_lines.join("\n"))
    }

    fn hunk(content: &str) -> DiffLine {
        DiffLine::new(DiffLineType::Hunk, content)
    }

    fn ctx(content: &str, old_num: u32, new_num: u32) -> DiffLine {
        DiffLine {
            old_num: Some(old_num),
            new_num: Some(new_num),
            ..DiffLine::new(DiffLineType::Ctx, content)
        }
    }

    fn del(content: &str, old_num: u32) -> DiffLine {
        DiffLine {
            old_num: Some(old_num),
            ..DiffLine::new(DiffLineType::Del, content)
        }
    }

    fn add(content: &str, new_num: u32) -> DiffLine {
        DiffLine {
            new_num: Some(new_num),
            ..DiffLine::new(DiffLineType::Add, content)
        }
    }

    fn contents(lines: &[DiffLine]) -> Vec<&str> {
        lines.iter().map(|line| line.content.as_str()).collect()
    }

    // --- parsing a unified diff ---

    #[test]
    fn numbers_the_lines_from_the_hunk_header() {
        let lines = parse_unified_diff(&diff_with(&[
            "@@ -69,3 +69,3 @@ module.exports = {",
            " const a = 1",
            "-const b = 2",
            "+const b = 3",
        ]));

        assert_eq!(
            lines,
            [
                hunk("@@ -69,3 +69,3 @@ module.exports = {"),
                ctx("const a = 1", 69, 69),
                del("const b = 2", 70),
                add("const b = 3", 70),
            ]
        );
    }

    #[test]
    fn drops_the_file_header() {
        let lines = parse_unified_diff(&diff_with(&["@@ -1,1 +1,1 @@", " ok"]));

        assert_eq!(
            lines.iter().map(|line| line.kind).collect::<Vec<_>>(),
            [DiffLineType::Hunk, DiffLineType::Ctx]
        );
    }

    // El motivo de todo esto: `git show <hash> -- <archivo>` antepone el commit
    // entero, y esas líneas se numeraban como si fueran código del archivo.
    #[test]
    fn ignores_the_commit_that_git_show_puts_before_the_patch() {
        let lines = parse_unified_diff(
            &[
                "commit b46d17dbf12a4565b5971f798f787e88c872502d",
                "Author: Esteban Tapia <tapiam.esteban@gmail.com>",
                "Date:   Sun Jul 26 22:09:44 2026 -0400",
                "",
                "    Deja de comprobar actualizaciones contra un feed que no existe",
                "",
                "    - las releases están en un repositorio privado",
                "",
                HEADER,
                "@@ -69,1 +69,1 @@",
                " sigue igual",
                "",
            ]
            .join("\n"),
        );

        assert_eq!(
            lines,
            [hunk("@@ -69,1 +69,1 @@"), ctx("sigue igual", 69, 69)]
        );
    }

    // Una viñeta del mensaje empieza por `-` y antes se leía como un borrado, así
    // que el commit aparecía a medias dentro del diff.
    #[test]
    fn does_not_read_the_commit_message_as_changes() {
        let lines = parse_unified_diff(
            &[
                "commit abc123",
                "",
                "    - arregla el parser",
                "    + añade tests",
                "",
                HEADER,
                "@@ -1,1 +1,1 @@",
                " intacto",
                "",
            ]
            .join("\n"),
        );

        assert!(lines.iter().all(|line| !line.kind.is_change()));
    }

    // Quitar `-- comentario` (SQL, Lua, Haskell) produce `--- comentario`, que es
    // un borrado y no una cabecera. Antes desaparecía sin dejar rastro.
    #[test]
    fn keeps_a_removed_line_that_starts_like_a_header() {
        let lines = parse_unified_diff(&diff_with(&[
            "@@ -1,2 +1,2 @@",
            "--- comentario viejo",
            "+++ comentario nuevo",
        ]));

        assert_eq!(
            lines,
            [
                hunk("@@ -1,2 +1,2 @@"),
                del("-- comentario viejo", 1),
                add("++ comentario nuevo", 1),
            ]
        );
    }

    // La nota es de git sobre la línea anterior, no una línea del archivo: si
    // consume numeración, corre todo lo que viene después.
    #[test]
    fn does_not_let_the_no_newline_note_shift_the_numbering() {
        let lines = parse_unified_diff(&diff_with(&[
            "@@ -1,3 +1,3 @@",
            "-última sin salto",
            "\\ No newline at end of file",
            "+última con salto",
            " siguiente",
        ]));

        assert_eq!(
            lines[2],
            DiffLine::new(DiffLineType::Note, "No newline at end of file")
        );
        assert_eq!(lines.last(), Some(&ctx("siguiente", 2, 2)));
    }

    #[test]
    fn keeps_a_blank_context_line_which_git_sends_as_a_single_space() {
        let lines = parse_unified_diff(&diff_with(&["@@ -1,3 +1,3 @@", " antes", " ", " después"]));

        assert_eq!(
            contents(&lines),
            ["@@ -1,3 +1,3 @@", "antes", "", "después"]
        );
        assert_eq!(lines.last().and_then(|line| line.old_num), Some(3));
    }

    #[test]
    fn re_anchors_on_each_hunk() {
        let lines = parse_unified_diff(&diff_with(&[
            "@@ -1,1 +1,1 @@",
            " primera",
            "@@ -50,1 +60,1 @@",
            " lejos",
        ]));

        let last = lines.last().unwrap();
        assert_eq!((last.old_num, last.new_num), (Some(50), Some(60)));
    }

    // Un diff de varios archivos: la cabecera del segundo no debe leerse como
    // contenido del primero.
    #[test]
    fn starts_over_on_the_next_file_of_the_same_diff() {
        let lines = parse_unified_diff(
            &[
                HEADER,
                "@@ -1,1 +1,1 @@",
                " del primero",
                "diff --git a/src/otro.js b/src/otro.js",
                "index 111..222 100644",
                "--- a/src/otro.js",
                "+++ b/src/otro.js",
                "@@ -10,1 +10,1 @@",
                " del segundo",
                "",
            ]
            .join("\n"),
        );

        assert_eq!(
            contents(&lines),
            [
                "@@ -1,1 +1,1 @@",
                "del primero",
                "@@ -10,1 +10,1 @@",
                "del segundo"
            ]
        );
    }

    #[test]
    fn has_nothing_to_show_without_a_diff() {
        assert_eq!(parse_unified_diff(""), []);
    }

    // --- folding a diff into two columns ---

    // Lo mismo que en unified, para que las dos vistas no puedan discrepar.
    fn split(hunk_lines: &[&str]) -> Vec<SplitRow> {
        to_split_rows(&parse_unified_diff(&diff_with(hunk_lines)))
    }

    fn cell(kind: SplitCellType, content: &str, num: u32) -> SplitCell {
        SplitCell {
            kind,
            content: content.to_string(),
            num: Some(num),
            inline: None,
        }
    }

    #[test]
    fn puts_a_modified_line_at_the_same_height_on_both_sides() {
        let rows = split(&["@@ -1,1 +1,1 @@", "-antes", "+después"]);

        assert_eq!(
            rows.last(),
            Some(&SplitRow {
                left: cell(SplitCellType::Del, "antes", 1),
                right: cell(SplitCellType::Add, "después", 1),
            })
        );
    }

    #[test]
    fn leaves_a_gap_when_one_side_has_more_lines() {
        let rows = split(&["@@ -1,1 +1,2 @@", "-una", "+una", "+dos"]);

        assert_eq!(
            rows.last(),
            Some(&SplitRow {
                left: SplitCell::empty(),
                right: cell(SplitCellType::Add, "dos", 2),
            })
        );
    }

    #[test]
    fn repeats_context_on_both_sides_with_its_own_numbering() {
        let rows = split(&["@@ -5,1 +9,1 @@", " igual"]);

        assert_eq!(
            rows.last(),
            Some(&SplitRow {
                left: cell(SplitCellType::Ctx, "igual", 5),
                right: cell(SplitCellType::Ctx, "igual", 9),
            })
        );
    }

    #[test]
    fn agrees_with_the_unified_view_on_what_changed() {
        let lines = parse_unified_diff(&diff_with(&[
            "@@ -1,4 +1,4 @@",
            " contexto",
            "--- comentario viejo",
            "+++ comentario nuevo",
            " final",
        ]));
        let rows = to_split_rows(&lines);

        let unified_changes = lines.iter().filter(|line| line.kind.is_change()).count();
        let split_changes = rows
            .iter()
            .filter(|row| row.left.kind.is_change() || row.right.kind.is_change())
            .count();

        assert_eq!(unified_changes, 2);
        assert_eq!(split_changes, 1);
    }

    #[test]
    fn hunks_and_notes_speak_for_both_sides() {
        let rows = split(&[
            "@@ -1,1 +1,1 @@",
            "-a",
            "\\ No newline at end of file",
            "+b",
        ]);

        assert_eq!(rows[0].left.kind, SplitCellType::Hunk);
        assert_eq!(rows[0].left, rows[0].right);
        // La nota corta el emparejado: el borrado se vuelca antes que ella.
        assert_eq!(rows[1].left.kind, SplitCellType::Del);
        assert_eq!(rows[1].right.kind, SplitCellType::Empty);
        assert_eq!(rows[2].left.kind, SplitCellType::Note);
        assert_eq!(rows[3].right.kind, SplitCellType::Add);
    }

    // --- highlighting what changed inside a line ---

    fn slices<'a>(before: &'a str, after: &'a str) -> (&'a str, &'a str) {
        let ranges = compute_inline_ranges(before, after).expect("ranges");
        (
            &before[ranges.before[0]..ranges.before[1]],
            &after[ranges.after[0]..ranges.after[1]],
        )
    }

    // Lo que separa "algo cambió aquí" de "cambió este identificador".
    #[test]
    fn narrows_the_change_down_to_the_word_that_moved() {
        assert_eq!(
            slices(
                "const total = price * quantity",
                "const total = price * amount"
            ),
            ("quantity", "amount")
        );
    }

    #[test]
    fn finds_a_change_in_the_middle() {
        assert_eq!(
            slices("call(alpha, beta)", "call(gamma, beta)"),
            ("alpha", "gamma")
        );
    }

    // Cortar por la mitad de una palabra deja un resaltado que confunde más de lo
    // que ayuda: `getUserName` → `getUserId` marca la palabra, no la "N".
    #[test]
    fn does_not_cut_a_word_in_half() {
        assert_eq!(
            slices("const getUserName = 1", "const getUserId = 1"),
            ("getUserName", "getUserId")
        );
    }

    // El paréntesis de cierre es común a las dos versiones: queda fuera del cambio.
    #[test]
    fn marks_an_addition_without_dragging_along_what_stayed() {
        assert_eq!(slices("run(a)", "run(a, b)").1, ", b");
    }

    // Dos líneas que no se parecen: resaltar un trozo común sería ruido.
    #[test]
    fn says_nothing_when_the_two_lines_have_little_in_common() {
        assert_eq!(
            compute_inline_ranges("const total = 1", "export default createStore()"),
            None
        );
    }

    #[test]
    fn says_nothing_when_almost_everything_changed() {
        assert_eq!(
            compute_inline_ranges("a = 1", "a = 99999999999999999999"),
            None
        );
    }

    #[test]
    fn says_nothing_about_a_line_without_a_counterpart() {
        assert_eq!(compute_inline_ranges("sola", ""), None);
        assert_eq!(compute_inline_ranges("igual", "igual"), None);
    }

    // El borde de la línea cuenta como palabra (ver `is_word_character`): al
    // añadir un cierre tras un identificador, se marca el identificador entero.
    #[test]
    fn stretches_to_the_whole_word_when_one_version_is_a_prefix_of_the_other() {
        assert_eq!(slices("let x = foo", "let x = foo)"), ("foo", "foo)"));
    }

    #[test]
    fn returns_byte_ranges_that_never_split_a_character() {
        let (before, after) = slices("título = «año viejo»", "título = «año nuevo»");

        assert_eq!((before, after), ("viejo", "nuevo"));
    }

    // --- marking a whole diff ---

    fn marked(hunk_lines: &[&str]) -> Vec<DiffLine> {
        mark_inline_changes(&parse_unified_diff(&diff_with(hunk_lines)))
    }

    fn inline_text(line: &DiffLine) -> &str {
        let [from, to] = line.inline.expect("inline range");
        &line.content[from..to]
    }

    #[test]
    fn pairs_each_removal_with_the_addition_that_replaced_it() {
        let lines = marked(&[
            "@@ -1,2 +1,2 @@",
            "-const a = uno",
            "-const b = dos",
            "+const a = UNO",
            "+const b = DOS",
        ]);

        assert_eq!(lines.iter().filter(|line| line.inline.is_some()).count(), 4);
        assert_eq!(inline_text(&lines[1]), "uno");
        assert_eq!(inline_text(&lines[4]), "DOS");
    }

    // Tres fuera y una dentro: solo la primera tiene con quién compararse.
    #[test]
    fn leaves_the_unpaired_ones_alone() {
        let lines = marked(&[
            "@@ -1,3 +1,1 @@",
            "-uno viejo",
            "-dos",
            "-tres",
            "+uno nuevo",
        ]);

        assert_eq!(lines.iter().filter(|line| line.inline.is_some()).count(), 2);
    }

    #[test]
    fn does_not_touch_context_lines() {
        let lines = marked(&["@@ -1,2 +1,2 @@", " igual", "-a = 1", "+a = 2"]);

        assert_eq!(lines[1].inline, None);
    }

    #[test]
    fn does_not_modify_the_lines_it_was_given() {
        let original = parse_unified_diff(&diff_with(&[
            "@@ -1,1 +1,1 @@",
            "-call(alpha)",
            "+call(gamma)",
        ]));
        let marked = mark_inline_changes(&original);

        assert!(original.iter().all(|line| line.inline.is_none()));
        assert!(marked.iter().any(|line| line.inline.is_some()));
    }

    fn segment(text: &str, changed: bool) -> InlineSegment {
        InlineSegment {
            text: text.to_string(),
            changed,
        }
    }

    #[test]
    fn splits_the_line_into_pieces_to_paint() {
        let lines = marked(&["@@ -1,1 +1,1 @@", "-call(alpha)", "+call(gamma)"]);
        let line = lines
            .iter()
            .find(|line| line.kind == DiffLineType::Del)
            .unwrap();

        assert_eq!(
            to_inline_segments(line),
            [
                segment("call(", false),
                segment("alpha", true),
                segment(")", false)
            ]
        );
    }

    #[test]
    fn gives_a_single_piece_when_nothing_stands_out() {
        assert_eq!(
            to_inline_segments(&DiffLine::new(DiffLineType::Ctx, "sin cambios marcados")),
            [segment("sin cambios marcados", false)]
        );
        // Una línea vacía sigue siendo un trozo, para que la fila tenga qué pintar.
        assert_eq!(inline_segments("", None), [segment("", false)]);
        // Un rango que no cae en el texto no rompe nada.
        assert_eq!(
            inline_segments("año", Some([2, 9])),
            [segment("año", false)]
        );
    }

    // La vista partida tiene que pintar lo mismo que la unificada.
    #[test]
    fn carries_the_mark_into_the_split_view() {
        let rows = to_split_rows(&marked(&[
            "@@ -1,1 +1,1 @@",
            "-call(alpha)",
            "+call(gamma)",
        ]));
        let row = rows.last().unwrap();

        assert!(row.left.inline.is_some());
        assert!(row.right.inline.is_some());
        assert_eq!(
            row.left.inline_segments(),
            [
                segment("call(", false),
                segment("alpha", true),
                segment(")", false)
            ]
        );
    }

    #[test]
    fn serializes_with_the_same_field_names_as_the_renderer() {
        let lines = marked(&["@@ -3,2 +3,2 @@", " igual", "-call(alpha)", "+call(gamma)"]);
        let json = serde_json::to_value(&lines).unwrap();

        assert_eq!(
            json[0],
            serde_json::json!({ "type": "hunk", "content": "@@ -3,2 +3,2 @@" })
        );
        assert_eq!(
            json[1],
            serde_json::json!({ "type": "ctx", "content": "igual", "oldNum": 3, "newNum": 3 })
        );
        assert_eq!(
            json[2],
            serde_json::json!({ "type": "del", "content": "call(alpha)", "oldNum": 4, "inline": [5, 10] })
        );

        let rows = serde_json::to_value(to_split_rows(&lines)).unwrap();
        assert_eq!(
            rows[1]["left"],
            serde_json::json!({ "type": "ctx", "content": "igual", "num": 3 })
        );
    }
}
