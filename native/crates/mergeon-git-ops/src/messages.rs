//! Limpieza de mensajes de git y clasificación de errores por su texto.
//!
//! Git se ejecuta siempre con `LC_ALL=C`, así que los patrones en inglés son
//! estables.

use regex::Regex;
use std::sync::LazyLock;

const AUTH_ERROR_PATTERNS: [&str; 10] = [
    "authentication failed",
    "could not read username",
    "could not read password",
    "permission denied",
    "repository not found",
    "invalid username or password",
    "access denied",
    "http basic: access denied",
    "support for password authentication was removed",
    "not authorized",
];

const TRANSIENT_FETCH_PATTERNS: [&str; 11] = [
    "could not resolve host",
    "connection timed out",
    "connection reset",
    "connection refused",
    "network is unreachable",
    "the remote end hung up unexpectedly",
    "early eof",
    "failed to connect",
    "returned error: 502",
    "returned error: 503",
    "returned error: 504",
];

/// Espacio en blanco según `\s` / `String.prototype.trim` de JavaScript.
fn is_js_whitespace(character: char) -> bool {
    character.is_whitespace() || character == '\u{feff}'
}

/// `String.prototype.trim`.
pub(crate) fn js_trim(value: &str) -> &str {
    value.trim_matches(is_js_whitespace)
}

/// Quita `label` del inicio (sin distinguir mayúsculas) y el espacio que le sigue.
fn strip_label<'a>(text: &'a str, label: &str) -> &'a str {
    match text.get(..label.len()) {
        Some(head) if head.eq_ignore_ascii_case(label) => {
            text[label.len()..].trim_start_matches(is_js_whitespace)
        }
        _ => text,
    }
}

/// `cleanMessage`: quita los prefijos `error:` / `fatal:` del inicio y recorta;
/// si no queda nada devuelve `fallback`.
pub fn clean_message(message: &str, fallback: &str) -> String {
    let normalized = js_trim(strip_label(strip_label(message, "error:"), "fatal:"));
    if normalized.is_empty() {
        fallback.to_string()
    } else {
        normalized.to_string()
    }
}

/// `capitalize`: primera letra en mayúscula.
pub fn capitalize(value: &str) -> String {
    let mut characters = value.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().chain(characters).collect(),
        None => String::new(),
    }
}

/// `hasAuthError`.
pub fn has_auth_error(message: &str) -> bool {
    let normalized = message.to_lowercase();
    AUTH_ERROR_PATTERNS
        .iter()
        .any(|pattern| normalized.contains(pattern))
}

/// `isAlreadyUpToDate`.
pub fn is_already_up_to_date(message: &str) -> bool {
    let normalized = message.to_lowercase();
    normalized.contains("already up to date")
        || normalized.contains("already up-to-date")
        || normalized.contains("up to date")
        || normalized.contains("up-to-date")
        || normalized.contains("everything up-to-date")
}

/// `isTransientFetchError`: fallos de red que merecen un reintento.
pub fn is_transient_fetch_error(message: &str) -> bool {
    let normalized = message.to_lowercase();
    TRANSIENT_FETCH_PATTERNS
        .iter()
        .any(|pattern| normalized.contains(pattern))
}

/// `hasConflictOutput`.
pub fn has_conflict_output(message: &str) -> bool {
    let normalized = message.to_lowercase();
    normalized.contains("conflict")
        || normalized.contains("automatic merge failed")
        || normalized.contains("resolve all conflicts manually")
        || normalized.contains("after resolving the conflicts")
        || normalized.contains("could not apply")
        || normalized.contains("could not revert")
}

/// `isContinueBlocked`: el `--continue` no pudo avanzar porque quedan conflictos.
pub fn is_continue_blocked(message: &str) -> bool {
    let normalized = message.to_lowercase();
    has_conflict_output(&normalized)
        || normalized.contains("no changes - did you forget to use")
        || normalized.contains("you must edit all merge conflicts")
        || normalized.contains("cannot continue")
        || normalized.contains("needs merge")
}

