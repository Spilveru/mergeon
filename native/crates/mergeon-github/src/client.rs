// Cliente de GitHub sin backend propio: device flow + token personal.
//
// El device flow no necesita `client_secret` (ni para canjear el código ni para
// refrescar), así que no queda ningún secreto dentro del binario. El client_id
// de una GitHub App es público.
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::config::GitHubConfig;
use crate::error::{GitHubError, GitHubErrorCode, GitHubResult};
use crate::http::{HttpMethod, HttpRequest, HttpTransport, TransportError, UreqTransport};
use crate::types::{
    AccountResult, DeviceLoginSession, GitHubAccount, GitHubInstallation, GitHubRepository,
    PollLoginResult, RepositoryOwner, RepositoryPage,
};

const DEVICE_GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:device_code";

// Las OAuth App sí usan scopes; las GitHub App los ignoran (mandan sus permisos).
const OAUTH_SCOPE: &str = "repo";

const MAX_INSTALLATION_PAGES: u32 = 10;
const INSTALLATIONS_PER_PAGE: usize = 100;

type Clock = Arc<dyn Fn() -> SystemTime + Send + Sync>;

/// Cliente de GitHub. Es barato de clonar y se puede compartir entre hilos;
/// todas las llamadas son síncronas y bloqueantes.
#[derive(Clone)]
pub struct GitHubClient {
    pub(crate) config: GitHubConfig,
    pub(crate) transport: Arc<dyn HttpTransport>,
    pub(crate) clock: Clock,
}

impl std::fmt::Debug for GitHubClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitHubClient")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

#[derive(Default)]
struct RequestOptions<'a> {
    method: Option<HttpMethod>,
    access_token: Option<&'a str>,
    body: Option<Value>,
    query: Vec<(&'static str, String)>,
}

impl GitHubClient {
    /// Cliente con el transporte real (`ureq`).
    pub fn new(config: GitHubConfig) -> Self {
        Self::with_transport(config, Arc::new(UreqTransport::new()))
    }

    /// Cliente con la configuración resuelta desde el entorno
    /// (ver [`GitHubConfig`] para el orden de precedencia).
    pub fn from_env() -> Self {
        Self::new(GitHubConfig::from_env())
    }

    /// Cliente con un transporte propio; los tests inyectan uno falso.
    pub fn with_transport(config: GitHubConfig, transport: Arc<dyn HttpTransport>) -> Self {
        Self {
            config,
            transport,
            clock: Arc::new(SystemTime::now),
        }
    }

    /// Sustituye el reloj con el que se calcula `expiresAt`.
    pub fn with_clock(mut self, clock: impl Fn() -> SystemTime + Send + Sync + 'static) -> Self {
        self.clock = Arc::new(clock);
        self
    }

    pub fn config(&self) -> &GitHubConfig {
        &self.config
    }

    /// `getGitHubClientId()`.
    pub fn client_id(&self) -> &str {
        &self.config.client_id
    }

    /// El error de una petición que no obtuvo respuesta.
    pub(crate) fn transport_failure(
        &self,
        request: &HttpRequest,
        error: TransportError,
    ) -> GitHubError {
        // El detalle del transporte solo va al log; nunca incluye
        // cabeceras ni cuerpo, así que no arrastra tokens.
        log::warn!(
            "GitHub request failed: {} {} ({error})",
            request.method.as_str(),
            request.url
        );

        match error {
            TransportError::Cancelled => GitHubError::new(
                "GitHub request was cancelled",
                GitHubErrorCode::GithubCancelled,
                499,
            ),
            TransportError::Timeout => GitHubError::new(
                "GitHub request timed out",
                GitHubErrorCode::GithubTimeout,
                504,
            ),
            TransportError::Network(_) => GitHubError::new(
                format!("GitHub is unreachable at {}", self.config.api_url),
                GitHubErrorCode::GithubUnavailable,
                503,
            ),
        }
    }

