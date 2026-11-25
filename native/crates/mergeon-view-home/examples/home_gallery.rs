//! Galería de `mergeon-view-home`: el inicio, Settings y sus diálogos sobre un
//! `AppModel` real (repositorios de demostración, configuración en memoria y
//! un GitHub de mentira), para capturarlos y compararlos con
//! `native/target-shots/reference/`.
//!
//! ```bash
//! cargo run -p mergeon-view-home --example home_gallery
//! MERGEON_THEME=light MERGEON_HOME_STATE=recents cargo run -p mergeon-view-home --example home_gallery
//! ```
//!
//! Variables de entorno:
//!
//! - `MERGEON_THEME=dark|light` (por defecto `dark`).
//! - `MERGEON_HOME_VIEW=home|settings` (por defecto `home`).
//! - `MERGEON_HOME_STATE=empty|recents|open`: sin nada; con recientes y tres
//!   cuentas de GitHub; como `recents` y con los repos de demostración
//!   abiertos (el activo es `demo`).
//! - `MERGEON_HOME_SELECT=<nombre>`, `MERGEON_HOME_TAB=statistics`,
//!   `MERGEON_HOME_COLLAPSE=<grupo>`: repositorio elegido, pestaña del resumen
//!   y grupo plegado del inicio.
//! - `MERGEON_HOME_ACCOUNT=ana-torres|acme-bot|luis-vega`: cuenta de GitHub
//!   activa (con repositorios, sin la GitHub App instalada, sin repositorios).
//! - `MERGEON_SETTINGS_SECTION=appearance|git|accounts|help`.
//! - `MERGEON_UPDATER=unsupported|available|downloading|ready|error|uptodate`.
//! - `MERGEON_HOME_OPEN=`, lista separada por comas de lo que se abre al
//!   arrancar: `launcher`, `account`, `picker`, `accounts`, `preferences`,
//!   `shortcuts`, `about`.
//! - `MERGEON_HOME_PICK=<ruta>`: lo que responde el selector de carpetas
//!   (por defecto la carpeta `projects` de la demostración; `none` cancela).
//! - `MERGEON_GALLERY_INPUT=`, pasos separados por `;` (uno cada 300ms):
//!   `id:<id>` (clic en el elemento con ese id), `dbl:<id>`, `click:x,y`,
//!   `move:x,y`, `type:<texto>`, `key:<tecla>`, `scroll:<id>,<dy>`,
//!   `wait:<ms>`.
//! - `MERGEON_HOME_CHROME=0`: sin el hueco del rail y de la barra de pestañas
//!   (que deja la vista donde la pone la cáscara, para comparar capturas).
//! - `MERGEON_HOME_WORK=<dir>`: dónde crear los repositorios de demostración
//!   (por defecto `~/.cache/mergeon-home-gallery`; se reutilizan entre
//!   ejecuciones).

use std::cell::RefCell;
use std::io::Read as _;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use futures::channel::oneshot;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::*;
use mergeon_app::model_types::dialogs::{
    ConfirmAnswer, ConfirmOptions, ConfirmValue, PromptAnswer, PromptOptions,
};
use mergeon_app::model_types::notify::{NotifyType, Toast, ToastActionKind, ToastId};
use mergeon_app::testing::{FakeGitHubTransport, TestBackend};
use mergeon_app::{AddRepository, Answer, AppEvent, AppModel, UiHost};
use mergeon_store::{GitHubAccount, RecentRepository};
use mergeon_ui::dialog;
use mergeon_ui::toast::{self, ToastKind, ToastOptions};
use mergeon_ui::{ActivePaletteExt as _, ThemePreference};
use mergeon_updater::{ErrorPhase, UpdateStatus, UpdaterState};
use mergeon_view_home::{HomeView, SettingsEvent, SettingsView};
use serde_json::{Value, json};

