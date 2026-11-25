//! Cifrado de tokens de la app: el papel de `safeStorage` de Electron.
//!
//! `safeStorage` cifra con una clave que guarda el llavero del sistema. Aquí
//! es igual: una clave maestra aleatoria de 256 bits vive en el llavero
//! (Credential Manager en Windows, Keychain en macOS, Secret Service en Linux)
//! y cada token se cifra con ella con AES-256-GCM. El archivo de configuración
//! solo contiene el resultado (`safe:<base64>`), inútil sin la clave.
//!
//! Si el llavero no está disponible (sin sesión de escritorio, sin servicio de
//! secretos, bloqueado...), [`KeychainCipher::is_available`] devuelve `false`
//! y `mergeon-store` rechaza guardar tokens nuevos en claro
//! ([`mergeon_store::StoreError::KeyringUnavailable`]) salvo consentimiento
//! explícito vía [`KeychainCipher::set_allow_plaintext_fallback`].

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use aes_gcm::Aes256Gcm;
use aes_gcm::aead::{Aead, Generate, Key, KeyInit, Nonce};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use mergeon_store::SecretCipher;

/// Servicio y cuenta bajo los que se guarda la clave maestra. Distintos de
/// los de Electron ("Mergeon Safe Storage" / "Mergeon"), cuya clave tiene otro
/// formato y sigue siendo de la app anterior.
pub const KEYRING_SERVICE: &str = "Mergeon";
pub const KEYRING_ACCOUNT: &str = "token-encryption-key";

const KEY_LENGTH: usize = 32;
const NONCE_LENGTH: usize = 12;
const TAG_LENGTH: usize = 16;

/// Primer byte del contenido cifrado: permite cambiar el formato más adelante
/// sin confundir lo ya guardado.
const PAYLOAD_VERSION: u8 = 1;

/// Tras un fallo del llavero no se vuelve a preguntar hasta pasado este
/// tiempo: cada consulta es una llamada al sistema que puede incluso pedir al
/// usuario que desbloquee el llavero.
const DEFAULT_RETRY_INTERVAL: Duration = Duration::from_secs(30);

/// Fallo del almacén de la clave maestra.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyStoreError(String);

impl KeyStoreError {
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl std::fmt::Display for KeyStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for KeyStoreError {}

/// Dónde vive la clave maestra. La implementación real es el llavero del
/// sistema ([`KeyringKeyStore`]); los tests usan una en memoria.
pub trait MasterKeyStore: Send + Sync {
    /// La clave guardada, o `None` si todavía no hay ninguna. `Err` si el
    /// almacén no se pudo consultar: no es lo mismo que "no hay clave".
    fn load(&self) -> Result<Option<String>, KeyStoreError>;

    /// Guarda la clave, sustituyendo la que hubiera.
    fn save(&self, secret: &str) -> Result<(), KeyStoreError>;
}

/// La clave maestra en el llavero del sistema (crate `keyring`).
#[derive(Debug, Clone)]
pub struct KeyringKeyStore {
    service: String,
    account: String,
}

impl KeyringKeyStore {
    pub fn new(service: impl Into<String>, account: impl Into<String>) -> Self {
        Self {
            service: service.into(),
            account: account.into(),
        }
    }

    /// La entrada de Mergeon: [`KEYRING_SERVICE`] / [`KEYRING_ACCOUNT`].
    pub fn for_app() -> Self {
        Self::new(KEYRING_SERVICE, KEYRING_ACCOUNT)
    }

    fn entry(&self) -> Result<keyring::Entry, KeyStoreError> {
        keyring::Entry::new(&self.service, &self.account)
            .map_err(|error| KeyStoreError::new(error.to_string()))
    }
}

impl MasterKeyStore for KeyringKeyStore {
    fn load(&self) -> Result<Option<String>, KeyStoreError> {
        match self.entry()?.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(KeyStoreError::new(error.to_string())),
        }
    }

    fn save(&self, secret: &str) -> Result<(), KeyStoreError> {
        self.entry()?
            .set_password(secret)
            .map_err(|error| KeyStoreError::new(error.to_string()))
    }
}

enum KeyState {
    /// Todavía no se consultó el almacén.
    Unknown,
    Ready(Box<Aes256Gcm>),
    Unavailable {
        since: Instant,
    },
}

/// [`SecretCipher`] de la app: AES-256-GCM con la clave maestra del llavero.
pub struct KeychainCipher {
    store: Arc<dyn MasterKeyStore>,
    state: Mutex<KeyState>,
    retry_interval: Duration,
    allow_plaintext_fallback: AtomicBool,
}