    fn github_request(&self, url: &str, options: RequestOptions<'_>) -> GitHubResult<Value> {
        let method = options.method.unwrap_or(HttpMethod::Get);
        let target = append_query(url, &options.query);

        let mut headers = vec![
            ("Accept".to_string(), "application/json".to_string()),
            ("X-GitHub-Api-Version".to_string(), "2022-11-28".to_string()),
        ];
        if options.body.is_some() {
            headers.push(("Content-Type".to_string(), "application/json".to_string()));
        }
        if let Some(access_token) = options.access_token.filter(|token| !token.is_empty()) {
            headers.push((
                "Authorization".to_string(),
                format!("Bearer {access_token}"),
            ));
        }

        let request = HttpRequest {
            method,
            url: target,
            headers,
            body: options.body.as_ref().map(Value::to_string),
            timeout: self.config.timeout,
        };

        let response = match self.transport.send(&request) {
            Ok(response) => response,
            Err(error) => return Err(self.transport_failure(&request, error)),
        };

        log::debug!(
            "GitHub {} {} -> {}",
            request.method.as_str(),
            request.url,
            response.status
        );

        let payload = parse_payload(&response.body)?;

        if !response.is_ok() {
            // Un 401 significa que el token ya no sirve: el store lo trata como fallo
            // permanente y pide reconectar la cuenta.
            return Err(GitHubError::new(
                truthy_text(payload.get("message"))
                    .unwrap_or_else(|| format!("GitHub request failed: {}", response.status)),
                if response.status == 401 {
                    GitHubErrorCode::AuthRequired
                } else {
                    GitHubErrorCode::GithubError
                },
                response.status,
            ));
        }

        Ok(payload)
    }

    /// Recorre todas las páginas: con `per_page` por defecto GitHub devuelve solo 30
    /// instalaciones, y las que sobran desaparecen en silencio junto con sus repos.
    fn fetch_installations(&self, access_token: &str) -> GitHubResult<Vec<GitHubInstallation>> {
        // Con un token de OAuth App o un PAT este endpoint no aplica: se lista por
        // afiliación en su lugar.
        if !self.config.is_github_app() {
            return Ok(Vec::new());
        }

        let mut installations = Vec::new();

        for page in 1..=MAX_INSTALLATION_PAGES {
            let payload = self.github_request(
                &format!("{}/user/installations", self.config.api_url),
                RequestOptions {
                    access_token: Some(access_token),
                    query: vec![
                        ("page", page.to_string()),
                        ("per_page", INSTALLATIONS_PER_PAGE.to_string()),
                    ],
                    ..Default::default()
                },
            )?;

            let batch = payload
                .get("installations")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or_default();

            installations.extend(batch.iter().map(|installation| {
                let account = installation.get("account");
                GitHubInstallation {
                    id: uint(installation.get("id")),
                    account: truthy_text(account.and_then(|account| account.get("login"))),
                    avatar_url: truthy_text(account.and_then(|account| account.get("avatar_url"))),
                    repository_selection: truthy_text(installation.get("repository_selection")),
                }
            }));

            if batch.len() < INSTALLATIONS_PER_PAGE {
                break;
            }
        }

        Ok(installations)
    }

    fn build_account(
        &self,
        token_payload: &Value,
        is_personal_access_token: bool,
    ) -> GitHubResult<GitHubAccount> {
        let access_token = text(token_payload.get("access_token")).unwrap_or_default();
        let user = self.github_request(
            &format!("{}/user", self.config.api_url),
            RequestOptions {
                access_token: Some(&access_token),
                ..Default::default()
            },
        )?;

        // Un token personal no pertenece a ninguna instalación de la App: el endpoint
        // de instalaciones es de tokens user-to-server y con un PAT falla o va vacío.
        // Esas cuentas listan por /user/repos, así que no necesitan instalación.
        let installations = if is_personal_access_token {
            Vec::new()
        } else {
            self.fetch_installations(&access_token)?
        };

        let is_github_app = self.config.is_github_app();
        let username = text(user.get("login"));

        Ok(GitHubAccount {
            id: js_string(user.get("id")),
            display_name: truthy_text(user.get("name")).or_else(|| username.clone()),
            username,
            avatar_url: text(user.get("avatar_url")),
            refresh_token: truthy_text(token_payload.get("refresh_token")),
            expires_at: self.expires_at(token_payload.get("expires_in")),
            active_installation_id: installations
                .first()
                .and_then(|installation| installation.id)
                .filter(|id| *id != 0),
            install_url: is_github_app.then(|| self.config.install_url()),
            // Solo una GitHub App necesita instalarse en alguna cuenta.
            requires_installation: is_github_app
                && !is_personal_access_token
                && installations.is_empty(),
            installations,
            token: access_token,
            is_personal_access_token: false,
        })
    }

