//! GitHub: el store `src/stores/github.js` (cuentas, cuenta activa, contexto
//! de autenticación para git), `src/composables/useGitHubAuthFlow.js` (alta de
//! cuentas por device flow o token personal) y
//! `src/composables/useGitHubRepositories.js` (listado de repositorios).
//!
//! Una cuenta es un `mergeon_store::GitHubAccount`: la forma que se guarda, con
//! sus claves desconocidas conservadas. Las mezclas que el original hacía con
//! `{ ...a, ...b }` se hacen igual, sobre el objeto JSON de la cuenta, para
//! que un campo que llega nulo pise al anterior como lo hacía allí.

use std::time::Duration;

use anyhow::anyhow;
use gpui_kit::{AsyncApp, ClipboardItem, Context, Entity, Task};
use mergeon_github::{
    AuthContext, GitHubError, GitHubErrorCode, GitHubRepository, GitHubResult, PollLoginResult,
    RepositorySearchResult,
};
use mergeon_model::format::{now_iso_string, parse_js_date};
use mergeon_store::{GitHubAccount, GitHubInstallation};
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::AppModel;
use crate::tasks::{InFlightMap, flow, settled};

/// Cuenta tal como la devuelve el cliente de GitHub al autenticar.
pub type ApiAccount = mergeon_github::GitHubAccount;

/// `Date.parse(text) || 0`.
fn parse_updated_at(text: Option<&str>) -> i64 {
    text.filter(|text| !text.is_empty())
        .and_then(|text| parse_js_date(text, &chrono::Local))
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Estado
// ---------------------------------------------------------------------------

/// Estado visible del alta de una cuenta (`useGitHubAuthFlow`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitHubAuthFlow {
    pub is_authenticating: bool,
    pub status_message: String,
    pub validation_error: String,
    pub install_url: String,
    /// Código que el usuario introduce en github.com/login/device.
    pub user_code: String,
    pub verification_uri: String,
    /// Cambia al cancelar o al empezar otro intento: lo que llegue de un
    /// intento anterior se descarta.
    auth_flow_token: u64,
    install_auto_opened: bool,
}

/// Estado de las peticiones de repositorios (`useGitHubRepositories`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RepositoryRequests {
    pub is_loading: bool,
    pub error: Option<String>,
    pub active_request_id: u64,
}

#[derive(Debug)]
pub(crate) struct GitHubState {
    pub accounts: Vec<GitHubAccount>,
    pub active_account_id: Option<String>,
    pub loaded: bool,
    pub initializing: bool,
    pub auth_service_available: bool,
    pub auth_service_message: String,
    /// Single-flight por cuenta: el inicio y el selector pueden pedir un
    /// refresh de la misma cuenta; la segunda llamada reutiliza la que está en
    /// vuelo.
    pub account_refresh_requests: InFlightMap,
    pub auth_flow: GitHubAuthFlow,
    pub repository_requests: RepositoryRequests,
    /// Pull requests y checks por repositorio (`pull_requests.rs`).
    pub pull_requests: crate::pull_requests::PullRequestsState,
}

impl Default for GitHubState {
    fn default() -> Self {
        Self {
            accounts: Vec::new(),
            active_account_id: None,
            loaded: false,
            initializing: false,
            auth_service_available: true,
            auth_service_message: String::new(),
            account_refresh_requests: InFlightMap::default(),
            auth_flow: GitHubAuthFlow::default(),
            repository_requests: RepositoryRequests::default(),
            pull_requests: Default::default(),
        }
    }
}

fn account_refresh_requests(model: &mut AppModel) -> &mut InFlightMap {
    &mut model.github.account_refresh_requests
}

// ---------------------------------------------------------------------------
// Cuentas como objetos
// ---------------------------------------------------------------------------

fn to_object(value: &impl Serialize) -> Map<String, Value> {
    match serde_json::to_value(value) {
        Ok(Value::Object(object)) => object,
        _ => Map::new(),
    }
}

/// Verdadero en JavaScript.
fn truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(flag)) => *flag,
        Some(Value::Number(number)) => number.as_f64().is_some_and(|number| number != 0.0),
        Some(Value::String(text)) => !text.is_empty(),
        Some(Value::Array(_) | Value::Object(_)) => true,
    }
}

fn truthy_or_null(value: Option<&Value>) -> Value {
    value
        .filter(|value| truthy(Some(value)))
        .cloned()
        .unwrap_or(Value::Null)
}

/// `{ ...base, ...over }`.
fn spread(mut base: Map<String, Value>, over: Map<String, Value>) -> Map<String, Value> {
    base.extend(over);
    base
}

fn first_installation_id(installations: &Value) -> Value {
    truthy_or_null(
        installations
            .as_array()
            .and_then(|installations| installations.first())
            .and_then(|installation| installation.get("id")),
    )
}

/// `normalizeAccount` sobre el objeto de la cuenta.
fn normalize_account_object(mut account: Map<String, Value>) -> Map<String, Value> {
    let installations = account
        .get("installations")
        .filter(|value| truthy(Some(value)))
        .cloned()
        .unwrap_or_else(|| json!([]));
    let active_installation_id = match truthy_or_null(account.get("activeInstallationId")) {
        Value::Null => first_installation_id(&installations),
        id => id,
    };

    account.insert(
        "installUrl".to_string(),
        truthy_or_null(account.get("installUrl")),
    );
    // Solo las cuentas de GitHub App necesitan instalación; las de OAuth App y
    // las de token personal no tienen instalaciones y eso es lo normal, así que
    // se respeta la marca que trae la cuenta en vez de deducirla.
    let requires_installation = match account.get("requiresInstallation") {
        None | Some(Value::Null) => Value::Bool(!truthy(Some(&active_installation_id))),
        Some(value) => value.clone(),
    };
    account.insert("requiresInstallation".to_string(), requires_installation);
    account.insert(
        "requiresReconnect".to_string(),
        Value::Bool(truthy(account.get("requiresReconnect"))),
    );
    account.insert("installations".to_string(), installations);
    account.insert("activeInstallationId".to_string(), active_installation_id);
    account
}

