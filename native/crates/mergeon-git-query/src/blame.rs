//! Blame de un archivo del working tree.

use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::error::{QueryResult, read};
use crate::exec;
use crate::fs_util::stat;
use crate::paths::{assert_path_inside_repository, normalize_repository_file_path};
use crate::text::{parse_int, utf16_prefix};

/// Blame de un archivo mayor que esto no se calcula: `--line-porcelain` genera
/// decenas de MB y un objeto por línea que congelan la interfaz.
pub const MAX_BLAME_FILE_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlameLine {
    pub hash: String,
    pub short_hash: String,
    /// Número de línea en el commit que la introdujo, como texto.
    pub original_line: String,
    /// Número de línea en el archivo actual, como texto.
    pub final_line: String,
    pub author: String,
    pub email: String,
    /// Fecha de autor en milisegundos desde época (el `Date` del original).
    pub date: Option<i64>,
    pub summary: String,
    pub content: String,
    /// Ruta del archivo en el commit que introdujo la línea (`filename`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub filename: String,
    /// Padre del commit que introdujo la línea (`previous`), si lo tiene: el
    /// blame de "la revisión anterior" es el de `previous_path` en él.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_path: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlameResult {
    /// Vacío cuando `too_large`.
    pub lines: Vec<BlameLine>,
    /// Bytes del archivo en el working tree.
    pub size: u64,
    pub too_large: bool,
}

