//! De dónde se baja el manifiesto del canal y los artefactos.

use std::time::{SystemTime, UNIX_EPOCH};

pub const GITHUB_RELEASES_OWNER: &str = "Spilveru";
pub const GITHUB_RELEASES_REPO: &str = "mergeon";

/// Comprueba que `url` use `https://` y no contenga espacios ni caracteres de control.
pub fn is_https_url(url: &str) -> bool {
    let url = url.trim();
    let Some(rest) = url
        .get(..8)
        .filter(|prefix| prefix.eq_ignore_ascii_case("https://"))
        .and_then(|_| url.get(8..))
    else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    !authority.is_empty() && !url.chars().any(|ch| ch.is_whitespace() || ch.is_control())
}

/// Proveedor de actualizaciones, los dos que configura `electron-builder.config.cjs`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateFeed {
    /// Feed genérico (`APP_UPDATE_FEED_URL`): un directorio estático con el
    /// manifiesto y los artefactos, `<url>/<archivo>`.
    Generic { url: String },
    /// GitHub Releases: `https://github.com/<owner>/<repo>/releases/latest/download/<archivo>`.
    GitHub { owner: String, repo: String },
}

impl UpdateFeed {
    pub fn generic(url: impl Into<String>) -> Self {
        Self::Generic { url: url.into() }
    }

    pub fn github(owner: impl Into<String>, repo: impl Into<String>) -> Self {
        Self::GitHub {
            owner: owner.into(),
            repo: repo.into(),
        }
    }

    /// Base sin `/` final y, aparte, la query del feed si la trae.
    fn base_and_query(&self) -> (String, Option<String>) {
        match self {
            Self::Generic { url } => {
                let url = url.trim();
                let url = url.split('#').next().unwrap_or(url);
                let (base, query) = match url.split_once('?') {
                    Some((base, query)) if !query.is_empty() => (base, Some(query.to_string())),
                    Some((base, _)) => (base, None),
                    None => (url, None),
                };
                (base.trim_end_matches('/').to_string(), query)
            }
            Self::GitHub { owner, repo } => (
                format!("https://github.com/{owner}/{repo}/releases/latest/download"),
                None,
            ),
        }
    }

    /// URL del manifiesto del canal.
    ///
    /// Como electron-updater, la query del feed se propaga y, si no hay, el feed
    /// genérico lleva `noCache=<marca>` para saltarse cachés intermedias.
    pub fn manifest_url(&self, file_name: &str, cache_buster: Option<&str>) -> String {
        let (base, query) = self.base_and_query();
        let mut url = format!("{base}/{}", encode_path(file_name));

        match (query, cache_buster, self) {
            (Some(query), _, _) => {
                url.push('?');
                url.push_str(&query);
            }
            (None, Some(stamp), Self::Generic { .. }) => {
                url.push_str("?noCache=");
                url.push_str(stamp);
            }
            _ => {}
        }
        url
    }

    /// URL de un artefacto: el `url` del manifiesto puede ser absoluto,
    /// relativo a la raíz del servidor o (lo normal) relativo al feed.
    pub fn file_url(&self, file: &str) -> String {
        let file = file.trim();
        if file.starts_with("http://") || file.starts_with("https://") {
            return file.to_string();
        }

        let (base, query) = self.base_and_query();
        let mut url = match file.strip_prefix('/') {
            Some(absolute_path) => format!("{}/{}", origin(&base), encode_path(absolute_path)),
            None => format!("{base}/{}", encode_path(file)),
        };
        if let Some(query) = query
            && !file.contains('?')
        {
            url.push('?');
            url.push_str(&query);
        }
        url
    }
}

/// `esquema://host[:puerto]` de una URL.
fn origin(url: &str) -> &str {
    let after_scheme = url.find("://").map_or(0, |index| index + 3);
    match url[after_scheme..].find('/') {
        Some(index) => &url[..after_scheme + index],
        None => url,
    }
}

/// Codifica lo que `new URL()` codificaría en una ruta (espacios, no ASCII...),
/// respetando separadores, query y secuencias `%XX` ya codificadas.
fn encode_path(path: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::with_capacity(path.len());

    for byte in path.bytes() {
        let keep = byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'-' | b'.'
                    | b'_'
                    | b'~'
                    | b'/'
                    | b'%'
                    | b'!'
                    | b'$'
                    | b'&'
                    | b'\''
                    | b'('
                    | b')'
                    | b'*'
                    | b'+'
                    | b','
                    | b';'
                    | b'='
                    | b':'
                    | b'@'
                    | b'?'
            );
        if keep {
            encoded.push(byte as char);
        } else {
            encoded.push('%');
            encoded.push(HEX[(byte >> 4) as usize] as char);
            encoded.push(HEX[(byte & 0x0f) as usize] as char);
        }
    }
    encoded
}

/// `Date.now().toString(32)`: la marca que electron-updater pone en `noCache`.
pub(crate) fn cache_buster_now() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or(0);
    to_base32(millis)
}

fn to_base32(mut value: u128) -> String {
    const DIGITS: &[u8; 32] = b"0123456789abcdefghijklmnopqrstuv";
    if value == 0 {
        return "0".to_string();
    }
    let mut digits = Vec::new();
    while value > 0 {
        digits.push(DIGITS[(value % 32) as usize]);
        value /= 32;
    }
    digits.reverse();
    String::from_utf8_lossy(&digits).into_owned()
}