// ---------------------------------------------------------------------------
// Repositorios de demostración
// ---------------------------------------------------------------------------

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn path(&self, relative: &str) -> String {
        self.root.join(relative).to_string_lossy().into_owned()
    }

    fn demo(&self) -> String {
        self.path("projects/demo")
    }
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "Ana Torres")
        .env("GIT_AUTHOR_EMAIL", "ana@example.com")
        .env("GIT_COMMITTER_NAME", "Ana Torres")
        .env("GIT_COMMITTER_EMAIL", "ana@example.com")
        .env("GIT_AUTHOR_DATE", "2026-09-10T10:00:00+00:00")
        .env("GIT_COMMITTER_DATE", "2026-09-10T10:00:00+00:00")
        .status();
    if !status.is_ok_and(|status| status.success()) {
        eprintln!("git {args:?} falló en {}", dir.display());
    }
}

fn small_repository(dir: &Path, readme: Option<&str>) {
    std::fs::create_dir_all(dir.join("src")).expect("crear el repositorio");
    git(dir, &["init", "-q", "-b", "main"]);
    std::fs::write(dir.join("src/index.js"), "export default {}\n").expect("escribir archivo");
    if let Some(readme) = readme {
        std::fs::write(dir.join("README.md"), readme).expect("escribir README");
    }
    git(dir, &["add", "--all"]);
    git(dir, &["commit", "-q", "-m", "Initial commit"]);
}

/// Crea (una vez) los repositorios de las capturas de referencia: `demo` con
/// `native/tools/make-fixture.sh` y unos cuantos pequeños a su lado.
fn build_fixture() -> Fixture {
    let root = std::env::var("MERGEON_HOME_WORK")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
            PathBuf::from(home).join(".cache/mergeon-home-gallery")
        });
    let fixture = Fixture { root };

    // La identidad global que enseña Settings › Git.
    let gitconfig = fixture.root.join("gitconfig");
    // SAFETY: todavía no hay más hilos que este.
    unsafe {
        std::env::set_var("GIT_CONFIG_GLOBAL", &gitconfig);
        std::env::set_var("GIT_CONFIG_SYSTEM", "/dev/null");
        std::env::set_var("GIT_TERMINAL_PROMPT", "0");
    }

    if fixture.root.join("projects/demo/.git").exists()
        && std::env::var("MERGEON_HOME_REBUILD").is_err()
    {
        return fixture;
    }

    let _ = std::fs::remove_dir_all(&fixture.root);
    std::fs::create_dir_all(fixture.root.join("clients")).expect("crear la carpeta de trabajo");
    std::fs::write(
        &gitconfig,
        "[user]\n\tname = Ana Torres\n\temail = ana@example.com\n[init]\n\tdefaultBranch = main\n",
    )
    .expect("escribir gitconfig");

    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/make-fixture.sh");
    let made = Command::new("bash")
        .arg(script)
        .arg(fixture.root.join("projects"))
        .output();
    if !made.is_ok_and(|output| output.status.success()) {
        eprintln!("make-fixture.sh falló");
    }

    small_repository(
        &fixture.root.join("projects/website"),
        Some("# Website\n\nThe public landing page.\n\n- Static HTML\n- No build step\n"),
    );
    small_repository(
        &fixture.root.join("projects/api-server"),
        Some(
            "# API server\n\nHTTP API that backs the Acme portal.\n\n```sh\nnpm install\nnpm \
             start\n```\n\nSee [the guide](https://example.com/guide) for **details**.\n",
        ),
    );
    small_repository(&fixture.root.join("clients/acme-portal"), None);
    std::fs::create_dir_all(fixture.root.join("plain")).expect("crear carpeta sin git");
    fixture
}

// ---------------------------------------------------------------------------
// Configuración y GitHub de mentira
// ---------------------------------------------------------------------------

fn recent(path: String, name: &str) -> RecentRepository {
    RecentRepository {
        id: name.to_string(),
        name: name.to_string(),
        path,
        current_branch: "main".to_string(),
        ..Default::default()
    }
}

