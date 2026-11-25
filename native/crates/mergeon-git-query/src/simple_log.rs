//! El `git.log()` / `git.stashList()` de simple-git: mismo `--pretty` y mismo
//! parser, porque la forma de sus entradas es la que viaja a la interfaz.

use serde::{Deserialize, Serialize};

use crate::text::js_trim;

const START_BOUNDARY: &str = "òòòòòò ";
const COMMIT_BOUNDARY: &str = " òò";
const SPLITTER: &str = " ò ";

/// Una entrada de `git.log()` con el formato por defecto de simple-git.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogEntry {
    pub hash: String,
    /// Fecha de autor en ISO 8601 estricto.
    pub date: String,
    pub message: String,
    pub refs: String,
    pub body: String,
    pub author_name: String,
    pub author_email: String,
}

/// Argumento `--pretty` que arma `parseLogOptions` sin opciones.
pub(crate) fn pretty_argument() -> String {
    let format = ["%H", "%aI", "%s", "%D", "%b", "%aN", "%aE"].join(SPLITTER);
    format!("--pretty=format:{START_BOUNDARY}{format}{COMMIT_BOUNDARY}")
}

/// `createListLogSummaryParser`: solo la lista `all`.
pub(crate) fn parse_log_entries(stdout: &str) -> Vec<LogEntry> {
    js_trim(stdout)
        .split(START_BOUNDARY)
        .filter(|item| !item.is_empty())
        .map(|item| {
            let line = item.split(COMMIT_BOUNDARY).next().unwrap_or_default();
            let mut tokens = line.split(SPLITTER);
            let mut next = || tokens.next().unwrap_or_default().to_string();
            LogEntry {
                hash: next(),
                date: next(),
                message: next(),
                refs: next(),
                body: next(),
                author_name: next(),
                author_email: next(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_the_same_pretty_format_as_simple_git() {
        assert_eq!(
            pretty_argument(),
            "--pretty=format:òòòòòò %H ò %aI ò %s ò %D ò %b ò %aN ò %aE òò"
        );
    }

    #[test]
    fn parses_entries_with_empty_and_multiline_fields() {
        let stdout = concat!(
            "òòòòòò aaa ò 2024-01-02T03:04:05+00:00 ò first ò HEAD -> main ò body line\nmore\n ò Ann ò ann@x òò\n",
            "òòòòòò bbb ò 2024-01-01T00:00:00+00:00 ò second ò  ò  ò Bob ò bob@x òò\n",
        );

        let entries = parse_log_entries(stdout);

        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries[0],
            LogEntry {
                hash: "aaa".into(),
                date: "2024-01-02T03:04:05+00:00".into(),
                message: "first".into(),
                refs: "HEAD -> main".into(),
                body: "body line\nmore\n".into(),
                author_name: "Ann".into(),
                author_email: "ann@x".into(),
            }
        );
        assert_eq!(entries[1].refs, "");
        assert_eq!(entries[1].body, "");
        assert_eq!(entries[1].author_name, "Bob");
    }

    #[test]
    fn empty_output_has_no_entries() {
        assert!(parse_log_entries("").is_empty());
        assert!(parse_log_entries("\n").is_empty());
    }
}