fn account_from_object(account: Map<String, Value>) -> GitHubAccount {
    serde_json::from_value(Value::Object(account)).unwrap_or_default()
}

/// `normalizeAccount`: instalación activa, marca de instalación y de
/// reconexión siempre presentes.
pub fn normalize_account(account: &impl Serialize) -> GitHubAccount {
    account_from_object(normalize_account_object(to_object(account)))
}

fn normalized_username(username: &str) -> String {
    username.trim().to_lowercase()
}

/// `getAccountDedupKey`.
fn account_dedup_key(account: &GitHubAccount) -> String {
    let username = normalized_username(&account.username);
    if username.is_empty() {
        format!("id:{}", account.id)
    } else {
        format!("username:{username}")
    }
}

/// `dedupeAccounts`: una cuenta por usuario. La primera manda en `id` y fecha
/// de alta; las repetidas se devuelven aparte para borrarlas del disco.
fn dedupe_accounts(accounts: &[GitHubAccount]) -> (Vec<GitHubAccount>, Vec<GitHubAccount>) {
    let mut deduped: Vec<GitHubAccount> = Vec::new();
    let mut duplicates = Vec::new();

    for raw_account in accounts {
        let normalized = normalize_account(raw_account);
        let key = account_dedup_key(&normalized);

        let Some(existing_index) = deduped
            .iter()
            .position(|existing| account_dedup_key(existing) == key)
        else {
            deduped.push(normalized);
            continue;
        };

        let existing = &deduped[existing_index];
        let mut merged = spread(to_object(existing), to_object(&normalized));
        merged.insert("id".to_string(), json!(existing.id));
        let added_at = existing
            .added_at
            .clone()
            .filter(|added_at| !added_at.is_empty())
            .or_else(|| normalized.added_at.clone());
        match added_at {
            Some(added_at) => merged.insert("addedAt".to_string(), json!(added_at)),
            None => merged.remove("addedAt"),
        };

        deduped[existing_index] = account_from_object(normalize_account_object(merged));
        duplicates.push(normalized);
    }

    (deduped, duplicates)
}

/// `toAccountPayload` de `useGitHubAuthFlow`: lo que se pasa a `addAccount`.
pub fn to_account_payload(account_data: &ApiAccount) -> Value {
    let installations = serde_json::to_value(&account_data.installations).unwrap_or(json!([]));
    let active_installation_id = match account_data.active_installation_id.filter(|id| *id != 0) {
        Some(id) => json!(id),
        None => first_installation_id(&installations),
    };

    json!({
        "id": account_data.id,
        "username": account_data.username,
        "displayName": account_data.display_name,
        "avatarUrl": account_data.avatar_url,
        "token": account_data.token,
        "refreshToken": account_data.refresh_token.clone().unwrap_or_default(),
        "isPersonalAccessToken": account_data.is_personal_access_token,
        "expiresAt": account_data.expires_at,
        "installUrl": account_data.install_url.clone().filter(|url| !url.is_empty()),
        "requiresInstallation": account_data.requires_installation,
        "installations": installations,
        "activeInstallationId": active_installation_id
    })
}

// ---------------------------------------------------------------------------
// Repositorios de todas las instalaciones
// ---------------------------------------------------------------------------

/// Opciones de [`AppModel::list_all_repositories`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListAllRepositoriesOptions {
    pub per_page: u32,
    pub max_pages_per_installation: u32,
    /// Se recorre la última, para que quede activa en la sesión.
    pub active_installation_id: Option<u64>,
}

impl Default for ListAllRepositoriesOptions {
    fn default() -> Self {
        Self {
            per_page: 100,
            max_pages_per_installation: 20,
            active_installation_id: None,
        }
    }
}

/// Un repositorio con la instalación por la que se llegó a él.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallationRepository {
    #[serde(flatten)]
    pub repository: GitHubRepository,
    pub installation_id: u64,
}

/// Una instalación cuyos repositorios no se pudieron listar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallationFailure {
    pub installation_id: u64,
    pub account_login: String,
    pub message: String,
}

/// Resultado de [`AppModel::list_all_repositories`].
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct AllRepositories {
    /// Sin repetidos, del actualizado más reciente al más antiguo.
    pub repositories: Vec<InstallationRepository>,
    pub failures: Vec<InstallationFailure>,
}

const REPOSITORY_REQUEST_FAILED: &str = "GitHub repository request failed";

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

impl AppModel {
    /// `accounts`.
    pub fn github_accounts(&self) -> &[GitHubAccount] {
        &self.github.accounts
    }

    /// `activeAccountId`.
    pub fn active_github_account_id(&self) -> Option<&str> {
        self.github.active_account_id.as_deref()
    }

    /// `activeAccount`.
    pub fn active_github_account(&self) -> Option<&GitHubAccount> {
        let active_id = self.github.active_account_id.as_deref()?;
        self.github_account_by_id(active_id)
    }

    /// `activeInstallation`.
    pub fn active_github_installation(&self) -> Option<&GitHubInstallation> {
        let account = self.active_github_account()?;
        let active_id = account.active_installation_id?;
        account
            .installations
            .iter()
            .find(|installation| installation.id == active_id)
    }

    /// `hasAccounts`.
    pub fn has_github_accounts(&self) -> bool {
        !self.github.accounts.is_empty()
    }

    /// `getAccountById`.
    pub fn github_account_by_id(&self, id: &str) -> Option<&GitHubAccount> {
        self.github.accounts.iter().find(|account| account.id == id)
    }

    /// `getGitAuthContext`: token e instalación de la cuenta activa, o `None`
    /// si no hay cuenta, no tiene token o hay que reconectarla.
    pub fn get_git_auth_context(&self) -> Option<AuthContext> {
        let account = self.active_github_account()?;
        if account.token.is_empty() || account.requires_reconnect {
            return None;
        }

        Some(AuthContext::new(
            account.token.clone(),
            account.active_installation_id,
        ))
    }

    /// El token de [`Self::get_git_auth_context`], que es lo que las
    /// operaciones de red de git reciben como `auth_token`.
    pub fn git_auth_token(&self) -> Option<String> {
        self.get_git_auth_context().map(|context| context.token)
    }