impl std::fmt::Debug for KeychainCipher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = match *self.lock_state() {
            KeyState::Unknown => "unknown",
            KeyState::Ready(_) => "ready",
            KeyState::Unavailable { .. } => "unavailable",
        };
        f.debug_struct("KeychainCipher")
            .field("state", &state)
            .field(
                "allow_plaintext_fallback",
                &self.allow_plaintext_fallback.load(Ordering::SeqCst),
            )
            .finish_non_exhaustive()
    }
}

impl KeychainCipher {
    /// Cifrador con la clave maestra en el llavero del sistema.
    pub fn system() -> Self {
        Self::with_store(Arc::new(KeyringKeyStore::for_app()))
    }

    /// Cifrador con un almacén de clave propio.
    pub fn with_store(store: Arc<dyn MasterKeyStore>) -> Self {
        Self {
            store,
            state: Mutex::new(KeyState::Unknown),
            retry_interval: DEFAULT_RETRY_INTERVAL,
            allow_plaintext_fallback: AtomicBool::new(false),
        }
    }

    /// Cuánto esperar antes de volver a consultar un almacén que falló.
    pub fn with_retry_interval(mut self, retry_interval: Duration) -> Self {
        self.retry_interval = retry_interval;
        self
    }

    /// Autoriza explícitamente guardar tokens como `plain:` cuando el llavero
    /// no está disponible. Por defecto es `false`.
    pub fn set_allow_plaintext_fallback(&self, allow: bool) {
        self.allow_plaintext_fallback.store(allow, Ordering::SeqCst);
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, KeyState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Ejecuta `task` con el cifrador listo; `None` si no hay clave maestra.
    fn with_cipher<T>(&self, task: impl FnOnce(&Aes256Gcm) -> Option<T>) -> Option<T> {
        let mut state = self.lock_state();

        let must_load = match &*state {
            KeyState::Unknown => true,
            KeyState::Ready(_) => false,
            KeyState::Unavailable { since } => since.elapsed() >= self.retry_interval,
        };
        if must_load {
            *state = match load_or_create_master_key(self.store.as_ref()) {
                Ok(cipher) => KeyState::Ready(Box::new(cipher)),
                Err(error) => {
                    log::warn!(
                        "El llavero del sistema no está disponible; no se guardarán tokens en claro sin consentimiento: {error}"
                    );
                    KeyState::Unavailable {
                        since: Instant::now(),
                    }
                }
            };
        }

        match &*state {
            KeyState::Ready(cipher) => task(cipher),
            _ => None,
        }
    }
}

/// Lee la clave maestra del almacén o, si todavía no existe, la crea.
///
/// Una clave que existe pero no se puede leer o no tiene el formato esperado
/// **no** se sustituye: hacerlo dejaría ilegibles los tokens ya cifrados con
/// ella. En ese caso el cifrado queda como no disponible.
fn load_or_create_master_key(store: &dyn MasterKeyStore) -> Result<Aes256Gcm, KeyStoreError> {
    if let Some(stored) = store.load()? {
        return decode_master_key(&stored);
    }

    let key = Key::<Aes256Gcm>::try_generate()
        .map_err(|error| KeyStoreError::new(format!("no random source: {error}")))?;
    let encoded = BASE64.encode(key);
    store.save(&encoded)?;

    // Se confía en la clave solo si el almacén la devuelve: un llavero que
    // acepta escrituras y luego no las conserva cifraría tokens irrecuperables.
    match store.load()? {
        Some(stored) if stored == encoded => Ok(Aes256Gcm::new(&key)),
        _ => Err(KeyStoreError::new(
            "the keyring did not keep the encryption key",
        )),
    }
}

fn decode_master_key(stored: &str) -> Result<Aes256Gcm, KeyStoreError> {
    let bytes = BASE64
        .decode(stored.trim())
        .map_err(|_| KeyStoreError::new("the stored encryption key is not valid base64"))?;
    if bytes.len() != KEY_LENGTH {
        return Err(KeyStoreError::new(
            "the stored encryption key has an unexpected length",
        ));
    }
    Aes256Gcm::new_from_slice(&bytes)
        .map_err(|_| KeyStoreError::new("the stored encryption key has an unexpected length"))
}

impl SecretCipher for KeychainCipher {
    fn is_available(&self) -> bool {
        self.with_cipher(|_| Some(())).is_some()
    }

