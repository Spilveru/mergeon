//! El store: estado en memoria, persistencia atómica y las operaciones de
//! `store.js` (una por cada función exportada / canal `storage:*`).

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use crate::cipher::{SecretCipher, decode_token, encode_token};
use crate::error::StoreError;
use crate::paths::{
    ELECTRON_CONFIG_FILE_NAME, SETTINGS_FILE_NAME, default_app_data_dir, default_user_data_dir,
    migrate_legacy_config,
};
use crate::schema::{
    ACTIVE_GITHUB_ACCOUNT_ID, GITHUB_ACCOUNTS, RECENT_REPOSITORIES, SCHEMA_KEYS, SETTINGS,
    STARRED_BRANCHES, WORKSPACE_SESSION, apply_defaults_and_validate, default_workspace_session,
    is_valid_item, schema_violation, validate_key,
};
use crate::settings::{
    Settings, SettingsUpdate, merge_settings_update, normalize_settings, recent_repositories_limit,
};
use crate::time::iso_now;
use crate::types::{GitHubAccount, RecentRepository, RepoWorkspace, WorkspaceSession};

type State = Map<String, Value>;

/// Lo que se importa del `config.json` de Electron. Las cuentas de GitHub (y la
/// cuenta activa) quedan fuera: sus tokens están cifrados con `safeStorage` y
/// no se pueden descifrar fuera de Electron.
const ELECTRON_IMPORT_KEYS: [&str; 4] = [
    RECENT_REPOSITORIES,
    WORKSPACE_SESSION,
    SETTINGS,
    STARRED_BRANCHES,
];

/// Cada token se guarda cifrado en su campo `*Cipher`; el campo en claro solo
/// existe en memoria (y en configuraciones antiguas, que se migran al leer).
const TOKEN_FIELDS: [(&str, &str); 3] = [
    ("token", "tokenCipher"),
    ("refreshToken", "refreshTokenCipher"),
    ("sessionToken", "sessionTokenCipher"),
];

/// Opciones de [`Store::open`].
#[derive(Clone)]
pub struct StoreOptions {
    /// Cifrador de tokens (el `safeStorage` de Electron).
    pub cipher: Arc<dyn SecretCipher>,
    /// Importar del `config.json` de Electron cuando el archivo propio todavía
    /// no existe.
    pub import_electron_config: bool,
    /// De dónde importar. `None` = `config.json` junto al archivo propio.
    pub electron_config_path: Option<PathBuf>,
}

impl StoreOptions {
    pub fn new(cipher: Arc<dyn SecretCipher>) -> Self {
        Self {
            cipher,
            import_electron_config: true,
            electron_config_path: None,
        }
    }

    pub fn with_electron_config_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.electron_config_path = Some(path.into());
        self
    }

    pub fn without_electron_import(mut self) -> Self {
        self.import_electron_config = false;
        self
    }
}

impl std::fmt::Debug for StoreOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoreOptions")
            .field("import_electron_config", &self.import_electron_config)
            .field("electron_config_path", &self.electron_config_path)
            .finish_non_exhaustive()
    }
}

/// De dónde salió el estado inicial.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadStatus {
    /// Se leyó el archivo existente.
    Loaded,
    /// No había archivo: se creó con los defaults (más lo importado, si hubo).
    Created,
    /// El archivo estaba vacío: se partió de los defaults.
    Empty,
    /// El archivo no era un documento JSON válido: se partió de los defaults.
    Corrupt,
    /// Store sin archivo ([`Store::in_memory`]).
    InMemory,
}

/// Importación desde el `config.json` de la app Electron.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElectronImport {
    pub source: PathBuf,
    /// Claves que se importaron.
    pub keys: Vec<String>,
}

/// Qué pasó al abrir el store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadReport {
    pub status: LoadStatus,
    pub electron_import: Option<ElectronImport>,
    /// Copia del archivo original, cuando hubo que descartar algo de él.
    pub backup_path: Option<PathBuf>,
    /// Qué estaba mal en el archivo (errores de sintaxis o del esquema).
    pub problems: Vec<String>,
}