    /// `accountRequiresInstallation`.
    pub fn account_requires_installation(&self, account_id: &str) -> bool {
        self.github_account_by_id(account_id)
            .is_some_and(|account| {
                !account.token.is_empty() && account.requires_installation == Some(true)
            })
    }

    /// `accountRequiresReconnect`.
    pub fn account_requires_reconnect(&self, account_id: &str) -> bool {
        self.github_account_by_id(account_id)
            .is_some_and(|account| account.requires_reconnect)
    }

    /// `authServiceAvailable`: GitHub respondió la última vez que se le
    /// preguntó.
    pub fn auth_service_available(&self) -> bool {
        self.github.auth_service_available
    }

    /// `authServiceMessage`.
    pub fn auth_service_message(&self) -> &str {
        &self.github.auth_service_message
    }

    /// `loaded` del store de GitHub.
    pub fn github_loaded(&self) -> bool {
        self.github.loaded
    }

    /// `initializing` del store de GitHub.
    pub fn github_initializing(&self) -> bool {
        self.github.initializing
    }

    fn save_github_account(
        &self,
        account: &GitHubAccount,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), mergeon_backend::StoreError>> {
        // `toPersistableAccount`
        let account = normalize_account(account);
        self.storage(cx, move |store| {
            store.save_github_account(&account).map(|_| ())
        })
    }

    /// `loadFromStorage` del store de GitHub: lee las cuentas guardadas, deja
    /// una por usuario y borra del disco las repetidas.
    pub fn load_github_accounts_from_storage(&mut self, cx: &mut Context<Self>) -> Task<()> {
        if self.github.loaded {
            return Task::ready(());
        }

        let read = self.storage(cx, |store| {
            store
                .get_github_accounts()
                .map(|accounts| (accounts, store.get_active_github_account_id()))
                .map_err(|error| error.to_string())
        });

        flow(cx, async move |this, cx| {
            let (accounts, active_id) = match read.await {
                Ok(read) => read,
                Err(error) => {
                    log::error!("Failed to persist active GitHub account: {error}");
                    return;
                }
            };

            let cleanup = this.update(cx, |this, cx| {
                if this.github.loaded {
                    return None;
                }
                let (deduped, duplicates) = dedupe_accounts(&accounts);
                let active_account_id = active_id
                    .as_ref()
                    .filter(|id| deduped.iter().any(|account| account.id == **id))
                    .cloned()
                    .or_else(|| {
                        deduped
                            .first()
                            .map(|account| account.id.clone())
                            .filter(|id| !id.is_empty())
                    });

                this.github.accounts = deduped;
                this.github.active_account_id = active_account_id.clone();
                this.github.loaded = true;
                cx.notify();

                let duplicate_ids: Vec<String> = duplicates
                    .into_iter()
                    .map(|duplicate| duplicate.id)
                    .filter(|id| !id.is_empty())
                    .collect();
                let active_changed = active_account_id != active_id;
                Some(this.storage(cx, move |store| {
                    for id in &duplicate_ids {
                        store.remove_github_account(id)?;
                    }
                    if active_changed {
                        store.set_active_github_account_id(active_account_id.as_deref())?;
                    }
                    Ok::<(), mergeon_store::StoreError>(())
                }))
            });

            if let Some(cleanup) = cleanup
                && let Err(error) = cleanup.await
            {
                log::error!("Failed to persist active GitHub account: {error}");
            }
        })
    }

    /// `initialize`: carga las cuentas y renueva las que caducan. Una sesión
    /// que GitHub ya no reconoce queda marcada para reconectar; un fallo
    /// pasajero deja la cuenta como estaba y anota que GitHub no responde.
    pub fn initialize_github(&mut self, cx: &mut Context<Self>) -> Task<()> {
        if self.github.initializing {
            return Task::ready(());
        }

        self.github.initializing = true;
        cx.notify();

        flow(cx, async move |this, cx| {
            this.update(cx, |this, cx| this.load_github_accounts_from_storage(cx))
                .await;

            let account_ids: Vec<String> = this.read_with(cx, |this, _| {
                this.github
                    .accounts
                    .iter()
                    .map(|account| account.id.clone())
                    .collect()
            });

            for account_id in account_ids {
                let Some(account) = this.read_with(cx, |this, _| {
                    this.github_account_by_id(&account_id).cloned()
                }) else {
                    continue;
                };

                // Cuentas creadas con el backend retirado: su sesión ya no vale.
                if !account.session_token.is_empty() && account.token.is_empty() {
                    this.update(cx, |this, cx| {
                        let mut updates = to_object(&account);
                        updates.insert("requiresReconnect".to_string(), json!(true));
                        this.update_account(&account_id, &updates, cx);
                    });
                    continue;
                }

                // Sin refresh token el token es de larga vida (OAuth App o personal):
                // no caduca y no hay nada que refrescar.
                if account.refresh_token.is_empty() {
                    continue;
                }

                let refresh_token = account.refresh_token.clone();
                let refreshed = this
                    .update(cx, |this, cx| {
                        this.blocking(cx, move |backend| {
                            backend.github().auth_refresh(&refresh_token)
                        })
                    })
                    .await;

                this.update(cx, |this, cx| {
                    this.apply_refresh_outcome(&account_id, &account, &refreshed, cx);
                });
            }

            let fallback = this.read_with(cx, |this, _| {
                let active_id = this.github.active_account_id.as_deref()?;
                if this.github_account_by_id(active_id).is_some() {
                    return None;
                }
                Some(
                    this.github
                        .accounts
                        .first()
                        .map(|account| account.id.clone()),
                )
            });
            if let Some(fallback) = fallback {
                let set = this
                    .update(cx, |this, cx| {
                        this.set_active_account(fallback.as_deref(), cx)
                    })
                    .await;
                if let Err(error) = set {
                    log::error!("Failed to persist active GitHub account: {error}");
                }
            }

            this.update(cx, |this, cx| {
                this.github.initializing = false;
                cx.notify();
            });
        })
    }

