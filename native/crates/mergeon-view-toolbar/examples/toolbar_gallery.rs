//! Galería de la barra de herramientas: la vista sola sobre un `AppModel` real
//! y el repositorio de demostración (`native/tools/make-fixture.sh`), para
//! compararla con `native/target-shots/reference/`.
//!
//! ```bash
//! cargo run -p mergeon-view-toolbar --example toolbar_gallery
//! MERGEON_THEME=light MERGEON_TOOLBAR_OPEN=fetch cargo run -p mergeon-view-toolbar --example toolbar_gallery
//! ```
//!
//! - `MERGEON_THEME=dark|light|system` (por defecto `dark`).
//! - `MERGEON_TOOLBAR_STATE=`, lista separada por comas del estado del
//!   repositorio: `merge`, `merge-partial`, `merge-ready`, `rebase`,
//!   `cherry-pick`, `revert` (operación a medias con conflictos), `detached`
//!   (HEAD desanclado), `empty` (repositorio sin commits), `none` (sin
//!   repositorio), `push-running` (un push que no termina), `queue` (push en
//!   curso y fetch y pull en cola) y `log` (operaciones reales para el log).
//! - `MERGEON_TOOLBAR_OPEN=`, lista separada por comas de lo que se abre al
//!   arrancar: `fetch`, `pull`, `push`, `create-branch`, `new-branch` (el flujo
//!   real del botón), `stash` (su prompt), `operations-log`, `reflog`.
//! - `MERGEON_TOOLBAR_INPUT=`, pasos separados por `;` que se ejecutan después
//!   (uno cada 300ms): `key:<tecla>`, `type:<texto>`, `focus:<n>`, `move:x,y`,
//!   `click:x,y`, `wait:<ms>`. Las coordenadas son de la ventana.
//! - `MERGEON_TOOLBAR_WIDTH=` / `MERGEON_TOOLBAR_HEIGHT=`: tamaño de la ventana
//!   (1440×900 por defecto); con menos de 948px de ancho la barra pasa a su
//!   disposición de dos filas.
//! - `MERGEON_FIXTURE=<dir>`: usa ese directorio (ya creado con
//!   `make-fixture.sh`) en vez de uno temporal.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::time::Duration;

use futures::channel::oneshot;
use gpui_kit::*;
use mergeon_app::testing::TestBackend;
use mergeon_app::{
    AddRepository, Answer, AppEvent, AppModel, FetchRequest, PullRequest, PushRequest, UiHost,
};
use mergeon_model::dialogs::{
    ConfirmAnswer, ConfirmOptions, ConfirmValue, PromptAnswer, PromptOptions,
};
use mergeon_model::notify::{NotifyType, Toast};
use mergeon_ui::dialog::{self, ConfirmVariant};
use mergeon_ui::toast::{self, ToastKind, ToastOptions};
use mergeon_ui::{ActivePaletteExt as _, ThemePreference};
use mergeon_view_toolbar::{
    CreateBranchDialogOptions, OperationStatusSlot, ToolbarView, handle_dialog_request,
    open_create_branch_dialog,
};

// ---------------------------------------------------------------------------
// Repositorio de demostración
// ---------------------------------------------------------------------------

