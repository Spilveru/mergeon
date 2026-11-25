//! Los commits a los que se puede dirigir un fixup ("Fixup into…" de la caja
//! de commit): los últimos de la rama actual, diciendo cuáles ya están
//! publicados.

use std::collections::HashSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Inner, QueryResult, read};
use crate::exec;

/// Cuántos commits ofrece "Fixup into…" por defecto.
pub const MAX_FIXUP_CANDIDATES: usize = 30;

/// Un commit de la rama actual que puede recibir un fixup.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FixupCandidate {
    pub hash: String,
    pub short_hash: String,
    pub subject: String,
    /// Ya está en el upstream de la rama o en alguna rama remota: combinar el
    /// fixup reescribe historia publicada.
    pub published: bool,
}

/// Los últimos `limit` commits de HEAD sin merges (del más nuevo al más
/// viejo), marcando los publicados. Vacío en un repositorio sin commits.
///
/// Dos lecturas acotadas a `limit`: `git log` y un `git rev-list` con las
/// mismas reglas de recorrido que excluye lo alcanzable desde las ramas
/// remotas y el upstream. Lo no publicado de los `limit` primeros del log
/// sale siempre entre los `limit` primeros del rev-list.
pub fn get_fixup_candidates(repo_path: &Path, limit: usize) -> QueryResult<Vec<FixupCandidate>> {
    read("Failed to read the commits of the branch", || {
        fixup_candidates(repo_path, limit)
    })
}

fn fixup_candidates(repo_path: &Path, limit: usize) -> Inner<Vec<FixupCandidate>> {
    let git = exec::open(repo_path)?;
    if limit == 0 || !exec::exec(&git, &["rev-parse", "--verify", "--quiet", "HEAD"])?.success() {
        return Ok(Vec::new());
    }
    let max_count = format!("--max-count={limit}");

    let log = exec::raw(
        &git,
        &[
            "log",
            "-z",
            "--no-merges",
            max_count.as_str(),
            "--format=%H%x1f%h%x1f%s",
            "HEAD",
            "--",
        ],
    )?;

    let upstream = exec::exec(
        &git,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            "--symbolic-full-name",
            "@{upstream}",
        ],
    )?;
    let upstream = upstream
        .success()
        .then(|| upstream.stdout_text().trim().to_string())
        .filter(|upstream| !upstream.is_empty());
    let mut arguments = vec![
        "rev-list",
        "--no-merges",
        max_count.as_str(),
        "HEAD",
        "--not",
        "--remotes",
    ];
    if let Some(upstream) = &upstream {
        arguments.push(upstream.as_str());
    }
    arguments.push("--");
    let unpublished: HashSet<String> = exec::raw(&git, &arguments)?
        .lines()
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
        .collect();

    Ok(parse_candidates(&log, &unpublished))
}

fn parse_candidates(log: &str, unpublished: &HashSet<String>) -> Vec<FixupCandidate> {
    log.split('\0')
        .filter_map(|record| {
            let record = record.trim_start_matches('\n');
            let mut fields = record.splitn(3, '\u{1f}');
            let hash = fields.next()?.trim();
            if hash.is_empty() {
                return None;
            }
            let short_hash = fields.next().unwrap_or_default().trim();
            let subject = fields.next().unwrap_or_default().trim_end();
            Some(FixupCandidate {
                hash: hash.to_string(),
                short_hash: short_hash.to_string(),
                subject: subject.to_string(),
                published: !unpublished.contains(hash),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TestRepo;

    fn subjects(candidates: &[FixupCandidate]) -> Vec<(&str, bool)> {
        candidates
            .iter()
            .map(|candidate| (candidate.subject.as_str(), candidate.published))
            .collect()
    }

    #[test]
    fn an_empty_repository_has_no_candidates() {
        let repo = TestRepo::new();
        assert!(get_fixup_candidates(&repo.path, 30).unwrap().is_empty());
    }

    #[test]
    fn without_a_remote_every_commit_is_unpublished() {
        let repo = TestRepo::new();
        repo.commit_file("a.txt", "a\n", "first");
        let second = repo.commit_file("b.txt", "b\n", "second: ñandú");

        let candidates = get_fixup_candidates(&repo.path, 30).unwrap();

        assert_eq!(
            subjects(&candidates),
            [("second: ñandú", false), ("first", false)]
        );
        assert_eq!(candidates[0].hash, second);
        assert!(second.starts_with(&candidates[0].short_hash));
        assert!(candidates[0].short_hash.len() >= 7);
    }

    #[test]
    fn marks_what_the_upstream_already_has_and_respects_the_limit() {
        let repo = TestRepo::new();
        repo.add_bare_origin();
        repo.commit_file("a.txt", "a\n", "pushed one");
        repo.commit_file("b.txt", "b\n", "pushed two");
        repo.git(&["push", "-u", "origin", "main"]);
        repo.commit_file("c.txt", "c\n", "local one");
        repo.commit_file("d.txt", "d\n", "local two");

        let candidates = get_fixup_candidates(&repo.path, 30).unwrap();
        assert_eq!(
            subjects(&candidates),
            [
                ("local two", false),
                ("local one", false),
                ("pushed two", true),
                ("pushed one", true),
            ]
        );

        let limited = get_fixup_candidates(&repo.path, 3).unwrap();
        assert_eq!(
            subjects(&limited),
            [
                ("local two", false),
                ("local one", false),
                ("pushed two", true)
            ]
        );
    }

    #[test]
    fn skips_merge_commits() {
        let repo = TestRepo::new();
        repo.commit_file("a.txt", "a\n", "base");
        repo.git(&["checkout", "-q", "-b", "topic"]);
        repo.commit_file("t.txt", "t\n", "topic work");
        repo.git(&["checkout", "-q", "main"]);
        repo.commit_file("m.txt", "m\n", "main work");
        repo.git(&["merge", "--no-ff", "-q", "-m", "merge topic", "topic"]);

        let candidates = get_fixup_candidates(&repo.path, 30).unwrap();
        assert!(
            candidates
                .iter()
                .all(|candidate| candidate.subject != "merge topic")
        );
        assert_eq!(candidates.len(), 3);
    }

    #[test]
    fn parses_records_without_trailing_separator() {
        let unpublished = HashSet::from(["b".repeat(40)]);
        let log = format!(
            "{}\u{1f}aaaaaaa\u{1f}old\0\n{}\u{1f}bbbbbbb\u{1f}new one\0",
            "a".repeat(40),
            "b".repeat(40)
        );
        let candidates = parse_candidates(&log, &unpublished);
        assert_eq!(subjects(&candidates), [("old", true), ("new one", false)]);
    }
}
