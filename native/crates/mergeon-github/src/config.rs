use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use crate::http::DEFAULT_TIMEOUT;

pub const GITHUB_BASE_URL: &str = "https://github.com";
pub const GITHUB_API_URL: &str = "https://api.github.com";

/// App de GitHub por defecto. El client_id es público: el device flow no usa
/// `client_secret`, así que no queda ningún secreto dentro del binario.
pub const DEFAULT_CLIENT_ID: &str = "Ov23liHW92VyuIBmLnUQ";
pub const DEFAULT_APP_SLUG: &str = "app-git-service";

pub const ENV_CLIENT_ID: &str = "MERGEON_GITHUB_CLIENT_ID";
pub const ENV_APP_MODE: &str = "MERGEON_GITHUB_APP_MODE";
pub const ENV_APP_SLUG: &str = "MERGEON_GITHUB_APP_SLUG";

// Valores embebidos al compilar: permiten que un binario empaquetado lleve su
// propio client id sin depender del entorno de quien lo ejecuta.
const EMBEDDED: Embedded = Embedded {
    client_id: option_env!("MERGEON_GITHUB_CLIENT_ID"),
    app_mode: option_env!("MERGEON_GITHUB_APP_MODE"),
    app_slug: option_env!("MERGEON_GITHUB_APP_SLUG"),
};

#[derive(Debug, Clone, Copy, Default)]
struct Embedded {
    client_id: Option<&'static str>,
    app_mode: Option<&'static str>,
    app_slug: Option<&'static str>,
}

/// Una GitHub App solo ve los repos de sus instalaciones; una OAuth App ve todo
/// lo que el usuario puede ver, incluidos los repos donde solo es colaborador.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AppMode {
    OAuth,
    GitHubApp,
}

impl AppMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OAuth => "oauth",
            Self::GitHubApp => "github-app",
        }
    }

    /// Solo `github-app` activa el modo GitHub App; cualquier otro valor se
    /// comporta como OAuth App, igual que `appMode === 'github-app'`.
    pub fn parse(value: &str) -> Self {
        if value == "github-app" {
            Self::GitHubApp
        } else {
            Self::OAuth
        }
    }

    /// GitHub emite client_id con prefijo `Iv` para App y `Ov` para OAuth App.
    pub fn from_client_id(client_id: &str) -> Self {
        if client_id.starts_with("Iv") {
            Self::GitHubApp
        } else {
            Self::OAuth
        }
    }
}

/// Configuración del cliente de GitHub.
///
/// Orden de precedencia de cada valor (`client_id`, `app_mode`, `app_slug`),
/// de mayor a menor; un valor vacío cuenta como ausente:
///
/// 1. Variable de entorno del proceso en tiempo de ejecución
///    (`MERGEON_GITHUB_CLIENT_ID`, `MERGEON_GITHUB_APP_MODE`,
///    `MERGEON_GITHUB_APP_SLUG`).
/// 2. Archivo `.env`, solo con [`GitHubConfig::from_env_with_dotenv`]. Como
///    dotenv, no pisa una variable ya definida en el proceso.
/// 3. Valor embebido al compilar: las mismas variables leídas con
///    `option_env!` cuando se construyó el binario.
/// 4. Valor por defecto: [`DEFAULT_CLIENT_ID`], [`DEFAULT_APP_SLUG`] y, para
///    el modo, el que se deduce del prefijo del client id resuelto.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubConfig {
    pub client_id: String,
    pub app_slug: String,
    pub app_mode: AppMode,
    /// Base del sitio (device flow y enlace de instalación).
    pub base_url: String,
    /// Base de la API REST.
    pub api_url: String,
    /// Tiempo máximo por petición.
    pub timeout: Duration,
}

impl Default for GitHubConfig {
    /// Valores por defecto, sin leer el entorno ni los valores embebidos.
    fn default() -> Self {
        Self::new(DEFAULT_CLIENT_ID)
    }
}

