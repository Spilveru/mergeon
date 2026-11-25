//! Reflog de HEAD (`electron/services/reflog.js`).

use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::commit_log::{COMMIT_SEPARATOR, FIELD_SEPARATOR};
use crate::error::{QueryResult, read};
use crate::exec;
use crate::text::js_trim;

const DEFAULT_MAX_COUNT: u32 = 100;

/// Opciones de `git:getReflog`. Un `max_count` ausente o 0 son 100 entradas.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ReflogOptions {
    pub max_count: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReflogEntry {
    pub full_hash: String,
    pub hash: String,
    /// `HEAD@{n}`.
    pub selector: String,
    /// Por qué se movió HEAD (`%gs`).
    pub subject: String,
    pub date: String,
}

/*
 * Reflog de HEAD: la red de seguridad. Cada entrada dice a qué commit apuntaba
 * HEAD y por qué se movió (%gs), en orden del más reciente al más viejo.
 */
pub(crate) fn parse_reflog_output(output: &str) -> Vec<ReflogEntry> {
    output
        .split(COMMIT_SEPARATOR)
        .map(js_trim)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            let mut fields = entry.split(FIELD_SEPARATOR);
            let mut next = || fields.next().unwrap_or_default().to_string();
            ReflogEntry {
                full_hash: next(),
                hash: next(),
                selector: next(),
                subject: next(),
                date: next(),
            }
        })
        .filter(|entry| !entry.full_hash.is_empty())
        .collect()
}

/// Canal `git:getReflog`.
pub fn get_reflog(repo_path: &Path, options: &ReflogOptions) -> QueryResult<Vec<ReflogEntry>> {
    read("Failed to read the reflog", || {
        let max_count = options
            .max_count
            .filter(|count| *count != 0)
            .unwrap_or(DEFAULT_MAX_COUNT);
        let format = ["%H", "%h", "%gd", "%gs", "%aI"].join(&FIELD_SEPARATOR.to_string());

        // Sin --date: con él, %gd muestra timestamps en vez de HEAD@{n}.
        // %aI ya es ISO estricto por sí mismo.
        let output = exec::raw(
            &exec::open(repo_path)?,
            &[
                "reflog".to_string(),
                format!("--max-count={max_count}"),
                format!("--pretty=format:{format}{COMMIT_SEPARATOR}"),
            ],
        )?;

        Ok(parse_reflog_output(&output))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TestRepo;

    #[test]
    fn lists_head_movements_newest_first_with_selector_and_subject() {
        let repo = TestRepo::new();
        let first = repo.commit_file("file.txt", "one\n", "first commit");
        repo.commit_file("file.txt", "two\n", "second commit");

        repo.git(&["reset", "--hard", &first]);

        let entries = get_reflog(&repo.path, &ReflogOptions::default()).unwrap();

        assert!(entries.len() >= 3);
        assert_eq!(entries[0].selector, "HEAD@{0}");
        assert_eq!(entries[0].full_hash, first);
        assert!(first.starts_with(&entries[0].hash));
        assert!(entries[0].subject.contains("reset"));
        assert!(entries[0].date.contains('T'));
        // La entrada del segundo commit sigue ahí: es la red de seguridad
        assert!(
            entries
                .iter()
                .any(|entry| entry.subject.contains("second commit"))
        );
    }

    #[test]
    fn respects_max_count() {
        let repo = TestRepo::new();
        repo.commit_file("file.txt", "one\n", "first commit");
        repo.commit_file("file.txt", "two\n", "second commit");
        repo.commit_file("file.txt", "three\n", "third commit");

        let options = ReflogOptions { max_count: Some(2) };
        let entries = get_reflog(&repo.path, &options).unwrap();
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn skips_entries_without_a_hash() {
        let entries = parse_reflog_output("\u{1f}x\u{1e}\nabc\u{1f}ab\u{1f}HEAD@{0}\u{1e}");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].full_hash, "abc");
        assert_eq!(entries[0].selector, "HEAD@{0}");
        assert_eq!(entries[0].subject, "");
    }

    #[test]
    fn fails_with_the_read_prefix_outside_a_repository() {
        let dir = tempfile::tempdir().unwrap();
        let error = get_reflog(dir.path(), &ReflogOptions::default()).unwrap_err();
        assert!(
            error.message().starts_with("Failed to read the reflog: "),
            "{error}"
        );
    }
}