fn accounts() -> Vec<GitHubAccount> {
    let account = |value: Value| serde_json::from_value::<GitHubAccount>(value).expect("cuenta");
    vec![
        account(json!({
            "id": "ana-torres",
            "username": "ana-torres",
            "name": "Ana Torres",
            "email": "ana@example.com",
            "avatarUrl": "https://avatars.githubusercontent.com/u/1001",
            "token": "fixture-token",
            "installations": [],
            "requiresInstallation": false,
            "addedAt": "2026-09-01T10:00:00.000Z"
        })),
        account(json!({
            "id": "acme-bot",
            "username": "acme-bot",
            "name": "Acme Deploy Bot",
            "email": "",
            "avatarUrl": "",
            "token": "fixture-token-app",
            "installations": [],
            "installUrl": "https://github.com/apps/mergeon/installations/new",
            "requiresInstallation": true,
            "addedAt": "2026-09-10T10:00:00.000Z"
        })),
        account(json!({
            "id": "luis-vega",
            "username": "luis-vega",
            "name": "Luis Vega",
            "email": "luis.vega@example.com",
            "avatarUrl": "https://avatars.githubusercontent.com/u/1003",
            "token": "fixture-token-empty",
            "installations": [],
            "requiresInstallation": false,
            "addedAt": "2026-09-12T10:00:00.000Z"
        })),
    ]
}

/// Los repositorios que devuelve la cuenta `ana-torres`, con el formato de la
/// API de GitHub. `clone_base` apunta a un servidor local que nunca responde:
/// un clon se queda "Cloning..." sin tocar la red.
fn github_repositories(clone_base: &str) -> Value {
    let now = chrono_like_now_ms();
    // Dueño, nombre, descripción, lenguaje, estrellas, forks, privado y días
    // desde la última actualización.
    type Row = (
        &'static str,
        &'static str,
        &'static str,
        &'static str,
        u64,
        u64,
        bool,
        i64,
    );
    let rows: [Row; 8] = [
        (
            "ana-torres",
            "demo",
            "A small project used to exercise the Git client.",
            "JavaScript",
            12,
            3,
            false,
            1,
        ),
        (
            "ana-torres",
            "website",
            "The public landing page.",
            "HTML",
            4,
            0,
            false,
            2,
        ),
        (
            "ana-torres",
            "dotfiles",
            "Shell, editor and git configuration.",
            "Shell",
            31,
            6,
            false,
            6,
        ),
        (
            "acme",
            "api-server",
            "HTTP API that backs the Acme portal.",
            "TypeScript",
            87,
            14,
            true,
            0,
        ),
        (
            "acme",
            "acme-portal",
            "Customer portal.",
            "Python",
            9,
            2,
            true,
            3,
        ),
        (
            "acme",
            "design-tokens",
            "Colors, spacing and type shared by every Acme app.",
            "CSS",
            54,
            8,
            false,
            9,
        ),
        (
            "acme",
            "infra",
            "Terraform modules and deployment pipelines.",
            "HCL",
            21,
            5,
            true,
            12,
        ),
        (
            "luis-vega",
            "charts",
            "Tiny charting helpers.",
            "JavaScript",
            240,
            31,
            false,
            40,
        ),
    ];
    Value::Array(
        rows.iter()
            .enumerate()
            .map(|(index, (owner, name, description, language, stars, forks, private, days))| {
                json!({
                    "id": index + 1,
                    "name": name,
                    "full_name": format!("{owner}/{name}"),
                    "clone_url": format!("{clone_base}/{owner}/{name}.git"),
                    "description": description,
                    "language": language,
                    "stargazers_count": stars,
                    "forks_count": forks,
                    "private": private,
                    "updated_at": iso_from_ms(now - days * 86_400_000 - 3_600_000),
                    "owner": {
                        "login": owner,
                        "avatar_url": format!("https://avatars.githubusercontent.com/u/{}", 2000 + index)
                    }
                })
            })
            .collect(),
    )
}

fn chrono_like_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

fn iso_from_ms(epoch_ms: i64) -> String {
    mergeon_app::model_types::format::to_iso_string(epoch_ms)
}

