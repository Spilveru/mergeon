//! Galería del sidebar: una ventana con el sidebar real sobre un `AppModel`
//! real y el repositorio de demostración de `native/tools/make-fixture.sh`.
//!
//! ```bash
//! cargo run -p mergeon-view-sidebar --example sidebar_gallery
//! MERGEON_THEME=light MERGEON_SIDEBAR_STEPS="sections:all" cargo run -p mergeon-view-sidebar --example sidebar_gallery
//! ```
//!
//! La ventana mide 1440×900 y coloca el sidebar donde lo tiene la app (a la
//! derecha del rail, bajo la barra de pestañas y el toolbar), para comparar las
//! capturas con `native/target-shots/reference/` píxel a píxel.
//!
//! - `MERGEON_THEME=dark|light|system` (por defecto `dark`).
//! - `MERGEON_SIDEBAR_FIXTURE=demo|detached|behind|submodules|empty` (por
//!   defecto `demo`): qué repositorio se abre.
//! - `MERGEON_SIDEBAR_REPO=<ruta>`: abre ese repositorio en vez de un fixture.
//! - `MERGEON_SIDEBAR_STARRED=main,feature/search`: ramas destacadas (así
//!   por defecto en `demo`; vacío para ninguna).
//! - `MERGEON_SIDEBAR_SECTIONS=all|none|default|starred,branches,...`: secciones
//!   desplegadas al arrancar (por defecto `all`).
//! - `MERGEON_SIDEBAR_STEPS=`: pasos separados por `;` que se ejecutan en orden,
//!   uno cada 400 ms, cuando el repositorio ya cargó:
//!   - `filter:<texto>`, `search:<texto>` (abre la búsqueda y escribe),
//!     `panel:repository|search`, `toggle:<sección>`, `refresh`
//!   - `view:<rama>`, `view-remote:<remoto>/<rama>`, `view-tag:<tag>`
//!   - órdenes: `checkout:<rama>`, `checkout-remote:<remoto>/<rama>`,
//!     `branch-action:<merge|rebase|compare>:<rama>`, `drop:<origen>:<destino>`,
//!     `new-branch:<rama>`, `rename-branch:<rama>`, `delete-branch:<rama>`,
//!     `delete-remote-branch:<remoto>/<rama>`, `add-remote`, `edit-remote-url:<remoto>`,
//!     `rename-remote:<remoto>`, `remove-remote:<remoto>`, `create-tag`,
//!     `tag-details:<tag>`, `checkout-tag:<tag>`, `delete-tag:<tag>`,
//!     `stash-branch`, `stash-pop`, `stash-drop` (sobre el primer stash)
//!   - entrada simulada: `click:x,y`, `dblclick:x,y`, `rclick:x,y`, `move:x,y`,
//!     `key:<tecla>`, `type:<texto>`, `wait:<ms>`

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::time::{Duration, Instant};

use futures::channel::oneshot;
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyWindowHandle, App, Bounds, Context, Entity, InputEvent as _, KeyUpEvent, Keystroke,
    Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PathPromptOptions,
    SharedString, Window, WindowBounds, WindowOptions, div, point, px, size,
};
use mergeon_app::testing::TestBackend;
use mergeon_app::{AddRepository, Answer, AppModel, UiHost};
use mergeon_model::dialogs::{
    ConfirmAnswer, ConfirmOptions, ConfirmValue, PromptAnswer, PromptOptions,
};
use mergeon_model::notify::{NotifyType, Toast, ToastId};
use mergeon_ui::dialog::{self, ConfirmOption, ConfirmVariant};
use mergeon_ui::toast::{self, ToastKind, ToastOptions};
use mergeon_ui::{ActivePaletteExt as _, ThemePreference};
use mergeon_view_sidebar::{
    BranchAction, Section, Sidebar, SidebarCommand, SidebarEvent, SidebarResizer,
};

// ---------------------------------------------------------------------------
// UiHost: avisos, confirmaciones y prompts con los componentes de `mergeon_ui`
// ---------------------------------------------------------------------------

