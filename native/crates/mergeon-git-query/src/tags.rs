//! Tags: listado y detalle.

use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::error::{QueryResult, read};
use crate::exec;
use crate::text::js_trim;

/// Canal `git:getTags`: nombres de tag, del más reciente al más antiguo.
pub fn get_tags(repo_path: &Path) -> QueryResult<Vec<String>> {
    read("Failed to get tags", || {
        let raw = exec::raw(&exec::open(repo_path)?, &["tag", "--sort=-creatordate"])?;
        Ok(raw
            .split('\n')
            .map(js_trim)
            .filter(|tag| !tag.is_empty())
            .map(str::to_string)
            .collect())
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagDetails {
    pub name: String,
    pub is_annotated: bool,
    /// Commit al que apunta el tag (ya desreferenciado si es anotado).
    pub target_hash: String,
    pub tagger_name: Option<String>,
    pub tagger_email: Option<String>,
    pub tagger_date: Option<String>,
    /// Mensaje de la anotación; vacío en tags ligeros.
    pub message: String,
}

fn non_empty(text: &str) -> Option<String> {
    (!text.is_empty()).then(|| text.to_string())
}

pub(crate) fn parse_tag_details(tag_name: &str, raw: &str) -> TagDetails {
    let raw = raw.strip_suffix('\n').unwrap_or(raw);
    let mut parts = raw.split('\u{1f}');
    let mut next = || js_trim(parts.next().unwrap_or_default());

    let object_type = next();
    let object_name = next();
    let deref_name = next();
    let tagger_name = next();
    let tagger_email = next();
    let tagger_email = tagger_email.strip_prefix('<').unwrap_or(tagger_email);
    let tagger_email = tagger_email.strip_suffix('>').unwrap_or(tagger_email);
    let tagger_date = next();
    let contents = next();

    let is_annotated = object_type == "tag";

    TagDetails {
        name: tag_name.to_string(),
        is_annotated,
        target_hash: if deref_name.is_empty() {
            object_name.to_string()
        } else {
            deref_name.to_string()
        },
        tagger_name: non_empty(tagger_name),
        tagger_email: non_empty(tagger_email),
        tagger_date: non_empty(tagger_date),
        // %(contents) on a lightweight tag returns the *commit* message, not a tag annotation.
        message: if is_annotated {
            contents.to_string()
        } else {
            String::new()
        },
    }
}

/// Canal `git:getTagDetails`: anotación, autor del tag y commit de destino.
pub fn get_tag_details(repo_path: &Path, tag_name: &str) -> QueryResult<TagDetails> {
    read("Failed to get tag details", || {
        let git = exec::open(repo_path)?;
        // Use a unit-separator delimiter (robust against empty fields and multi-line contents).
        let format = [
            "%(objecttype)",
            "%(objectname)",
            "%(*objectname)",
            "%(taggername)",
            "%(taggeremail)",
            "%(taggerdate:iso)",
            "%(contents)",
        ]
        .join("\u{1f}");
        let raw = exec::raw(
            &git,
            &[
                "for-each-ref".to_string(),
                format!("refs/tags/{tag_name}"),
                format!("--format={format}"),
            ],
        )?;

        // for-each-ref no falla con un patrón que no casa: devuelve vacío. La
        // app Electron devolvía entonces un detalle en blanco (sin commit de
        // destino); un tag que no existe es un error de lectura.
        if js_trim(&raw).is_empty() {
            return Err(format!("Tag '{tag_name}' not found.").into());
        }

        Ok(parse_tag_details(tag_name, &raw))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TestRepo;
    use mergeon_gitcmd::Git;

    #[test]
    fn lists_tags_newest_first() {
        let repo = TestRepo::new();
        repo.commit_file("file.txt", "a\n", "first");
        assert!(get_tags(&repo.path).unwrap().is_empty());

        let dated = |date: &str, args: &[&str]| {
            Git::new(&repo.path)
                .with_env("GIT_COMMITTER_DATE", date)
                .raw(args)
                .unwrap();
        };
        dated("2024-01-01T00:00:00Z", &["tag", "-a", "v1.0", "-m", "one"]);
        dated(
            "2024-03-01T00:00:00Z",
            &["tag", "-a", "v1.2", "-m", "three"],
        );
        dated("2024-02-01T00:00:00Z", &["tag", "-a", "v1.1", "-m", "two"]);

        assert_eq!(get_tags(&repo.path).unwrap(), ["v1.2", "v1.1", "v1.0"]);
    }

    #[test]
    fn describes_an_annotated_tag() {
        let repo = TestRepo::new();
        let commit = repo.commit_file("file.txt", "a\n", "first");
        repo.git(&["tag", "-a", "v1.0", "-m", "Release 1.0\n\nWith notes"]);

        let details = get_tag_details(&repo.path, "v1.0").unwrap();

        assert_eq!(details.name, "v1.0");
        assert!(details.is_annotated);
        assert_eq!(details.target_hash, commit);
        assert_eq!(details.tagger_name.as_deref(), Some("Test User"));
        assert_eq!(details.tagger_email.as_deref(), Some("test@example.com"));
        assert!(details.tagger_date.is_some());
        assert_eq!(details.message, "Release 1.0\n\nWith notes");
    }

    #[test]
    fn a_lightweight_tag_has_no_annotation() {
        let repo = TestRepo::new();
        let commit = repo.commit_file("file.txt", "a\n", "commit message");
        repo.git(&["tag", "light"]);

        let details = get_tag_details(&repo.path, "light").unwrap();

        assert_eq!(
            details,
            TagDetails {
                name: "light".into(),
                is_annotated: false,
                target_hash: commit,
                tagger_name: None,
                tagger_email: None,
                tagger_date: None,
                message: String::new(),
            }
        );

        let json = serde_json::to_value(&details).unwrap();
        assert_eq!(json["isAnnotated"], false);
        assert_eq!(json["taggerName"], serde_json::Value::Null);
        assert!(json["targetHash"].is_string());
    }

    // La app Electron devolvía aquí un detalle vacío (sin commit de destino).
    #[test]
    fn an_unknown_tag_fails_with_the_read_prefix() {
        let repo = TestRepo::new();
        repo.commit_file("file.txt", "a\n", "first");
        repo.git(&["tag", "v1"]);

        assert_eq!(
            get_tag_details(&repo.path, "nope").unwrap_err().message(),
            "Failed to get tag details: Tag 'nope' not found."
        );
        assert!(get_tag_details(&repo.path, "v1").is_ok());
    }

    #[test]
    fn fails_with_the_read_prefix_outside_a_repository() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            get_tags(dir.path())
                .unwrap_err()
                .message()
                .starts_with("Failed to get tags: ")
        );
        assert!(
            get_tag_details(dir.path(), "v1")
                .unwrap_err()
                .message()
                .starts_with("Failed to get tag details: ")
        );
    }
}