/// Configuración persistente de la app. Seguro entre hilos: cada operación es
/// atómica respecto de las demás.
pub struct Store {
    path: Option<PathBuf>,
    cipher: Arc<dyn SecretCipher>,
    state: Mutex<State>,
    load_report: LoadReport,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store")
            .field("path", &self.path)
            .field("load_report", &self.load_report)
            .finish_non_exhaustive()
    }
}

enum Parsed {
    Empty,
    Corrupt(String),
    Document(State),
}

fn parse_document(bytes: &[u8]) -> Parsed {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Parsed::Corrupt("the file is not valid UTF-8".to_string());
    };
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    if text.trim().is_empty() {
        return Parsed::Empty;
    }

    match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(document)) => Parsed::Document(document),
        Ok(_) => Parsed::Corrupt("`` must be object".to_string()),
        Err(error) => Parsed::Corrupt(error.to_string()),
    }
}

/// Deja el documento cumpliendo el esquema descartando lo que no lo cumple, y
/// devuelve qué se encontró. De las listas de cuentas y de recientes solo se
/// quitan los elementos inválidos; el resto de claves vuelve a su default.
fn repair(state: &mut State) -> Vec<String> {
    let mut problems = Vec::new();

    for key in SCHEMA_KEYS {
        let Some(value) = state.get_mut(key) else {
            continue;
        };
        let errors = validate_key(key, value);
        if errors.is_empty() {
            continue;
        }
        problems.extend(errors);

        let has_items = matches!(key, GITHUB_ACCOUNTS | RECENT_REPOSITORIES);
        match state.get_mut(key) {
            Some(Value::Array(items)) if has_items => {
                items.retain(|item| is_valid_item(key, item));
            }
            _ => {
                state.remove(key);
            }
        }
    }

    problems
}

fn import_electron_config(source: &Path) -> Option<(State, Vec<String>)> {
    let bytes = match fs::read(source) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return None,
        Err(error) => {
            log::warn!(
                "No se pudo leer la configuración de Electron {}: {error}",
                source.display()
            );
            return None;
        }
    };

    let Parsed::Document(config) = parse_document(&bytes) else {
        log::warn!(
            "La configuración de Electron {} no es un JSON válido; no se importa",
            source.display()
        );
        return None;
    };

    let mut imported: State = ELECTRON_IMPORT_KEYS
        .iter()
        .filter_map(|key| {
            config
                .get(*key)
                .map(|value| (key.to_string(), value.clone()))
        })
        .collect();

    for problem in repair(&mut imported) {
        log::warn!("Configuración de Electron: {problem}; se descarta");
    }

    let keys = ELECTRON_IMPORT_KEYS
        .iter()
        .filter(|key| imported.contains_key(**key))
        .map(|key| key.to_string())
        .collect();
    Some((imported, keys))
}

fn backup_path_for(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".corrupt");
    path.with_file_name(name)
}

/// Guarda una copia del archivo antes de reescribirlo sin parte de su
/// contenido. Si la copia falla el arranque sigue igual.
fn backup_original(path: &Path) -> Option<PathBuf> {
    let backup = backup_path_for(path);
    match fs::copy(path, &backup) {
        Ok(_) => Some(backup),
        Err(error) => {
            log::warn!(
                "No se pudo conservar una copia de {}: {error}",
                path.display()
            );
            None
        }
    }
}

// Mismo formato que `conf`: `JSON.stringify(value, undefined, '\t')`.
fn serialize(state: &State) -> Result<Vec<u8>, StoreError> {
    let mut bytes = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(b"\t");
    let mut serializer = serde_json::Serializer::with_formatter(&mut bytes, formatter);
    state
        .serialize(&mut serializer)
        .map_err(|error| StoreError::Serialize(error.to_string()))?;
    Ok(bytes)
}

static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

fn write_new_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // El archivo lleva tokens (en claro si no hay cifrado): solo lo lee su dueño.
        options.mode(0o600);
    }

    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// Escribe a un temporal en el mismo directorio y lo renombra encima: quien