/// Nombre de archivo local para un artefacto: el último segmento de su URL,
/// sin nada que pueda sacar la descarga del directorio de caché.
pub(crate) fn artifact_file_name(url: &str) -> String {
    let end = url.find(['?', '#']).unwrap_or(url.len());
    let last = url[..end].rsplit(['/', '\\']).next().unwrap_or("");
    let decoded = percent_decode(last);
    let cleaned: String = decoded
        .chars()
        .map(|ch| {
            if ch.is_control() || matches!(ch, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
            {
                '_'
            } else {
                ch
            }
        })
        .collect();
    let cleaned = cleaned.trim().trim_start_matches('.').to_string();
    if cleaned.is_empty() {
        "update.bin".to_string()
    } else {
        cleaned
    }
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && let Some(hex) = text.get(index + 1..index + 3)
            && let Ok(byte) = u8::from_str_radix(hex, 16)
        {
            decoded.push(byte);
            index += 3;
            continue;
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_generic_manifest_urls() {
        let feed = UpdateFeed::generic("https://updates.example.com/mac");
        assert_eq!(
            feed.manifest_url("latest-mac.yml", None),
            "https://updates.example.com/mac/latest-mac.yml"
        );
        assert_eq!(
            feed.manifest_url("latest-mac.yml", Some("1j2k3")),
            "https://updates.example.com/mac/latest-mac.yml?noCache=1j2k3"
        );
        // La barra final del feed no duplica el separador.
        assert_eq!(
            UpdateFeed::generic("https://updates.example.com/win/")
                .manifest_url("latest.yml", None),
            "https://updates.example.com/win/latest.yml"
        );
    }

    #[test]
    fn propagates_the_feed_query_instead_of_the_cache_buster() {
        let feed = UpdateFeed::generic("https://updates.example.com/win?sig=abc");
        assert_eq!(
            feed.manifest_url("latest.yml", Some("stamp")),
            "https://updates.example.com/win/latest.yml?sig=abc"
        );
        assert_eq!(
            feed.file_url("Mergeon-Setup-0.9.1.exe"),
            "https://updates.example.com/win/Mergeon-Setup-0.9.1.exe?sig=abc"
        );
    }

    #[test]
    fn builds_github_release_urls() {
        let feed = UpdateFeed::github(GITHUB_RELEASES_OWNER, GITHUB_RELEASES_REPO);
        assert_eq!(
            feed.manifest_url("latest-linux.yml", Some("stamp")),
            "https://github.com/Spilveru/mergeon/releases/latest/download/latest-linux.yml"
        );
        assert_eq!(
            feed.file_url("Mergeon-0.9.1.AppImage"),
            "https://github.com/Spilveru/mergeon/releases/latest/download/Mergeon-0.9.1.AppImage"
        );
    }

    #[test]
    fn validates_https_urls() {
        assert!(is_https_url("https://github.com/Spilveru/mergeon"));
        assert!(is_https_url("HTTPS://updates.example.com/latest.yml"));
        assert!(!is_https_url("http://updates.example.com/latest.yml"));
        assert!(!is_https_url("https://"));
        assert!(!is_https_url("file:///tmp/latest.yml"));
        assert!(!is_https_url("https://updates.example.com/a b"));
    }

    #[test]
    fn resolves_artifact_urls_relative_to_the_feed() {
        let feed = UpdateFeed::generic("https://updates.example.com/releases/linux");
        assert_eq!(
            feed.file_url("Mergeon-0.9.1.AppImage"),
            "https://updates.example.com/releases/linux/Mergeon-0.9.1.AppImage"
        );
        assert_eq!(
            feed.file_url("Mergeon Setup 0.9.1 ñ.exe"),
            "https://updates.example.com/releases/linux/Mergeon%20Setup%200.9.1%20%C3%B1.exe"
        );
        assert_eq!(
            feed.file_url("already%20encoded.exe"),
            "https://updates.example.com/releases/linux/already%20encoded.exe"
        );
        assert_eq!(
            feed.file_url("/downloads/Mergeon.exe"),
            "https://updates.example.com/downloads/Mergeon.exe"
        );
        assert_eq!(
            feed.file_url("https://cdn.example.com/Mergeon.exe"),
            "https://cdn.example.com/Mergeon.exe"
        );
    }

    #[test]
    fn formats_the_cache_buster_like_number_to_string_32() {
        assert_eq!(to_base32(0), "0");
        assert_eq!(to_base32(31), "v");
        assert_eq!(to_base32(32), "10");
        // (1759363200000).toString(32) en Node.
        assert_eq!(to_base32(1_759_363_200_000), "1j6h3ft00");
        assert!(!cache_buster_now().is_empty());
    }

    #[test]
    fn derives_a_safe_local_file_name() {
        assert_eq!(
            artifact_file_name("https://cdn.example.com/dl/Mergeon-0.9.1.AppImage?sig=1"),
            "Mergeon-0.9.1.AppImage"
        );
        assert_eq!(
            artifact_file_name("Mergeon%20Setup.exe"),
            "Mergeon Setup.exe"
        );
        assert_eq!(artifact_file_name("../../etc/passwd"), "passwd");
        assert_eq!(artifact_file_name("..%2F..%2Fevil.exe"), "_.._evil.exe");
        assert_eq!(artifact_file_name("dir\\..\\evil.exe"), "evil.exe");
        assert_eq!(artifact_file_name("https://cdn.example.com/"), "update.bin");
        assert_eq!(artifact_file_name(".."), "update.bin");
    }
}
