//! Galería de la vista de commits: la vista real (y la pestaña de archivo
//! histórico) sobre un `AppModel` real y un repositorio de demostración, con
//! el sidebar real a su izquierda para que las capturas se comparen con
//! `native/target-shots/reference/` píxel a píxel.
//!
//! ```bash
//! cargo run -p mergeon-view-commits --example commits_gallery
//! MERGEON_THEME=light MERGEON_COMMITS_STEPS="select:Initial commit" cargo run -p mergeon-view-commits --example commits_gallery
//! ```
//!
//! - `MERGEON_THEME=dark|light|system` (por defecto `dark`).
//! - `MERGEON_COMMITS_FIXTURE=demo|website|api|empty` (por defecto `demo`):
//!   `demo` es el de `tools/make-fixture.sh`; `website` trae un commit que
//!   reindenta y un binario; `api` es una historia lineal.
//! - `MERGEON_COMMITS_REPO=<ruta>`: abre ese repositorio en vez de un fixture.
//! - `MERGEON_COMMITS_SETUP=<bash>`: se ejecuta en el repositorio antes de
//!   abrirlo (crear ramas, traer commits de un remoto...).
//! - `MERGEON_HISTORY_WIDTH=<px>`: ancho guardado del panel de historial.
//! - `MERGEON_COMMITS_STEPS=`: pasos separados por `;`, uno cada 400 ms, cuando
//!   el historial ya cargó:
//!   - `select:<mensaje>` (clic en la fila cuyo mensaje empieza así),
//!     `row-dclick:<mensaje>`, `row-rclick:<mensaje>` (abre su menú)
//!   - `command:<acción>:<mensaje>`: `checkout`, `create-branch`, `create-tag`,
//!     `squash`, `reword`, `drop`, `rebase`, `cherry-pick`, `revert`,
//!     `reset-mixed`, `reset-hard`, `copy-sha`
//!   - `expand` (despliega la cabecera), `repo-tree`, `file:<ruta>` (abre el
//!     archivo del commit seleccionado en su pestaña), `commits` (vuelve a la
//!     pestaña Commits), `files:off|on` (lista de la pestaña de archivo)
//!   - `tree:<ruta>` (clic en una entrada del árbol de la pestaña activa)
//!   - `split`, `ignore-spaces` (conmutan las preferencias de diff)
//!   - `rebase-action:<fila>:<acción>`, `rebase-message:<fila>:<texto>`
//!   - `view:<rama>` (clic en una rama del sidebar)
//!   - entrada simulada: `click:x,y`, `dblclick:x,y`, `rclick:x,y`, `move:x,y`,
//!     `key:<tecla>`, `type:<texto>`, `wait:<ms>`

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::time::{Duration, Instant};

use futures::channel::oneshot;
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyWindowHandle, App, Bounds, Context, Entity, InputEvent as _, KeyUpEvent, Keystroke,
    Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PathPromptOptions,
    Pixels, Point, SharedString, Subscription, Window, WindowBounds, WindowOptions, div, point, px,
    size,
};
use mergeon_app::testing::TestBackend;
use mergeon_app::{AddRepository, Answer, AppEvent, AppModel, UiHost};
use mergeon_model::dialogs::{
    ConfirmAnswer, ConfirmOptions, ConfirmValue, PromptAnswer, PromptOptions,
};
use mergeon_model::notify::{NotifyType, Toast, ToastId};
use mergeon_model::workspace_tabs::{CHANGES_TAB, FileTab, HOME_TAB};
use mergeon_ui::dialog::{self, ConfirmOption, ConfirmVariant};
use mergeon_ui::theme::with_alpha;
use mergeon_ui::toast::{self, ToastKind, ToastOptions};
use mergeon_ui::{ActivePaletteExt as _, IconTone, Lucide, ThemePreference};
use mergeon_view_commits::{
    CommitCommand, CommitsView, CommitsViewEvent, HistoricalFileTabView, RebaseAction,
};
use mergeon_view_sidebar::{Sidebar, SidebarEvent, SidebarResizer};

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