    fn expires_at(&self, expires_in: Option<&Value>) -> Option<String> {
        let seconds = number(expires_in).filter(|seconds| *seconds != 0.0)?;
        let now_ms = (self.clock)()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis() as f64)
            .unwrap_or(0.0);

        to_iso_string(now_ms + seconds * 1000.0)
    }

    /// Arranca el device flow: GitHub devuelve un código que el usuario introduce en
    /// `verification_uri`. No hay callback, así que la app no necesita registrar un
    /// protocolo propio ni levantar un servidor local.
    pub fn start_login(&self) -> GitHubResult<DeviceLoginSession> {
        let mut body = json!({ "client_id": self.config.client_id });
        if !self.config.is_github_app() {
            body["scope"] = json!(OAUTH_SCOPE);
        }

        let payload = self.github_request(
            &format!("{}/login/device/code", self.config.base_url),
            RequestOptions {
                method: Some(HttpMethod::Post),
                body: Some(body),
                ..Default::default()
            },
        )?;

        if let Some(error) = truthy_text(payload.get("error")) {
            return Err(GitHubError::new(
                truthy_text(payload.get("error_description")).unwrap_or_else(|| error.clone()),
                GitHubErrorCode::from_code(&error),
                400,
            ));
        }

        Ok(DeviceLoginSession {
            device_code: text(payload.get("device_code")),
            user_code: text(payload.get("user_code")),
            verification_uri: text(payload.get("verification_uri")),
            expires_in: uint(payload.get("expires_in")),
            poll_interval_ms: interval_ms(payload.get("interval"), 5.0),
        })
    }

    /// Consulta si el usuario ya autorizó. Devuelve el mismo contrato de estados que
    /// usaba el backend (`pending` / `completed` / `installation_required`) para no
    /// cambiar la máquina de estados del renderer.
    pub fn poll_login(&self, device_code: &str) -> GitHubResult<PollLoginResult> {
        let payload = self.github_request(
            &format!("{}/login/oauth/access_token", self.config.base_url),
            RequestOptions {
                method: Some(HttpMethod::Post),
                body: Some(json!({
                    "client_id": self.config.client_id,
                    "device_code": device_code,
                    "grant_type": DEVICE_GRANT_TYPE,
                })),
                ..Default::default()
            },
        )?;

        if let Some(error) = truthy_text(payload.get("error")) {
            if error == "authorization_pending" {
                return Ok(PollLoginResult::Pending {
                    poll_interval_ms: None,
                });
            }

            if error == "slow_down" {
                return Ok(PollLoginResult::Pending {
                    poll_interval_ms: Some(interval_ms(payload.get("interval"), 10.0)),
                });
            }

            let code = if error == "expired_token" {
                GitHubErrorCode::Expired
            } else {
                GitHubErrorCode::from_code(&error)
            };
            return Err(GitHubError::new(
                truthy_text(payload.get("error_description")).unwrap_or(error),
                code,
                400,
            ));
        }

        let account = self.build_account(&payload, false)?;
        let install_url = account.install_url.clone();

        Ok(if account.requires_installation {
            PollLoginResult::InstallationRequired {
                account: Box::new(account),
                install_url,
            }
        } else {
            PollLoginResult::Completed {
                account: Box::new(account),
                install_url,
            }
        })
    }

    /// Renueva el token. GitHub permite refrescar sin `client_secret` cuando el token
    /// se obtuvo por device flow, que es justo nuestro caso.
    pub fn refresh_session(&self, refresh_token: &str) -> GitHubResult<AccountResult> {
        if refresh_token.is_empty() {
            return Err(GitHubError::new(
                "Missing refresh token",
                GitHubErrorCode::AuthRequired,
                401,
            ));
        }

        let payload = self.github_request(
            &format!("{}/login/oauth/access_token", self.config.base_url),
            RequestOptions {
                method: Some(HttpMethod::Post),
                body: Some(json!({
                    "client_id": self.config.client_id,
                    "grant_type": "refresh_token",
                    "refresh_token": refresh_token,
                })),
                ..Default::default()
            },
        )?;

        if let Some(error) = truthy_text(payload.get("error")) {
            let permanent = matches!(
                error.as_str(),
                "bad_refresh_token" | "invalid_grant" | "unauthorized"
            );
            let code = if permanent {
                GitHubErrorCode::AuthRequired
            } else {
                GitHubErrorCode::from_code(&error)
            };
            return Err(GitHubError::new(
                truthy_text(payload.get("error_description")).unwrap_or(error),
                code,
                if permanent { 401 } else { 400 },
            ));
        }

        Ok(AccountResult {
            account: self.build_account(&payload, false)?,
        })
    }

    /// Alta con un token personal (PAT) pegado por el usuario.
    pub fn authenticate_with_token(
        &self,
        personal_access_token: &str,
    ) -> GitHubResult<AccountResult> {
        let token = personal_access_token.trim();

        if token.is_empty() {
            return Err(GitHubError::new(
                "A GitHub token is required",
                GitHubErrorCode::InvalidToken,
                400,
            ));
        }

        let mut account = self.build_account(&json!({ "access_token": token }), true)?;
        account.is_personal_access_token = true;

        Ok(AccountResult { account })
    }

    /// Sin instalación (cuentas con token personal) el listado va por /user/repos con
    /// afiliación explícita, que sí incluye los repos donde el usuario es
    /// colaborador aunque el dueño no tenga instalada la GitHub App.
    ///
    /// `query` filtra localmente la página recibida por nombre completo y
    /// descripción; un `installation_id` de 0 cuenta como ausente.
    pub fn get_repositories(
        &self,
        access_token: &str,
        installation_id: Option<u64>,
        page: u32,
        per_page: u32,
        query: Option<&str>,
    ) -> GitHubResult<RepositoryPage> {
        let installation_id = installation_id.filter(|id| *id != 0);

        let (raw_repositories, total_count) = match installation_id {
            Some(installation_id) => {
                let payload = self.github_request(
                    &format!(
                        "{}/user/installations/{installation_id}/repositories",
                        self.config.api_url
                    ),
                    RequestOptions {
                        access_token: Some(access_token),
                        query: vec![
                            ("page", page.to_string()),
                            ("per_page", per_page.to_string()),
                        ],
                        ..Default::default()
                    },
                )?;

                let total_count = uint(payload.get("total_count")).unwrap_or(0);
                (take_array(payload, Some("repositories")), total_count)
            }
            None => {
                let payload = self.github_request(
                    &format!("{}/user/repos", self.config.api_url),
                    RequestOptions {
                        access_token: Some(access_token),
                        query: vec![
                            ("page", page.to_string()),
                            ("per_page", per_page.to_string()),
                            (
                                "affiliation",
                                "owner,collaborator,organization_member".to_string(),
                            ),
                            ("sort", "updated".to_string()),
                        ],
                        ..Default::default()
                    },
                )?;

                let repositories = take_array(payload, None);
                let total_count = repositories.len() as u64;
                (repositories, total_count)
            }
        };

        let normalized_query = query.unwrap_or_default().trim().to_lowercase();
        let repositories: Vec<GitHubRepository> = raw_repositories
            .iter()
            .filter(|repo| {
                if normalized_query.is_empty() {
                    return true;
                }

                format!(
                    "{} {}",
                    text(repo.get("full_name")).unwrap_or_default(),
                    truthy_text(repo.get("description")).unwrap_or_default()
                )
                .to_lowercase()
                .contains(&normalized_query)
            })
            .map(normalize_repository)
            .collect();

        Ok(RepositoryPage {
            total_count: if total_count != 0 {
                total_count
            } else {
                repositories.len() as u64
            },
            repositories,
            installation_id,
        })
    }
}