struct GalleryUiHost {
    window: AnyWindowHandle,
    shown: Rc<RefCell<Vec<(ToastId, Instant)>>>,
}

fn text(value: Option<String>) -> SharedString {
    value.unwrap_or_default().into()
}

fn confirm_options(options: &ConfirmOptions) -> dialog::ConfirmOptions {
    dialog::ConfirmOptions {
        title: text(options.title.clone()),
        description: text(options.description.clone()),
        confirm_text: text(options.confirm_text.clone()),
        cancel_text: text(options.cancel_text.clone()),
        dismiss_value: matches!(options.dismiss_value, Some(Some(true))),
        variant: match options.variant.as_deref() {
            Some("destructive") | Some("danger") => ConfirmVariant::Destructive,
            _ => ConfirmVariant::Default,
        },
    }
}

impl UiHost for GalleryUiHost {
    fn notify(&self, toast: Toast, cx: &mut App) {
        let window = self.window;
        let shown = self.shown.clone();
        // El modelo está tomado durante la llamada: se pinta al terminar.
        cx.defer(move |cx| {
            let duration = toast.duration();
            let kind = match toast.kind {
                NotifyType::Success => ToastKind::Success,
                NotifyType::Info => ToastKind::Info,
                NotifyType::Warning => ToastKind::Warning,
                NotifyType::Error => ToastKind::Error,
            };
            let mut options = ToastOptions::new().duration(duration);
            if toast.full_message != toast.title || toast.kind == NotifyType::Error {
                options = options.details(toast.full_message.clone());
            }
            let title = toast.title.clone();
            let result = window.update(cx, |_, window, cx| {
                toast::show(window, cx, kind, &title, options);
            });
            if result.is_ok() {
                let now = Instant::now();
                let mut shown = shown.borrow_mut();
                shown.retain(|(id, until)| *until > now && *id != toast.id);
                shown.push((toast.id, now + duration.min(Duration::from_secs(8))));
            }
        });
    }

    fn visible_toast_ids(&self, _cx: &App) -> Vec<ToastId> {
        let now = Instant::now();
        self.shown
            .borrow()
            .iter()
            .filter(|(_, until)| *until > now)
            .map(|(id, _)| id.clone())
            .collect()
    }

    fn confirm(&self, options: ConfirmOptions, cx: &mut App) -> Answer<ConfirmValue> {
        let (sender, receiver) = oneshot::channel();
        let window = self.window;
        cx.defer(move |cx| {
            let _ = window.update(cx, |_, window, cx| {
                dialog::confirm(window, cx, confirm_options(&options), move |ok, _, _| {
                    let _ = sender.send(Some(ok));
                });
            });
        });
        receiver
    }

    fn confirm_with_option(&self, options: ConfirmOptions, cx: &mut App) -> Answer<ConfirmAnswer> {
        let (sender, receiver) = oneshot::channel();
        let window = self.window;
        cx.defer(move |cx| {
            let option = ConfirmOption {
                label: text(options.option_label.clone()),
                checked: options.option_checked,
            };
            let _ = window.update(cx, |_, window, cx| {
                dialog::confirm_with_option(
                    window,
                    cx,
                    confirm_options(&options),
                    option,
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
            let validate = options
                .pattern
                .as_deref()
                .and_then(|pattern| regex::Regex::new(pattern).ok())
                .map(|pattern| {
                    Rc::new(move |value: &str| pattern.is_match(value)) as dialog::PromptValidator
                });
            let ui_options = dialog::PromptOptions {
                title: text(options.title.clone()),
                description: text(options.description.clone()),
                confirm_text: text(options.confirm_text.clone()),
                cancel_text: text(options.cancel_text.clone()),
                placeholder: text(options.placeholder.clone()),
                default_value: text(options.default_value.clone()),
                validate,
                input_error_message: text(options.input_error_message.clone()),
                textarea: options.textarea,
                rows: options.rows.map(|rows| rows as usize).unwrap_or_default(),
            };
            let _ = window.update(cx, |_, window, cx| {
                dialog::prompt(window, cx, ui_options, move |value, _, _| {
                    let _ = sender.send(PromptAnswer {
                        confirmed: value.is_some(),
                        value: value.map(|value| value.to_string()),
                    });
                });
            });
        });
        receiver
    }

    fn pick_directory(&self, cx: &mut App) -> Answer<Option<PathBuf>> {
        let (sender, receiver) = oneshot::channel();
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: None,
        });
        cx.spawn(async move |_| {
            let path = match picked.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                _ => None,
            };
            let _ = sender.send(path);
        })
        .detach();
        receiver
    }
}