    /// Lo que `initialize` y `performAccountRefresh` hacen con la respuesta de
    /// un refresh.
    fn apply_refresh_outcome(
        &mut self,
        account_id: &str,
        account: &GitHubAccount,
        outcome: &GitHubResult<mergeon_github::AccountResult>,
        cx: &mut Context<Self>,
    ) -> Option<GitHubAccount> {
        match outcome {
            Ok(result) => {
                let mut refreshed = spread(to_object(account), to_object(&result.account));
                refreshed.insert("requiresReconnect".to_string(), json!(false));
                let refreshed = normalize_account_object(refreshed);
                self.update_account(account_id, &refreshed, cx);
                self.github.auth_service_available = true;
                self.github.auth_service_message = String::new();
                cx.notify();
                Some(account_from_object(refreshed))
            }
            Err(error) => {
                if error.is_permanent_auth_failure() {
                    let mut updates = to_object(account);
                    updates.insert("requiresReconnect".to_string(), json!(true));
                    self.update_account(account_id, &updates, cx);
                    self.github.auth_service_available = true;
                    self.github.auth_service_message = String::new();
                } else if error.is_transient_failure() {
                    self.github.auth_service_available = false;
                    self.github.auth_service_message = if error.message.is_empty() {
                        "GitHub is unreachable".to_string()
                    } else {
                        error.message.clone()
                    };
                }
                cx.notify();
                None
            }
        }
    }

    /// `addAccount`: añade la cuenta o, si ya existe una con ese `id` o ese
    /// usuario, la actualiza (y le quita la marca de reconexión). Devuelve el
    /// `id` de la cuenta.
    ///
    /// `account_data` es el objeto de la cuenta con los nombres de campo del
    /// original; [`to_account_payload`] lo arma desde la respuesta de GitHub.
    pub fn add_account(
        &mut self,
        account_data: Value,
        cx: &mut Context<Self>,
    ) -> Task<anyhow::Result<String>> {
        let data = match account_data {
            Value::Object(data) => data,
            _ => Map::new(),
        };
        let text = |key: &str| {
            data.get(key)
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
                .map(str::to_string)
        };
        let name = text("name").or_else(|| text("displayName"));
        let username = normalized_username(
            data.get("username")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        );
        let data_id = text("id");

        let existing_index = self.github.accounts.iter().position(|account| {
            Some(&account.id) == data_id.as_ref()
                || normalized_username(&account.username) == username
        });

        let account = match existing_index {
            Some(index) => {
                let existing = &self.github.accounts[index];
                let mut merged = spread(to_object(existing), data.clone());
                match name.or_else(|| existing.name.clone()) {
                    Some(name) => merged.insert("name".to_string(), json!(name)),
                    None => merged.remove("name"),
                };
                match text("email").or_else(|| existing.email.clone()) {
                    Some(email) => merged.insert("email".to_string(), json!(email)),
                    None => merged.remove("email"),
                };
                merged.insert("requiresReconnect".to_string(), json!(false));

                let account = account_from_object(normalize_account_object(merged));
                self.github.accounts[index] = account.clone();
                account
            }
            None => {
                let installations = data
                    .get("installations")
                    .filter(|value| truthy(Some(value)))
                    .cloned()
                    .unwrap_or_else(|| json!([]));
                let active_installation_id = match truthy_or_null(data.get("activeInstallationId"))
                {
                    Value::Null => first_installation_id(&installations),
                    id => id,
                };
                let mut account = Map::new();
                account.insert(
                    "id".to_string(),
                    json!(data_id.unwrap_or_else(|| {
                        // `Date.now().toString()`
                        mergeon_model::format::now_epoch_ms().to_string()
                    })),
                );
                account.insert(
                    "username".to_string(),
                    data.get("username").cloned().unwrap_or(Value::Null),
                );
                if let Some(name) = name {
                    account.insert("name".to_string(), json!(name));
                }
                account.insert(
                    "email".to_string(),
                    json!(text("email").unwrap_or_default()),
                );
                if let Some(avatar_url) = text("avatarUrl") {
                    account.insert("avatarUrl".to_string(), json!(avatar_url));
                }
                account.insert(
                    "token".to_string(),
                    data.get("token").cloned().unwrap_or(Value::Null),
                );
                account.insert(
                    "refreshToken".to_string(),
                    json!(text("refreshToken").unwrap_or_default()),
                );
                account.insert(
                    "isPersonalAccessToken".to_string(),
                    json!(truthy(data.get("isPersonalAccessToken"))),
                );
                account.insert(
                    "expiresAt".to_string(),
                    data.get("expiresAt").cloned().unwrap_or(Value::Null),
                );
                account.insert(
                    "installUrl".to_string(),
                    truthy_or_null(data.get("installUrl")),
                );
                account.insert(
                    "requiresInstallation".to_string(),
                    json!(truthy(data.get("requiresInstallation"))),
                );
                account.insert("installations".to_string(), installations);
                account.insert("activeInstallationId".to_string(), active_installation_id);
                account.insert("addedAt".to_string(), json!(now_iso_string()));
                account.insert("requiresReconnect".to_string(), json!(false));

                let account = account_from_object(normalize_account_object(account));
                self.github.accounts.push(account.clone());
                account
            }
        };
        cx.notify();

        let save = self.save_github_account(&account, cx);
        flow(cx, async move |this, cx| {
            let persisted = match save.await {
                Ok(()) => true,
                Err(mergeon_backend::StoreError::KeyringUnavailable) => {
                    this.update(cx, |this, cx| {
                        this.notify_warning(
                            "System keyring is unavailable. Your GitHub account will stay signed in for this session only and was not saved in plain text.",
                            cx,
                        );
                    });
                    false
                }
                Err(error) => return Err(anyhow!(error)),
            };

            let should_activate = this.read_with(cx, |this, _| {
                this.github.active_account_id.is_none() || this.github.accounts.len() == 1
            });
            if should_activate {
                if persisted {
                    this.update(cx, |this, cx| {
                        this.set_active_account(Some(&account.id), cx)
                    })
                    .await?;
                } else {
                    this.update(cx, |this, cx| {
                        this.github.active_account_id = Some(account.id.clone());
                        cx.notify();
                    });
                }
            }

            Ok(account.id)
        })
    }