fn git(repo: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Ana Torres")
        .env("GIT_AUTHOR_EMAIL", "ana@example.com")
        .env("GIT_COMMITTER_NAME", "Ana Torres")
        .env("GIT_COMMITTER_EMAIL", "ana@example.com")
        .env("GIT_AUTHOR_DATE", "2026-09-11T11:35:00+00:00")
        .env("GIT_COMMITTER_DATE", "2026-09-11T11:35:00+00:00")
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn edit(repo: &Path, file: &str, from: &str, to: &str) {
    let path = repo.join(file);
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    std::fs::write(&path, text.replacen(from, to, 1)).expect("no se pudo editar el fixture");
}

fn commit(repo: &Path, message: &str) {
    git(repo, &["add", "--all"]);
    git(repo, &["commit", "-q", "-m", message]);
}

fn clean_working_tree(repo: &Path) {
    git(repo, &["reset", "-q", "--hard"]);
    git(repo, &["clean", "-fdq"]);
}

/// Merge a medias con dos archivos en conflicto, como el de las capturas de
/// referencia (`makeMergeConflict` de `reference-shots.mjs`).
fn make_merge_conflict(repo: &Path) {
    clean_working_tree(repo);
    git(repo, &["checkout", "-q", "-b", "release/2.0"]);
    edit(
        repo,
        "src/app.js",
        "import { formatDate } from './lib/util.js'",
        "import { formatDate, VERSION } from './lib/util.js'",
    );
    edit(
        repo,
        "src/app.js",
        "return 0",
        "return process.exitCode || 0",
    );
    edit(repo, "styles/main.css", "margin: 0;", "margin: 0 auto;");
    std::fs::write(
        repo.join("src/lib/format.js"),
        "export function upper(text) {\n  return String(text).toUpperCase()\n}\n",
    )
    .expect("no se pudo escribir el fixture");
    commit(repo, "Prepare the 2.0 release");
    git(repo, &["checkout", "-q", "main"]);
    edit(
        repo,
        "src/app.js",
        "import { formatDate } from './lib/util.js'",
        "import { clamp, formatDate } from './lib/util.js'",
    );
    edit(
        repo,
        "src/app.js",
        "return 0",
        "return clamp(Number(process.exitCode), 0, 255)",
    );
    edit(repo, "styles/main.css", "margin: 0;", "margin: 8px;");
    commit(repo, "Clamp the exit code on main");
    git(repo, &["merge", "release/2.0"]);
}

fn resolve_conflict(repo: &Path, file: &str) {
    git(repo, &["checkout", "--ours", "--", file]);
    git(repo, &["add", "--", file]);
}

/// Deja el repositorio en el estado pedido antes de abrirlo. Devuelve la ruta
/// a abrir (`None`: ninguna).
fn prepare_fixture(root: &Path, states: &[String]) -> Option<PathBuf> {
    let has = |name: &str| states.iter().any(|state| state == name);
    if has("none") {
        return None;
    }
    if has("empty") {
        let scratch = root.join("scratch");
        std::fs::create_dir_all(&scratch).expect("no se pudo crear el repositorio vacío");
        git(&scratch, &["init", "-q", "-b", "main"]);
        return Some(scratch);
    }

    let demo = root.join("demo");
    if has("merge") || has("merge-partial") || has("merge-ready") {
        make_merge_conflict(&demo);
        if has("merge-partial") || has("merge-ready") {
            resolve_conflict(&demo, "styles/main.css");
        }
        if has("merge-ready") {
            resolve_conflict(&demo, "src/app.js");
        }
    } else if has("rebase") {
        // hotfix/crash toca la línea contigua a la que cambió main: conflicto seguro.
        clean_working_tree(&demo);
        git(&demo, &["checkout", "-q", "hotfix/crash"]);
        git(&demo, &["rebase", "main"]);
    } else if has("cherry-pick") {
        clean_working_tree(&demo);
        git(&demo, &["cherry-pick", "hotfix/crash"]);
    } else if has("revert") {
        clean_working_tree(&demo);
        if let Some(clamp) = git(
            &demo,
            &["log", "--format=%H", "--grep=Add clamp helper", "main"],
        ) {
            git(&demo, &["revert", "--no-edit", &clamp]);
        }
    } else if has("detached") {
        clean_working_tree(&demo);
        git(&demo, &["checkout", "-q", "--detach", "v1.0.0"]);
    }

    if has("push-running") || has("queue") {
        // Un hook pre-push que tarda: el push queda «running» de verdad y lo
        // que se pida después se encola.
        let hook = demo.join(".git/hooks/pre-push");
        std::fs::create_dir_all(hook.parent().expect("hooks")).ok();
        std::fs::write(&hook, "#!/bin/sh\nsleep 60\n").expect("no se pudo escribir el hook");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).ok();
        }
    }
    Some(demo)
}

fn make_fixture(root: &Path) {
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tools/make-fixture.sh");
    let status = Command::new("bash")
        .arg(script)
        .arg(root)
        .stdout(std::process::Stdio::null())
        .status()
        .expect("no se pudo ejecutar make-fixture.sh");
    assert!(status.success(), "make-fixture.sh falló");
}

// ---------------------------------------------------------------------------
// Interfaz global (`UiHost`): avisos, confirmaciones y prompts de mergeon_ui
// ---------------------------------------------------------------------------

/// Lo que en la app instala la cáscara. Las llamadas llegan con el modelo
/// tomado (y a veces con la ventana tomada), así que todo se difiere.
struct GalleryUiHost {
    window: AnyWindowHandle,
}

fn shared(text: Option<String>) -> SharedString {
    text.unwrap_or_default().into()
}

fn confirm_options(options: &ConfirmOptions) -> dialog::ConfirmOptions {
    dialog::ConfirmOptions {
        title: shared(options.title.clone()),
        description: shared(options.description.clone()),
        confirm_text: shared(options.confirm_text.clone()),
        cancel_text: shared(options.cancel_text.clone()),
        dismiss_value: options.dismiss_value.flatten().unwrap_or(false),
        variant: if options.variant.as_deref() == Some("destructive") {
            ConfirmVariant::Destructive
        } else {
            ConfirmVariant::Default
        },
    }
}