/// Un servidor que acepta conexiones y nunca contesta.
fn silent_server() -> String {
    let Ok(listener) = TcpListener::bind("127.0.0.1:0") else {
        return "http://127.0.0.1:9".to_string();
    };
    let port = listener
        .local_addr()
        .map(|address| address.port())
        .unwrap_or(9);
    std::thread::spawn(move || {
        let mut held = Vec::new();
        for stream in listener.incoming().flatten() {
            let mut reader = stream.try_clone().ok();
            held.push(stream);
            if let Some(reader) = reader.as_mut() {
                let _ = reader.set_read_timeout(Some(Duration::from_millis(200)));
                let mut buffer = [0u8; 1024];
                let _ = reader.read(&mut buffer);
            }
        }
    });
    format!("http://127.0.0.1:{port}")
}

fn script_github(github: &FakeGitHubTransport, clone_base: String) {
    let repositories = github_repositories(&clone_base);
    github.respond_with(move |request| {
        let url = request.url.as_str();
        if url.contains("/user/repos") {
            let ours = request.header("authorization") == Some("Bearer fixture-token");
            let first_page = url.contains("page=1&") || url.ends_with("page=1");
            return FakeGitHubTransport::json(
                200,
                if ours && first_page {
                    repositories.clone()
                } else {
                    json!([])
                },
            );
        }
        if url.ends_with("/login/device/code") {
            return FakeGitHubTransport::json(
                200,
                json!({
                    "device_code": "fixture-device-code",
                    "user_code": "WDJB-MJHT",
                    "verification_uri": "https://github.com/login/device",
                    "expires_in": 900,
                    "interval": 600
                }),
            );
        }
        if url.ends_with("/login/oauth/access_token") {
            return FakeGitHubTransport::json(200, json!({ "error": "authorization_pending" }));
        }
        FakeGitHubTransport::json(401, json!({ "message": "Bad credentials" }))
    });
}

/// El avatar de mentira de las capturas de referencia: un color por cuenta.
fn avatar_svg(url: &str) -> Vec<u8> {
    let seed: f32 = url
        .rsplit('/')
        .next()
        .and_then(|seed| seed.parse().ok())
        .unwrap_or(0.);
    let hue = (seed * 47.) % 360.;
    let color = |saturation: f32, lightness: f32| {
        let rgb = Rgba::from(hsla(hue / 360., saturation, lightness, 1.));
        format!(
            "#{:02x}{:02x}{:02x}",
            (rgb.r * 255.) as u8,
            (rgb.g * 255.) as u8,
            (rgb.b * 255.) as u8
        )
    };
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"96\" height=\"96\">\
         <rect width=\"96\" height=\"96\" fill=\"{background}\"/>\
         <circle cx=\"48\" cy=\"38\" r=\"17\" fill=\"{figure}\"/>\
         <rect x=\"18\" y=\"62\" width=\"60\" height=\"40\" rx=\"20\" fill=\"{figure}\"/></svg>",
        background = color(0.55, 0.48),
        figure = color(0.60, 0.86),
    )
    .into_bytes()
}

// ---------------------------------------------------------------------------
// Interfaz global de la galería
// ---------------------------------------------------------------------------

/// El `UiHost` que pondrá la cáscara, en pequeño: avisos, confirmaciones y
/// prompts con `mergeon_ui`, y un selector de carpetas que responde lo que
/// diga `MERGEON_HOME_PICK`.
struct GalleryHost {
    window: AnyWindowHandle,
    directory: RefCell<Option<PathBuf>>,
}

fn text(value: &Option<String>) -> SharedString {
    value.clone().unwrap_or_default().into()
}

impl UiHost for GalleryHost {
    fn notify(&self, toast: Toast, cx: &mut App) {
        let window = self.window;
        cx.defer(move |cx| {
            let _ = window.update(cx, |_, window, cx| {
                let kind = match toast.kind {
                    NotifyType::Success => ToastKind::Success,
                    NotifyType::Info => ToastKind::Info,
                    NotifyType::Warning => ToastKind::Warning,
                    NotifyType::Error => ToastKind::Error,
                };
                let mut options = ToastOptions::new().duration(toast.duration());
                if let ToastId::Named(id) = &toast.id {
                    options = options.id(id.clone());
                }
                if toast.full_message != toast.title {
                    options = options.details(toast.full_message.clone());
                }
                if let Some(action) = &toast.action
                    && let ToastActionKind::Custom(key) = &action.kind
                {
                    let key = key.clone();
                    options = options.action(action.label.clone(), move |_, cx| {
                        AppModel::global(cx).update(cx, |model, cx| {
                            model.run_toast_action(&key, cx);
                        });
                    });
                }
                toast::show(window, cx, kind, &toast.title, options);
            });
        });
    }