// ---------------------------------------------------------------------------
// Repositorios de demostración
// ---------------------------------------------------------------------------

fn sh(dir: &Path, script: &str) {
    let status = Command::new("bash")
        .arg("-c")
        .arg(script)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Ana Torres")
        .env("GIT_AUTHOR_EMAIL", "ana@example.com")
        .env("GIT_COMMITTER_NAME", "Ana Torres")
        .env("GIT_COMMITTER_EMAIL", "ana@example.com")
        .status();
    match status {
        Ok(status) if status.success() => {}
        other => eprintln!("el paso del fixture falló ({other:?}): {script}"),
    }
}

/// Crea el fixture pedido bajo `root` y devuelve la ruta del repositorio.
fn make_fixture(kind: &str, root: &Path) -> PathBuf {
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tools/make-fixture.sh");
    match kind {
        "empty" => {
            let repo = root.join("scratch");
            std::fs::create_dir_all(&repo).expect("directorio del fixture");
            sh(
                &repo,
                "git init -q -b main . && git config user.name 'Ana Torres' && git config user.email ana@example.com",
            );
            repo
        }
        "submodules" => {
            sh(
                root,
                r##"set -e
for name in ui-kit docs-theme build-tools; do
  git init -q -b main "$name"
  (cd "$name" && echo "# $name" > README.md && git add . && git commit -q -m "Initial commit")
done
git init -q -b main platform
cd platform
git config user.name 'Ana Torres'; git config user.email ana@example.com
echo '# Platform' > README.md && git add . && git commit -q -m 'Initial commit'
for name in ui-kit docs-theme build-tools; do
  git -c protocol.file.allow=always submodule --quiet add "../$name" "vendor/$name"
done
git commit -q -m 'Add submodules'
git submodule --quiet deinit -f vendor/docs-theme
(cd vendor/build-tools && echo change >> README.md && git commit -q -am 'Local change')
"##,
            );
            root.join("platform")
        }
        other => {
            let status = Command::new("bash").arg(script).arg(root).status();
            if !matches!(status, Ok(status) if status.success()) {
                eprintln!("make-fixture.sh falló: {status:?}");
            }
            let repo = root.join("demo");
            match other {
                "detached" => sh(&repo, "git checkout -q --detach HEAD"),
                "behind" => sh(
                    root,
                    r#"set -e
git clone -q origin.git colleague
cd colleague
git config user.name 'Luis Vega'; git config user.email luis@example.com
echo 'export const a = 1' > src/a.js && git add . && git commit -q -m 'Add module a'
echo 'export const b = 2' > src/b.js && git add . && git commit -q -m 'Add module b'
git push -q origin main
cd ../demo && git fetch -q origin
"#,
                ),
                _ => {}
            }
            repo
        }
    }
}

// ---------------------------------------------------------------------------
// Ventana
// ---------------------------------------------------------------------------

/// El sidebar donde lo tiene la app: a la derecha del rail y bajo las barras.
struct Gallery {
    sidebar: Entity<Sidebar>,
    last_event: SharedString,
}

impl Render for Gallery {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        div()
            .flex()
            .size_full()
            .bg(palette.background)
            .text_color(palette.foreground)
            .child(
                // Activity rail.
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
                        // Barra de pestañas, toolbar y pestañas del workspace.
                        div()
                            .h(px(121.))
                            .flex_shrink_0()
                            .border_b_1()
                            .border_color(palette.border),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_h_0()
                            .child(self.sidebar.clone())
                            .child(SidebarResizer::new(&self.sidebar))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .p_4()
                                    .text_xs()
                                    .text_color(palette.muted_foreground)
                                    .child(self.last_event.clone()),
                            ),
                    ),
            )
    }
}