impl UiHost for GalleryUiHost {
    fn notify(&self, toast: Toast, cx: &mut App) {
        let window = self.window;
        cx.defer(move |cx| {
            let kind = match toast.kind {
                NotifyType::Success => ToastKind::Success,
                NotifyType::Info => ToastKind::Info,
                NotifyType::Warning => ToastKind::Warning,
                NotifyType::Error => ToastKind::Error,
            };
            let mut options = ToastOptions::new().duration(toast.duration());
            if toast.full_message != toast.title {
                options = options.details(toast.full_message.clone());
            }
            window
                .update(cx, |_, window, cx| {
                    toast::show(window, cx, kind, &toast.title, options);
                })
                .ok();
        });
    }

    fn confirm(&self, options: ConfirmOptions, cx: &mut App) -> Answer<ConfirmValue> {
        let (sender, receiver) = oneshot::channel();
        let window = self.window;
        cx.defer(move |cx| {
            window
                .update(cx, |_, window, cx| {
                    dialog::confirm(window, cx, confirm_options(&options), move |ok, _, _| {
                        sender.send(Some(ok)).ok();
                    });
                })
                .ok();
        });
        receiver
    }

    fn confirm_with_option(&self, options: ConfirmOptions, cx: &mut App) -> Answer<ConfirmAnswer> {
        let (sender, receiver) = oneshot::channel();
        let window = self.window;
        cx.defer(move |cx| {
            let option = dialog::ConfirmOption {
                label: shared(options.option_label.clone()),
                checked: options.option_checked,
            };
            window
                .update(cx, |_, window, cx| {
                    dialog::confirm_with_option(
                        window,
                        cx,
                        confirm_options(&options),
                        option,
                        move |outcome, _, _| {
                            sender
                                .send(ConfirmAnswer::WithOption {
                                    confirmed: outcome.confirmed,
                                    checked: outcome.checked,
                                })
                                .ok();
                        },
                    );
                })
                .ok();
        });
        receiver
    }

    fn prompt(&self, options: PromptOptions, cx: &mut App) -> Answer<PromptAnswer> {
        let (sender, receiver) = oneshot::channel();
        let window = self.window;
        cx.defer(move |cx| {
            let pattern = options
                .pattern
                .as_deref()
                .and_then(|pattern| regex::Regex::new(pattern).ok());
            let ui_options = dialog::PromptOptions {
                title: shared(options.title.clone()),
                description: shared(options.description.clone()),
                confirm_text: shared(options.confirm_text.clone()),
                cancel_text: shared(options.cancel_text.clone()),
                placeholder: shared(options.placeholder.clone()),
                default_value: shared(options.default_value.clone()),
                validate: pattern.map(|pattern| {
                    Rc::new(move |value: &str| pattern.is_match(value)) as dialog::PromptValidator
                }),
                input_error_message: shared(options.input_error_message.clone()),
                textarea: options.textarea,
                rows: options.rows.unwrap_or(0) as usize,
            };
            window
                .update(cx, |_, window, cx| {
                    dialog::prompt(window, cx, ui_options, move |value, _, _| {
                        sender
                            .send(PromptAnswer {
                                confirmed: value.is_some(),
                                value: value.map(|value| value.to_string()),
                            })
                            .ok();
                    });
                })
                .ok();
        });
        receiver
    }

    fn pick_directory(&self, _: &mut App) -> Answer<Option<PathBuf>> {
        let (sender, receiver) = oneshot::channel();
        sender.send(None).ok();
        receiver
    }
}

// ---------------------------------------------------------------------------
// Ventana
// ---------------------------------------------------------------------------

/// La barra y su franja de estado donde las pone la cáscara: a la derecha
/// del rail de actividad y bajo la fila de pestañas (aquí, huecos vacíos de
/// su mismo tamaño para que las capturas se puedan superponer con las de
/// referencia).
struct Gallery {
    toolbar: Entity<ToolbarView>,
    status: Entity<OperationStatusSlot>,
    _subscription: Subscription,
}

impl Gallery {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let model = AppModel::global(cx);
        // Lo que hará la cáscara: delegar los diálogos que piden los flujos.
        let subscription = cx.subscribe_in(&model, window, |_, _, event: &AppEvent, window, cx| {
            if let AppEvent::ShowDialog(request) = event {
                handle_dialog_request(request, window, cx);
            }
        });
        Self {
            toolbar: cx.new(|cx| ToolbarView::new(window, cx)),
            status: cx.new(|cx| OperationStatusSlot::new(window, cx)),
            _subscription: subscription,
        }
    }
}