    fn confirm(&self, options: ConfirmOptions, cx: &mut App) -> Answer<ConfirmValue> {
        let (sender, receiver) = oneshot::channel();
        let window = self.window;
        cx.defer(move |cx| {
            let _ = window.update(cx, |_, window, cx| {
                dialog::confirm(
                    window,
                    cx,
                    dialog::ConfirmOptions {
                        title: text(&options.title),
                        description: text(&options.description),
                        confirm_text: text(&options.confirm_text),
                        cancel_text: text(&options.cancel_text),
                        dismiss_value: options.dismiss_value.flatten().unwrap_or(false),
                        variant: if options.variant.as_deref() == Some("destructive") {
                            dialog::ConfirmVariant::Destructive
                        } else {
                            dialog::ConfirmVariant::Default
                        },
                    },
                    move |confirmed, _, _| {
                        let _ = sender.send(Some(confirmed));
                    },
                );
            });
        });
        receiver
    }

    fn confirm_with_option(&self, options: ConfirmOptions, cx: &mut App) -> Answer<ConfirmAnswer> {
        let (sender, receiver) = oneshot::channel();
        let window = self.window;
        cx.defer(move |cx| {
            let _ = window.update(cx, |_, window, cx| {
                dialog::confirm_with_option(
                    window,
                    cx,
                    dialog::ConfirmOptions {
                        title: text(&options.title),
                        description: text(&options.description),
                        confirm_text: text(&options.confirm_text),
                        cancel_text: text(&options.cancel_text),
                        ..Default::default()
                    },
                    dialog::ConfirmOption {
                        label: text(&options.option_label),
                        checked: options.option_checked,
                    },
                    move |outcome, _, _| {
                        let _ = sender.send(ConfirmAnswer::WithOption {
                            confirmed: outcome.confirmed,
                            checked: outcome.checked,
                        });
                    },
                );
            });
        });
        receiver
    }

    fn prompt(&self, options: PromptOptions, cx: &mut App) -> Answer<PromptAnswer> {
        let (sender, receiver) = oneshot::channel();
        let window = self.window;
        cx.defer(move |cx| {
            let _ = window.update(cx, |_, window, cx| {
                dialog::prompt(
                    window,
                    cx,
                    dialog::PromptOptions {
                        title: text(&options.title),
                        description: text(&options.description),
                        confirm_text: text(&options.confirm_text),
                        cancel_text: text(&options.cancel_text),
                        placeholder: text(&options.placeholder),
                        default_value: text(&options.default_value),
                        ..Default::default()
                    },
                    move |value, _, _| {
                        let _ = sender.send(PromptAnswer {
                            confirmed: value.is_some(),
                            value: value.map(|value| value.to_string()),
                        });
                    },
                );
            });
        });
        receiver
    }

    fn pick_directory(&self, _cx: &mut App) -> Answer<Option<PathBuf>> {
        let (sender, receiver) = oneshot::channel();
        let _ = sender.send(self.directory.borrow().clone());
        receiver
    }
}

// ---------------------------------------------------------------------------
// Ventana
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Home,
    Settings,
}

struct Gallery {
    page: Page,
    chrome: bool,
    home: Entity<HomeView>,
    settings: Entity<SettingsView>,
    _subscriptions: Vec<Subscription>,
}