/// `isCheckoutBlocked`: cambios locales impiden cambiar de rama.
pub fn is_checkout_blocked(message: &str) -> bool {
    let normalized = message.to_lowercase();
    normalized.contains("local changes to the following files would be overwritten")
        || normalized.contains("your local changes to the following files would be overwritten")
        || normalized
            .contains("please commit your changes or stash them before you switch branches")
        || normalized.contains("would be overwritten by checkout")
}

/// `isTagAlreadyExists`.
pub fn is_tag_already_exists(message: &str) -> bool {
    let normalized = message.to_lowercase();
    normalized.contains("tag") && normalized.contains("already exists")
}

static NOISE_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"(?i)^updating(?-u:\b)",
        r"(?i)^fast-forward$",
        r"(?i)^from\s",
        r"(?i)^to\s",
        r"(?i)^merge made by(?-u:\b)",
        r"(?i)^[0-9]+ files? changed(?-u:\b)",
        r"(?i)^[0-9]+ insertions?\(\+\)",
        r"(?i)^[0-9]+ deletions?\(-\)",
        r"^.+\s+\|\s+[0-9]+\s+[-+\s]+$",
        // Resumen de la diffstat: `create mode 100644 ruta`, `delete mode …`,
        // `mode change 100644 => 100755 ruta`, `rename a => b (90%)`,
        // `copy …`, `rewrite …` y binarios (`img.png | Bin 0 -> 12 bytes`).
        r"^(create|delete) mode [0-7]+ ",
        r"^mode change [0-7]+ => [0-7]+ ",
        r"^(rename|copy|rewrite) .+ \([0-9]+%\)$",
        r"^.+\s+\|\s+Bin\b",
    ]
    .iter()
    .map(|pattern| Regex::new(pattern).expect("constant regex"))
    .collect()
});

/// `isGitOutputNoise`: líneas de estadística que no dicen nada al usuario.
pub fn is_git_output_noise(line: &str) -> bool {
    NOISE_PATTERNS.iter().any(|pattern| pattern.is_match(line))
}

/// `normalizeSuccessMessage`: primera línea con significado de la salida de
/// git, o `fallback` si no hay ninguna o es demasiado larga.
pub fn normalize_success_message(raw_output: &str, fallback: &str) -> String {
    let normalized = raw_output.replace("\r\n", "\n");
    let meaningful_line = normalized
        .split('\n')
        .map(js_trim)
        .filter(|line| !line.is_empty())
        .find(|line| !is_git_output_noise(line));

    match meaningful_line {
        Some(line) if line.encode_utf16().count() <= 160 => line.to_string(),
        _ => fallback.to_string(),
    }
}

