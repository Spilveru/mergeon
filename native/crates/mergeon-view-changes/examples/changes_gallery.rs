//! Galería de la vista de cambios locales: la vista sola sobre un `AppModel`
//! real y el repositorio de demostración (`native/tools/make-fixture.sh`),
//! para compararla con `native/target-shots/reference/` (`3xx`, `445`, `5xx`).
//!
//! ```bash
//! cargo run -p mergeon-view-changes --example changes_gallery
//! MERGEON_THEME=light MERGEON_CHANGES_MODE=file:working:src/app.js \
//!   cargo run -p mergeon-view-changes --example changes_gallery
//! ```
//!
//! - `MERGEON_THEME=dark|light|system` (por defecto `dark`).
//! - `MERGEON_CHANGES_MODE=`: `list` (por defecto, la pestaña "Local
//!   Changes"), `full` (lista, diff y caja de commit juntos) o
//!   `file:<working|index>:<ruta>` (la pestaña de ese archivo).
//! - `MERGEON_CHANGES_STATE=`, lista separada por comas del estado del
//!   repositorio: `merge`, `merge-partial`, `merge-ready`, `rebase` (operación
//!   a medias con conflictos), `clean` (sin cambios), `all-staged` (todo
//!   preparado), `pushed` (el último commit ya está en el remoto: el aviso de
//!   amend), `stage-app` (`src/app.js` preparado, lo que deja "Stage
//!   block"), `empty` (repositorio sin commits), `many` (400 archivos sin
//!   seguir), `files-closed` (pestaña de archivo con la lista oculta),
//!   `split` e `ignore-spaces` (preferencias del diff) y `slow-diff` (el diff
//!   no llega: se ve "Loading diff...").
//! - `MERGEON_CHANGES_OPEN=`, lo que se abre al arrancar: `stash` (el diálogo
//!   del primer stash), `blame:<ruta>`.
//! - `MERGEON_CHANGES_INPUT=`, pasos separados por `;` que se ejecutan después
//!   (uno cada 300ms): `key:<tecla>`, `type:<texto>`, `move:x,y`, `click:x,y`,
//!   `dclick:x,y`, `rclick:x,y`, `scroll:x,y,dy`, `wait:<ms>`. Coordenadas de
//!   la ventana.
//! - `MERGEON_CHANGES_WIDTH=` / `MERGEON_CHANGES_HEIGHT=`: tamaño de la ventana
//!   (1440×900 por defecto).
//! - `MERGEON_CHANGES_TOP=`: alto del hueco que en la app ocupan las pestañas,
//!   la barra de herramientas y la cabecera (121 en la lista, 152 en una
//!   pestaña de archivo; con un banner de operación, 54 más).
//! - `MERGEON_FIXTURE=<dir>`: usa ese directorio (ya creado con
//!   `make-fixture.sh`) en vez de uno temporal.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::time::Duration;

use futures::channel::oneshot;
use gpui_kit::*;
use mergeon_app::testing::TestBackend;
use mergeon_app::{AddRepository, Answer, AppModel, SyncRequest, UiHost};
use mergeon_model::dialogs::{
    ConfirmAnswer, ConfirmOptions, ConfirmValue, PromptAnswer, PromptOptions,
};
use mergeon_model::notify::{NotifyType, Toast};
use mergeon_model::workspace_tabs::{FileTabDescriptor, TabSource};
use mergeon_ui::dialog::{self, ConfirmVariant};
use mergeon_ui::toast::{self, ToastKind, ToastOptions};
use mergeon_ui::{ActivePaletteExt as _, ThemePreference};
use mergeon_view_changes::{ChangesMode, ChangesView, open_blame_dialog, open_stash_diff_dialog};

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