fn section(name: &str) -> Option<Section> {
    Some(match name {
        "starred" => Section::Starred,
        "branches" => Section::Branches,
        "remotes" => Section::Remotes,
        "tags" => Section::Tags,
        "stashes" => Section::Stashes,
        "submodules" => Section::Submodules,
        _ => return None,
    })
}

fn split_remote(value: &str) -> (String, String) {
    let (remote, branch) = value.split_once('/').unwrap_or((value, ""));
    (remote.to_string(), branch.to_string())
}

fn first_stash(cx: &App) -> Option<String> {
    AppModel::global(cx)
        .read(cx)
        .active_repository()
        .and_then(|repo| repo.stashes.first().map(|stash| stash.hash.clone()))
}

fn pointer(kind: &str, value: &str, window: &mut Window, cx: &mut App) {
    let mut numbers = value
        .split(',')
        .filter_map(|number| number.trim().parse::<f32>().ok());
    let (Some(x), Some(y)) = (numbers.next(), numbers.next()) else {
        eprintln!("coordenadas inválidas `{value}`");
        return;
    };
    let position = point(px(x), px(y));
    // Un clic donde ya está el puntero no lo mueve: moverlo rearmaría el
    // tooltip de lo que haya debajo y saldría encima del menú recién abierto.
    if window.mouse_position() != position {
        window.dispatch_event(
            MouseMoveEvent {
                position,
                pressed_button: None,
                modifiers: Modifiers::default(),
            }
            .to_platform_input(),
            cx,
        );
    }
    let (button, clicks) = match kind {
        "click" => (MouseButton::Left, 1),
        "dblclick" => (MouseButton::Left, 2),
        "rclick" => (MouseButton::Right, 1),
        _ => return,
    };
    for click_count in 1..=clicks {
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
}

fn press(keystroke: Keystroke, window: &mut Window, cx: &mut App) {
    window.dispatch_keystroke(keystroke.clone(), cx);
    window.dispatch_event(KeyUpEvent { keystroke }.to_platform_input(), cx);
}

/// Ejecuta un paso de `MERGEON_SIDEBAR_STEPS`.
fn run_step(step: &str, sidebar: &Entity<Sidebar>, window: &mut Window, cx: &mut App) {
    let (kind, value) = step.split_once(':').unwrap_or((step, ""));
    let command = |command: SidebarCommand, window: &mut Window, cx: &mut App| {
        sidebar.update(cx, |sidebar, cx| sidebar.run_command(command, window, cx));
    };

    match kind {
        "filter" => sidebar.update(cx, |sidebar, cx| {
            sidebar.set_filter_text(value, window, cx);
        }),
        "search" => sidebar.update(cx, |sidebar, cx| {
            sidebar.show_search_panel(window, cx);
            sidebar.set_search_query(value, window, cx);
        }),
        "panel" => sidebar.update(cx, |sidebar, cx| {
            if value == "search" {
                sidebar.show_search_panel(window, cx);
            } else {
                sidebar.show_repository_panel(cx);
            }
        }),
        "toggle" => match section(value) {
            Some(section) => sidebar.update(cx, |sidebar, cx| sidebar.toggle_section(section, cx)),
            None => eprintln!("sección desconocida `{value}`"),
        },
        "refresh" => sidebar.update(cx, |sidebar, cx| sidebar.refresh(cx)),
        "view" => sidebar.update(cx, |sidebar, cx| sidebar.view_branch(value, cx)),
        "view-remote" => {
            let (remote, branch) = split_remote(value);
            sidebar.update(cx, |sidebar, cx| {
                sidebar.view_remote_branch(&remote, &branch, cx)
            });
        }
        "view-tag" => sidebar.update(cx, |sidebar, cx| sidebar.view_tag(value, cx)),
        "checkout" => command(
            SidebarCommand::CheckoutBranch(value.to_string()),
            window,
            cx,
        ),
        "checkout-remote" => {
            let (remote, branch) = split_remote(value);
            command(
                SidebarCommand::CheckoutRemoteBranch { remote, branch },
                window,
                cx,
            );
        }
        "branch-action" => {
            let (action, branch) = value.split_once(':').unwrap_or((value, ""));
            let action = match action {
                "merge" => BranchAction::Merge,
                "rebase" => BranchAction::Rebase,
                _ => BranchAction::Compare,
            };
            command(
                SidebarCommand::BranchAction {
                    branch: branch.to_string(),
                    action,
                },
                window,
                cx,
            );
        }
        "drop" => {
            let (source, target) = value.split_once(':').unwrap_or((value, ""));
            sidebar.update(cx, |sidebar, cx| {
                sidebar.drop_branch(source, target, window, cx);
            });
        }
        "new-branch" => command(
            SidebarCommand::NewBranchFromBranch(value.to_string()),
            window,
            cx,
        ),
        "rename-branch" => command(SidebarCommand::RenameBranch(value.to_string()), window, cx),
        "delete-branch" => command(SidebarCommand::DeleteBranch(value.to_string()), window, cx),
        "delete-remote-branch" => {
            let (remote, branch) = split_remote(value);
            command(
                SidebarCommand::DeleteRemoteBranch { remote, branch },
                window,
                cx,
            );
        }
        "add-remote" => command(SidebarCommand::AddRemote, window, cx),
        "edit-remote-url" => command(SidebarCommand::EditRemoteUrl(value.to_string()), window, cx),
        "rename-remote" => command(SidebarCommand::RenameRemote(value.to_string()), window, cx),
        "remove-remote" => command(SidebarCommand::RemoveRemote(value.to_string()), window, cx),
        "create-tag" => command(SidebarCommand::CreateTag, window, cx),
        "tag-details" => command(
            SidebarCommand::ViewTagDetails(value.to_string()),
            window,
            cx,
        ),
        "checkout-tag" => command(SidebarCommand::CheckoutTag(value.to_string()), window, cx),
        "delete-tag" => command(SidebarCommand::DeleteTag(value.to_string()), window, cx),
        "stash-branch" | "stash-pop" | "stash-drop" => {
            let Some(hash) = first_stash(cx) else {
                eprintln!("no hay stashes");
                return;
            };
            command(
                match kind {
                    "stash-branch" => SidebarCommand::BranchFromStash(hash),
                    "stash-pop" => SidebarCommand::PopStash(hash),
                    _ => SidebarCommand::DropStash(hash),
                },
                window,
                cx,
            );
        }
        "click" | "dblclick" | "rclick" | "move" => pointer(kind, value, window, cx),
        "key" => match Keystroke::parse(value) {
            Ok(keystroke) => press(keystroke, window, cx),
            Err(error) => eprintln!("tecla inválida `{value}`: {error}"),
        },
        "type" => {
            for character in value.chars() {
                let text = character.to_string();
                if let Ok(mut keystroke) =
                    Keystroke::parse(if character == ' ' { "space" } else { &text })
                {
                    keystroke.key_char = Some(text);
                    window.dispatch_keystroke(keystroke, cx);
                }
            }
        }
        "wait" => {}
        other => eprintln!("MERGEON_SIDEBAR_STEPS: no sé hacer `{other}`"),
    }
}

fn env_list(name: &str) -> Option<Vec<String>> {
    std::env::var(name).ok().map(|value| {
        value
            .split(',')
            .map(|item| item.trim().to_string())
            .filter(|item| !item.is_empty())
            .collect()
    })
}

fn main() {
    if std::env::var("MERGEON_LOG").is_ok() {
        env_logger::Builder::new().parse_filters("warn").init();
    }
    let preference = ThemePreference::parse(&std::env::var("MERGEON_THEME").unwrap_or_default());
    let fixture = std::env::var("MERGEON_SIDEBAR_FIXTURE").unwrap_or_else(|_| "demo".to_string());
    let steps: Vec<String> = std::env::var("MERGEON_SIDEBAR_STEPS")
        .unwrap_or_default()
        .split(';')
        .map(|step| step.trim().to_string())
        .filter(|step| !step.is_empty())
        .collect();

    // El fixture vive en un temporal que se borra al salir.
    let temp = tempfile::Builder::new()
        .prefix("mergeon-sidebar-gallery-")
        .tempdir()
        .expect("directorio temporal");
    let repo_path = match std::env::var_os("MERGEON_SIDEBAR_REPO") {
        Some(path) => PathBuf::from(path),
        None => make_fixture(&fixture, temp.path()),
    };
    let repo_path = mergeon_gitcmd::paths::canonicalize(&repo_path).unwrap_or(repo_path);
    let repo_path = repo_path.to_string_lossy().into_owned();

    let starred = env_list("MERGEON_SIDEBAR_STARRED").unwrap_or_else(|| {
        if fixture == "demo" || fixture == "behind" {
            vec!["main".to_string(), "feature/search".to_string()]
        } else {
            Vec::new()
        }
    });
    let sections = std::env::var("MERGEON_SIDEBAR_SECTIONS").unwrap_or_else(|_| "all".to_string());

    // Ajustes en memoria: nada de esto toca la configuración del usuario.
    let store = TestBackend::memory_store();
    for branch in &starred {
        let _ = store.add_starred_branch(&repo_path, branch);
    }
    let expanded = |name: &str| match sections.as_str() {
        "all" => true,
        "none" => false,
        "default" => matches!(name, "starred" | "branches"),
        list => list.split(',').any(|item| item.trim() == name),
    };
    let _ = store.update_settings_json(&serde_json::json!({
        "sidebarSections": {
            "starred": expanded("starred"),
            "branches": expanded("branches"),
            "remotes": expanded("remotes"),
            "tags": expanded("tags"),
            "stashes": expanded("stashes"),
            "submodules": expanded("submodules"),
        }
    }));

    gpui_kit::application()
        .with_assets(mergeon_ui::Assets)
        .run(move |cx| {
            mergeon_ui::init(cx);
            mergeon_ui::theme::apply(preference, cx);
            mergeon_view_sidebar::init(cx);

            let (backend, events) = TestBackend::new().with_store(store).build();
            let model = AppModel::init(backend, events, cx);

            let bounds = Bounds::centered(None, size(px(1440.), px(900.)), cx);
            let (window, gallery) = gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                cx,
                |window, cx| {
                    cx.new(|cx| {
                        let sidebar = cx.new(|cx| Sidebar::new(window, cx));
                        cx.subscribe(
                            &sidebar,
                            |this: &mut Gallery, _, event: &SidebarEvent, cx| {
                                this.last_event = format!("{event:?}").into();
                                cx.notify();
                            },
                        )
                        .detach();
                        Gallery {
                            sidebar,
                            last_event: SharedString::default(),
                        }
                    })
                },
            )
            .expect("no se pudo abrir la ventana");

            model.update(cx, |model, cx| {
                model.set_ui_host(
                    Rc::new(GalleryUiHost {
                        window,
                        shown: Rc::default(),
                    }),
                    cx,
                );
            });
            let opened = model.update(cx, |model, cx| {
                model.add_repository(&repo_path, AddRepository::default(), cx)
            });

            let sidebar = gallery.read(cx).sidebar.clone();
            cx.spawn(async move |cx| {
                if let Err(error) = opened.await {
                    eprintln!("no se pudo abrir el repositorio: {error}");
                }
                // La primera lectura del repositorio (ramas, remotos...).
                cx.background_executor()
                    .timer(Duration::from_millis(900))
                    .await;
                // En el display virtual el puntero real queda en el centro de la
                // pantalla y deja su hover sobre lo que haya allí: se aparta.
                let _ = window.update(cx, |_, window, cx| pointer("move", "1,1", window, cx));
                for step in steps {
                    let pause = step
                        .strip_prefix("wait:")
                        .and_then(|ms| ms.parse::<u64>().ok())
                        .unwrap_or(400);
                    let _ = window.update(cx, |_, window, cx| {
                        run_step(&step, &sidebar, window, cx);
                    });
                    cx.background_executor()
                        .timer(Duration::from_millis(pause))
                        .await;
                }
            })
            .detach();
        });
    // El temporal vive hasta que la app termina.
    drop(temp);
}