/// Las utilidades de bash que comparten los fixtures pequeños: `commit
/// <mensaje> [autor]` con fechas fijas, como `reference-shots.mjs`.
const HELPERS: &str = r#"set -e
CLOCK=0
commit() {
  CLOCK=$((CLOCK + 1))
  local day; day=$(printf '%02d' $((10 + CLOCK / 4)))
  local stamp="2026-09-${day}T09:$(printf '%02d' $((CLOCK * 7 % 60))):00+00:00"
  local name="${2:-Ana Torres}"; local mail; mail="$(echo "$name" | tr 'A-Z ' 'a-z.')@example.com"
  git add --all
  GIT_AUTHOR_DATE="$stamp" GIT_COMMITTER_DATE="$stamp" GIT_AUTHOR_NAME="$name" GIT_AUTHOR_EMAIL="$mail" \
    git commit -q -m "$1"
}
git init -q -b main .
git config user.name 'Ana Torres'; git config user.email ana@example.com; git config commit.gpgsign false
"#;

const WEBSITE: &str = r##"
printf '# Website\n\nThe public landing page.\n\n## Develop\n\nOpen `index.html` in a browser.\n' > README.md
cat > index.html <<'X'
<!doctype html>
<html lang="en">
<head>
  <title>Acme</title>
  <link rel="stylesheet" href="styles.css" />
</head>
<body>
<nav>
  <a href="/">Home</a>
  <a href="/pricing">Pricing</a>
  <a href="/docs">Docs</a>
</nav>
<main>
  <h1>Ship faster</h1>
  <p>Everything your team needs to release.</p>
</main>
</body>
</html>
X
printf 'body {\n  margin: 0;\n}\n\nnav a {\n  padding: 8px;\n}\n' > styles.css
mkdir -p assets scripts
python3 -c "import sys; sys.stdout.buffer.write(bytes([0x89,0x50,0x4e,0x47,0x0d,0x0a,0x1a,0x0a] + [(i*37)%256 for i in range(480)]))" > assets/logo.png
commit 'Scaffold the landing page'
printf "export function highlight(path) {\n  document.querySelector(\`a[href='\${path}']\`)?.classList.add('active')\n}\n" > scripts/nav.js
commit 'Add navigation script' 'Marta Ruiz'
perl -0pi -e 's|<nav>\n  <a href="/">Home</a>\n  <a href="/pricing">Pricing</a>\n  <a href="/docs">Docs</a>\n</nav>|<header>\n  <nav>\n    <a href="/">Home</a>\n    <a href="/plans">Plans</a>\n    <a href="/docs">Docs</a>\n  </nav>\n</header>|' index.html
commit 'Wrap the nav in a header and rename the pricing link' 'Luis Vega'
git tag launch
perl -0pi -e 's|Everything your team needs to release.|Release with confidence.|' index.html
commit 'Tighten the hero copy'
git checkout -q -b redesign
perl -0pi -e 's|margin: 0;|margin: 0;\n  font-family: Inter, sans-serif;|' styles.css
commit 'Try the new typeface' 'Marta Ruiz'
git checkout -q main
"##;

const API: &str = r##"
printf '{\n  "name": "api-server",\n  "version": "0.3.0"\n}\n' > package.json
mkdir -p src/routes
printf "import { createServer } from 'node:http'\n\ncreateServer((_request, response) => response.end('ok')).listen(3000)\n" > src/server.js
commit 'Bootstrap the HTTP server'
printf "export const health = () => ({ status: 'ok' })\n" > src/routes/health.js
commit 'Add health route' 'Luis Vega'
perl -pi -e 's/0\.3\.0/0.4.0/' package.json
commit 'Bump version to 0.4.0'
printf 'export const users = () => []\n' > src/routes/users.js
commit 'Add users route' 'Marta Ruiz'
perl -pi -e "s/\[\]/[{ id: 1, name: 'Ana' }]/" src/routes/users.js
commit 'Return a sample user' 'Marta Ruiz'
perl -pi -e "s/'ok'/'healthy'/" src/routes/health.js
commit 'Report healthy instead of ok'
perl -pi -e 's/listen\(3000\)/listen(process.env.PORT || 3000)/' src/server.js
"##;