impl GitHubConfig {
    /// Configuración explícita: el modo se deduce del prefijo del client id.
    pub fn new(client_id: impl Into<String>) -> Self {
        let client_id = client_id.into();
        let app_mode = AppMode::from_client_id(&client_id);

        Self {
            client_id,
            app_slug: DEFAULT_APP_SLUG.to_string(),
            app_mode,
            base_url: GITHUB_BASE_URL.to_string(),
            api_url: GITHUB_API_URL.to_string(),
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// Resuelve la configuración desde las variables de entorno del proceso,
    /// con los valores embebidos al compilar y los por defecto como respaldo.
    pub fn from_env() -> Self {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// Como [`GitHubConfig::from_env`], pero lee además un archivo `.env`
    /// (la app Electron carga `dotenv/config` al arrancar). Si el archivo no
    /// existe o no se puede leer, se ignora.
    pub fn from_env_with_dotenv(dotenv_path: &Path) -> Self {
        let content = std::fs::read_to_string(dotenv_path).unwrap_or_default();

        Self::from_lookup(with_dotenv(|name| std::env::var(name).ok(), &content))
    }

    /// Resuelve la configuración con una fuente de variables arbitraria, que
    /// ocupa el lugar del entorno del proceso en el orden de precedencia.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Self {
        Self::resolve(lookup, EMBEDDED)
    }

    fn resolve(lookup: impl Fn(&str) -> Option<String>, embedded: Embedded) -> Self {
        let pick = |name: &str, embedded_value: Option<&'static str>| {
            lookup(name).filter(|value| !value.is_empty()).or_else(|| {
                embedded_value
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
            })
        };

        let client_id = pick(ENV_CLIENT_ID, embedded.client_id)
            .unwrap_or_else(|| DEFAULT_CLIENT_ID.to_string());
        let app_slug =
            pick(ENV_APP_SLUG, embedded.app_slug).unwrap_or_else(|| DEFAULT_APP_SLUG.to_string());
        let app_mode = pick(ENV_APP_MODE, embedded.app_mode)
            .map(|mode| AppMode::parse(&mode))
            .unwrap_or_else(|| AppMode::from_client_id(&client_id));

        Self {
            client_id,
            app_slug,
            app_mode,
            base_url: GITHUB_BASE_URL.to_string(),
            api_url: GITHUB_API_URL.to_string(),
            timeout: DEFAULT_TIMEOUT,
        }
    }

    pub fn with_app_mode(mut self, app_mode: AppMode) -> Self {
        self.app_mode = app_mode;
        self
    }

    pub fn with_app_slug(mut self, app_slug: impl Into<String>) -> Self {
        self.app_slug = app_slug.into();
        self
    }

    /// Cambia la base del sitio (`https://github.com`); pensado para tests.
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = trim_trailing_slashes(base_url.into());
        self
    }

    /// Cambia la base de la API (`https://api.github.com`); pensado para tests.
    pub fn with_api_url(mut self, api_url: impl Into<String>) -> Self {
        self.api_url = trim_trailing_slashes(api_url.into());
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn is_github_app(&self) -> bool {
        self.app_mode == AppMode::GitHubApp
    }

    /// Página donde el usuario instala la GitHub App en una cuenta.
    pub fn install_url(&self) -> String {
        format!("{}/apps/{}/installations/new", self.base_url, self.app_slug)
    }
}

/// Pone el contenido de un `.env` por debajo de `process`: como dotenv, no
/// pisa una variable que ya está definida, aunque esté vacía.
fn with_dotenv(
    process: impl Fn(&str) -> Option<String>,
    dotenv_content: &str,
) -> impl Fn(&str) -> Option<String> {
    let file_values: HashMap<String, String> = parse_dotenv(dotenv_content).into_iter().collect();

    move |name| process(name).or_else(|| file_values.get(name).cloned())
}

fn trim_trailing_slashes(mut url: String) -> String {
    while url.ends_with('/') {
        url.pop();
    }
    url
}

/// Parser mínimo de `.env`: líneas `CLAVE=valor` con `export` opcional,
/// comentarios con `#` y valores entre comillas. No soporta valores multilínea.
pub fn parse_dotenv(content: &str) -> Vec<(String, String)> {
    let mut entries = Vec::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let line = line
            .strip_prefix("export")
            .filter(|rest| rest.starts_with(char::is_whitespace))
            .map(str::trim_start)
            .unwrap_or(line);

        let Some((key, raw_value)) = line.split_once('=') else {
            continue;
        };

        let key = key.trim();
        if key.is_empty()
            || !key
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
        {
            continue;
        }

        entries.push((key.to_string(), parse_dotenv_value(raw_value.trim())));
    }