    fn allows_plaintext_fallback(&self) -> bool {
        self.allow_plaintext_fallback.load(Ordering::SeqCst)
    }

    /// `base64(versión || nonce || texto cifrado || tag)`, con un nonce
    /// aleatorio por mensaje.
    fn encrypt(&self, plain_text: &str) -> Option<String> {
        self.with_cipher(|cipher| {
            let nonce = Nonce::<Aes256Gcm>::try_generate().ok()?;
            let cipher_text = cipher.encrypt(&nonce, plain_text.as_bytes()).ok()?;

            let mut payload = Vec::with_capacity(1 + NONCE_LENGTH + cipher_text.len());
            payload.push(PAYLOAD_VERSION);
            payload.extend_from_slice(&nonce);
            payload.extend_from_slice(&cipher_text);
            Some(BASE64.encode(payload))
        })
    }

    fn decrypt(&self, payload: &str) -> Option<String> {
        let payload = BASE64.decode(payload).ok()?;
        let (version, rest) = payload.split_first()?;
        if *version != PAYLOAD_VERSION || rest.len() < NONCE_LENGTH + TAG_LENGTH {
            return None;
        }
        let (nonce, cipher_text) = rest.split_at(NONCE_LENGTH);
        let nonce = Nonce::<Aes256Gcm>::try_from(nonce).ok()?;

        self.with_cipher(|cipher| {
            let plain_text = cipher.decrypt(&nonce, cipher_text).ok()?;
            String::from_utf8(plain_text).ok()
        })
    }
}

/// Almacén de clave en memoria, para tests: permite simular un llavero que no
/// responde o que no conserva lo que se le guarda.
#[derive(Debug, Default)]
pub struct MemoryKeyStore {
    state: Mutex<MemoryKeyStoreState>,
}

#[derive(Debug, Default)]
struct MemoryKeyStoreState {
    secret: Option<String>,
    unavailable: bool,
    read_only: bool,
    forgetful: bool,
    loads: usize,
    saves: usize,
}

impl MemoryKeyStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Almacén que ya contiene `secret`.
    pub fn with_secret(secret: impl Into<String>) -> Self {
        let store = Self::default();
        store.lock().secret = Some(secret.into());
        store
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, MemoryKeyStoreState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// El almacén deja de responder (llavero bloqueado o sin servicio).
    pub fn set_unavailable(&self, unavailable: bool) {
        self.lock().unavailable = unavailable;
    }

    /// Las escrituras fallan.
    pub fn set_read_only(&self, read_only: bool) {
        self.lock().read_only = read_only;
    }

    /// Las escrituras "funcionan" pero no se conservan.
    pub fn set_forgetful(&self, forgetful: bool) {
        self.lock().forgetful = forgetful;
    }

    pub fn secret(&self) -> Option<String> {
        self.lock().secret.clone()
    }

    /// Cuántas veces se consultó el almacén.
    pub fn loads(&self) -> usize {
        self.lock().loads
    }

    /// Cuántas veces se escribió en el almacén.
    pub fn saves(&self) -> usize {
        self.lock().saves
    }
}

impl MasterKeyStore for MemoryKeyStore {
    fn load(&self) -> Result<Option<String>, KeyStoreError> {
        let mut state = self.lock();
        state.loads += 1;
        if state.unavailable {
            return Err(KeyStoreError::new("keyring unavailable"));
        }
        Ok(state.secret.clone())
    }