/// `normalizeBranchName`: `None` si no hay nombre o es `HEAD` (HEAD desanclado).
pub fn normalize_branch_name(branch_name: Option<&str>) -> Option<String> {
    match branch_name {
        Some(name) if !name.is_empty() && name != "HEAD" => Some(name.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_message_strips_git_prefixes_and_falls_back() {
        assert_eq!(clean_message("error: boom\n", "fallback"), "boom");
        assert_eq!(clean_message("FATAL:   boom", "fallback"), "boom");
        assert_eq!(clean_message("error: fatal: boom", "fallback"), "boom");
        // El orden importa: `fatal:` solo se quita después de `error:`.
        assert_eq!(
            clean_message("fatal: error: boom", "fallback"),
            "error: boom"
        );
        assert_eq!(clean_message("  error: boom", "fallback"), "error: boom");
        assert_eq!(clean_message("fatal:\n", "fallback"), "fallback");
        assert_eq!(clean_message("", "fallback"), "fallback");
    }

    #[test]
    fn capitalize_only_touches_the_first_letter() {
        assert_eq!(capitalize("mixed"), "Mixed");
        assert_eq!(capitalize("cherry-pick"), "Cherry-pick");
        assert_eq!(capitalize(""), "");
    }

    #[test]
    fn classifies_errors_by_their_text() {
        assert!(has_auth_error(
            "fatal: Authentication failed for 'https://x'"
        ));
        assert!(has_auth_error("remote: Repository not found."));
        assert!(!has_auth_error("fatal: bad revision"));

        assert!(is_already_up_to_date("Already up to date.\n"));
        assert!(is_already_up_to_date("Everything up-to-date"));
        assert!(!is_already_up_to_date(
            "branch 'main' set up to track 'origin/main'."
        ));

        assert!(is_transient_fetch_error(
            "fatal: Could not resolve host: github.com"
        ));
        assert!(is_transient_fetch_error(
            "The requested URL returned error: 503"
        ));
        assert!(!is_transient_fetch_error("fatal: repository not found"));

        assert!(has_conflict_output(
            "CONFLICT (content): Merge conflict in a.txt"
        ));
        assert!(has_conflict_output(
            "error: could not apply 1234567... change"
        ));
        assert!(!has_conflict_output("Already up to date."));

        assert!(is_continue_blocked(
            "error: Committing is not possible because you have unmerged files.\nfatal: Exiting because of an unresolved conflict."
        ));
        assert!(is_continue_blocked("a.txt: needs merge"));
        assert!(!is_continue_blocked("fatal: no rebase in progress"));

        assert!(is_checkout_blocked(
            "error: Your local changes to the following files would be overwritten by checkout:"
        ));
        assert!(!is_checkout_blocked("error: pathspec 'x' did not match"));

        assert!(is_tag_already_exists("fatal: tag 'v1' already exists"));
        assert!(!is_tag_already_exists("fatal: branch 'v1' already exists"));
    }

    #[test]
    fn success_message_skips_git_statistics() {
        let fast_forward = "Updating 1234567..89abcde\nFast-forward\n file.txt | 2 +-\n 1 file changed, 1 insertion(+), 1 deletion(-)\n";
        assert_eq!(
            normalize_success_message(fast_forward, "Merged."),
            "Merged."
        );
        assert_eq!(
            normalize_success_message("From ../origin\r\nAuto-merging a.txt\r\n", "Merged."),
            "Auto-merging a.txt"
        );
        assert_eq!(normalize_success_message("", "Merged."), "Merged.");
        assert_eq!(
            normalize_success_message(&"x".repeat(161), "Merged."),
            "Merged."
        );
        assert_eq!(
            normalize_success_message(&"x".repeat(160), "Merged."),
            "x".repeat(160)
        );
        assert!(is_git_output_noise("Merge made by the 'ort' strategy."));
        assert!(is_git_output_noise("3 insertions(+)"));
        assert!(is_git_output_noise("To ../origin.git"));
        assert!(is_git_output_noise(
            "create mode 100644 apps/api/src/common/multer-error.interceptor.ts"
        ));
        assert!(is_git_output_noise("delete mode 100755 scripts/old.sh"));
        assert!(is_git_output_noise("mode change 100644 => 100755 run.sh"));
        assert!(is_git_output_noise("rename src/{a.js => b.js} (100%)"));
        assert!(is_git_output_noise("logo.png | Bin 0 -> 1234 bytes"));
        assert!(!is_git_output_noise("Created commit abc1234."));
        // Fast-forward que agrega un archivo: nada de la salida sirve de
        // resumen, así que queda el texto propio de la operación.
        assert_eq!(
            normalize_success_message(
                "Updating 1234567..89abcde\nFast-forward\n apps/api/new.ts | 12 ++++\n 1 file changed, 12 insertions(+)\n create mode 100644 apps/api/new.ts\n",
                "Pull completed successfully."
            ),
            "Pull completed successfully."
        );
        assert!(!is_git_output_noise("Tomorrow"));
    }

    #[test]
    fn head_is_not_a_branch_name() {
        assert_eq!(
            normalize_branch_name(Some("main")),
            Some("main".to_string())
        );
        assert_eq!(normalize_branch_name(Some("HEAD")), None);
        assert_eq!(normalize_branch_name(Some("")), None);
        assert_eq!(normalize_branch_name(None), None);
    }
}