impl Render for Gallery {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        div()
            .size_full()
            .flex()
            .bg(palette.background)
            .text_color(palette.foreground)
            .child(
                div()
                    .w(px(48.))
                    .h_full()
                    .flex_shrink_0()
                    .border_r_1()
                    .border_color(palette.border),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .h(px(36.))
                            .flex_shrink_0()
                            .border_b_1()
                            .border_color(palette.border),
                    )
                    .child(self.toolbar.clone())
                    .child(self.status.clone())
                    .child(div().flex_1()),
            )
    }
}

fn env_list(name: &str, separator: char) -> Vec<String> {
    std::env::var(name)
        .unwrap_or_default()
        .split(separator)
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
        .collect()
}

fn env_px(name: &str, default: f32) -> Pixels {
    px(std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default))
}

/// Abre lo que pida `MERGEON_TOOLBAR_OPEN`.
fn open_overlay(what: &str, window: &mut Window, cx: &mut App) {
    let model = AppModel::global(cx);
    match what {
        "fetch" => model.update(cx, |model, cx| model.handle_fetch(cx)),
        "pull" => model.update(cx, |model, cx| model.handle_pull(cx)),
        "push" => model.update(cx, |model, cx| model.handle_push(cx)),
        "new-branch" => model.update(cx, |model, cx| model.handle_new_branch(cx).detach()),
        "stash" => model.update(cx, |model, cx| model.handle_stash(cx).detach()),
        "create-branch" => {
            let (label, has_local_changes) = {
                let model = model.read(cx);
                (
                    model.active_repo_current_branch().to_string(),
                    model.active_repo_has_local_changes(),
                )
            };
            open_create_branch_dialog(
                CreateBranchDialogOptions::new(label).has_local_changes(has_local_changes),
                window,
                cx,
            );
        }
        "operations-log" => {
            mergeon_view_toolbar::open_operations_log(window, cx);
        }
        "reflog" => {
            mergeon_view_toolbar::open_reflog(window, cx);
        }
        other => eprintln!("MERGEON_TOOLBAR_OPEN: no sé abrir `{other}`"),
    }
}