/// Crea el fixture pedido bajo `root` y devuelve la ruta del repositorio.
fn make_fixture(kind: &str, root: &Path) -> PathBuf {
    let small = |name: &str, script: &str| {
        let repo = root.join(name);
        std::fs::create_dir_all(&repo).expect("directorio del fixture");
        sh(&repo, &format!("{HELPERS}{script}"));
        repo
    };
    match kind {
        "website" => small("website", WEBSITE),
        "api" => small("api-server", API),
        "empty" => small("scratch", ""),
        _ => {
            let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tools/make-fixture.sh");
            let status = Command::new("bash")
                .arg(script)
                .arg(root)
                .stdout(std::process::Stdio::null())
                .status();
            if !matches!(status, Ok(status) if status.success()) {
                eprintln!("make-fixture.sh falló: {status:?}");
            }
            root.join("demo")
        }
    }
}

// ---------------------------------------------------------------------------
// Ventana: el espacio de trabajo del repositorio, como lo monta la cáscara
// ---------------------------------------------------------------------------

struct Gallery {
    model: Entity<AppModel>,
    repo_id: Option<String>,
    sidebar: Entity<Sidebar>,
    commits: Option<Entity<CommitsView>>,
    file_tabs: HashMap<String, Entity<HistoricalFileTabView>>,
    files_open: bool,
    _subscriptions: Vec<Subscription>,
}