    fn save(&self, secret: &str) -> Result<(), KeyStoreError> {
        let mut state = self.lock();
        state.saves += 1;
        if state.unavailable {
            return Err(KeyStoreError::new("keyring unavailable"));
        }
        if state.read_only {
            return Err(KeyStoreError::new("keyring is read-only"));
        }
        if !state.forgetful {
            state.secret = Some(secret.to_string());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mergeon_store::{decode_token, encode_token};

    fn cipher_over(store: &Arc<MemoryKeyStore>) -> KeychainCipher {
        KeychainCipher::with_store(store.clone()).with_retry_interval(Duration::ZERO)
    }

    #[test]
    fn round_trips_a_token_with_the_master_key_from_the_store() {
        let store = Arc::new(MemoryKeyStore::new());
        let cipher = cipher_over(&store);

        assert!(cipher.is_available());
        let payload = cipher.encrypt("gho_tóken-secreto").unwrap();
        assert!(!payload.contains("gho_"));
        assert_eq!(
            cipher.decrypt(&payload).as_deref(),
            Some("gho_tóken-secreto")
        );

        // La clave maestra se creó una sola vez: 32 bytes aleatorios en base64.
        assert_eq!(store.saves(), 1);
        let key = BASE64.decode(store.secret().unwrap()).unwrap();
        assert_eq!(key.len(), KEY_LENGTH);
        assert_ne!(key, vec![0u8; KEY_LENGTH]);
    }

    #[test]
    fn the_payload_is_versioned_nonce_then_ciphertext_and_tag() {
        let cipher = cipher_over(&Arc::new(MemoryKeyStore::new()));
        let payload = BASE64.decode(cipher.encrypt("abc").unwrap()).unwrap();

        assert_eq!(payload[0], PAYLOAD_VERSION);
        assert_eq!(payload.len(), 1 + NONCE_LENGTH + "abc".len() + TAG_LENGTH);
        // Un token vacío también se puede cifrar y recuperar.
        let empty = cipher.encrypt("").unwrap();
        assert_eq!(cipher.decrypt(&empty).as_deref(), Some(""));
    }

    #[test]
    fn every_encryption_uses_a_fresh_nonce() {
        let cipher = cipher_over(&Arc::new(MemoryKeyStore::new()));
        let first = cipher.encrypt("gho_token").unwrap();
        let second = cipher.encrypt("gho_token").unwrap();

        assert_ne!(first, second);
        assert_eq!(cipher.decrypt(&first), cipher.decrypt(&second));
    }

    #[test]
    fn another_instance_reads_what_the_first_one_encrypted() {
        let store = Arc::new(MemoryKeyStore::new());
        let payload = cipher_over(&store).encrypt("gho_token").unwrap();

        // Otro arranque de la app: misma clave en el llavero.
        let restarted = cipher_over(&store);
        assert_eq!(restarted.decrypt(&payload).as_deref(), Some("gho_token"));
        assert_eq!(store.saves(), 1);
    }

    #[test]
    fn a_different_master_key_cannot_decrypt() {
        let payload = cipher_over(&Arc::new(MemoryKeyStore::new()))
            .encrypt("gho_token")
            .unwrap();
        let other = cipher_over(&Arc::new(MemoryKeyStore::new()));

        assert!(other.is_available());
        assert_eq!(other.decrypt(&payload), None);
    }

    #[test]
    fn tampered_or_malformed_payloads_are_rejected() {
        let cipher = cipher_over(&Arc::new(MemoryKeyStore::new()));
        let payload = cipher.encrypt("gho_token").unwrap();
        let bytes = BASE64.decode(&payload).unwrap();

        // Un bit cambiado en cualquier parte rompe la autenticación.
        for index in [1, NONCE_LENGTH + 1, bytes.len() - 1] {
            let mut tampered = bytes.clone();
            tampered[index] ^= 0x01;
            assert_eq!(cipher.decrypt(&BASE64.encode(tampered)), None, "{index}");
        }

        // Versión desconocida, contenido corto, base64 inválido.
        let mut other_version = bytes.clone();
        other_version[0] = 2;
        assert_eq!(cipher.decrypt(&BASE64.encode(other_version)), None);
        assert_eq!(cipher.decrypt(&BASE64.encode(&bytes[..10])), None);
        assert_eq!(cipher.decrypt("not base64!"), None);
        assert_eq!(cipher.decrypt(""), None);
    }

    #[test]
    fn an_unavailable_keyring_disables_encryption_until_it_comes_back() {
        let store = Arc::new(MemoryKeyStore::new());
        store.set_unavailable(true);
        let cipher = cipher_over(&store);

        assert!(!cipher.is_available());
        assert_eq!(cipher.encrypt("gho_token"), None);
        assert_eq!(cipher.decrypt("AAAA"), None);
        assert_eq!(store.secret(), None);

        store.set_unavailable(false);
        assert!(cipher.is_available());
        let payload = cipher.encrypt("gho_token").unwrap();
        assert_eq!(cipher.decrypt(&payload).as_deref(), Some("gho_token"));
    }

    #[test]
    fn a_failed_keyring_is_not_queried_again_before_the_retry_interval() {
        let store = Arc::new(MemoryKeyStore::new());
        store.set_unavailable(true);
        let cipher = KeychainCipher::with_store(store.clone())
            .with_retry_interval(Duration::from_secs(3600));

        assert!(!cipher.is_available());
        assert!(!cipher.is_available());
        assert_eq!(cipher.encrypt("gho_token"), None);
        assert_eq!(store.loads(), 1);
    }

    #[test]
    fn a_ready_key_is_cached() {
        let store = Arc::new(MemoryKeyStore::new());
        let cipher = cipher_over(&store);
        assert!(cipher.is_available());
        let loads = store.loads();

        // El llavero puede dejar de responder después: la clave ya está en memoria.
        store.set_unavailable(true);
        let payload = cipher.encrypt("gho_token").unwrap();
        assert_eq!(cipher.decrypt(&payload).as_deref(), Some("gho_token"));
        assert_eq!(store.loads(), loads);
    }

    #[test]
    fn a_keyring_that_cannot_store_the_key_disables_encryption() {
        let read_only = Arc::new(MemoryKeyStore::new());
        read_only.set_read_only(true);
        assert!(!cipher_over(&read_only).is_available());

        let forgetful = Arc::new(MemoryKeyStore::new());
        forgetful.set_forgetful(true);
        let cipher = cipher_over(&forgetful);
        assert!(!cipher.is_available());
        assert_eq!(cipher.encrypt("gho_token"), None);
    }

    #[test]
    fn a_malformed_stored_key_is_never_overwritten() {
        for stored in ["not base64!", "c2hvcnQ=", ""] {
            let store = Arc::new(MemoryKeyStore::with_secret(stored));
            let cipher = cipher_over(&store);

            assert!(!cipher.is_available(), "{stored:?}");
            assert_eq!(cipher.encrypt("gho_token"), None);
            assert_eq!(store.secret().as_deref(), Some(stored));
            assert_eq!(store.saves(), 0);
        }
    }

    #[test]
    fn the_store_uses_the_safe_scheme_when_the_keyring_works() {
        let cipher = cipher_over(&Arc::new(MemoryKeyStore::new()));

        let encoded = encode_token(&cipher, "gho_token").unwrap();
        assert!(encoded.starts_with("safe:"));
        assert!(!encoded.contains("gho_token"));
        assert_eq!(decode_token(&cipher, &encoded), "gho_token");
        // El contenido es base64: no lleva `:` que confunda al separar el esquema.
        assert!(!encoded["safe:".len()..].contains(':'));
    }

    #[test]
    fn the_store_refuses_plaintext_by_default_when_the_keyring_is_unavailable() {
        let store = Arc::new(MemoryKeyStore::new());
        let cipher = cipher_over(&store);
        let safe = encode_token(&cipher, "gho_token").unwrap();

        store.set_unavailable(true);
        let unavailable = cipher_over(&store);
        assert!(matches!(
            encode_token(&unavailable, "gho_token"),
            Err(mergeon_store::StoreError::KeyringUnavailable)
        ));
        // Un `safe:` sin llavero se lee como vacío.
        assert_eq!(decode_token(&unavailable, &safe), "");
        assert_eq!(decode_token(&unavailable, "plain:gho_token"), "gho_token");

        // Solo con autorización explícita cae a `plain:`.
        unavailable.set_allow_plaintext_fallback(true);
        assert_eq!(
            encode_token(&unavailable, "gho_token").unwrap(),
            "plain:gho_token"
        );
    }

    /// Contra el llavero real de la máquina, en una entrada propia del test
    /// que se borra al terminar. No corre por defecto: en CI no hay llavero y
    /// en un escritorio puede pedir que se desbloquee.
    ///
    /// `cargo test -p mergeon-backend -- --ignored system_keyring`
    #[test]
    #[ignore = "usa el llavero real del sistema"]
    fn system_keyring_round_trip() {
        const SERVICE: &str = "Mergeon (test)";
        let cleanup = || {
            if let Ok(entry) = keyring::Entry::new(SERVICE, KEYRING_ACCOUNT) {
                let _ = entry.delete_credential();
            }
        };
        cleanup();

        let store = Arc::new(KeyringKeyStore::new(SERVICE, KEYRING_ACCOUNT));
        let cipher = KeychainCipher::with_store(store.clone());
        if !cipher.is_available() {
            // Sin llavero el cifrador se declara no disponible, sin colgarse.
            assert_eq!(cipher.encrypt("gho_token"), None);
            eprintln!("system keyring not available: {:?}", store.load());
            return;
        }

        let payload = cipher.encrypt("gho_token").unwrap();
        let restarted = KeychainCipher::with_store(store);
        assert_eq!(restarted.decrypt(&payload).as_deref(), Some("gho_token"));
        cleanup();
    }
}