/// lea el archivo ve el contenido anterior o el nuevo, nunca uno a medias.
fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(directory) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        // Se asegura en cada escritura: el directorio pudo borrarse entretanto.
        fs::create_dir_all(directory)?;
    }

    let mut temp_name = std::ffi::OsString::from(".");
    temp_name.push(path.file_name().unwrap_or_default());
    temp_name.push(format!(
        ".{}.{}.tmp",
        std::process::id(),
        TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let temp_path = path.with_file_name(temp_name);

    let written = write_new_file(&temp_path, bytes).and_then(|()| fs::rename(&temp_path, path));
    match written {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = fs::remove_file(&temp_path);
            // Igual que `conf`: a veces en Windows el rename atómico falla con
            // EXDEV aun dentro del mismo directorio; ahí se escribe sin atomicidad.
            if error.kind() == io::ErrorKind::CrossesDevices {
                return write_new_file(path, bytes);
            }
            Err(error)
        }
    }
}

fn persist_to(path: &Path, state: &State) -> Result<(), StoreError> {
    let bytes = serialize(state)?;
    write_atomic(path, &bytes).map_err(|error| StoreError::io(path, error))
}

fn to_object<T: Serialize>(value: &T) -> Result<State, StoreError> {
    match serde_json::to_value(value) {
        Ok(Value::Object(object)) => Ok(object),
        Ok(_) => Err(StoreError::Serialize("expected a JSON object".to_string())),
        Err(error) => Err(StoreError::Serialize(error.to_string())),
    }
}

fn typed<T: DeserializeOwned + Default>(value: Value) -> T {
    serde_json::from_value(value).unwrap_or_default()
}

fn array_of(state: &State, key: &str) -> Vec<Value> {
    state
        .get(key)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn text<'a>(object: &'a State, key: &str) -> &'a str {
    object.get(key).and_then(Value::as_str).unwrap_or("")
}

fn without_token_fields(account: &State) -> State {
    let mut rest = account.clone();
    for (plain, ciphered) in TOKEN_FIELDS {
        rest.remove(plain);
        rest.remove(ciphered);
    }
    rest
}

// Espacio en blanco para `String.prototype.trim`.
fn is_js_whitespace(character: char) -> bool {
    const SINGLE: [char; 14] = [
        '\t', '\n', '\u{b}', '\u{c}', '\r', ' ', '\u{a0}', '\u{1680}', '\u{2028}', '\u{2029}',
        '\u{202f}', '\u{205f}', '\u{3000}', '\u{feff}',
    ];
    SINGLE.contains(&character) || ('\u{2000}'..='\u{200a}').contains(&character)
}

/// `path.trim().length === 0`.
fn is_blank(text: &str) -> bool {
    text.chars().all(is_js_whitespace)
}