impl Gallery {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let model = AppModel::global(cx);
        let sidebar = cx.new(|cx| Sidebar::new(window, cx));
        let subscriptions = vec![
            cx.observe(&model, |_, _, cx| cx.notify()),
            cx.subscribe(&sidebar, |this, _, event: &SidebarEvent, cx| {
                if let SidebarEvent::CommitSelectionRequested { request, .. } = event
                    && let Some(commits) = &this.commits
                {
                    commits.update(cx, |view, cx| {
                        view.request_commit_selection(&request.hash, cx)
                    });
                }
            }),
            cx.subscribe(&model, |this, _, event: &AppEvent, _| {
                // El panel de una pestaña reutilizada sigue montado.
                if let AppEvent::WorkspaceTabReplaced { from, to, .. } = event
                    && let Some(view) = this.file_tabs.remove(from)
                {
                    this.file_tabs.insert(to.clone(), view);
                }
            }),
        ];
        Self {
            model,
            repo_id: None,
            sidebar,
            commits: None,
            file_tabs: HashMap::new(),
            files_open: true,
            _subscriptions: subscriptions,
        }
    }

    fn attach(&mut self, repo_id: String, window: &mut Window, cx: &mut Context<Self>) {
        let commits = cx.new(|cx| CommitsView::new(repo_id.clone(), window, cx));
        let sidebar = self.sidebar.clone();
        self._subscriptions.push(cx.subscribe(
            &commits,
            move |_, _, event: &CommitsViewEvent, cx| {
                if let CommitsViewEvent::CommitPicked { repo_id, .. } = event {
                    sidebar.update(cx, |sidebar, cx| sidebar.clear_viewed_ref(repo_id, cx));
                }
            },
        ));
        self.commits = Some(commits);
        self.repo_id = Some(repo_id);
        cx.notify();
    }

    fn repo_path(&self, cx: &App) -> Option<String> {
        let repo_id = self.repo_id.as_deref()?;
        self.model
            .read(cx)
            .repository(repo_id)
            .map(|repo| repo.path.clone())
    }

    fn file_tab_view(
        &mut self,
        tab: &FileTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<HistoricalFileTabView>> {
        let repo_id = self.repo_id.clone()?;
        let files_open = self.files_open;
        let view = self
            .file_tabs
            .entry(tab.id.clone())
            .or_insert_with(|| {
                cx.new(|cx| HistoricalFileTabView::new(repo_id, tab.id.clone(), window, cx))
            })
            .clone();
        view.update(cx, |view, cx| view.set_files_open(files_open, cx));
        Some(view)
    }

    /// La tira de pestañas del espacio de trabajo (`RepositoryWorkspace.vue`),
    /// solo para la captura.
    fn render_workspace_tabs(&self, active: &str, tabs: &[FileTab], cx: &App) -> impl IntoElement {
        let palette = *cx.palette();
        let changes = self
            .repo_id
            .as_deref()
            .and_then(|id| self.model.read(cx).repository(id))
            .map_or(0, |repo| repo.changes_count);
        let indicator = || {
            div()
                .absolute()
                .left_0()
                .right_0()
                .bottom_0()
                .h(px(2.))
                .bg(palette.primary)
        };
        let fixed = |label: &'static str, icon: Lucide, is_active: bool, count: usize| {
            div()
                .relative()
                .flex()
                .items_center()
                .gap(px(7.))
                .px_3()
                .pt_2()
                .pb(px(10.))
                .text_size(px(12.))
                .border_r_1()
                .border_color(palette.border)
                .text_color(if is_active {
                    palette.foreground
                } else {
                    palette.muted_foreground
                })
                .when(is_active, |tab| {
                    tab.bg(palette.background).child(indicator())
                })
                .child(mergeon_ui::icon(icon).size(px(15.)).tone(IconTone::Current))
                .child(label)
                .when(count > 0, |tab| {
                    tab.child(
                        div()
                            .min_w(px(18.))
                            .px(px(5.))
                            .rounded(px(6.))
                            .bg(with_alpha(palette.primary, 0.14))
                            .text_color(palette.primary_text)
                            .text_size(px(11.))
                            .text_center()
                            .child(count.to_string()),
                    )
                })
        };
        div()
            .flex()
            .flex_shrink_0()
            .min_h(px(37.))
            .border_b_1()
            .border_color(palette.border)
            .bg(palette.card)
            .child(fixed(
                "Commits",
                Lucide::GitCommitHorizontal,
                active == HOME_TAB,
                0,
            ))
            .child(fixed(
                "Local Changes",
                Lucide::FilePenLine,
                active == CHANGES_TAB,
                changes,
            ))
            .children(tabs.iter().map(|tab| {
                let is_active = active == tab.id;
                let icon = if tab.path.is_empty() {
                    Lucide::Folders
                } else {
                    Lucide::FileCodeCorner
                };
                div()
                    .relative()
                    .flex()
                    .items_center()
                    .border_r_1()
                    .border_color(palette.border)
                    .when(is_active, |el| el.bg(palette.background).child(indicator()))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(7.))
                            .px_3()
                            .py_2()
                            .text_size(px(12.))
                            .text_color(if is_active {
                                palette.foreground
                            } else {
                                palette.muted_foreground
                            })
                            .child(mergeon_ui::icon(icon).size(px(14.)).tone(IconTone::Current))
                            .child(tab.label().to_string())
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(palette.muted_foreground)
                                    .child(tab.context()),
                            ),
                    )
                    .child(
                        div().px_2().child(
                            mergeon_ui::icon(Lucide::X)
                                .size(px(13.))
                                .tone(IconTone::Current),
                        ),
                    )
            }))
    }
}

impl Render for Gallery {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let workspace = self
            .repo_path(cx)
            .map(|path| self.model.read(cx).repository_workspace(&path))
            .unwrap_or_default();
        let active = workspace.active_tab.clone();
        let sidebar_width = self.model.read(cx).sidebar_width().width;

