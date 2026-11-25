//! Modo de prueba para el e2e de `native/e2e/`: dobles de lo que no debe salir
//! de la máquina, equivalentes a los que instala la referencia de Electron
//! (`native/tools/reference-shots.mjs`). Solo se activa con variables de
//! entorno; sin ellas la app no cambia en nada.
//!
//! - `MERGEON_TEST_GITHUB_FIXTURE=<json>`: GitHub simulado. El JSON trae
//!   `repositories` (formato de la API de GitHub) y el token que los ve
//!   (`token`); el resto de tokens no ve ninguno, el device flow queda en
//!   `authorization_pending` y un token personal se rechaza con 401 (salvo
//!   que el fixture traiga `user`: entonces el token del fixture entra como
//!   token personal). Con `pullRequests` (por `owner/repo`), `checkRuns` y
//!   `statuses` (por sha) y `reviews` (por número) contesta además la API de
//!   pull requests. Los avatares no se descargan: se generan con el mismo
//!   SVG que la referencia.
//! - `MERGEON_TEST_SHELL_LOG=<archivo>`: abrir URL, carpeta, terminal o
//!   explorador no lanza nada; se anota una línea por orden en el archivo.
//! - `MERGEON_TEST_CONTROL=<archivo>`: la app lee las líneas JSON que se
//!   añadan al archivo: `{"cmd":"updater-event","payload":{...}}` (igual que
//!   un `updater:event`) y `{"cmd":"pick-directory","path":"..."|null}` (la
//!   respuesta del próximo selector de carpeta), `{"cmd":"close-overlays"}` y
//!   `{"cmd":"clear-toasts"}`.

use std::cell::RefCell;
use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui_kit::App;
use mergeon_app::AppModel;
use mergeon_backend::{BackendOptions, LaunchError, ProcessLauncher, Shell, ShellPlatform};
use mergeon_github::{
    GitHubClient, GitHubConfig, HttpRequest, HttpResponse, HttpTransport, TransportError,
};
use mergeon_updater::UpdaterState;
use serde_json::{Value, json};

/// Respuesta preparada para el próximo selector de carpeta.
pub type PickDirectoryAnswer = Rc<RefCell<Option<Option<PathBuf>>>>;

pub fn configure_backend(mut options: BackendOptions) -> BackendOptions {
    if let Some(path) = std::env::var_os("MERGEON_TEST_GITHUB_FIXTURE") {
        match std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        {
            Some(fixture) => {
                let transport = Arc::new(FixtureGitHub { fixture });
                options = options.github(GitHubClient::with_transport(
                    GitHubConfig::from_env(),
                    transport,
                ));
            }
            None => log::error!("MERGEON_TEST_GITHUB_FIXTURE ilegible"),
        }
    }
    if let Some(path) = std::env::var_os("MERGEON_TEST_SHELL_LOG") {
        options = options.shell(Shell::with_launcher(
            Arc::new(LoggingLauncher {
                path: PathBuf::from(path),
                lock: Mutex::new(()),
            }),
            ShellPlatform::current(),
        ));
    }
    options
}

/// Los dobles que viven en las vistas y no en el backend.
pub fn install_ui_doubles(cx: &mut App) {
    if std::env::var_os("MERGEON_TEST_GITHUB_FIXTURE").is_some() {
        mergeon_view_home::set_avatar_fetcher(Arc::new(|url| Ok(fixture_avatar(url))), cx);
    }
}

/// El avatar de `installDoubles` en la referencia: color según el número final
/// de la URL.
fn fixture_avatar(url: &str) -> Vec<u8> {
    let seed: u64 = url
        .rsplit('/')
        .next()
        .and_then(|tail| tail.split('?').next())
        .and_then(|id| id.parse().ok())
        .unwrap_or(0);
    let hue = (seed * 47) % 360;
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"96\" height=\"96\">\
         <rect width=\"96\" height=\"96\" fill=\"hsl({hue}, 55%, 48%)\"/>\
         <circle cx=\"48\" cy=\"38\" r=\"17\" fill=\"hsl({hue}, 60%, 86%)\"/>\
         <rect x=\"18\" y=\"62\" width=\"60\" height=\"40\" rx=\"20\" fill=\"hsl({hue}, 60%, 86%)\"/></svg>"
    )
    .into_bytes()
}

/// Empieza a leer el canal de control, si está configurado.
pub fn start_control(
    window: gpui_kit::AnyWindowHandle,
    pick_directory: PickDirectoryAnswer,
    cx: &mut App,
) {
    let Some(path) = std::env::var_os("MERGEON_TEST_CONTROL").map(PathBuf::from) else {
        return;
    };
    cx.spawn(async move |cx| {
        let mut consumed = 0usize;
        loop {
            cx.background_executor()
                .timer(Duration::from_millis(100))
                .await;
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            if text.len() <= consumed {
                continue;
            }
            let fresh = text[consumed..].to_string();
            let Some(complete) = fresh.rfind('\n') else {
                continue;
            };
            consumed += complete + 1;
            for line in fresh[..complete]
                .lines()
                .filter(|line| !line.trim().is_empty())
            {
                let Ok(command) = serde_json::from_str::<Value>(line) else {
                    log::error!("orden de control ilegible: {line}");
                    continue;
                };
                let answer = pick_directory.clone();
                cx.update(|cx| run_command(&command, window, &answer, cx));
            }
        }
    })
    .detach();
}