fn starred_of(state: &State, repo_path: &str) -> Vec<Value> {
    state
        .get(STARRED_BRANCHES)
        .and_then(Value::as_object)
        .and_then(|all| all.get(repo_path))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn set_starred(state: &mut State, repo_path: &str, branches: Vec<Value>) {
    let all = state
        .entry(STARRED_BRANCHES)
        .or_insert_with(|| Value::Object(Map::new()));
    if let Some(all) = all.as_object_mut() {
        all.insert(repo_path.to_string(), Value::Array(branches));
    }
}

fn branch_names(branches: &[Value]) -> Vec<String> {
    branches
        .iter()
        .filter_map(|branch| branch.as_str().map(str::to_string))
        .collect()
}

impl Store {
    /// Abre (o crea) el store sobre `path`.
    ///
    /// Un archivo ausente, vacío o corrupto no impide arrancar: se parte de los
    /// defaults, y de lo que no se pudo aprovechar queda una copia en
    /// `<path>.corrupt`. Solo falla si el archivo existe pero no se puede leer,
    /// o si no se puede escribir el estado inicial; en ese caso la app puede
    /// seguir con [`Store::in_memory`].
    ///
    /// Si el archivo no existe y hay un `config.json` de la app Electron (ver
    /// [`StoreOptions`]), se importan de él recientes, sesión de workspace,
    /// ajustes y ramas destacadas. Las cuentas de GitHub no se importan.
    pub fn open(path: impl Into<PathBuf>, options: StoreOptions) -> Result<Self, StoreError> {
        let path = path.into();
        let mut report = LoadReport {
            status: LoadStatus::Loaded,
            electron_import: None,
            backup_path: None,
            problems: Vec::new(),
        };
        let mut on_disk = None;
        let mut state = State::new();

        match fs::read(&path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                report.status = LoadStatus::Created;
                if options.import_electron_config {
                    let source = options
                        .electron_config_path
                        .clone()
                        .unwrap_or_else(|| path.with_file_name(ELECTRON_CONFIG_FILE_NAME));
                    if let Some((imported, keys)) = import_electron_config(&source) {
                        state = imported;
                        report.electron_import = Some(ElectronImport { source, keys });
                    }
                }
            }
            // No se parte de defaults: reescribir encima destruiría un archivo
            // que quizá está bien y solo no se pudo leer ahora.
            Err(error) => return Err(StoreError::io(&path, error)),
            Ok(bytes) => match parse_document(&bytes) {
                Parsed::Empty => report.status = LoadStatus::Empty,
                Parsed::Corrupt(problem) => {
                    log::warn!(
                        "{} está corrupto ({problem}); se parte de los valores por defecto",
                        path.display()
                    );
                    report.status = LoadStatus::Corrupt;
                    report.problems.push(problem);
                    report.backup_path = backup_original(&path);
                }
                Parsed::Document(document) => {
                    on_disk = Some(document.clone());
                    state = document;
                }
            },
        }

        let problems = repair(&mut state);
        if !problems.is_empty() {
            for problem in &problems {
                log::warn!("{}: {problem}; se descarta", path.display());
            }
            if on_disk.is_some() {
                report.backup_path = backup_original(&path);
            }
            report.problems.extend(problems);
        }

        if !apply_defaults_and_validate(&mut state).is_empty() {
            // Tras reparar no debería quedar nada inválido; si quedara, defaults.
            state = State::new();
            apply_defaults_and_validate(&mut state);
        }

        // Como `conf` al construirse: si lo que hay en disco no es ya el estado
        // completo (archivo nuevo, defaults que faltaban, reparación), se escribe.
        if on_disk.as_ref() != Some(&state) {
            persist_to(&path, &state)?;
        }

        Ok(Self {
            path: Some(path),
            cipher: options.cipher,
            state: Mutex::new(state),
            load_report: report,
        })
    }

    /// Abre el store de la app en su ubicación por defecto:
    /// `<userData>/settings.json` (ver [`default_user_data_dir`]).
    pub fn open_default(options: StoreOptions) -> Result<Self, StoreError> {
        let user_data_dir = default_user_data_dir().ok_or(StoreError::NoUserDataDir)?;
        Self::open_in(&user_data_dir, default_app_data_dir().as_deref(), options)
    }

    /// Abre `<user_data_dir>/settings.json`.
    ///
    /// La primera vez recupera antes la configuración de la app anterior al
    /// renombre ([`migrate_legacy_config`], que la busca en `app_data_dir`),
    /// igual que hace la app Electron al arrancar, para que la importación la
    /// encuentre.
    pub fn open_in(
        user_data_dir: &Path,
        app_data_dir: Option<&Path>,
        options: StoreOptions,
    ) -> Result<Self, StoreError> {
        let path = user_data_dir.join(SETTINGS_FILE_NAME);

        if options.import_electron_config
            && options.electron_config_path.is_none()
            && !path.exists()
        {
            migrate_legacy_config(Some(user_data_dir), app_data_dir);
        }

        Self::open(path, options)
    }

    /// Store sin archivo: parte de los defaults y no persiste nada. Para tests
    /// y para seguir funcionando cuando el archivo no se puede usar.
    pub fn in_memory(options: StoreOptions) -> Self {
        let mut state = State::new();
        apply_defaults_and_validate(&mut state);

        Self {
            path: None,
            cipher: options.cipher,
            state: Mutex::new(state),
            load_report: LoadReport {
                status: LoadStatus::InMemory,
                electron_import: None,
                backup_path: None,
                problems: Vec::new(),
            },
        }
    }

    /// Ruta del archivo, o `None` si el store es solo en memoria.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Qué pasó al abrir: si se creó, se importó de Electron o hubo que
    /// descartar contenido.
    pub fn load_report(&self) -> &LoadReport {
        &self.load_report
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        // Un pánico con el lock tomado no deja el estado a medias: los cambios
        // se preparan en una copia y solo se asignan una vez escritos.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn persist(&self, state: &State) -> Result<(), StoreError> {
        match &self.path {
            Some(path) => persist_to(path, state),
            None => Ok(()),
        }
    }

    /// `store.set(...)`: valida el documento entero, lo escribe y solo entonces
    /// lo da por bueno en memoria.
    fn commit(
        &self,
        guard: &mut MutexGuard<'_, State>,
        mut draft: State,
    ) -> Result<(), StoreError> {
        let errors = apply_defaults_and_validate(&mut draft);
        if !errors.is_empty() {
            return Err(schema_violation(&errors));
        }

        self.persist(&draft)?;
        **guard = draft;
        Ok(())
    }

    /// Para las lecturas que de paso migran o normalizan lo guardado: si esa
    /// escritura falla, la lectura sigue siendo válida.
    fn commit_quietly(&self, guard: &mut MutexGuard<'_, State>, draft: State) {
        if let Err(error) = self.commit(guard, draft) {
            log::warn!("No se pudo guardar la configuración normalizada: {error}");
        }
    }

    fn mutate<T>(
        &self,
        change: impl FnOnce(&mut State) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        let mut guard = self.lock();
        let mut draft = guard.clone();
        let result = change(&mut draft)?;
        self.commit(&mut guard, draft)?;
        Ok(result)
    }

    // ---- Tokens ----

    /// `normalizeAccountFromStorage`: cuenta tal como se usa en memoria, con
    /// los tokens descifrados.
    fn account_from_storage(&self, account: &State) -> Result<State, StoreError> {
        let cipher = self.cipher.as_ref();
        let mut normalized = without_token_fields(account);

        for (plain, ciphered) in TOKEN_FIELDS {
            let token = text(account, plain);
            let token_cipher = text(account, ciphered);

            let (decoded, stored_cipher) = if token_cipher.is_empty() {
                // Configuración antigua con el token en claro.
                (decode_token(cipher, token), encode_token(cipher, token)?)
            } else {
                (decode_token(cipher, token_cipher), token_cipher.to_string())
            };

            normalized.insert(plain.to_string(), Value::String(decoded));
            normalized.insert(ciphered.to_string(), Value::String(stored_cipher));
        }

        Ok(normalized)
    }

    /// `normalizeAccountForStorage`: sin tokens en claro. Un token legible se
    /// vuelve a cifrar; si no se pudo descifrar (queda vacío), se conserva su
    /// cifrado tal cual para no perderlo.
    fn account_for_storage(&self, account: &State) -> Result<State, StoreError> {
        let cipher = self.cipher.as_ref();
        let mut stored = without_token_fields(account);

        for (plain, ciphered) in TOKEN_FIELDS {
            let token = text(account, plain);
            let token_cipher = if token.is_empty() {
                text(account, ciphered).to_string()
            } else {
                encode_token(cipher, token)?
            };
            stored.insert(ciphered.to_string(), Value::String(token_cipher));
        }

        Ok(stored)
    }

    /// Cuenta que llega para guardarse. A diferencia de lo leído del archivo,
    /// aquí manda el token en claro: quien refresca una sesión trae el token
    /// nuevo junto al `*Cipher` que leyó antes, ya obsoleto.
    fn incoming_account(&self, account: &State) -> Result<State, StoreError> {
        let cipher = self.cipher.as_ref();
        let mut normalized = without_token_fields(account);

        for (plain, ciphered) in TOKEN_FIELDS {
            let token = text(account, plain);
            let token_cipher = text(account, ciphered);

            let (decoded, stored_cipher) = if token.is_empty() {
                (decode_token(cipher, token_cipher), token_cipher.to_string())
            } else {
                (token.to_string(), encode_token(cipher, token)?)
            };

            normalized.insert(plain.to_string(), Value::String(decoded));
            normalized.insert(ciphered.to_string(), Value::String(stored_cipher));
        }

        Ok(normalized)
    }

    /// Las cuentas guardadas ya normalizadas, y si alguna necesita reescribirse
    /// (token en claro de una configuración antigua, o `*Cipher` ausente).
    fn read_accounts(&self, state: &State) -> Result<(Vec<State>, bool), StoreError> {
        let mut migrated = false;
        let mut accounts = Vec::new();

        for stored in array_of(state, GITHUB_ACCOUNTS) {
            let Some(account) = stored.as_object() else {
                continue;
            };
            let normalized = self.account_from_storage(account)?;

            if normalized.get("tokenCipher") != account.get("tokenCipher")
                || normalized.get("sessionTokenCipher") != account.get("sessionTokenCipher")
                || !text(account, "token").is_empty()
                || !text(account, "sessionToken").is_empty()
            {
                migrated = true;
            }

            accounts.push(normalized);
        }

        Ok((accounts, migrated))
    }

    fn accounts_for_storage(&self, accounts: &[State]) -> Result<Value, StoreError> {
        accounts
            .iter()
            .map(|account| self.account_for_storage(account).map(Value::Object))
            .collect::<Result<Vec<Value>, StoreError>>()
            .map(Value::Array)
    }

    // ---- GitHub Accounts ----

    /// `storage:getGitHubAccounts`. Devuelve las cuentas con los tokens
    /// descifrados. Solo falla si hay que cifrar un token antiguo en claro y el
    /// cifrador no puede.
    pub fn get_github_accounts(&self) -> Result<Vec<GitHubAccount>, StoreError> {
        let mut guard = self.lock();
        let (accounts, migrated) = self.read_accounts(&guard)?;

        if migrated {
            let mut draft = guard.clone();
            draft.insert(
                GITHUB_ACCOUNTS.to_string(),
                self.accounts_for_storage(&accounts)?,
            );
            self.commit_quietly(&mut guard, draft);
        }

        Ok(accounts
            .into_iter()
            .map(|account| typed(Value::Object(account)))
            .collect())
    }

    /// `storage:saveGitHubAccount`. Reemplaza la cuenta con el mismo `id` o la
    /// añade al final, y devuelve la cuenta normalizada.
    pub fn save_github_account(
        &self,
        account: &GitHubAccount,
    ) -> Result<GitHubAccount, StoreError> {
        let incoming = to_object(account)?;

        self.mutate(|draft| {
            let (mut accounts, _) = self.read_accounts(draft)?;
            let normalized = self.incoming_account(&incoming)?;

            match accounts
                .iter()
                .position(|existing| existing.get("id") == incoming.get("id"))
            {
                Some(index) => accounts[index] = normalized.clone(),
                None => accounts.push(normalized.clone()),
            }

            draft.insert(
                GITHUB_ACCOUNTS.to_string(),
                self.accounts_for_storage(&accounts)?,
            );
            Ok(typed(Value::Object(normalized)))
        })
    }

    /// `storage:removeGitHubAccount`.
    pub fn remove_github_account(&self, account_id: &str) -> Result<(), StoreError> {
        let id = Value::String(account_id.to_string());

        self.mutate(|draft| {
            let (accounts, _) = self.read_accounts(draft)?;
            let filtered: Vec<State> = accounts
                .into_iter()
                .filter(|account| account.get("id") != Some(&id))
                .collect();
            draft.insert(
                GITHUB_ACCOUNTS.to_string(),
                self.accounts_for_storage(&filtered)?,
            );

            // Si era la cuenta activa, limpiarla
            if draft.get(ACTIVE_GITHUB_ACCOUNT_ID) == Some(&id) {
                draft.insert(ACTIVE_GITHUB_ACCOUNT_ID.to_string(), Value::Null);
            }
            Ok(())
        })
    }

    /// `storage:getActiveGitHubAccountId`.
    pub fn get_active_github_account_id(&self) -> Option<String> {
        self.lock()
            .get(ACTIVE_GITHUB_ACCOUNT_ID)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// `storage:setActiveGitHubAccountId`.
    pub fn set_active_github_account_id(&self, account_id: Option<&str>) -> Result<(), StoreError> {
        self.mutate(|draft| {
            draft.insert(
                ACTIVE_GITHUB_ACCOUNT_ID.to_string(),
                account_id.map_or(Value::Null, |id| Value::String(id.to_string())),
            );
            Ok(())
        })
    }

    // ---- Recent Repositories ----

    /// `storage:getRecentRepositories`.
    pub fn get_recent_repositories(&self) -> Vec<RecentRepository> {
        array_of(&self.lock(), RECENT_REPOSITORIES)
            .into_iter()
            .map(typed)
            .collect()
    }

    /// `storage:addRecentRepository`. Lo pone el primero con `lastOpened` de
    /// ahora (el que traiga `repo` se ignora), quita el que ya hubiera con la
    /// misma ruta y recorta a `settings.maxRecentRepos`. Devuelve la lista.
    pub fn add_recent_repository(
        &self,
        repo: &RecentRepository,
    ) -> Result<Vec<RecentRepository>, StoreError> {
        let mut entry = to_object(repo)?;
        entry.insert("lastOpened".to_string(), Value::String(iso_now()));

        self.mutate(|draft| {
            // Remover si ya existe
            let mut recent: Vec<Value> = array_of(draft, RECENT_REPOSITORIES)
                .into_iter()
                .filter(|existing| existing.get("path") != entry.get("path"))
                .collect();

            // Agregar al inicio
            recent.insert(0, Value::Object(entry));

            // Limitar a maxRecentRepos
            let limit = recent_repositories_limit(draft.get(SETTINGS), recent.len());
            recent.truncate(limit);

            draft.insert(
                RECENT_REPOSITORIES.to_string(),
                Value::Array(recent.clone()),
            );
            Ok(recent.into_iter().map(typed).collect())
        })
    }

    /// `storage:removeRecentRepository`.
    pub fn remove_recent_repository(&self, repo_path: &str) -> Result<(), StoreError> {
        let path = Value::String(repo_path.to_string());

        self.mutate(|draft| {
            let filtered: Vec<Value> = array_of(draft, RECENT_REPOSITORIES)
                .into_iter()
                .filter(|existing| existing.get("path") != Some(&path))
                .collect();
            draft.insert(RECENT_REPOSITORIES.to_string(), Value::Array(filtered));
            Ok(())
        })
    }

    /// `storage:clearRecentRepositories`.
    pub fn clear_recent_repositories(&self) -> Result<(), StoreError> {
        self.mutate(|draft| {
            draft.insert(RECENT_REPOSITORIES.to_string(), Value::Array(Vec::new()));
            Ok(())
        })
    }

    // ---- Workspace Session ----

    /// `storage:getWorkspaceSession`.
    pub fn get_workspace_session(&self) -> WorkspaceSession {
        self.lock()
            .get(WORKSPACE_SESSION)
            .cloned()
            .map(typed)
            .unwrap_or_default()
    }

    /// `storage:setWorkspaceSession`. Descarta las rutas vacías o en blanco y
    /// devuelve la sesión tal como quedó guardada.
    pub fn set_workspace_session(
        &self,
        session: &WorkspaceSession,
    ) -> Result<WorkspaceSession, StoreError> {
        let mut workspaces: Vec<RepoWorkspace> = Vec::new();
        for workspace in &session.workspaces {
            let id = workspace.id.trim();
            let name = workspace.name.trim();
            if is_blank(id) || is_blank(name) {
                continue;
            }
            if workspaces.iter().any(|existing| existing.id == id) {
                continue;
            }
            let mut repositories = Vec::new();
            for path in &workspace.repositories {
                if !is_blank(path) && !repositories.contains(path) {
                    repositories.push(path.clone());
                }
            }
            let active_repository_path = workspace
                .active_repository_path
                .clone()
                .filter(|path| !is_blank(path) && repositories.contains(path));
            workspaces.push(RepoWorkspace {
                id: id.to_string(),
                name: name.to_string(),
                repositories,
                active_repository_path,
            });
        }
        let active_workspace_id = session
            .active_workspace_id
            .as_deref()
            .map(str::trim)
            .filter(|id| !is_blank(id) && workspaces.iter().any(|ws| ws.id == *id))
            .map(str::to_string);

        let normalized = WorkspaceSession {
            repository_workspaces: session.repository_workspaces.clone(),
            open_repositories: session
                .open_repositories
                .iter()
                .filter(|path| !is_blank(path))
                .cloned()
                .collect(),
            active_repository_path: session
                .active_repository_path
                .clone()
                .filter(|path| !is_blank(path)),
            workspaces,
            active_workspace_id,
        };
        let value = Value::Object(to_object(&normalized)?);

        self.mutate(|draft| {
            draft.insert(WORKSPACE_SESSION.to_string(), value);
            Ok(())
        })?;
        Ok(normalized)
    }

    /// `storage:clearWorkspaceSession`.
    pub fn clear_workspace_session(&self) -> Result<(), StoreError> {
        self.mutate(|draft| {
            draft.insert(WORKSPACE_SESSION.to_string(), default_workspace_session());
            Ok(())
        })
    }

    // ---- Settings ----

    /// `storage:getSettings`. Devuelve los ajustes normalizados; si lo guardado
    /// no lo estaba (faltaban claves nuevas, por ejemplo), lo reescribe.
    pub fn get_settings(&self) -> Settings {
        let mut guard = self.lock();
        let normalized = normalize_settings(guard.get(SETTINGS));

        if guard.get(SETTINGS) != Some(&normalized) {
            let mut draft = guard.clone();
            draft.insert(SETTINGS.to_string(), normalized.clone());
            self.commit_quietly(&mut guard, draft);
        }

        Settings::from_value(&normalized)
    }

    /// `storage:updateSettings`. Solo cambia los campos presentes;
    /// `sidebar_sections` y `commit_view` se mezclan con lo guardado.
    pub fn update_settings(&self, updates: &SettingsUpdate) -> Result<Settings, StoreError> {
        let updates = serde_json::to_value(updates)
            .map_err(|error| StoreError::Serialize(error.to_string()))?;
        self.update_settings_json(&updates)
    }

    /// `storage:updateSettings` con los cambios en JSON, tal como los manda el
    /// renderer. Las claves desconocidas se guardan tal cual.
    pub fn update_settings_json(&self, updates: &Value) -> Result<Settings, StoreError> {
        let mut guard = self.lock();
        let current = normalize_settings(guard.get(SETTINGS));
        let new_settings = merge_settings_update(&current, updates);

        let mut draft = guard.clone();
        draft.insert(SETTINGS.to_string(), new_settings.clone());
        self.commit(&mut guard, draft)?;

        Ok(Settings::from_value(&new_settings))
    }

    // ---- Starred Branches ----
    // Formato: { "repoPath": ["branchName1", "branchName2"] }

    /// `storage:getStarredBranches`.
    pub fn get_starred_branches(&self, repo_path: &str) -> Vec<String> {
        branch_names(&starred_of(&self.lock(), repo_path))
    }

    /// `storage:addStarredBranch`. Devuelve las ramas destacadas del repo.
    pub fn add_starred_branch(
        &self,
        repo_path: &str,
        branch_name: &str,
    ) -> Result<Vec<String>, StoreError> {
        let mut guard = self.lock();
        let mut repo_starred = starred_of(&guard, repo_path);
        let branch = Value::String(branch_name.to_string());

        if !repo_starred.contains(&branch) {
            repo_starred.push(branch);
            let mut draft = guard.clone();
            set_starred(&mut draft, repo_path, repo_starred.clone());
            self.commit(&mut guard, draft)?;
        }

        Ok(branch_names(&repo_starred))
    }

    /// `storage:removeStarredBranch`. Devuelve las ramas destacadas del repo.
    pub fn remove_starred_branch(
        &self,
        repo_path: &str,
        branch_name: &str,
    ) -> Result<Vec<String>, StoreError> {
        let branch = Value::String(branch_name.to_string());

        self.mutate(|draft| {
            let filtered: Vec<Value> = starred_of(draft, repo_path)
                .into_iter()
                .filter(|existing| existing != &branch)
                .collect();
            set_starred(draft, repo_path, filtered.clone());
            Ok(branch_names(&filtered))
        })
    }
}