        let body = if active == HOME_TAB || active == CHANGES_TAB {
            div()
                .flex()
                .flex_1()
                .min_h_0()
                .child(
                    div()
                        .flex_shrink_0()
                        .h_full()
                        .w(px(sidebar_width as f32))
                        .child(self.sidebar.clone()),
                )
                .child(SidebarResizer::new(&self.sidebar))
                .children(self.commits.clone())
                .into_any_element()
        } else if let Some(tab) = workspace.tabs.iter().find(|tab| tab.id == active).cloned() {
            let title = if tab.path.is_empty() {
                tab.label().to_string()
            } else {
                tab.path.clone()
            };
            let view = self.file_tab_view(&tab, window, cx);
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .child(
                    div()
                        .id("file-context")
                        .flex()
                        .flex_shrink_0()
                        .items_center()
                        .gap(px(10.))
                        .px_3()
                        .py(px(6.))
                        .border_b_1()
                        .border_color(palette.border)
                        .child(
                            div()
                                .id("toggle-file-list")
                                .cursor_pointer()
                                .child(mergeon_ui::icon(Lucide::PanelLeft).size(px(16.)))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.files_open = !this.files_open;
                                    cx.notify();
                                })),
                        )
                        .child(div().flex_1().text_size(px(12.)).child(title))
                        .child(
                            div()
                                .text_size(px(10.))
                                .text_color(palette.muted_foreground)
                                .child(tab.context()),
                        ),
                )
                .children(view)
                .into_any_element()
        } else {
            div().flex_1().into_any_element()
        };

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
                        // Barra de pestañas de la app y toolbar.
                        div()
                            .h(px(84.))
                            .flex_shrink_0()
                            .border_b_1()
                            .border_color(palette.border),
                    )
                    .child(self.render_workspace_tabs(&active, &workspace.tabs, cx))
                    .child(body),
            )
    }
}

// ---------------------------------------------------------------------------
// Pasos
// ---------------------------------------------------------------------------