impl Gallery {
    fn new(page: Page, chrome: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let model = AppModel::global(cx);
        let home = cx.new(|cx| HomeView::new(window, cx));
        let settings = cx.new(|cx| SettingsView::new(window, cx));
        let subscriptions = vec![
            cx.subscribe(&settings, |this, _, event: &SettingsEvent, cx| {
                if *event == SettingsEvent::Close {
                    this.page = Page::Home;
                    cx.notify();
                }
            }),
            cx.subscribe_in(
                &model,
                window,
                |this, _, event: &AppEvent, window, cx| match event {
                    AppEvent::OpenSettings { .. } => {
                        this.page = Page::Settings;
                        cx.notify();
                    }
                    AppEvent::ShowDialog(request) => {
                        mergeon_view_home::handle_dialog_request(request, window, cx);
                    }
                    AppEvent::ThemeChanged { dark } => {
                        mergeon_ui::theme::apply(
                            if *dark {
                                ThemePreference::Dark
                            } else {
                                ThemePreference::Light
                            },
                            cx,
                        );
                        window.refresh();
                    }
                    _ => {}
                },
            ),
        ];
        Self {
            page,
            chrome,
            home,
            settings,
            _subscriptions: subscriptions,
        }
    }
}

impl Render for Gallery {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let view: AnyElement = match self.page {
            Page::Home => self.home.clone().into_any_element(),
            Page::Settings => self.settings.clone().into_any_element(),
        };

        div()
            .flex()
            .size_full()
            .bg(palette.background)
            .text_color(palette.foreground)
            // El hueco del activity rail (48px) y de la barra de pestañas
            // (36px) de la cáscara.
            .when(self.chrome, |this| {
                this.child(
                    div()
                        .w(px(48.))
                        .flex_shrink_0()
                        .border_r_1()
                        .border_color(palette.border),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .when(self.chrome, |this| {
                        this.child(
                            div()
                                .h(px(36.))
                                .flex_shrink_0()
                                .border_b_1()
                                .border_color(palette.border),
                        )
                    })
                    .child(div().flex_1().min_h_0().child(view)),
            )
    }
}

fn updater_state(name: &str) -> Option<UpdaterState> {
    let base = UpdaterState {
        current_version: Some("1.4.0".to_string()),
        is_packaged: true,
        updates_configured: true,
        ..UpdaterState::default()
    };
    let latest = Some("1.5.0".to_string());
    Some(match name {
        "unsupported" => UpdaterState {
            status: UpdateStatus::Unsupported,
            is_packaged: false,
            ..base
        },
        "available" => UpdaterState {
            status: UpdateStatus::Available,
            latest_version: latest,
            ..base
        },
        "downloading" => UpdaterState {
            status: UpdateStatus::Downloading,
            latest_version: latest,
            percent: 42,
            ..base
        },
        "ready" => UpdaterState {
            status: UpdateStatus::Downloaded,
            latest_version: latest,
            can_auto_install: true,
            ..base
        },
        "error" => UpdaterState {
            status: UpdateStatus::Error,
            error: Some("network unreachable".to_string()),
            error_phase: Some(ErrorPhase::Check),
            ..base
        },
        "uptodate" => UpdaterState {
            status: UpdateStatus::NotAvailable,
            latest_version: Some("1.4.0".to_string()),
            ..base
        },
        _ => return None,
    })
}

/// Ejecuta un paso de `MERGEON_GALLERY_INPUT` como si viniera del usuario.
fn run_step(step: &str, window: &mut Window, cx: &mut App) {
    let Some((kind, value)) = step.split_once(':') else {
        eprintln!("MERGEON_GALLERY_INPUT: paso inválido `{step}`");
        return;
    };
    match kind {
        "id" | "dbl" => {
            let id = SharedString::from(value.to_string());
            if window.try_find(id.clone()).is_none() {
                eprintln!("MERGEON_GALLERY_INPUT: no hay elemento `{value}`");
                return;
            }
            if kind == "id" {
                window.click(id, cx);
            } else {
                window.double_click(id, cx);
            }
        }
        "type" => window.input(value, cx),
        "key" => window.press(value, cx),
        "scroll" => {
            let Some((id, delta)) = value.rsplit_once(',') else {
                return;
            };
            let delta = delta.trim().parse::<f32>().unwrap_or(0.);
            window.scroll(
                SharedString::from(id.to_string()),
                ScrollDelta::Pixels(point(px(0.), px(delta))),
                cx,
            );
        }
        "click" | "move" => {
            let mut numbers = value
                .split(',')
                .filter_map(|n| n.trim().parse::<f32>().ok());
            let (Some(x), Some(y)) = (numbers.next(), numbers.next()) else {
                eprintln!("MERGEON_GALLERY_INPUT: coordenadas inválidas `{value}`");
                return;
            };
            let position = point(px(x), px(y));
            window.dispatch_event(
                MouseMoveEvent {
                    position,
                    pressed_button: None,
                    modifiers: Modifiers::default(),
                }
                .to_platform_input(),
                cx,
            );
            if kind == "click" {
                window.dispatch_event(
                    MouseDownEvent {
                        button: MouseButton::Left,
                        position,
                        modifiers: Modifiers::default(),
                        click_count: 1,
                        first_mouse: false,
                    }
                    .to_platform_input(),
                    cx,
                );
                window.dispatch_event(
                    MouseUpEvent {
                        button: MouseButton::Left,
                        position,
                        modifiers: Modifiers::default(),
                        click_count: 1,
                    }
                    .to_platform_input(),
                    cx,
                );
            }
        }
        other => eprintln!("MERGEON_GALLERY_INPUT: no sé hacer `{other}`"),
    }
}

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_default()
}

