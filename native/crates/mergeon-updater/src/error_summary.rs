use std::sync::LazyLock;

use regex::Regex;

pub const MAX_ERROR_MESSAGE_LENGTH: usize = 140;

// `\b` de JS es ASCII; el de `regex` es Unicode salvo que se le quite la `u`.
static STATUS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?-u:\b)(40[0-9]|41[0-9]|4[2-9][0-9]|5[0-9][0-9])(?-u:\b)")
        .expect("regex constante")
});
static NETWORK_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)ENOTFOUND|EAI_AGAIN|ECONNREFUSED|ECONNRESET|ETIMEDOUT|network")
        .expect("regex constante")
});
static REQUEST_DUMP_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s(?:method:|Headers:)").expect("regex constante"));
static WHITESPACE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s+").expect("regex constante"));

// electron-updater mete la respuesta HTTP entera en `error.message`: cuerpo,
// cabeceras y hasta la cookie de sesión de GitHub. Eso terminaba tal cual en un
// toast de varias pantallas. El volcado completo sigue yendo al log.
pub fn summarize_updater_error(message: &str) -> String {
    let raw = message.trim();

    if raw.is_empty() {
        return "Error desconocido".to_string();
    }

    let status = STATUS_RE
        .captures(raw)
        .and_then(|captures| captures.get(1))
        .map(|found| found.as_str());

    if status == Some("404") {
        return "No se encontró el canal de actualizaciones (404). Revisa que las releases del repositorio sean públicas.".to_string();
    }

    if let Some(status @ ("401" | "403")) = status {
        return format!("El servidor de actualizaciones rechazó la petición ({status}).");
    }

    if NETWORK_RE.is_match(raw) {
        return "No se pudo contactar con el servidor de actualizaciones.".to_string();
    }

    // Caso general: primera línea, cortada antes del volcado de la petición.
    let first_line = raw.split('\n').next().unwrap_or("");
    let before_dump = REQUEST_DUMP_RE.split(first_line).next().unwrap_or("");
    let trimmed = WHITESPACE_RE.replace_all(before_dump, " ");
    let trimmed = trimmed.trim();

    if trimmed.chars().count() > MAX_ERROR_MESSAGE_LENGTH {
        let head: String = trimmed.chars().take(MAX_ERROR_MESSAGE_LENGTH - 1).collect();
        format!("{}…", head.trim_end())
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    // Mensaje de ejemplo estilo electron-updater al consultar el feed de un repo privado.
    pub(crate) const GITHUB_404_MESSAGE: &str = "404 \"method: GET url: https://github.com/Spilveru/mergeon/releases.atom\n\nPlease double check that your authentication token is correct. Due to security reasons, actual status maybe not reported, but 404.\\n\" Headers: {\n  \"cache-control\": \"no-cache\",\n  \"set-cookie\": \"_gh_sess=example-dummy-session-token; path=/; HttpOnly; secure; SameSite=Lax\"\n}";

    #[test]
    fn condenses_updater_errors_instead_of_surfacing_the_raw_http_dump() {
        let summary = summarize_updater_error(GITHUB_404_MESSAGE);

        assert_eq!(
            summary,
            "No se encontró el canal de actualizaciones (404). Revisa que las releases del repositorio sean públicas."
        );
        // Lo que hacía ilegible el aviso: cabeceras, cuerpo y la cookie de sesión.
        assert!(!summary.contains("set-cookie"));
        assert!(!summary.contains("_gh_sess"));
        assert!(!summary.contains("Headers"));
        assert!(summary.chars().count() < 200);
    }

    #[test]
    fn keeps_short_updater_errors_readable_and_caps_the_long_ones() {
        assert_eq!(
            summarize_updater_error("feed unavailable"),
            "feed unavailable"
        );
        assert_eq!(
            summarize_updater_error("getaddrinfo ENOTFOUND updates.example.com"),
            "No se pudo contactar con el servidor de actualizaciones."
        );
        assert_eq!(
            summarize_updater_error("403 Forbidden"),
            "El servidor de actualizaciones rechazó la petición (403)."
        );
        let capped = summarize_updater_error(&"x".repeat(400));
        assert_eq!(capped.chars().count(), 140);
        assert!(capped.ends_with('…'));
        assert_eq!(summarize_updater_error(""), "Error desconocido");
    }

    #[test]
    fn cuts_the_general_case_before_the_request_dump_and_collapses_whitespace() {
        assert_eq!(
            summarize_updater_error(
                "Cannot  parse\tupdate info method: GET url: https://x\nsecond line"
            ),
            "Cannot parse update info"
        );
        assert_eq!(
            summarize_updater_error("401 Unauthorized"),
            "El servidor de actualizaciones rechazó la petición (401)."
        );
        // Un número que no es un estado HTTP aislado no cuenta.
        assert_eq!(
            summarize_updater_error("build 14043 failed"),
            "build 14043 failed"
        );
        assert_eq!(summarize_updater_error("   "), "Error desconocido");
    }

    #[test]
    fn maps_the_native_transport_errors_to_the_same_messages() {
        use crate::error::UpdaterError;

        let network = UpdaterError::Network {
            url: "https://updates.example.com/latest.yml".to_string(),
            detail: "io: failed to lookup address information".to_string(),
        };
        assert_eq!(
            summarize_updater_error(&network.to_string()),
            "No se pudo contactar con el servidor de actualizaciones."
        );

        let not_found = UpdaterError::HttpStatus {
            status: 404,
            url: "https://updates.example.com/latest.yml".to_string(),
        };
        assert_eq!(
            summarize_updater_error(&not_found.to_string()),
            "No se encontró el canal de actualizaciones (404). Revisa que las releases del repositorio sean públicas."
        );

        let forbidden = UpdaterError::HttpStatus {
            status: 403,
            url: "https://updates.example.com/latest.yml".to_string(),
        };
        assert_eq!(
            summarize_updater_error(&forbidden.to_string()),
            "El servidor de actualizaciones rechazó la petición (403)."
        );

        let server = UpdaterError::HttpStatus {
            status: 503,
            url: "https://updates.example.com/latest.yml".to_string(),
        };
        assert_eq!(
            summarize_updater_error(&server.to_string()),
            "El servidor de actualizaciones respondió 503."
        );

        // El hash no llega al aviso: ni se trunca ni se confunde con un estado HTTP.
        let checksum = UpdaterError::ChecksumMismatch {
            expected: "+404/aaaa".to_string(),
            actual: "bbbb".to_string(),
        };
        assert_eq!(
            summarize_updater_error(&checksum.to_string()),
            "La descarga de la actualización está dañada (el sha512 no coincide)."
        );
    }
}