fn normalize_repository(repo: &Value) -> GitHubRepository {
    let owner = repo.get("owner");

    GitHubRepository {
        id: uint(repo.get("id")),
        name: text(repo.get("name")),
        full_name: text(repo.get("full_name")),
        description: text(repo.get("description")),
        private: repo.get("private").and_then(Value::as_bool),
        clone_url: text(repo.get("clone_url")),
        html_url: text(repo.get("html_url")),
        language: text(repo.get("language")),
        stars: uint(repo.get("stargazers_count")),
        forks: uint(repo.get("forks_count")),
        updated_at: text(repo.get("updated_at")),
        owner: RepositoryOwner {
            login: text(owner.and_then(|owner| owner.get("login"))),
            avatar_url: text(owner.and_then(|owner| owner.get("avatar_url"))),
        },
    }
}

/// `parsePayload`: el cuerpo debe ser JSON y además un objeto (o un array).
pub(crate) fn parse_payload(body: &[u8]) -> GitHubResult<Value> {
    let invalid = || {
        GitHubError::new(
            "Invalid response from GitHub",
            GitHubErrorCode::InvalidResponse,
            502,
        )
    };

    let payload: Value = serde_json::from_slice(body).map_err(|_| invalid())?;
    if !payload.is_object() && !payload.is_array() {
        return Err(invalid());
    }

    Ok(payload)
}