/// Ejecuta un paso de `MERGEON_TOOLBAR_INPUT` como si viniera del usuario.
fn run_step(step: &str, window: &mut Window, cx: &mut App) {
    let Some((kind, value)) = step.split_once(':') else {
        eprintln!("MERGEON_TOOLBAR_INPUT: paso inválido `{step}`");
        return;
    };
    match kind {
        "wait" => return,
        "focus" => {
            for _ in 0..value.parse::<usize>().unwrap_or(1) {
                window.focus_next(cx);
            }
            return;
        }
        "key" => {
            match Keystroke::parse(value) {
                Ok(keystroke) => {
                    window.dispatch_keystroke(keystroke.clone(), cx);
                    window.dispatch_event(KeyUpEvent { keystroke }.to_platform_input(), cx);
                }
                Err(error) => eprintln!("MERGEON_TOOLBAR_INPUT: tecla inválida `{value}`: {error}"),
            }
            return;
        }
        "type" => {
            for character in value.chars() {
                let text = character.to_string();
                let keystroke = Keystroke {
                    modifiers: Modifiers::default(),
                    key: text.clone(),
                    key_char: Some(text),
                };
                window.dispatch_keystroke(keystroke, cx);
            }
            return;
        }
        _ => {}
    }

    let mut numbers = value
        .split(',')
        .filter_map(|number| number.trim().parse::<f32>().ok());
    let (Some(x), Some(y)) = (numbers.next(), numbers.next()) else {
        eprintln!("MERGEON_TOOLBAR_INPUT: coordenadas inválidas `{value}`");
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
    if kind != "click" {
        return;
    }
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

async fn pause(cx: &mut AsyncApp, millis: u64) {
    cx.background_executor()
        .timer(Duration::from_millis(millis))
        .await;
}

/// Estados que se alcanzan pidiéndole operaciones reales al modelo.
async fn drive_model(model: &Entity<AppModel>, states: &[String], cx: &mut AsyncApp) {
    let has = |name: &str| states.iter().any(|state| state == name);
    let push = || PushRequest {
        branch: "main".to_string(),
        destination: "origin/main".to_string(),
        push_tags: false,
        force: false,
    };
    let fetch = || FetchRequest {
        remote: "origin".to_string(),
        all_remotes: false,
    };
    let pull = || PullRequest {
        remote: "origin".to_string(),
        branch: Some("main".to_string()),
        rebase: false,
        autostash: false,
    };

    if has("log") {
        // Operaciones de verdad, para que el log tenga horas, duraciones y un
        // mensaje de git completo.
        model
            .update(cx, |model, cx| model.handle_fetch_submit(fetch(), cx))
            .await;
        model
            .update(cx, |model, cx| model.handle_pull_submit(pull(), cx))
            .await;
        let checkout = model.update(cx, |model, cx| {
            let repository = model.active_repository()?;
            let (id, path) = (repository.id.clone(), repository.path.clone());
            Some((id, model.checkout(&path, "hotfix/crash", cx)))
        });
        if let Some((repo_id, checkout)) = checkout {
            let result = checkout.await;
            model.update(cx, |model, cx| {
                model.apply_repository_operation_result(repo_id.as_str(), &result, cx);
            });
        }
        model
            .update(cx, |model, cx| model.handle_push_submit(push(), cx))
            .await;
    }

    if has("push-running") || has("queue") {
        model
            .update(cx, |model, cx| model.handle_push_submit(push(), cx))
            .detach();
        pause(cx, 400).await;
    }
    if has("queue") {
        model
            .update(cx, |model, cx| model.handle_fetch_submit(fetch(), cx))
            .detach();
        pause(cx, 300).await;
        model
            .update(cx, |model, cx| model.handle_pull_submit(pull(), cx))
            .detach();
        pause(cx, 300).await;
    }
}

fn main() {
    if std::env::var("MERGEON_LOG").is_ok() {
        env_logger::Builder::new().parse_filters("warn").init();
    }
    let preference = ThemePreference::parse(&std::env::var("MERGEON_THEME").unwrap_or_default());
    let states = env_list("MERGEON_TOOLBAR_STATE", ',');
    let open = env_list("MERGEON_TOOLBAR_OPEN", ',');
    let steps = env_list("MERGEON_TOOLBAR_INPUT", ';');
    let window_size = size(
        env_px("MERGEON_TOOLBAR_WIDTH", 1440.),
        env_px("MERGEON_TOOLBAR_HEIGHT", 900.),
    );

    // El repositorio vive en un temporal que se borra al salir, salvo que se
    // indique uno propio.
    let temporary = tempfile::Builder::new()
        .prefix("mergeon-toolbar-gallery-")
        .tempdir()
        .expect("no se pudo crear el directorio temporal");
    let root = match std::env::var_os("MERGEON_FIXTURE") {
        Some(root) => PathBuf::from(root),
        None => {
            make_fixture(temporary.path());
            temporary.path().to_path_buf()
        }
    };
    let repository = prepare_fixture(&root, &states);

    gpui_kit::application()
        .with_assets(mergeon_ui::Assets)
        .run(move |cx| {
            mergeon_ui::init(cx);
            mergeon_ui::theme::apply(preference, cx);
            mergeon_view_toolbar::init(cx);

            // Configuración en memoria y sin vigilar el disco: la galería no
            // toca los ajustes del usuario.
            let (backend, events) = TestBackend::new().build();
            let model = AppModel::init(backend, events, cx);

            let bounds = Bounds::centered(None, window_size, cx);
            let (window, _gallery) = gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                cx,
                |window, cx| cx.new(|cx| Gallery::new(window, cx)),
            )
            .expect("no se pudo abrir la ventana");
            let host = Rc::new(GalleryUiHost { window });
            model.update(cx, |model, cx| model.set_ui_host(host, cx));

            cx.spawn(async move |cx| {
                if let Some(repository) = repository {
                    let path = repository.to_string_lossy().into_owned();
                    model
                        .update(cx, |model, cx| {
                            model.add_repository(&path, AddRepository::default(), cx)
                        })
                        .await
                        .ok();
                    // Los diálogos leen ramas y remotos: que estén cargados.
                    model
                        .update(cx, |model, cx| model.hydrate_opened_repository(cx))
                        .await;
                }
                drive_model(&model, &states, cx).await;

                for what in &open {
                    window
                        .update(cx, |_, window, cx| open_overlay(what, window, cx))
                        .ok();
                    pause(cx, 200).await;
                }
                for step in steps {
                    let millis = step
                        .strip_prefix("wait:")
                        .and_then(|millis| millis.parse().ok())
                        .unwrap_or(300);
                    pause(cx, millis).await;
                    window
                        .update(cx, |_, window, cx| run_step(&step, window, cx))
                        .ok();
                }
            })
            .detach();
        });

    drop(temporary);
}