/// Deja el repositorio en el estado pedido antes de abrirlo.
fn prepare_fixture(root: &Path, states: &[String]) -> PathBuf {
    let has = |name: &str| states.iter().any(|state| state == name);
    if has("empty") {
        let scratch = root.join("scratch");
        std::fs::create_dir_all(&scratch).expect("no se pudo crear el repositorio vacío");
        git(&scratch, &["init", "-q", "-b", "main"]);
        std::fs::write(scratch.join("first.txt"), "first\n").ok();
        return scratch;
    }

    let demo = root.join("demo");
    if has("merge") || has("merge-partial") || has("merge-ready") {
        make_merge_conflict(&demo);
        if has("merge-partial") || has("merge-ready") {
            resolve_conflict(&demo, "src/app.js");
        }
        if has("merge-ready") {
            resolve_conflict(&demo, "styles/main.css");
        }
    } else if has("rebase") {
        // hotfix/crash toca la línea contigua a la que cambió main: conflicto seguro.
        clean_working_tree(&demo);
        git(&demo, &["checkout", "-q", "hotfix/crash"]);
        git(&demo, &["rebase", "main"]);
    }

    if has("pushed") {
        git(&demo, &["push", "-q", "origin", "main"]);
    }
    if has("clean") {
        clean_working_tree(&demo);
    }
    if has("stage-app") {
        // Lo que deja "Stage hunk" sobre el único bloque de `src/app.js`.
        git(&demo, &["add", "src/app.js"]);
    }
    if has("all-staged") {
        git(&demo, &["add", "--all"]);
    }
    if has("many") {
        let directory = demo.join("generated");
        std::fs::create_dir_all(&directory).ok();
        for index in 0..400 {
            std::fs::write(
                directory.join(format!("file-{index:03}.txt")),
                format!("generated {index}\n"),
            )
            .ok();
        }
    }
    if has("slow-diff") {
        // Un filtro de texto que tarda: `git diff` no vuelve y el panel se
        // queda en "Loading diff...".
        std::fs::write(demo.join(".gitattributes"), "*.js diff=slow\n").ok();
        git(&demo, &["config", "diff.slow.textconv", "sleep 60; cat"]);
    }
    demo
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

/// La vista donde la pone la cáscara: a la derecha del rail de actividad y
/// bajo las pestañas, la barra de herramientas y la tira de pestañas del
/// repositorio (aquí, huecos vacíos de su mismo tamaño para que las capturas
/// se puedan superponer con las de referencia).
struct Gallery {
    view: Option<Entity<ChangesView>>,
    top: Pixels,
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
                            .h(self.top - px(1.))
                            .flex_shrink_0()
                            .border_b_1()
                            .border_color(palette.border)
                            .bg(palette.card),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_h_0()
                            .children(self.view.clone()),
                    ),
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

fn env_number(name: &str) -> Option<f32> {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
}

/// Qué modo pide `MERGEON_CHANGES_MODE`. Para una pestaña de archivo, lo que
/// hay que abrir.
enum GalleryMode {
    List,
    Full,
    File(FileTabDescriptor),
}

fn gallery_mode() -> GalleryMode {
    let value = std::env::var("MERGEON_CHANGES_MODE").unwrap_or_default();
    if value == "full" {
        return GalleryMode::Full;
    }
    let Some(rest) = value.strip_prefix("file:") else {
        return GalleryMode::List;
    };
    let (source, path) = rest.split_once(':').unwrap_or(("working", rest));
    let source = TabSource::parse(source).unwrap_or(TabSource::Working);
    GalleryMode::File(FileTabDescriptor::new(source, path, None))
}

/// Abre lo que pida `MERGEON_CHANGES_OPEN`.
fn open_overlay(what: &str, repo_id: &str, window: &mut Window, cx: &mut App) {
    if what == "stash" {
        let stash = AppModel::global(cx)
            .read(cx)
            .repository(repo_id)
            .and_then(|repository| repository.stashes.first().map(|stash| stash.hash.clone()));
        match stash {
            Some(stash) => {
                open_stash_diff_dialog(repo_id, &stash, window, cx);
            }
            None => eprintln!("MERGEON_CHANGES_OPEN: el repositorio no tiene stashes"),
        }
    } else if let Some(path) = what.strip_prefix("blame:") {
        open_blame_dialog(repo_id, path, window, cx);
    } else {
        eprintln!("MERGEON_CHANGES_OPEN: no sé abrir `{what}`");
    }
}

fn mouse_click(
    window: &mut Window,
    cx: &mut App,
    position: Point<Pixels>,
    button: MouseButton,
    click_count: usize,
) {
    window.dispatch_event(
        MouseDownEvent {
            button,
            position,
            modifiers: Modifiers::default(),
            click_count,
            first_mouse: false,
        }
        .to_platform_input(),
        cx,
    );
    window.dispatch_event(
        MouseUpEvent {
            button,
            position,
            modifiers: Modifiers::default(),
            click_count,
        }
        .to_platform_input(),
        cx,
    );
}

/// Ejecuta un paso de `MERGEON_CHANGES_INPUT` como si viniera del usuario.
fn run_step(step: &str, window: &mut Window, cx: &mut App) {
    let Some((kind, value)) = step.split_once(':') else {
        eprintln!("MERGEON_CHANGES_INPUT: paso inválido `{step}`");
        return;
    };
    match kind {
        "wait" => return,
        "key" => {
            match Keystroke::parse(value) {
                Ok(keystroke) => {
                    window.dispatch_keystroke(keystroke.clone(), cx);
                    window.dispatch_event(KeyUpEvent { keystroke }.to_platform_input(), cx);
                }
                Err(error) => eprintln!("MERGEON_CHANGES_INPUT: tecla inválida `{value}`: {error}"),
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

    let numbers: Vec<f32> = value
        .split(',')
        .filter_map(|number| number.trim().parse::<f32>().ok())
        .collect();
    let (Some(x), Some(y)) = (numbers.first(), numbers.get(1)) else {
        eprintln!("MERGEON_CHANGES_INPUT: coordenadas inválidas `{value}`");
        return;
    };
    let position = point(px(*x), px(*y));
    window.dispatch_event(
        MouseMoveEvent {
            position,
            pressed_button: None,
            modifiers: Modifiers::default(),
        }
        .to_platform_input(),
        cx,
    );
    match kind {
        "click" => mouse_click(window, cx, position, MouseButton::Left, 1),
        "dclick" => {
            mouse_click(window, cx, position, MouseButton::Left, 1);
            mouse_click(window, cx, position, MouseButton::Left, 2);
        }
        "rclick" => mouse_click(window, cx, position, MouseButton::Right, 1),
        "scroll" => {
            let delta = numbers.get(2).copied().unwrap_or(0.);
            window.dispatch_event(
                ScrollWheelEvent {
                    position,
                    delta: ScrollDelta::Pixels(point(px(0.), px(delta))),
                    modifiers: Modifiers::default(),
                    touch_phase: TouchPhase::Moved,
                }
                .to_platform_input(),
                cx,
            );
        }
        _ => {}
    }
}

async fn pause(cx: &mut AsyncApp, millis: u64) {
    cx.background_executor()
        .timer(Duration::from_millis(millis))
        .await;
}

fn main() {
    if std::env::var("MERGEON_LOG").is_ok() {
        env_logger::Builder::new().parse_filters("warn").init();
    }
    let preference = ThemePreference::parse(&std::env::var("MERGEON_THEME").unwrap_or_default());
    let states = env_list("MERGEON_CHANGES_STATE", ',');
    let open = env_list("MERGEON_CHANGES_OPEN", ',');
    let steps = env_list("MERGEON_CHANGES_INPUT", ';');
    let mode = gallery_mode();
    let window_size = size(
        px(env_number("MERGEON_CHANGES_WIDTH").unwrap_or(1440.)),
        px(env_number("MERGEON_CHANGES_HEIGHT").unwrap_or(900.)),
    );
    let top = px(env_number("MERGEON_CHANGES_TOP").unwrap_or(match mode {
        GalleryMode::File(_) => 152.,
        _ => 121.,
    }));
    let has = |name: &str| states.iter().any(|state| state == name);
    let (files_closed, split, ignore_spaces) =
        (has("files-closed"), has("split"), has("ignore-spaces"));

    // El repositorio vive en un temporal que se borra al salir, salvo que se
    // indique uno propio.
    let temporary = tempfile::Builder::new()
        .prefix("mergeon-changes-gallery-")
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
            mergeon_view_diff::init(cx);
            mergeon_view_changes::init(cx);

            // Configuración en memoria y sin vigilar el disco: la galería no
            // toca los ajustes del usuario.
            let (backend, events) = TestBackend::new().build();
            let model = AppModel::init(backend, events, cx);

            let bounds = Bounds::centered(None, window_size, cx);
            let (window, gallery) = gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                cx,
                |_, cx| cx.new(|_| Gallery { view: None, top }),
            )
            .expect("no se pudo abrir la ventana");
            let host = Rc::new(GalleryUiHost { window });
            model.update(cx, |model, cx| model.set_ui_host(host, cx));

            cx.spawn(async move |cx| {
                let path = repository.to_string_lossy().into_owned();
                model
                    .update(cx, |model, cx| {
                        model.add_repository(&path, AddRepository::default(), cx)
                    })
                    .await
                    .ok();
                model
                    .update(cx, |model, cx| model.hydrate_opened_repository(cx))
                    .await;
                let Some((repo_id, repo_path)) = model.read_with(cx, |model, _| {
                    model
                        .active_repository()
                        .map(|repository| (repository.id.clone(), repository.path.clone()))
                }) else {
                    eprintln!("no se pudo abrir el repositorio de demostración");
                    return;
                };
                // El diálogo del stash toma su título de los stashes cargados.
                model
                    .update(cx, |model, cx| {
                        model.sync_repository_read_model(
                            repo_id.as_str(),
                            SyncRequest::new().stashes(true),
                            cx,
                        )
                    })
                    .await;
                model
                    .update(cx, |model, cx| model.load_diff_preferences(false, cx))
                    .await;
                if split {
                    model
                        .update(cx, |model, cx| model.toggle_diff_view(cx))
                        .await;
                }
                if ignore_spaces {
                    model
                        .update(cx, |model, cx| model.toggle_ignore_whitespace(cx))
                        .await;
                }

                let changes_mode = match &mode {
                    GalleryMode::List => ChangesMode::List,
                    GalleryMode::Full => ChangesMode::Full,
                    GalleryMode::File(descriptor) => {
                        model.update(cx, |model, cx| {
                            model.open_workspace_file(&repo_path, descriptor, false, cx);
                        });
                        ChangesMode::FileTab {
                            tab_id: descriptor.id(),
                        }
                    }
                };
                window
                    .update(cx, |_, window, cx| {
                        let view = cx.new(|cx| {
                            let mut view =
                                ChangesView::new(repo_id.clone(), changes_mode, window, cx);
                            view.set_files_open(!files_closed, cx);
                            view
                        });
                        gallery.update(cx, |gallery, cx| {
                            gallery.view = Some(view);
                            cx.notify();
                        });
                    })
                    .ok();
                pause(cx, 400).await;

                for what in &open {
                    window
                        .update(cx, |_, window, cx| open_overlay(what, &repo_id, window, cx))
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