/// Extrae el array de la respuesta (o de una de sus claves); vacío si no lo es.
fn take_array(payload: Value, key: Option<&str>) -> Vec<Value> {
    let value = match (payload, key) {
        (Value::Object(mut object), Some(key)) => object.remove(key).unwrap_or(Value::Null),
        (payload, None) => payload,
        _ => Value::Null,
    };

    match value {
        Value::Array(items) => items,
        _ => Vec::new(),
    }
}

/// Texto de un campo: cadenas tal cual; números y booleanos convertidos.
pub(crate) fn text(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(text) => Some(text.clone()),
        Value::Number(_) | Value::Bool(_) => Some(js_string(value)),
        _ => None,
    }
}

/// Como [`text`], pero una cadena vacía cuenta como ausente (`valor || null`).
pub(crate) fn truthy_text(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::Bool(false) => None,
        Value::Number(number) if number.as_f64() == Some(0.0) => None,
        _ => text(value).filter(|text| !text.is_empty()),
    }
}

/// Número de un campo; acepta también cadenas numéricas, como `Number(valor)`.
pub(crate) fn number(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse::<f64>().ok(),
        _ => None,
    }
    .filter(|number| number.is_finite())
}

pub(crate) fn uint(value: Option<&Value>) -> Option<u64> {
    let value = value?;
    value.as_u64().or_else(|| {
        value
            .as_f64()
            .filter(|number| {
                number.fract() == 0.0 && (0.0..=9_007_199_254_740_991.0).contains(number)
            })
            .map(|number| number as u64)
    })
}

/// `(payload.interval || porDefecto) * 1000`.
fn interval_ms(value: Option<&Value>, default_seconds: f64) -> u64 {
    let seconds = number(value)
        .filter(|seconds| *seconds > 0.0)
        .unwrap_or(default_seconds);

    (seconds * 1000.0).round() as u64
}

/// `String(valor)` de JS para los tipos que puede traer un id.
fn js_string(value: Option<&Value>) -> String {
    match value {
        None => "undefined".to_string(),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Number(number)) => match number.as_f64() {
            Some(float) if !number.is_i64() && !number.is_u64() && float.fract() == 0.0 => {
                format!("{float:.0}")
            }
            _ => number.to_string(),
        },
        Some(other) => other.to_string(),
    }
}

/// Añade la query a la URL, saltándose los valores vacíos.
pub(crate) fn append_query(url: &str, query: &[(&str, String)]) -> String {
    let pairs: Vec<String> = query
        .iter()
        .filter(|(_, value)| !value.is_empty())
        .map(|(key, value)| format!("{}={}", form_urlencode(key), form_urlencode(value)))
        .collect();

    if pairs.is_empty() {
        url.to_string()
    } else {
        format!("{url}?{}", pairs.join("&"))
    }
}