    /// `refreshAccount`: renueva el token de una cuenta. `Ok(None)` si la
    /// cuenta no se refresca (token personal o sin refresh token). El fallo se
    /// devuelve, y además deja la cuenta marcada para reconectar si es
    /// permanente.
    pub fn refresh_account(
        &mut self,
        account_id: &str,
        cx: &mut Context<Self>,
    ) -> Task<GitHubResult<Option<GitHubAccount>>> {
        let id = account_id.to_string();
        let shared = self.run_shared(
            account_refresh_requests,
            account_id.to_string(),
            false,
            cx,
            move |this, cx| this.perform_account_refresh(&id, cx),
        );
        cx.foreground_executor().spawn(async move {
            settled(shared).await.unwrap_or_else(|| {
                Err(GitHubError::new(
                    "GitHub request was cancelled",
                    GitHubErrorCode::GithubCancelled,
                    499,
                ))
            })
        })
    }

    /// `performAccountRefresh`.
    fn perform_account_refresh(
        &mut self,
        account_id: &str,
        cx: &mut Context<Self>,
    ) -> Task<GitHubResult<Option<GitHubAccount>>> {
        let account = self.github_account_by_id(account_id).cloned();
        // Un token personal no se refresca: vale hasta que el usuario lo revoque.
        let Some(account) = account.filter(|account| {
            account.is_personal_access_token != Some(true) && !account.refresh_token.is_empty()
        }) else {
            return Task::ready(Ok(None));
        };

        let account_id = account_id.to_string();
        let refresh_token = account.refresh_token.clone();
        let refresh = self.blocking(cx, move |backend| {
            backend.github().auth_refresh(&refresh_token)
        });
        cx.spawn(async move |this, cx| {
            let outcome = refresh.await;
            let refreshed = this
                .update(cx, |this, cx| {
                    this.apply_refresh_outcome(&account_id, &account, &outcome, cx)
                })
                .ok()
                .flatten();
            outcome.map(|_| refreshed)
        })
    }

    /// `removeAccount`.
    ///
    /// Sin backend propio no hay sesión remota que cerrar: revocar la
    /// autorización requeriría el client_secret. El usuario puede retirarla
    /// desde los ajustes de GitHub.
    pub fn remove_account(
        &mut self,
        account_id: &str,
        cx: &mut Context<Self>,
    ) -> Task<anyhow::Result<()>> {
        let Some(index) = self
            .github
            .accounts
            .iter()
            .position(|account| account.id == account_id)
        else {
            return Task::ready(Ok(()));
        };

        self.github.accounts.remove(index);
        cx.notify();

        let account_id = account_id.to_string();
        let remove = self.storage(cx, {
            let account_id = account_id.clone();
            move |store| {
                store
                    .remove_github_account(&account_id)
                    .map_err(|error| error.to_string())
            }
        });
        flow(cx, async move |this, cx| {
            remove.await.map_err(|error| anyhow!(error))?;

            let next_active = this.read_with(cx, |this, _| {
                (this.github.active_account_id.as_deref() == Some(account_id.as_str())).then(|| {
                    this.github
                        .accounts
                        .first()
                        .map(|account| account.id.clone())
                })
            });
            if let Some(next_active) = next_active {
                this.update(cx, |this, cx| {
                    this.set_active_account(next_active.as_deref(), cx)
                })
                .await?;
            }
            Ok(())
        })
    }

    /// `setActiveAccount`. Un `id` que no es de ninguna cuenta se ignora.
    pub fn set_active_account(
        &mut self,
        account_id: Option<&str>,
        cx: &mut Context<Self>,
    ) -> Task<anyhow::Result<()>> {
        if account_id.is_some_and(|id| self.github_account_by_id(id).is_none()) {
            return Task::ready(Ok(()));
        }

        let account_id = account_id.map(str::to_string);
        self.github.active_account_id = account_id.clone();
        cx.notify();

        let write = self.storage(cx, move |store| {
            store
                .set_active_github_account_id(account_id.as_deref())
                .map_err(|error| error.to_string())
        });
        cx.background_executor()
            .spawn(async move { write.await.map_err(|error| anyhow!(error)) })
    }

    /// `setActiveInstallation`.
    pub fn set_active_installation(
        &mut self,
        account_id: &str,
        installation_id: Option<u64>,
        cx: &mut Context<Self>,
    ) -> Task<anyhow::Result<()>> {
        let Some(account) = self
            .github
            .accounts
            .iter_mut()
            .find(|account| account.id == account_id)
        else {
            return Task::ready(Ok(()));
        };

        account.active_installation_id = installation_id;
        let account = account.clone();
        cx.notify();

        let save = self.save_github_account(&account, cx);
        cx.background_executor()
            .spawn(async move { save.await.map_err(|error| anyhow!(error)) })
    }

    /// `updateAccount`: mezcla `updates` (ya normalizado) sobre la cuenta y la
    /// guarda. Un fallo al guardar solo se anota en el log.
    pub fn update_account(
        &mut self,
        account_id: &str,
        updates: &impl Serialize,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self
            .github
            .accounts
            .iter()
            .position(|account| account.id == account_id)
        else {
            return;
        };

        let merged = spread(
            to_object(&self.github.accounts[index]),
            normalize_account_object(to_object(updates)),
        );
        let account = account_from_object(merged);
        self.github.accounts[index] = account.clone();
        cx.notify();

        let save = self.save_github_account(&account, cx);
        cx.background_executor()
            .spawn(async move {
                if let Err(error) = save.await {
                    log::error!("Failed to save the GitHub account: {error}");
                }
            })
            .detach();
    }

    // ------------------------------------------------------------------
    // useGitHubAuthFlow
    // ------------------------------------------------------------------

    /// Estado del alta de cuenta: si está en curso, el código a introducir, el
    /// mensaje de estado y el error de validación.
    pub fn github_auth_flow(&self) -> &GitHubAuthFlow {
        &self.github.auth_flow
    }

    /// `reset`.
    pub fn reset_login(&mut self, cx: &mut Context<Self>) {
        let flow = &mut self.github.auth_flow;
        flow.validation_error.clear();
        flow.status_message.clear();
        flow.install_url.clear();
        flow.user_code.clear();
        flow.verification_uri.clear();
        flow.install_auto_opened = false;
        cx.notify();
    }

