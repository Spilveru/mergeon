//! Autenticación HTTP para operaciones de red contra GitHub.
//!
//! El token de usuario de GitHub se entrega a `git` mediante el mismo helper
//! de credenciales con archivo efímero `0600` que [`crate::credentials`],
//! acotado exclusivamente a [`GITHUB_ORIGIN`]: nunca viaja en `argv`, en
//! `GIT_CONFIG_PARAMETERS`, en el `Debug` de [`crate::Git`] ni en el entorno
//! de los hooks.

use crate::credentials::HttpCredentials;

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub(crate) fn base64(input: &[u8]) -> String {
    let mut output = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let bytes = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let value = u32::from(bytes[0]) << 16 | u32::from(bytes[1]) << 8 | u32::from(bytes[2]);
        output.push(ALPHABET[(value >> 18) as usize & 63] as char);
        output.push(ALPHABET[(value >> 12) as usize & 63] as char);
        output.push(if chunk.len() > 1 {
            ALPHABET[(value >> 6) as usize & 63] as char
        } else {
            '='
        });
        output.push(if chunk.len() > 2 {
            ALPHABET[value as usize & 63] as char
        } else {
            '='
        });
    }
    output
}

/// El único origen al que se manda el token de la cuenta de GitHub.
pub const GITHUB_ORIGIN: &str = "https://github.com";

/// Credenciales HTTP para autenticar contra `github.com` con `token`, o `None`
/// si el token está ausente o vacío.
pub fn github_credentials(token: Option<&str>) -> Option<HttpCredentials> {
    let token = token.filter(|token| !token.is_empty())?;
    Some(HttpCredentials::new(GITHUB_ORIGIN, "x-access-token", token))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_github_credentials_only_with_a_non_empty_token() {
        assert!(github_credentials(None).is_none());
        assert!(github_credentials(Some("")).is_none());

        let creds = github_credentials(Some("gho_secret")).unwrap();
        assert_eq!(creds.origin(), GITHUB_ORIGIN);
        assert_eq!(creds.username(), "x-access-token");
        assert_eq!(creds.password(), "gho_secret");
        assert!(!format!("{creds:?}").contains("gho_secret"));
    }
}
