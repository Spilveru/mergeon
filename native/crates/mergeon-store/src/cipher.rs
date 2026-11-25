//! Cifrado de tokens: el papel que `safeStorage` cumple en `store.js`.

use std::sync::atomic::{AtomicBool, Ordering};

use crate::error::StoreError;

/// Cifrador de secretos respaldado por el sistema (el `safeStorage` de
/// Electron). El `Store` lo recibe inyectado; la implementación real con el
/// llavero del sistema la aporta la app.
pub trait SecretCipher: Send + Sync {
    /// `safeStorage.isEncryptionAvailable()`. Si devuelve `false`, los `safe:`
    /// no se pueden leer y los tokens nuevos solo se guardan como `plain:` si
    /// [`Self::allows_plaintext_fallback`] devuelve `true`.
    fn is_available(&self) -> bool;

    /// Si se permite guardar tokens nuevos en claro (`plain:<token>`) cuando
    /// [`Self::is_available`] es `false`. Por defecto es `false` para no
    /// persistir secretos sin cifrar en silencio.
    fn allows_plaintext_fallback(&self) -> bool {
        false
    }

    /// `safeStorage.encryptString(token).toString('base64')`: devuelve el
    /// texto que se guarda tras el prefijo `safe:`. `None` si no pudo cifrar.
    fn encrypt(&self, plain_text: &str) -> Option<String>;

    /// `safeStorage.decryptString(Buffer.from(payload, 'base64'))`. `None` si
    /// el contenido no se pudo descifrar.
    fn decrypt(&self, payload: &str) -> Option<String>;
}

/// Sin cifrado, de forma explícita: pensado para tests y stores en memoria.
/// Los tokens quedan en el archivo como `plain:<token>`, legibles por
/// cualquiera que pueda leerlo.
#[derive(Debug, Clone, Copy, Default)]
pub struct PlainTextCipher;

impl SecretCipher for PlainTextCipher {
    fn is_available(&self) -> bool {
        false
    }

    fn allows_plaintext_fallback(&self) -> bool {
        true
    }

    fn encrypt(&self, _plain_text: &str) -> Option<String> {
        None
    }

    fn decrypt(&self, _payload: &str) -> Option<String> {
        None
    }
}

/// Cifrador reversible para tests: **no protege nada**. Sirve para ejercitar
/// el camino `safe:` sin depender del llavero del sistema, y permite simular
/// que el cifrado deja de estar disponible.
#[derive(Debug)]
pub struct TestCipher {
    available: AtomicBool,
    allow_plaintext_fallback: AtomicBool,
}

const TEST_CIPHER_KEY: u8 = 0x5a;

impl TestCipher {
    pub fn new() -> Self {
        Self {
            available: AtomicBool::new(true),
            allow_plaintext_fallback: AtomicBool::new(false),
        }
    }

    /// Como `safeStorage.isEncryptionAvailable() === false`.
    pub fn unavailable() -> Self {
        Self {
            available: AtomicBool::new(false),
            allow_plaintext_fallback: AtomicBool::new(false),
        }
    }

    pub fn set_available(&self, available: bool) {
        self.available.store(available, Ordering::SeqCst);
    }

    pub fn set_allow_plaintext_fallback(&self, allow: bool) {
        self.allow_plaintext_fallback.store(allow, Ordering::SeqCst);
    }
}

impl Default for TestCipher {
    fn default() -> Self {
        Self::new()
    }
}

impl SecretCipher for TestCipher {
    fn is_available(&self) -> bool {
        self.available.load(Ordering::SeqCst)
    }

    fn allows_plaintext_fallback(&self) -> bool {
        self.allow_plaintext_fallback.load(Ordering::SeqCst)
    }

    fn encrypt(&self, plain_text: &str) -> Option<String> {
        if !self.is_available() {
            return None;
        }

        Some(
            plain_text
                .bytes()
                .rev()
                .map(|byte| format!("{:02x}", byte ^ TEST_CIPHER_KEY))
                .collect(),
        )
    }

    fn decrypt(&self, payload: &str) -> Option<String> {
        if !self.is_available() || !payload.len().is_multiple_of(2) || !payload.is_ascii() {
            return None;
        }

        let mut bytes = (0..payload.len())
            .step_by(2)
            .map(|index| u8::from_str_radix(&payload[index..index + 2], 16).ok())
            .map(|byte| byte.map(|byte| byte ^ TEST_CIPHER_KEY))
            .collect::<Option<Vec<u8>>>()?;
        bytes.reverse();
        String::from_utf8(bytes).ok()
    }
}