    /// `cancel`: abandona el intento en curso; lo que aún llegue de él se
    /// descarta.
    pub fn cancel_login(&mut self, cx: &mut Context<Self>) {
        self.github.auth_flow.auth_flow_token += 1;
        self.github.auth_flow.is_authenticating = false;
        self.reset_login(cx);
    }

    /// `openInstallPage`: `false` si no hay página de instalación.
    pub fn open_install_page(&mut self, cx: &mut Context<Self>) -> Task<bool> {
        let url = self.github.auth_flow.install_url.clone();
        self.open_login_page(url, cx)
    }

    /// `openVerificationPage`: `false` si aún no hay código.
    pub fn open_verification_page(&mut self, cx: &mut Context<Self>) -> Task<bool> {
        let url = self.github.auth_flow.verification_uri.clone();
        self.open_login_page(url, cx)
    }

    fn open_login_page(&mut self, url: String, cx: &mut Context<Self>) -> Task<bool> {
        if url.is_empty() {
            return Task::ready(false);
        }

        let open = self.open_external(&url, cx);
        cx.background_executor().spawn(async move {
            if let Err(error) = open.await {
                log::warn!("could not open {url}: {error}");
            }
            true
        })
    }

    /// `copyUserCode`: copia el código al portapapeles. `false` si aún no hay
    /// código.
    pub fn copy_user_code(&mut self, cx: &mut Context<Self>) -> bool {
        let user_code = self.github.auth_flow.user_code.clone();
        if user_code.is_empty() {
            return false;
        }

        cx.write_to_clipboard(ClipboardItem::new_string(user_code));
        self.notify_success("Code copied to clipboard", cx);
        true
    }

    /// `finishLogin`.
    fn finish_login(
        &mut self,
        account_data: &ApiAccount,
        cx: &mut Context<Self>,
    ) -> Task<anyhow::Result<String>> {
        let username = account_data.username.clone().unwrap_or_default();
        let added = self.add_account(to_account_payload(account_data), cx);
        flow(cx, async move |this, cx| {
            let account_id = added.await?;
            this.update(cx, |this, cx| {
                this.notify_success(&format!("Account @{username} added successfully"), cx);
            });
            Ok(account_id)
        })
    }

    /// `startLogin`. Device flow: GitHub entrega un código corto que el usuario
    /// introduce en su navegador. No hay callback, así que la app solo consulta
    /// hasta que autorice. Devuelve el `id` de la cuenta añadida, o `None` si
    /// se canceló, falló o ya había un alta en curso.
    pub fn start_login(&mut self, cx: &mut Context<Self>) -> Task<Option<String>> {
        if self.github.auth_flow.is_authenticating {
            return Task::ready(None);
        }

        self.github.auth_flow.is_authenticating = true;
        self.reset_login(cx);
        self.github.auth_flow.status_message = "Requesting a device code...".to_string();
        let flow_token = self.github.auth_flow.auth_flow_token + 1;
        self.github.auth_flow.auth_flow_token = flow_token;
        cx.notify();

        flow(cx, async move |this, cx| {
            let outcome = device_login(&this, cx, flow_token).await;
            let is_current = |this: &AppModel| this.github.auth_flow.auth_flow_token == flow_token;

            this.update(cx, |this, cx| {
                let current = is_current(this);
                let account_id = match outcome {
                    Ok(account_id) => account_id,
                    Err(_) if !current => None,
                    Err(error) => {
                        this.github.auth_flow.validation_error = if error.is_empty() {
                            "Failed to authenticate with GitHub".to_string()
                        } else {
                            error.clone()
                        };
                        this.notify_error(&format!("Authentication failed: {error}"), cx);
                        None
                    }
                };

                if current {
                    this.github.auth_flow.is_authenticating = false;
                    this.github.auth_flow.status_message.clear();
                    this.github.auth_flow.user_code.clear();
                }
                cx.notify();
                account_id
            })
        })
    }

    /// `loginWithToken`: alta alternativa con un token personal de GitHub.
    pub fn login_with_token(
        &mut self,
        personal_access_token: &str,
        cx: &mut Context<Self>,
    ) -> Task<Option<String>> {
        if self.github.auth_flow.is_authenticating {
            return Task::ready(None);
        }

        self.github.auth_flow.is_authenticating = true;
        self.reset_login(cx);
        self.github.auth_flow.status_message = "Validating token...".to_string();
        cx.notify();

        let token = personal_access_token.to_string();
        let authenticate = self.blocking(cx, move |backend| backend.github().auth_token(&token));

        flow(cx, async move |this, cx| {
            let outcome: Result<String, String> = match authenticate.await {
                Ok(result) => this
                    .update(cx, |this, cx| this.finish_login(&result.account, cx))
                    .await
                    .map_err(|error| error.to_string()),
                Err(error) => Err(error.message),
            };

            this.update(cx, |this, cx| {
                let account_id = match outcome {
                    Ok(account_id) => Some(account_id),
                    Err(error) => {
                        this.github.auth_flow.validation_error = if error.is_empty() {
                            "The token could not be validated".to_string()
                        } else {
                            error.clone()
                        };
                        this.notify_error(&format!("Authentication failed: {error}"), cx);
                        None
                    }
                };
                this.github.auth_flow.is_authenticating = false;
                this.github.auth_flow.status_message.clear();
                cx.notify();
                account_id
            })
        })
    }

    // ------------------------------------------------------------------
    // useGitHubRepositories
    // ------------------------------------------------------------------

    /// `isLoading` de `useGitHubRepositories`.
    pub fn github_repositories_loading(&self) -> bool {
        self.github.repository_requests.is_loading
    }

    /// `error` de `useGitHubRepositories`.
    pub fn github_repositories_error(&self) -> Option<&str> {
        self.github.repository_requests.error.as_deref()
    }