fn pointer_at(kind: &str, position: Point<Pixels>, window: &mut Window, cx: &mut App) {
    window.dispatch_event(
        MouseMoveEvent {
            position,
            pressed_button: None,
            modifiers: Modifiers::default(),
        }
        .to_platform_input(),
        cx,
    );
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

fn pointer(kind: &str, value: &str, window: &mut Window, cx: &mut App) {
    let mut numbers = value
        .split(',')
        .filter_map(|number| number.trim().parse::<f32>().ok());
    let (Some(x), Some(y)) = (numbers.next(), numbers.next()) else {
        eprintln!("coordenadas inválidas `{value}`");
        return;
    };
    pointer_at(kind, point(px(x), px(y)), window, cx);
}

fn press(keystroke: Keystroke, window: &mut Window, cx: &mut App) {
    window.dispatch_keystroke(keystroke.clone(), cx);
    window.dispatch_event(KeyUpEvent { keystroke }.to_platform_input(), cx);
}

fn command(name: &str) -> Option<CommitCommand> {
    Some(match name {
        "checkout" => CommitCommand::Checkout,
        "create-branch" => CommitCommand::CreateBranch,
        "create-tag" => CommitCommand::CreateTag,
        "squash" => CommitCommand::Squash,
        "reword" => CommitCommand::Reword,
        "drop" => CommitCommand::Drop,
        "rebase" => CommitCommand::InteractiveRebase,
        "cherry-pick" => CommitCommand::CherryPick,
        "revert" => CommitCommand::Revert,
        "reset-mixed" => CommitCommand::ResetMixed,
        "reset-hard" => CommitCommand::ResetHard,
        "copy-sha" => CommitCommand::CopySha,
        _ => return None,
    })
}

fn run_step(step: &str, gallery: &Entity<Gallery>, window: &mut Window, cx: &mut App) {
    let (kind, value) = step.split_once(':').unwrap_or((step, ""));
    let Some(commits) = gallery.read(cx).commits.clone() else {
        eprintln!("el repositorio no está abierto");
        return;
    };
    let commit_by_message = |message: &str, cx: &App| {
        commits
            .read(cx)
            .commits()
            .iter()
            .find(|commit| commit.message.starts_with(message))
            .cloned()
    };
    let active_file_tab = |cx: &App| {
        let gallery = gallery.read(cx);
        let path = gallery.repo_path(cx)?;
        let active = gallery
            .model
            .read(cx)
            .repository_workspace(&path)
            .active_tab;
        gallery.file_tabs.get(&active).cloned()
    };

    match kind {
        "select" | "row-dclick" | "row-rclick" | "row-click" => {
            let Some(commit) = commit_by_message(value, cx) else {
                eprintln!("no hay commit `{value}`");
                return;
            };
            if kind == "select" {
                commits.update(cx, |view, cx| view.handle_history_select(commit, cx));
                return;
            }
            let list = commits.read(cx).history_list().clone();
            let Some(bounds) = list.read(cx).row_bounds(&commit.full_hash) else {
                return;
            };
            let target = point(bounds.left() + px(120.), bounds.center().y);
            let action = match kind {
                "row-dclick" => "dblclick",
                "row-rclick" => "rclick",
                _ => "click",
            };
            pointer_at(action, target, window, cx);
        }
        "command" => {
            let (name, message) = value.split_once(':').unwrap_or((value, ""));
            let (Some(command), Some(commit)) = (command(name), commit_by_message(message, cx))
            else {
                eprintln!("orden inválida `{value}`");
                return;
            };
            commits.update(cx, |view, cx| {
                view.run_command(command, &commit, window, cx)
            });
        }
        "expand" => commits.update(cx, |view, cx| view.toggle_summary(cx)),
        "repo-tree" => commits.update(cx, |view, cx| view.open_repo_tree(cx)),
        "file" => {
            let file = commits
                .read(cx)
                .commit_files()
                .iter()
                .find(|file| file.path == value || file.path.ends_with(value))
                .cloned();
            match file {
                Some(file) => commits.update(cx, |view, cx| view.select_file(file, cx)),
                None => eprintln!("el commit no tiene `{value}`"),
            }
        }
        "commits" => {
            if let Some(path) = gallery.read(cx).repo_path(cx) {
                let model = gallery.read(cx).model.clone();
                model.update(cx, |model, cx| {
                    model.activate_workspace_tab(&path, HOME_TAB, cx)
                });
            }
        }
        "files" => gallery.update(cx, |gallery, cx| {
            gallery.files_open = value != "off";
            cx.notify();
        }),
        "tree" => match active_file_tab(cx) {
            Some(tab) => {
                let tree = tab.read(cx).tree().clone();
                tree.update(cx, |tree, cx| tree.activate(value, cx));
            }
            None => eprintln!("no hay pestaña de árbol activa"),
        },
        "split" => {
            let model = gallery.read(cx).model.clone();
            model.update(cx, |model, cx| model.toggle_diff_view(cx).detach());
        }
        "ignore-spaces" => {
            let model = gallery.read(cx).model.clone();
            model.update(cx, |model, cx| model.toggle_ignore_whitespace(cx).detach());
        }
        "rebase-action" | "rebase-message" => {
            let Some(dialog) = commits.read(cx).interactive_rebase_dialog().cloned() else {
                eprintln!("no hay rebase interactivo abierto");
                return;
            };
            let (row, rest) = value.split_once(':').unwrap_or((value, ""));
            let Ok(row) = row.parse::<usize>() else {
                return;
            };
            dialog.update(cx, |dialog, cx| {
                if kind == "rebase-action" {
                    if let Some(action) = RebaseAction::parse(rest) {
                        dialog.set_action(row, action, window, cx);
                    }
                } else {
                    dialog.set_message(row, rest, window, cx);
                }
            });
        }
        "view" => {
            let sidebar = gallery.read(cx).sidebar.clone();
            sidebar.update(cx, |sidebar, cx| sidebar.view_branch(value, cx));
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
        other => eprintln!("MERGEON_COMMITS_STEPS: no sé hacer `{other}`"),
    }
}

fn main() {
    if std::env::var("MERGEON_LOG").is_ok() {
        env_logger::Builder::new().parse_filters("warn").init();
    }
    let preference = ThemePreference::parse(&std::env::var("MERGEON_THEME").unwrap_or_default());
    let fixture = std::env::var("MERGEON_COMMITS_FIXTURE").unwrap_or_else(|_| "demo".to_string());
    let steps: Vec<String> = std::env::var("MERGEON_COMMITS_STEPS")
        .unwrap_or_default()
        .split(';')
        .map(|step| step.trim().to_string())
        .filter(|step| !step.is_empty())
        .collect();

    // El fixture vive en un temporal que se borra al salir.
    let temp = tempfile::Builder::new()
        .prefix("mergeon-commits-gallery-")
        .tempdir()
        .expect("directorio temporal");
    let repo_path = match std::env::var_os("MERGEON_COMMITS_REPO") {
        Some(path) => PathBuf::from(path),
        None => make_fixture(&fixture, temp.path()),
    };
    let repo_path = mergeon_gitcmd::paths::canonicalize(&repo_path).unwrap_or(repo_path);
    if let Ok(setup) = std::env::var("MERGEON_COMMITS_SETUP") {
        sh(&repo_path, &setup);
    }
    let repo_path = repo_path.to_string_lossy().into_owned();

    // Ajustes en memoria: nada de esto toca la configuración del usuario.
    let store = TestBackend::memory_store();
    if fixture == "demo" {
        for branch in ["main", "feature/search"] {
            let _ = store.add_starred_branch(&repo_path, branch);
        }
    }
    if let Some(width) = std::env::var("MERGEON_HISTORY_WIDTH")
        .ok()
        .and_then(|width| width.parse::<f64>().ok())
    {
        let _ = store.update_settings_json(&serde_json::json!({
            "commitView": { "historyWidth": width }
        }));
    }
    let _ = store.update_settings_json(&serde_json::json!({
        "sidebarSections": {
            "starred": true, "branches": true, "remotes": true,
            "tags": true, "stashes": true, "submodules": true,
        }
    }));

    gpui_kit::application()
        .with_assets(mergeon_ui::Assets)
        .run(move |cx| {
            mergeon_ui::init(cx);
            mergeon_ui::theme::apply(preference, cx);
            mergeon_view_sidebar::init(cx);
            mergeon_view_commits::init(cx);

            let (backend, events) = TestBackend::new().with_store(store).build();
            let model = AppModel::init(backend, events, cx);

            let bounds = Bounds::centered(None, size(px(1440.), px(900.)), cx);
            let (window, gallery) = gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                cx,
                |window, cx| cx.new(|cx| Gallery::new(window, cx)),
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

            cx.spawn(async move |cx| {
                if let Err(error) = opened.await {
                    eprintln!("no se pudo abrir el repositorio: {error}");
                }
                let hydrate = model.update(cx, |model, cx| model.hydrate_opened_repository(cx));
                hydrate.await;
                let repo_id = model.read_with(cx, |model, _| {
                    model.active_repository().map(|repo| repo.id.clone())
                });
                if let Some(repo_id) = repo_id {
                    let _ = window.update(cx, |_, window, cx| {
                        gallery.update(cx, |gallery, cx| gallery.attach(repo_id, window, cx));
                    });
                }
                // La primera carga del historial y de los detalles.
                cx.background_executor()
                    .timer(Duration::from_millis(1200))
                    .await;
                for step in steps {
                    let pause = step
                        .strip_prefix("wait:")
                        .and_then(|ms| ms.parse::<u64>().ok())
                        .unwrap_or(400);
                    let _ = window.update(cx, |_, window, cx| {
                        run_step(&step, &gallery, window, cx);
                    });
                    cx.background_executor()
                        .timer(Duration::from_millis(pause))
                        .await;
                }
            })
            .detach();
        });
    drop(temp);
}