fn run_command(
    command: &Value,
    window: gpui_kit::AnyWindowHandle,
    pick_directory: &PickDirectoryAnswer,
    cx: &mut App,
) {
    match command.get("cmd").and_then(Value::as_str) {
        Some("updater-event") => {
            let payload = command.get("payload").cloned().unwrap_or(Value::Null);
            match serde_json::from_value::<UpdaterState>(payload) {
                Ok(state) => AppModel::global(cx)
                    .update(cx, |model, cx| model.handle_updater_event(&state, cx)),
                Err(error) => log::error!("updater-event inválido: {error}"),
            }
        }
        Some("pick-directory") => {
            let path = command
                .get("path")
                .and_then(Value::as_str)
                .map(PathBuf::from);
            *pick_directory.borrow_mut() = Some(path);
        }
        // `closeOverlays` de la referencia: diálogos de arriba abajo. Los menús se
        // cierran con Escape desde el arnés.
        Some("close-overlays") => {
            let _ = window.update(cx, |_, window, cx| {
                mergeon_ui::dialog::close_all(window, cx)
            });
        }
        Some("clear-toasts") => {
            let _ = window.update(cx, |_, window, cx| {
                mergeon_ui::toast::dismiss_all(window, cx)
            });
        }
        other => log::error!("orden de control desconocida: {other:?}"),
    }
}

struct LoggingLauncher {
    path: PathBuf,
    lock: Mutex<()>,
}

impl LoggingLauncher {
    fn record(&self, kind: &str, command: &str, args: &[String], cwd: Option<&Path>) {
        let _guard = self.lock.lock();
        let line = json!({
            "kind": kind,
            "command": command,
            "args": args,
            "cwd": cwd.map(|cwd| cwd.display().to_string()),
        });
        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            let _ = writeln!(file, "{line}");
        }
    }
}

impl ProcessLauncher for LoggingLauncher {
    fn spawn_detached(
        &self,
        command: &str,
        args: &[String],
        cwd: Option<&Path>,
    ) -> Result<(), LaunchError> {
        self.record("spawn", command, args, cwd);
        Ok(())
    }

    fn run(
        &self,
        command: &str,
        args: &[String],
        cwd: Option<&Path>,
    ) -> Result<Option<i32>, LaunchError> {
        self.record("run", command, args, cwd);
        Ok(Some(0))
    }
}

struct FixtureGitHub {
    fixture: Value,
}

fn json_response(status: u16, body: Value) -> HttpResponse {
    HttpResponse {
        status,
        headers: vec![("content-type".to_string(), "application/json".to_string())],
        body: body.to_string().into_bytes(),
    }
}

impl FixtureGitHub {
    /// Pull requests, checks y revisiones del fixture (`pullRequests`,
    /// `checkRuns`, `statuses` y `reviews`, por `owner/repo`, sha y número).
    fn repository_api(&self, path: &str, authorized: bool) -> HttpResponse {
        let parts: Vec<&str> = path.trim_start_matches("/repos/").split('/').collect();
        let not_found = || json_response(404, json!({ "message": "Not Found" }));
        let [owner, repo, rest @ ..] = parts.as_slice() else {
            return not_found();
        };
        let Some(pulls) = self
            .fixture
            .get("pullRequests")
            .and_then(|all| all.get(format!("{owner}/{repo}")))
            .filter(|_| authorized)
        else {
            return not_found();
        };
        let lookup =
            |key: &str, id: &str| self.fixture.get(key).and_then(|map| map.get(id)).cloned();
        match rest {
            ["pulls"] => json_response(200, pulls.clone()),
            ["pulls", number, "reviews"] => {
                json_response(200, lookup("reviews", number).unwrap_or(json!([])))
            }
            ["commits", sha, "check-runs"] => json_response(
                200,
                json!({ "check_runs": lookup("checkRuns", sha).unwrap_or(json!([])) }),
            ),
            ["commits", sha, "status"] => json_response(
                200,
                json!({ "statuses": lookup("statuses", sha).unwrap_or(json!([])) }),
            ),
            _ => not_found(),
        }
    }
}

impl HttpTransport for FixtureGitHub {
    fn send(&self, request: &HttpRequest) -> Result<HttpResponse, TransportError> {
        let token = request
            .header("Authorization")
            .and_then(|value| value.strip_prefix("Bearer "))
            .unwrap_or_default();
        let fixture_token = self
            .fixture
            .get("token")
            .and_then(Value::as_str)
            .unwrap_or("fixture-token");
        let path = request
            .url
            .split("://")
            .nth(1)
            .and_then(|rest| rest.find('/').map(|at| &rest[at..]))
            .unwrap_or("/");
        let path = path.split('?').next().unwrap_or(path);
        Ok(match path {
            "/login/device/code" => json_response(
                200,
                json!({
                    "device_code": "fixture-device-code",
                    "user_code": "WDJB-MJHT",
                    "verification_uri": "https://github.com/login/device",
                    "expires_in": 900,
                    "interval": 600
                }),
            ),
            "/login/oauth/access_token" => {
                json_response(200, json!({ "error": "authorization_pending" }))
            }
            "/user/repos" => {
                let repositories = if token == fixture_token {
                    self.fixture
                        .get("repositories")
                        .cloned()
                        .unwrap_or(json!([]))
                } else {
                    json!([])
                };
                json_response(200, repositories)
            }
            "/user/installations" => {
                json_response(200, json!({ "installations": [], "total_count": 0 }))
            }
            "/user" => match self.fixture.get("user") {
                Some(user) if token == fixture_token => json_response(200, user.clone()),
                _ => json_response(401, json!({ "message": "Bad credentials" })),
            },
            _ if path.starts_with("/repos/") => self.repository_api(path, token == fixture_token),
            _ => json_response(404, json!({ "message": "Not Found" })),
        })
    }
}