    entries
}

fn parse_dotenv_value(raw: &str) -> String {
    if let Some(quote) = raw.chars().next().filter(|c| matches!(c, '"' | '\'' | '`'))
        && let Some(end) = raw[1..].find(quote)
    {
        let inner = &raw[1..=end];
        // Como dotenv, solo las comillas dobles expanden los saltos de línea.
        return if quote == '"' {
            inner.replace("\\n", "\n").replace("\\r", "\r")
        } else {
            inner.to_string()
        };
    }

    // Sin comillas, `#` abre un comentario hasta el final de la línea.
    raw.split('#').next().unwrap_or_default().trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup_from(
        values: &'static [(&'static str, &'static str)],
    ) -> impl Fn(&str) -> Option<String> {
        move |name| {
            values
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.to_string())
        }
    }

    #[test]
    fn falls_back_to_the_default_oauth_app() {
        let config = GitHubConfig::resolve(|_| None, Embedded::default());

        assert_eq!(config.client_id, DEFAULT_CLIENT_ID);
        assert_eq!(config.app_slug, DEFAULT_APP_SLUG);
        assert_eq!(config.app_mode, AppMode::OAuth);
        assert_eq!(config.base_url, "https://github.com");
        assert_eq!(config.api_url, "https://api.github.com");
        assert_eq!(config.timeout, Duration::from_secs(15));
        assert_eq!(config, GitHubConfig::default());
    }

    #[test]
    fn deduces_the_mode_from_the_client_id_prefix() {
        let config = GitHubConfig::resolve(
            lookup_from(&[(ENV_CLIENT_ID, "Iv1.abcdef")]),
            Embedded::default(),
        );

        assert_eq!(config.client_id, "Iv1.abcdef");
        assert!(config.is_github_app());
        assert_eq!(
            config.install_url(),
            "https://github.com/apps/app-git-service/installations/new"
        );
    }

    #[test]
    fn lets_the_mode_variable_override_the_deduced_mode() {
        let forced_app = GitHubConfig::resolve(
            lookup_from(&[(ENV_APP_MODE, "github-app"), (ENV_APP_SLUG, "my-app")]),
            Embedded::default(),
        );
        assert_eq!(forced_app.client_id, DEFAULT_CLIENT_ID);
        assert_eq!(forced_app.app_mode, AppMode::GitHubApp);
        assert_eq!(
            forced_app.install_url(),
            "https://github.com/apps/my-app/installations/new"
        );

        let forced_oauth = GitHubConfig::resolve(
            lookup_from(&[(ENV_CLIENT_ID, "Iv1.abcdef"), (ENV_APP_MODE, "oauth")]),
            Embedded::default(),
        );
        assert_eq!(forced_oauth.app_mode, AppMode::OAuth);

        // Un valor desconocido no es `github-app`: se comporta como OAuth App.
        let unknown = GitHubConfig::resolve(
            lookup_from(&[(ENV_CLIENT_ID, "Iv1.abcdef"), (ENV_APP_MODE, "whatever")]),
            Embedded::default(),
        );
        assert_eq!(unknown.app_mode, AppMode::OAuth);
    }

    #[test]
    fn prefers_runtime_variables_over_embedded_values_and_defaults() {
        let embedded = Embedded {
            client_id: Some("Iv1.embedded"),
            app_mode: None,
            app_slug: Some("embedded-app"),
        };

        let from_embedded = GitHubConfig::resolve(|_| None, embedded);
        assert_eq!(from_embedded.client_id, "Iv1.embedded");
        assert_eq!(from_embedded.app_slug, "embedded-app");
        assert_eq!(from_embedded.app_mode, AppMode::GitHubApp);

        let from_runtime =
            GitHubConfig::resolve(lookup_from(&[(ENV_CLIENT_ID, "Ov23runtime")]), embedded);
        assert_eq!(from_runtime.client_id, "Ov23runtime");
        assert_eq!(from_runtime.app_slug, "embedded-app");
        assert_eq!(from_runtime.app_mode, AppMode::OAuth);

        // Una variable vacía cuenta como ausente, igual que `process.env.X || ...`.
        let empty_runtime = GitHubConfig::resolve(lookup_from(&[(ENV_CLIENT_ID, "")]), embedded);
        assert_eq!(empty_runtime.client_id, "Iv1.embedded");

        let empty_embedded = GitHubConfig::resolve(
            |_| None,
            Embedded {
                client_id: Some(""),
                app_mode: Some(""),
                app_slug: Some(""),
            },
        );
        assert_eq!(empty_embedded, GitHubConfig::default());
    }

    #[test]
    fn normalizes_injected_base_urls() {
        let config = GitHubConfig::default()
            .with_base_url("http://127.0.0.1:9999/")
            .with_api_url("http://127.0.0.1:9999/api//");

        assert_eq!(config.base_url, "http://127.0.0.1:9999");
        assert_eq!(config.api_url, "http://127.0.0.1:9999/api");
    }

    #[test]
    fn layers_the_dotenv_file_below_the_process_environment() {
        let dotenv = "MERGEON_GITHUB_CLIENT_ID=Iv1.fromfile\nMERGEON_GITHUB_APP_SLUG=file-app\n";

        let only_file = GitHubConfig::resolve(with_dotenv(|_| None, dotenv), Embedded::default());
        assert_eq!(only_file.client_id, "Iv1.fromfile");
        assert_eq!(only_file.app_slug, "file-app");
        assert_eq!(only_file.app_mode, AppMode::GitHubApp);

        let process_wins = GitHubConfig::resolve(
            with_dotenv(lookup_from(&[(ENV_CLIENT_ID, "Ov23process")]), dotenv),
            Embedded::default(),
        );
        assert_eq!(process_wins.client_id, "Ov23process");
        assert_eq!(process_wins.app_slug, "file-app");

        // Definida pero vacía en el proceso: dotenv no la pisa y se cae al
        // valor embebido o al por defecto.
        let empty_process = GitHubConfig::resolve(
            with_dotenv(lookup_from(&[(ENV_CLIENT_ID, "")]), dotenv),
            Embedded::default(),
        );
        assert_eq!(empty_process.client_id, DEFAULT_CLIENT_ID);

        let missing_file =
            GitHubConfig::from_env_with_dotenv(Path::new("/nonexistent/mergeon-github/.env"));
        assert_eq!(missing_file, GitHubConfig::from_env());
    }

    #[test]
    fn parses_dotenv_files() {
        let content = "\
# Comentario
MERGEON_GITHUB_CLIENT_ID=Iv1.fromfile
export MERGEON_GITHUB_APP_SLUG = \"my app\"   # comentario
MERGEON_GITHUB_APP_MODE=oauth        # oauth | github-app
SINGLE='it is # not a comment'
MULTI=\"a\\nb\"
EMPTY=
not a variable
";

        assert_eq!(
            parse_dotenv(content),
            vec![
                (
                    "MERGEON_GITHUB_CLIENT_ID".to_string(),
                    "Iv1.fromfile".to_string()
                ),
                ("MERGEON_GITHUB_APP_SLUG".to_string(), "my app".to_string()),
                ("MERGEON_GITHUB_APP_MODE".to_string(), "oauth".to_string()),
                ("SINGLE".to_string(), "it is # not a comment".to_string()),
                ("MULTI".to_string(), "a\nb".to_string()),
                ("EMPTY".to_string(), String::new()),
            ]
        );
    }
}