fn list(name: &str, separator: char) -> Vec<String> {
    env(name)
        .split(separator)
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
        .collect()
}

fn main() {
    if std::env::var("MERGEON_LOG").is_ok() {
        env_logger::Builder::new().parse_filters("warn").init();
    }
    let fixture = build_fixture();
    // Las fechas de las capturas de referencia están en UTC.
    if std::env::var("TZ").is_err() {
        // SAFETY: todavía no hay más hilos que este.
        unsafe { std::env::set_var("TZ", "UTC") };
    }

    let theme = if env("MERGEON_THEME") == "light" {
        "light"
    } else {
        "dark"
    };
    let state = env("MERGEON_HOME_STATE");
    let page = if env("MERGEON_HOME_VIEW") == "settings" {
        Page::Settings
    } else {
        Page::Home
    };
    let chrome = env("MERGEON_HOME_CHROME") != "0";
    let section = env("MERGEON_SETTINGS_SECTION");
    let open = list("MERGEON_HOME_OPEN", ',');
    let steps = list("MERGEON_GALLERY_INPUT", ';');
    let directory = match env("MERGEON_HOME_PICK").as_str() {
        "none" => None,
        "" => Some(PathBuf::from(fixture.path("projects"))),
        path => Some(PathBuf::from(path)),
    };

    // Configuración en memoria, sembrada como el `config.json` de las
    // capturas de referencia.
    let store = TestBackend::memory_store();
    let _ = store.update_settings_json(&json!({ "theme": theme, "autoFetch": false }));
    if state == "recents" || state == "open" {
        let recents = [
            recent(fixture.path("clients/old-prototype"), "old-prototype"),
            recent(fixture.path("clients/acme-portal"), "acme-portal"),
            recent(fixture.path("projects/api-server"), "api-server"),
            recent(fixture.path("projects/website"), "website"),
            recent(fixture.demo(), "demo"),
        ];
        for recent in &recents {
            let _ = store.add_recent_repository(recent);
        }
        for account in accounts() {
            let _ = store.save_github_account(&account);
        }
        let active = env("MERGEON_HOME_ACCOUNT");
        let _ = store.set_active_github_account_id(Some(if active.is_empty() {
            "ana-torres"
        } else {
            &active
        }));
    }

    let mut test_backend = TestBackend::new().with_store(store);
    script_github(&test_backend.github, silent_server());

    gpui_kit::application()
        .with_assets(mergeon_ui::Assets)
        .run(move |cx| {
            mergeon_ui::init(cx);
            mergeon_ui::theme::apply(ThemePreference::parse(theme), cx);
            mergeon_view_home::init(cx);
            mergeon_view_home::set_avatar_fetcher(Arc::new(|url| Ok(avatar_svg(url))), cx);

            let (backend, events) = test_backend.build();
            let model = AppModel::init(backend, events, cx);
            if !section.is_empty() {
                model.update(cx, |model, cx| model.open_settings(&section, cx));
            }

            let bounds = Bounds::centered(None, size(px(1440.), px(900.)), cx);
            let (window, gallery) = gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                cx,
                |window, cx| cx.new(|cx| Gallery::new(page, chrome, window, cx)),
            )
            .expect("no se pudo abrir la ventana");

            model.update(cx, |model, cx| {
                model.set_ui_host(
                    Rc::new(GalleryHost {
                        window,
                        directory: RefCell::new(directory),
                    }),
                    cx,
                );
            });

            let demo = fixture.demo();
            let fixture_root = fixture.root.clone();
            cx.spawn(async move |cx| {
                // El `onMounted` de `App.vue`.
                model.update(cx, |model, cx| model.start(cx)).await;

                if state == "open" {
                    for path in [
                        fixture_root.join("projects/website"),
                        fixture_root.join("projects/api-server"),
                        PathBuf::from(&demo),
                    ] {
                        let path = path.to_string_lossy().into_owned();
                        let _ = model
                            .update(cx, |model, cx| {
                                model.add_repository(&path, AddRepository::default(), cx)
                            })
                            .await;
                    }
                    if page == Page::Home {
                        model.update(cx, |model, cx| model.show_home_tab(cx));
                    }
                }
                if let Some(updater) = updater_state(&env("MERGEON_UPDATER")) {
                    model.update(cx, |model, cx| model.apply_updater_state(&updater, cx));
                }

                // El inicio ya eligió repositorio: ahora lo que pida el entorno.
                cx.background_executor()
                    .timer(Duration::from_millis(400))
                    .await;
                let home = gallery.read_with(cx, |gallery, _| gallery.home.clone());
                home.update(cx, |home, cx| {
                    let select = env("MERGEON_HOME_SELECT");
                    if !select.is_empty() {
                        let path = mergeon_view_home::recent_entries(AppModel::global(cx).read(cx))
                            .into_iter()
                            .find(|entry| entry.name == select)
                            .map(|entry| entry.path);
                        match path {
                            Some(path) => home.select_repository(&path, cx),
                            None => eprintln!("MERGEON_HOME_SELECT: no hay repositorio `{select}`"),
                        }
                    }
                    let tab = env("MERGEON_HOME_TAB");
                    if !tab.is_empty() {
                        home.set_summary_tab(tab, cx);
                    }
                    let collapse = env("MERGEON_HOME_COLLAPSE");
                    if !collapse.is_empty() {
                        home.toggle_repository_group(&collapse, cx);
                    }
                });

                let _ = window.update(cx, |_, window, cx| {
                    for what in &open {
                        match what.as_str() {
                            "launcher" => {
                                mergeon_view_home::open_repository_launcher(window, cx);
                            }
                            "account" => {
                                mergeon_view_home::open_github_account_dialog(window, cx);
                            }
                            "picker" => {
                                mergeon_view_home::open_github_repository_picker(window, cx);
                            }
                            "accounts" => {
                                mergeon_view_home::open_github_accounts_dialog(window, cx);
                            }
                            "preferences" => {
                                mergeon_view_home::open_preferences_dialog(window, cx);
                            }
                            "shortcuts" => {
                                mergeon_view_home::open_shortcuts_dialog(window, cx);
                            }
                            "about" => {
                                mergeon_view_home::open_about_dialog(window, cx);
                            }
                            other => eprintln!("MERGEON_HOME_OPEN: no sé abrir `{other}`"),
                        }
                    }
                });

                for step in steps {
                    cx.background_executor()
                        .timer(Duration::from_millis(300))
                        .await;
                    if let Some(wait) = step.strip_prefix("wait:") {
                        let wait = wait.trim().parse::<u64>().unwrap_or(300);
                        cx.background_executor()
                            .timer(Duration::from_millis(wait))
                            .await;
                        continue;
                    }
                    let _ = window.update(cx, |_, window, cx| run_step(&step, window, cx));
                }
            })
            .detach();
        });
}