    /// `runRepositoryRequest`: solo la petición más reciente cuenta. Si llega
    /// otra (o se cancela) mientras esta corre, su resultado es `Ok(None)`.
    fn run_repository_request<T, F>(
        &mut self,
        cx: &mut Context<Self>,
        task: F,
    ) -> Task<GitHubResult<Option<T>>>
    where
        T: 'static,
        F: AsyncFnOnce(Entity<AppModel>, &mut AsyncApp, u64) -> GitHubResult<Option<T>> + 'static,
    {
        let requests = &mut self.github.repository_requests;
        requests.active_request_id += 1;
        let request_id = requests.active_request_id;
        requests.is_loading = true;
        requests.error = None;
        cx.notify();

        flow(cx, async move |this, cx| {
            let result = task(this.clone(), cx, request_id).await;
            this.update(cx, |this, cx| {
                let requests = &mut this.github.repository_requests;
                let stale = request_id != requests.active_request_id;
                if !stale {
                    requests.is_loading = false;
                    if let Err(error) = &result {
                        requests.error = Some(if error.message.is_empty() {
                            REPOSITORY_REQUEST_FAILED.to_string()
                        } else {
                            error.message.clone()
                        });
                    }
                }
                cx.notify();

                match result {
                    Ok(_) if stale => Ok(None),
                    other => other,
                }
            })
        })
    }

    /// `listRepositories`: una página de los repositorios del contexto.
    pub fn list_repositories(
        &mut self,
        auth_context: AuthContext,
        page: u32,
        per_page: u32,
        cx: &mut Context<Self>,
    ) -> Task<GitHubResult<Option<Vec<GitHubRepository>>>> {
        self.run_repository_request(cx, async move |this, cx, _| {
            this.update(cx, |this, cx| {
                this.blocking(cx, move |backend| {
                    backend
                        .github()
                        .get_user_repositories(&auth_context, page, per_page)
                })
            })
            .await
            .map(Some)
        })
    }

    /// `searchRepositories`.
    pub fn search_repositories(
        &mut self,
        auth_context: AuthContext,
        query: &str,
        page: u32,
        per_page: u32,
        cx: &mut Context<Self>,
    ) -> Task<GitHubResult<Option<RepositorySearchResult>>> {
        let query = query.to_string();
        self.run_repository_request(cx, async move |this, cx, _| {
            this.update(cx, |this, cx| {
                this.blocking(cx, move |backend| {
                    backend
                        .github()
                        .search_repositories(&auth_context, &query, page, per_page)
                })
            })
            .await
            .map(Some)
        })
    }

    /// `listAllRepositories`: reúne los repositorios de todas las
    /// instalaciones a las que la cuenta tiene acceso, para que aparezcan
    /// también los de colaborador y de organizaciones concedidos por otras
    /// instalaciones.
    ///
    /// Si falla una instalación se sigue con las demás y el fallo se informa
    /// en el resultado; si fallan todas, el error es el de la primera.
    pub fn list_all_repositories(
        &mut self,
        token: &str,
        installations: Vec<GitHubInstallation>,
        options: ListAllRepositoriesOptions,
        cx: &mut Context<Self>,
    ) -> Task<GitHubResult<Option<AllRepositories>>> {
        let token = token.to_string();
        self.run_repository_request(cx, async move |this, cx, request_id| {
            let is_stale = |cx: &AsyncApp| {
                this.read_with(cx, |this, _| {
                    request_id != this.github.repository_requests.active_request_id
                })
            };

            let mut ordered_installations = installations;
            ordered_installations.sort_by_key(|installation| {
                Some(installation.id) == options.active_installation_id
            });

            let mut repositories: Vec<InstallationRepository> = Vec::new();
            let mut failures = Vec::new();
            let mut first_error = None;

            for installation in &ordered_installations {
                let mut page = 1;
                while page <= options.max_pages_per_installation {
                    if is_stale(cx) {
                        return Ok(None);
                    }

                    let auth_context = AuthContext::new(token.clone(), Some(installation.id));
                    let per_page = options.per_page;
                    let batch = this
                        .update(cx, |this, cx| {
                            this.blocking(cx, move |backend| {
                                backend.github().get_user_repositories(
                                    &auth_context,
                                    page,
                                    per_page,
                                )
                            })
                        })
                        .await;

                    let batch = match batch {
                        Ok(batch) => batch,
                        Err(error) => {
                            failures.push(InstallationFailure {
                                installation_id: installation.id,
                                account_login: installation
                                    .extra
                                    .get("accountLogin")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_string(),
                                message: if error.message.is_empty() {
                                    REPOSITORY_REQUEST_FAILED.to_string()
                                } else {
                                    error.message.clone()
                                },
                            });
                            first_error.get_or_insert(error);
                            break;
                        }
                    };

                    if batch.is_empty() {
                        break;
                    }

                    let batch_len = batch.len();
                    for repository in batch {
                        if !repositories
                            .iter()
                            .any(|known| known.repository.id == repository.id)
                        {
                            repositories.push(InstallationRepository {
                                repository,
                                installation_id: installation.id,
                            });
                        }
                    }

                    if batch_len < options.per_page as usize {
                        break;
                    }
                    page += 1;
                }
            }

            if is_stale(cx) {
                return Ok(None);
            }

            if !ordered_installations.is_empty()
                && failures.len() == ordered_installations.len()
                && let Some(first_error) = first_error
            {
                return Err(first_error);
            }

            repositories.sort_by_key(|item| {
                std::cmp::Reverse(parse_updated_at(item.repository.updated_at.as_deref()))
            });

            Ok(Some(AllRepositories {
                repositories,
                failures,
            }))
        })
    }

    /// `cancelRepositoryRequests`.
    pub fn cancel_repository_requests(&mut self, cx: &mut Context<Self>) {
        self.github.repository_requests.active_request_id += 1;
        self.github.repository_requests.is_loading = false;
        cx.notify();
    }
}