fn starts_with_full_hash(line: &str) -> bool {
    let bytes = line.as_bytes();
    bytes.len() >= 40
        && bytes[..40]
            .iter()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

pub(crate) fn parse_line_porcelain(output: &str) -> Vec<BlameLine> {
    let mut blame_data = Vec::new();
    let mut current = BlameLine::default();

    for line in output.split('\n') {
        if line.is_empty() {
            continue;
        }

        // Headers
        if starts_with_full_hash(line) {
            let mut parts = line.split(' ');
            let hash = parts.next().unwrap_or_default();
            current = BlameLine {
                hash: hash.to_string(),
                short_hash: utf16_prefix(hash, 7).to_string(),
                original_line: parts.next().unwrap_or_default().to_string(),
                final_line: parts.next().unwrap_or_default().to_string(),
                ..BlameLine::default()
            };
        } else if let Some(author) = line.strip_prefix("author ") {
            current.author = author.to_string();
        } else if let Some(mail) = line.strip_prefix("author-mail ") {
            current.email = mail.replace(['<', '>'], "");
        } else if let Some(time) = line.strip_prefix("author-time ") {
            current.date = parse_int(time).map(|seconds| seconds.saturating_mul(1000));
        } else if let Some(summary) = line.strip_prefix("summary ") {
            current.summary = summary.to_string();
        } else if let Some(previous) = line.strip_prefix("previous ") {
            if let Some((hash, path)) = previous.split_once(' ') {
                current.previous_hash = Some(hash.to_string());
                current.previous_path = Some(unquote_path(path));
            }
        } else if let Some(filename) = line.strip_prefix("filename ") {
            current.filename = unquote_path(filename);
        } else if let Some(content) = line.strip_prefix('\t') {
            // Content line
            current.content = content.to_string();
            blame_data.push(std::mem::take(&mut current));
        }
    }

    blame_data
}

/// Una ruta de la salida porcelain: con `core.quotePath=false` solo llegan
/// entre comillas (estilo C) las que tienen comillas, barras invertidas o
/// caracteres de control.
fn unquote_path(path: &str) -> String {
    let Some(inner) = path
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
    else {
        return path.to_string();
    };
    let mut bytes = Vec::with_capacity(inner.len());
    let mut chars = inner.bytes().peekable();
    while let Some(byte) = chars.next() {
        if byte != b'\\' {
            bytes.push(byte);
            continue;
        }
        match chars.next() {
            Some(b'n') => bytes.push(b'\n'),
            Some(b't') => bytes.push(b'\t'),
            Some(b'r') => bytes.push(b'\r'),
            Some(b'a') => bytes.push(0x07),
            Some(b'b') => bytes.push(0x08),
            Some(b'f') => bytes.push(0x0c),
            Some(b'v') => bytes.push(0x0b),
            Some(digit @ b'0'..=b'7') => {
                let mut value = u32::from(digit - b'0');
                for _ in 0..2 {
                    match chars.peek() {
                        Some(next @ b'0'..=b'7') => {
                            value = value * 8 + u32::from(next - b'0');
                            chars.next();
                        }
                        _ => break,
                    }
                }
                bytes.push(value as u8);
            }
            Some(other) => bytes.push(other),
            None => bytes.push(b'\\'),
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Canal `git:blame`.
pub fn blame(repo_path: &Path, file_path: &str) -> QueryResult<BlameResult> {
    read("Failed to blame", || {
        let git = exec::open(repo_path)?;
        let normalized_path = normalize_repository_file_path(repo_path, file_path)?;

        // Medir antes de correr: `--line-porcelain` de un archivo grande genera
        // decenas de MB de salida y un objeto por línea.
        let absolute_path = assert_path_inside_repository(repo_path, &normalized_path)?;
        let size = stat(&absolute_path)?.len();
        if size > MAX_BLAME_FILE_BYTES {
            return Ok(BlameResult {
                lines: Vec::new(),
                size,
                too_large: true,
            });
        }

        let output = exec::raw(&git, &["blame", "--line-porcelain", &normalized_path])?;

        Ok(BlameResult {
            lines: parse_line_porcelain(&output),
            size,
            too_large: false,
        })
    })
}

/// Blame de un archivo tal como estaba en `revision` (un commit): lo que pide
/// "Blame previous revision" con el padre del commit de una línea y la ruta
/// que tenía el archivo en él.
pub fn blame_at_revision(
    repo_path: &Path,
    revision: &str,
    file_path: &str,
) -> QueryResult<BlameResult> {
    read("Failed to blame", || {
        let git = exec::open(repo_path)?;
        let normalized_path = normalize_repository_file_path(repo_path, file_path)?;
        let revision = revision.trim();
        if revision.is_empty() || revision.starts_with('-') {
            return Err(format!("Invalid revision: {revision}").into());
        }

        // Medir el blob antes de correr el blame, como con el working tree.
        let reference = format!("{revision}:{normalized_path}");
        let size = exec::raw(&git, &["cat-file", "-s", &reference])?
            .trim()
            .parse::<u64>()
            .unwrap_or(0);
        if size > MAX_BLAME_FILE_BYTES {
            return Ok(BlameResult {
                lines: Vec::new(),
                size,
                too_large: true,
            });
        }

        let output = exec::raw(
            &git,
            &[
                "blame",
                "--line-porcelain",
                revision,
                "--",
                &normalized_path,
            ],
        )?;

        Ok(BlameResult {
            lines: parse_line_porcelain(&output),
            size,
            too_large: false,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TestRepo;

    // Límites de blame con git de verdad: un archivo por encima del cap no debe
    // generar salida gigante, sino un payload con `tooLarge`.
    #[test]
    fn blames_small_files_but_refuses_oversized_ones_with_a_too_large_payload() {
        let repo = TestRepo::new();
        repo.commit_file("small.txt", "one\ntwo\n", "add small");

        let small = blame(&repo.path, "small.txt").unwrap();
        assert!(!small.too_large);
        assert_eq!(small.lines.len(), 2);
        assert_eq!(small.lines[0].content, "one");

        // >1 MB: no se corre el blame, solo se anuncia el tamaño.
        repo.commit_file("big.txt", &"x".repeat(1024 * 1024 + 10), "add big");

        let big = blame(&repo.path, "big.txt").unwrap();
        assert!(big.too_large);
        assert!(big.lines.is_empty());
        assert!(big.size > 1024 * 1024);
    }

    #[test]
    fn attributes_each_line_to_its_commit() {
        let repo = TestRepo::new();
        let first = repo.commit_file("file.txt", "one\ntwo\n", "first commit");
        repo.git(&["config", "user.name", "Other Person"]);
        repo.git(&["config", "user.email", "other@example.com"]);
        let second = repo.commit_file("file.txt", "one\nchanged\n", "second commit");

        let result = blame(&repo.path, "./file.txt").unwrap();

        assert_eq!(result.size, 12);
        let [one, changed] = result.lines.as_slice() else {
            panic!("expected two lines: {:?}", result.lines);
        };
        assert_eq!(one.hash, first);
        assert_eq!(one.short_hash, first[..7]);
        assert_eq!(
            (one.original_line.as_str(), one.final_line.as_str()),
            ("1", "1")
        );
        assert_eq!(one.author, "Test User");
        assert_eq!(one.email, "test@example.com");
        assert_eq!(one.summary, "first commit");
        assert_eq!(one.content, "one");
        assert!(one.date.unwrap() > 1_600_000_000_000);

        assert_eq!(changed.hash, second);
        assert_eq!(changed.author, "Other Person");
        assert_eq!(changed.email, "other@example.com");
        assert_eq!(changed.summary, "second commit");
        assert_eq!(changed.content, "changed");
        assert_eq!(changed.final_line, "2");
    }

    #[test]
    fn parses_line_porcelain_output() {
        let hash = "1234567890abcdef1234567890abcdef12345678";
        let output = [
            &format!("{hash} 3 1 1"),
            "author Ann Example",
            "author-mail <ann@example.com>",
            "author-time 1700000000",
            "author-tz +0000",
            "committer Someone Else",
            "committer-mail <else@example.com>",
            "committer-time 1700000001",
            "committer-tz +0000",
            "summary Add the thing",
            "previous abcdef7890abcdef1234567890abcdef12345678 file.txt",
            "filename file.txt",
            "\tlet x = 1;",
            &format!("{hash} 4 2"),
            "author Ann Example",
            "author-mail <ann@example.com>",
            "author-time 1700000000",
            "summary Add the thing",
            "filename file.txt",
            "\t",
            "",
        ]
        .join("\n");

        let lines = parse_line_porcelain(&output);

        assert_eq!(
            lines[0],
            BlameLine {
                hash: hash.into(),
                short_hash: "1234567".into(),
                original_line: "3".into(),
                final_line: "1".into(),
                author: "Ann Example".into(),
                email: "ann@example.com".into(),
                date: Some(1_700_000_000_000),
                summary: "Add the thing".into(),
                content: "let x = 1;".into(),
                filename: "file.txt".into(),
                previous_hash: Some("abcdef7890abcdef1234567890abcdef12345678".into()),
                previous_path: Some("file.txt".into()),
            }
        );
        // La primera versión de una línea no tiene revisión anterior.
        assert_eq!(lines[1].previous_hash, None);
        // Una línea vacía del archivo también cuenta.
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1].content, "");
        assert_eq!(lines[1].original_line, "4");

        let json = serde_json::to_value(&lines[0]).unwrap();
        assert_eq!(json["shortHash"], "1234567");
        assert_eq!(json["originalLine"], "3");
        assert_eq!(json["date"], 1_700_000_000_000i64);
    }

    #[test]
    fn failures_carry_the_read_prefix() {
        let repo = TestRepo::new();
        repo.commit_file("file.txt", "one\n", "first");

        let error = blame(&repo.path, "../file.txt").unwrap_err();
        assert_eq!(
            error.message(),
            "Failed to blame: The file path must stay inside the repository."
        );

        let error = blame(&repo.path, "gone.txt").unwrap_err();
        assert_eq!(
            error.message(),
            format!(
                "Failed to blame: ENOENT: no such file or directory, stat '{}'",
                repo.path.join("gone.txt").display()
            )
        );

        repo.write("untracked.txt", "x\n");
        let error = blame(&repo.path, "untracked.txt").unwrap_err();
        assert!(
            error.message().starts_with("Failed to blame: fatal:"),
            "{error}"
        );
    }

    #[test]
    fn unquotes_porcelain_paths() {
        assert_eq!(unquote_path("plain name.txt"), "plain name.txt");
        assert_eq!(unquote_path("\"a\\\"b\\\\c\\td.txt\""), "a\"b\\c\td.txt");
        assert_eq!(unquote_path("\"\\303\\261.txt\""), "ñ.txt");
    }

    // "Blame previous revision": el blame en el padre del commit de una línea,
    // con la ruta que tenía el archivo antes de renombrarse.
    #[test]
    fn blames_an_older_revision_through_a_rename() {
        let repo = TestRepo::new();
        let first = repo.commit_file("old.txt", "one\ntwo\n", "first");
        repo.git(&["mv", "old.txt", "new.txt"]);
        repo.git(&["commit", "-m", "rename"]);
        let third = repo.commit_file("new.txt", "one\nTWO\n", "third");

        let now = blame(&repo.path, "new.txt").unwrap();
        let changed = &now.lines[1];
        assert_eq!(changed.hash, third);
        assert_eq!(changed.filename, "new.txt");
        let parent = changed.previous_hash.clone().unwrap();
        assert_eq!(changed.previous_path.as_deref(), Some("new.txt"));

        let before = blame_at_revision(&repo.path, &parent, "new.txt").unwrap();
        assert!(!before.too_large);
        assert_eq!(before.lines.len(), 2);
        assert_eq!(before.lines[1].content, "two");
        assert_eq!(before.lines[1].hash, first);
        // Más atrás que el renombre, la línea vivía en la ruta vieja.
        assert_eq!(before.lines[1].filename, "old.txt");
        assert_eq!(before.lines[1].previous_hash, None);

        let error = blame_at_revision(&repo.path, &first, "new.txt").unwrap_err();
        assert!(
            error.message().starts_with("Failed to blame: fatal:"),
            "{error}"
        );
        let error = blame_at_revision(&repo.path, "--all", "new.txt").unwrap_err();
        assert_eq!(error.message(), "Failed to blame: Invalid revision: --all");
    }
}