/// Codificación `application/x-www-form-urlencoded`, la de `URLSearchParams`.
fn form_urlencode(input: &str) -> String {
    let mut encoded = String::with_capacity(input.len());

    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'*' | b'-' | b'.' | b'_' => {
                encoded.push(byte as char)
            }
            b' ' => encoded.push('+'),
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }

    encoded
}

/// `new Date(ms).toISOString()`: `AAAA-MM-DDTHH:MM:SS.mmmZ` en UTC.
fn to_iso_string(epoch_ms: f64) -> Option<String> {
    if !epoch_ms.is_finite() {
        return None;
    }

    let total_ms = epoch_ms.floor() as i64;
    let days = total_ms.div_euclid(86_400_000);
    let ms_of_day = total_ms.rem_euclid(86_400_000);

    // Días desde la época a fecha civil (algoritmo de Howard Hinnant).
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);

    if !(0..=9999).contains(&year) {
        return None;
    }

    Some(format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        ms_of_day / 3_600_000,
        ms_of_day / 60_000 % 60,
        ms_of_day / 1_000 % 60,
        ms_of_day % 1_000
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_timestamps_like_to_iso_string() {
        assert_eq!(
            to_iso_string(0.0).as_deref(),
            Some("1970-01-01T00:00:00.000Z")
        );
        assert_eq!(
            to_iso_string(1_767_225_600_000.0).as_deref(),
            Some("2026-01-01T00:00:00.000Z")
        );
        assert_eq!(
            to_iso_string(1_709_251_199_999.0).as_deref(),
            Some("2024-02-29T23:59:59.999Z")
        );
        assert_eq!(
            to_iso_string(-1.0).as_deref(),
            Some("1969-12-31T23:59:59.999Z")
        );
        assert_eq!(to_iso_string(f64::NAN), None);
    }

    #[test]
    fn encodes_query_values_like_url_search_params() {
        assert_eq!(
            append_query(
                "https://api.github.com/user/repos",
                &[
                    ("page", "1".to_string()),
                    ("empty", String::new()),
                    ("affiliation", "owner,collaborator".to_string()),
                    ("q", "a b&c".to_string()),
                ]
            ),
            "https://api.github.com/user/repos?page=1&affiliation=owner%2Ccollaborator&q=a+b%26c"
        );
        assert_eq!(append_query("https://x/y", &[]), "https://x/y");
    }

    #[test]
    fn converts_json_values_with_js_semantics() {
        assert_eq!(js_string(Some(&json!(7))), "7");
        assert_eq!(js_string(Some(&json!(7.0))), "7");
        assert_eq!(js_string(Some(&json!("abc"))), "abc");
        assert_eq!(js_string(Some(&Value::Null)), "null");
        assert_eq!(js_string(None), "undefined");

        assert_eq!(truthy_text(Some(&json!(""))), None);
        assert_eq!(truthy_text(Some(&Value::Null)), None);
        assert_eq!(truthy_text(Some(&json!("x"))).as_deref(), Some("x"));
        assert_eq!(text(Some(&json!(""))).as_deref(), Some(""));

        assert_eq!(interval_ms(Some(&json!(12)), 5.0), 12_000);
        assert_eq!(interval_ms(Some(&json!(0)), 5.0), 5_000);
        assert_eq!(interval_ms(None, 10.0), 10_000);
        assert_eq!(uint(Some(&json!(42))), Some(42));
        assert_eq!(uint(Some(&json!(-1))), None);
        assert_eq!(uint(Some(&json!("42"))), None);
    }

    #[test]
    fn rejects_payloads_that_are_not_json_objects() {
        for body in ["", "<html>", "null", "\"text\"", "42"] {
            let error = parse_payload(body.as_bytes()).unwrap_err();
            assert_eq!(error.code, GitHubErrorCode::InvalidResponse);
            assert_eq!(error.status, 502);
            assert_eq!(error.message, "Invalid response from GitHub");
        }

        assert!(parse_payload(b"{}").is_ok());
        assert!(parse_payload(b"[]").is_ok());
    }
}