/// El cuerpo de `startLogin`: pide el código, abre el navegador y consulta
/// hasta que el usuario autorice. `Err` lleva el mensaje del fallo.
async fn device_login(
    this: &Entity<AppModel>,
    cx: &mut AsyncApp,
    flow_token: u64,
) -> Result<Option<String>, String> {
    let is_current = |cx: &AsyncApp| {
        this.read_with(cx, |this, _| {
            this.github.auth_flow.auth_flow_token == flow_token
        })
    };
    let open_external =
        |url: String, cx: &mut AsyncApp| this.update(cx, |this, cx| this.open_external(&url, cx));

    let session = this
        .update(cx, |this, cx| {
            this.blocking(cx, |backend| backend.github().auth_start())
        })
        .await
        .map_err(|error| error.message)?
        .session;

    let (Some(device_code), Some(verification_uri)) = (
        session.device_code.filter(|code| !code.is_empty()),
        session.verification_uri.filter(|uri| !uri.is_empty()),
    ) else {
        return Err("Unexpected response from GitHub. Restart the app and try again.".to_string());
    };

    this.update(cx, |this, cx| {
        this.github.auth_flow.user_code = session.user_code.clone().unwrap_or_default();
        this.github.auth_flow.verification_uri = verification_uri.clone();
        cx.notify();
    });
    let mut poll_interval_ms = match session.poll_interval_ms {
        0 => 5000,
        interval => interval,
    };

    open_external(verification_uri, cx).await?;
    this.update(cx, |this, cx| {
        this.github.auth_flow.status_message =
            "Enter the code in your browser to authorize the app".to_string();
        cx.notify();
    });

    let mut account_data = None;
    while is_current(cx) {
        cx.background_executor()
            .timer(Duration::from_millis(poll_interval_ms))
            .await;
        let device_code = device_code.clone();
        let poll_result = this
            .update(cx, |this, cx| {
                this.blocking(cx, move |backend| backend.github().auth_poll(&device_code))
            })
            .await
            .map_err(|error| error.message)?;

        match poll_result {
            PollLoginResult::Completed { account, .. } => {
                account_data = Some(*account);
                break;
            }
            PollLoginResult::InstallationRequired { install_url, .. } => {
                let install_url = install_url.unwrap_or_default();
                let should_open = this.update(cx, |this, cx| {
                    let flow = &mut this.github.auth_flow;
                    flow.install_url = install_url.clone();
                    flow.status_message = "Waiting for GitHub App installation...".to_string();
                    let should_open = !flow.install_auto_opened && !flow.install_url.is_empty();
                    if should_open {
                        flow.install_auto_opened = true;
                    }
                    cx.notify();
                    should_open
                });

                if should_open {
                    open_external(install_url, cx).await?;
                }
            }
            PollLoginResult::Pending {
                poll_interval_ms: slower,
            } => {
                // GitHub pide bajar el ritmo cuando se consulta demasiado seguido.
                if let Some(slower) = slower.filter(|interval| *interval != 0) {
                    poll_interval_ms = slower;
                }
            }
        }
    }

    let Some(account_data) = account_data.filter(|_| is_current(cx)) else {
        return Ok(None);
    };

    this.update(cx, |this, cx| this.finish_login(&account_data, cx))
        .await
        .map(Some)
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(value: Value) -> GitHubAccount {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn normalizing_fills_the_active_installation_and_the_flags() {
        let normalized = normalize_account(&account(json!({
            "id": "1",
            "username": "octocat",
            "token": "ghu_token",
            "installations": [{ "id": 10, "account": "octocat" }]
        })));

        assert_eq!(normalized.active_installation_id, Some(10));
        assert_eq!(normalized.requires_installation, Some(false));
        assert!(!normalized.requires_reconnect);
        assert_eq!(normalized.install_url, None);
    }

    #[test]
    fn an_account_without_installations_needs_one_unless_it_says_otherwise() {
        let deduced = normalize_account(&account(json!({ "id": "1", "username": "octocat" })));
        assert_eq!(deduced.requires_installation, Some(true));

        // OAuth App o token personal: la marca que trae la cuenta manda.
        let explicit = normalize_account(&account(json!({
            "id": "1",
            "username": "octocat",
            "requiresInstallation": false
        })));
        assert_eq!(explicit.requires_installation, Some(false));
    }

    #[test]
    fn duplicates_merge_into_the_first_account_and_are_reported() {
        let (deduped, duplicates) = dedupe_accounts(&[
            account(json!({
                "id": "account-1",
                "username": "Spilveru",
                "sessionToken": "session-1",
                "addedAt": "2026-01-01T00:00:00.000Z",
                "activeInstallationId": 10,
                "installations": [{ "id": 10 }]
            })),
            account(json!({
                "id": "account-2",
                "username": " spilveru ",
                "sessionToken": "session-2",
                "addedAt": "2026-02-01T00:00:00.000Z",
                "activeInstallationId": 11,
                "installations": [{ "id": 11 }]
            })),
        ]);

        assert_eq!(deduped.len(), 1);
        assert_eq!(deduped[0].id, "account-1");
        assert_eq!(deduped[0].session_token, "session-2");
        assert_eq!(deduped[0].active_installation_id, Some(11));
        assert_eq!(
            deduped[0].added_at.as_deref(),
            Some("2026-01-01T00:00:00.000Z")
        );
        assert_eq!(duplicates.len(), 1);
        assert_eq!(duplicates[0].id, "account-2");
    }

    #[test]
    fn accounts_without_a_username_are_told_apart_by_id() {
        let (deduped, duplicates) = dedupe_accounts(&[
            account(json!({ "id": "1", "username": "" })),
            account(json!({ "id": "2", "username": "" })),
        ]);

        assert_eq!(deduped.len(), 2);
        assert!(duplicates.is_empty());
    }

    #[test]
    fn the_login_payload_carries_what_add_account_reads() {
        let api_account: ApiAccount = serde_json::from_value(json!({
            "id": "7",
            "username": "Spilveru",
            "displayName": "Spilveru",
            "avatarUrl": "https://example.com/avatar.png",
            "token": "ghu_token",
            "refreshToken": "ghr_token",
            "expiresAt": "2026-03-09T00:00:00.000Z",
            "installations": [{ "id": 10, "account": "Spilveru" }],
            "activeInstallationId": null
        }))
        .unwrap();

        let payload = to_account_payload(&api_account);

        assert_eq!(payload["username"], "Spilveru");
        assert_eq!(payload["token"], "ghu_token");
        assert_eq!(payload["refreshToken"], "ghr_token");
        // Sin instalación activa se toma la primera.
        assert_eq!(payload["activeInstallationId"], 10);
        assert_eq!(payload["isPersonalAccessToken"], false);
        assert_eq!(payload["installUrl"], Value::Null);
    }
}