/// `encodeToken`: `safe:<cifrado>` si hay cifrado; si no hay cifrado, solo cae
/// a `plain:<token>` cuando el cifrador lo autoriza explícitamente, y en caso
/// contrario falla con [`StoreError::KeyringUnavailable`]. Un token vacío se
/// guarda vacío.
pub fn encode_token(cipher: &dyn SecretCipher, token: &str) -> Result<String, StoreError> {
    if token.is_empty() {
        return Ok(String::new());
    }

    if cipher.is_available() {
        return cipher
            .encrypt(token)
            .map(|payload| format!("safe:{payload}"))
            .ok_or(StoreError::Encryption);
    }

    if cipher.allows_plaintext_fallback() {
        return Ok(format!("plain:{token}"));
    }

    Err(StoreError::KeyringUnavailable)
}

/// `decodeToken` de `store.js`. Un valor sin esquema es un token antiguo en
/// claro; un `safe:` que no se puede descifrar se lee como vacío.
pub fn decode_token(cipher: &dyn SecretCipher, encoded_token: &str) -> String {
    if encoded_token.is_empty() {
        return String::new();
    }

    let Some((scheme, payload)) = encoded_token.split_once(':') else {
        return encoded_token.to_string();
    };

    match scheme {
        "safe" => {
            if !cipher.is_available() {
                return String::new();
            }

            cipher.decrypt(payload).unwrap_or_default()
        }
        "plain" => payload.to_string(),
        _ => encoded_token.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_cipher_falls_back_to_the_plain_scheme() {
        let cipher = PlainTextCipher;
        assert_eq!(
            encode_token(&cipher, "gho_token").unwrap(),
            "plain:gho_token"
        );
        assert_eq!(decode_token(&cipher, "plain:gho_token"), "gho_token");
        assert_eq!(encode_token(&cipher, "").unwrap(), "");
    }

    #[test]
    fn decode_matches_the_original_edge_cases() {
        let cipher = PlainTextCipher;
        assert_eq!(decode_token(&cipher, ""), "");
        // Token antiguo guardado en claro, sin esquema.
        assert_eq!(decode_token(&cipher, "gho_legacy"), "gho_legacy");
        // Solo se corta en el primer `:`.
        assert_eq!(decode_token(&cipher, "plain:a:b"), "a:b");
        assert_eq!(decode_token(&cipher, "plain:"), "");
        // Esquema desconocido: se devuelve tal cual.
        assert_eq!(decode_token(&cipher, "other:a"), "other:a");
        assert_eq!(decode_token(&cipher, ":x"), ":x");
        // Sin cifrado disponible un `safe:` no se puede leer.
        assert_eq!(decode_token(&cipher, "safe:abcd"), "");
    }

    #[test]
    fn test_cipher_round_trips_through_the_safe_scheme() {
        let cipher = TestCipher::new();
        let encoded = encode_token(&cipher, "gho_tóken").unwrap();
        assert!(encoded.starts_with("safe:"));
        assert!(!encoded.contains("gho_"));
        assert_eq!(decode_token(&cipher, &encoded), "gho_tóken");

        // Contenido que no se puede descifrar: vacío, no error.
        assert_eq!(decode_token(&cipher, "safe:zz"), "");
        assert_eq!(decode_token(&cipher, "safe:abc"), "");

        cipher.set_available(false);
        assert_eq!(decode_token(&cipher, &encoded), "");
        assert!(matches!(
            encode_token(&cipher, "gho_x"),
            Err(StoreError::KeyringUnavailable)
        ));
        cipher.set_allow_plaintext_fallback(true);
        assert_eq!(encode_token(&cipher, "gho_x").unwrap(), "plain:gho_x");
    }

    #[test]
    fn a_cipher_that_cannot_encrypt_is_an_error() {
        struct Broken;
        impl SecretCipher for Broken {
            fn is_available(&self) -> bool {
                true
            }
            fn encrypt(&self, _plain_text: &str) -> Option<String> {
                None
            }
            fn decrypt(&self, _payload: &str) -> Option<String> {
                None
            }
        }

        assert!(matches!(
            encode_token(&Broken, "gho_token"),
            Err(StoreError::Encryption)
        ));
    }
}
