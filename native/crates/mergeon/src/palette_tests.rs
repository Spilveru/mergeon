//! Tests de la paleta de comandos: el registro (cada comando hace lo suyo
//! con `TestBackend`), el teclado, ramas y archivos sobre repositorios git
//! reales, el repositorio de destino y el tamaño (5.000 ramas, 20.000
//! archivos) con la lista virtual.

use std::cell::RefCell;
use std::path::Path;
use std::process::Command as Process;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::component::TitleBar;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Entity, Subscription, TestAppContext, Window,
    WindowDecorations, WindowOptions,
};
use gpui_kit::{ParentElement as _, Styled as _};
use mergeon_app::shell::DialogRequest;
use mergeon_app::testing::{RecordingLauncher, RecordingUiHost, TestBackend};
use mergeon_app::{AddRepository, AppEvent, AppModel};
use mergeon_model::command_palette::PaletteKind;
use mergeon_model::keyboard_shortcuts::ShortcutAction;
use mergeon_model::workspace_tabs::{FileTabDescriptor, WorkspaceSection};

use crate::commands::{self, COMMANDS, Command, CommandContext, Picker};
use crate::file_index::FileList;
use crate::palette::{CommandPalette, PaletteAction, PaletteEntry, RESULT_LIMIT};
use crate::shell::AppShell;
use crate::shell::PaletteTarget;

const PATIENCE: Duration = Duration::from_secs(20);

fn git(dir: &Path, args: &[&str]) {
    let status = Process::new("git")
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.com"])
        .args([
            "-c",
            "init.defaultBranch=main",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(dir)
        .status()
        .expect("git");
    assert!(status.success(), "git {args:?}");
}

/// Un repositorio con un commit en `main` y una rama `feature`.
fn repository(name: &str) -> tempfile::TempDir {
    let dir = tempfile::Builder::new().prefix(name).tempdir().unwrap();
    let root = dir.path();
    git(root, &["init", "-q"]);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("README.md"), "hola\n").unwrap();
    std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-q", "-m", "first"]);
    git(root, &["branch", "feature"]);
    dir
}

/// Repite `run_until_parked` hasta que se cumpla (el backend trabaja en sus
/// propios hilos).
fn wait_until(
    cx: &mut TestAppContext,
    what: &str,
    mut condition: impl FnMut(&mut TestAppContext) -> bool,
) {
    let deadline = Instant::now() + PATIENCE;
    loop {
        cx.run_until_parked();
        if condition(cx) {
            return;
        }
        assert!(Instant::now() < deadline, "no llegó: {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

struct Harness {
    model: Entity<AppModel>,
    host: Rc<RecordingUiHost>,
    launcher: Arc<RecordingLauncher>,
    events: Rc<RefCell<Vec<AppEvent>>>,
    window: AnyWindowHandle,
    shell: Option<Entity<AppShell>>,
    _subscription: Subscription,
}

impl Harness {
    /// El modelo sobre `TestBackend` y una ventana: con la cáscara entera
    /// (`with_shell`) o vacía.
    fn new(cx: &mut TestAppContext, with_shell: bool) -> Self {
        Self::with_backend(cx, with_shell, TestBackend::new())
    }

    /// Como [`Harness::new`], sobre un backend ya preparado (store, GitHub).
    fn with_backend(cx: &mut TestAppContext, with_shell: bool, mut backend: TestBackend) -> Self {
        cx.executor().allow_parking();
        let launcher = backend.launcher.clone();
        let (backend, events) = backend.build();
        let model = cx.update(|cx| {
            crate::init_bindings(cx);
            cx.set_reduce_motion(true);
            AppModel::init(backend, events, cx)
        });
        let host = RecordingUiHost::new();
        model.update(cx, |model, cx| model.set_ui_host(host.clone(), cx));
        let recorded = Rc::new(RefCell::new(Vec::new()));
        let subscription = cx.update(|cx| {
            let recorded = recorded.clone();
            cx.subscribe(&model, move |_, event: &AppEvent, _| {
                recorded.borrow_mut().push(event.clone());
            })
        });

        let mut shell = None;
        let window = cx.update(|cx| {
            let options = WindowOptions {
                window_decorations: Some(WindowDecorations::Client),
                ..TitleBar::window_options()
            };
            if with_shell {
                let (window, view) = gpui_kit::open_window(options, cx, |window, cx| {
                    cx.new(|cx| AppShell::new(window, cx))
                })
                .expect("ventana de prueba");
                shell = Some(view);
                window
            } else {
                struct Empty;
                impl gpui_kit::Render for Empty {
                    fn render(
                        &mut self,
                        _: &mut Window,
                        _: &mut gpui_kit::Context<Self>,
                    ) -> impl gpui_kit::IntoElement {
                        gpui_kit::div()
                    }
                }
                gpui_kit::open_window(options, cx, |_, cx| cx.new(|_| Empty))
                    .expect("ventana de prueba")
                    .0
            }
        });
        cx.run_until_parked();
        Self {
            model,
            host,
            launcher,
            events: recorded,
            window,
            shell,
            _subscription: subscription,
        }
    }

    async fn open(&self, dir: &tempfile::TempDir, cx: &mut TestAppContext) -> String {
        let path = dir.path().to_string_lossy().into_owned();
        self.model
            .update(cx, |model, cx| {
                model.add_repository(&path, AddRepository::default(), cx)
            })
            .await
            .expect("add repository");
        let model = self.model.clone();
        wait_until(cx, "ramas leídas", |cx| {
            model.read_with(cx, |model, _| {
                model
                    .active_repository()
                    .is_some_and(|repo| repo.branches.len() >= 2)
            })
        });
        self.model
            .read_with(cx, |model, _| model.active_repository().unwrap().id.clone())
    }

    fn in_window<R>(
        &self,
        cx: &mut TestAppContext,
        f: impl FnOnce(&mut Window, &mut gpui_kit::App) -> R,
    ) -> R {
        let result = cx
            .update_window(self.window, |_, window, cx| {
                window.render_frame(cx);
                f(window, cx)
            })
            .expect("ventana");
        cx.run_until_parked();
        result
    }

    fn palette(&self, cx: &mut TestAppContext) -> Option<Entity<CommandPalette>> {
        let shell = self.shell.clone()?;
        cx.update(|cx| shell.read(cx).command_palette().cloned())
    }

    fn labels(&self, cx: &mut TestAppContext) -> Vec<String> {
        let palette = self.palette(cx).expect("paleta abierta");
        cx.update(|cx| {
            palette
                .read(cx)
                .result_labels()
                .into_iter()
                .map(|(label, _)| label.to_string())
                .collect()
        })
    }

    fn open_palette(&self, cx: &mut TestAppContext) {
        self.in_window(cx, |window, cx| window.press("secondary-shift-p", cx));
        assert!(self.palette(cx).is_some(), "Ctrl+Shift+P abre la paleta");
    }

    fn type_text(&self, cx: &mut TestAppContext, text: &str) {
        self.in_window(cx, |window, cx| window.input(text, cx));
        let palette = self.palette(cx).expect("paleta abierta");
        wait_until(cx, "resultados", |cx| {
            cx.update(|cx| !palette.read(cx).is_ranking())
        });
    }
}

/// Todo lo que puede cambiar al ejecutar un comando.
#[derive(Debug, Clone, PartialEq)]
struct Observed {
    events: usize,
    toasts: usize,
    confirms: usize,
    prompts: usize,
    launched: usize,
    active: Option<usize>,
    open: usize,
    /// Pestaña activa del repositorio activo y cuántos documentos tiene.
    active_tab: Option<String>,
    documents: usize,
    view: WorkspaceSection,
    preferences: bool,
    section: String,
    dark: bool,
}

fn observe(harness: &Harness, cx: &mut TestAppContext) -> Observed {
    let (active, open, active_tab, documents, view, preferences, section, dark) =
        harness.model.read_with(cx, |model, _| {
            let workspace = model
                .active_repository()
                .map(|repo| model.repository_workspace(&repo.path));
            (
                model.active_repository_index(),
                model.open_repositories().len(),
                workspace
                    .as_ref()
                    .map(|workspace| workspace.active_tab.clone()),
                workspace.map_or(0, |workspace| workspace.tabs.len()),
                model.active_view(),
                model.show_preferences(),
                model.settings_section().to_string(),
                model.is_dark(),
            )
        });
    Observed {
        events: harness.events.borrow().len(),
        toasts: harness.host.toasts().len(),
        confirms: harness.host.confirms().len(),
        prompts: harness.host.prompts().len(),
        launched: harness.launcher.launched().len(),
        active,
        open,
        active_tab,
        documents,
        view,
        preferences,
        section,
        dark,
    }
}

/// Cada entrada del registro hace algo visible (un diálogo pedido, un
/// aviso, una pregunta, un programa lanzado, otra vista u otra pestaña) y,
/// para las que lo tienen, lo mismo que su atajo.
#[gpui_kit::test]
async fn every_registered_command_runs_its_action(cx: &mut TestAppContext) {
    let harness = Harness::new(cx, false);
    let first = repository("palette-a");
    let second = repository("palette-b");
    // Algo marcado para "Stash selected files…".
    std::fs::write(second.path().join("staged.txt"), "nuevo\n").unwrap();
    git(second.path(), &["add", "staged.txt"]);
    harness.open(&first, cx).await;
    harness.open(&second, cx).await;
    // Algo que guardar en un stash.
    std::fs::write(second.path().join("README.md"), "cambio\n").unwrap();
    let model = harness.model.clone();
    wait_until(cx, "lo preparado", |cx| {
        model.read_with(cx, |model, _| CommandContext::read(model).has_staged)
    });

    // Lo que cierra va al final (y "reabrir" después de cerrar). Los
    // selectores y los del sidebar los abre la cáscara, y los de PRs piden
    // GitHub (tests de abajo).
    let mut order: Vec<&crate::commands::CommandSpec> = COMMANDS
        .iter()
        .filter(|spec| {
            !matches!(
                spec.command,
                Command::Scope(_)
                    | Command::Pick(_)
                    | Command::Sidebar(_)
                    | Command::OpenCurrentPullRequest
                    // Necesitan algo en la pila (su test: `undo_and_redo_commands_follow_the_history`).
                    | Command::Shortcut(ShortcutAction::Undo | ShortcutAction::Redo)
            )
        })
        .collect();
    let last = ["close-tab", "close-repository", "reopen-repository"];
    order.sort_by_key(|spec| last.iter().position(|id| *id == spec.id));

    for spec in order {
        // Todo sobre un repositorio activo (salvo lo que lo cambia).
        harness.model.update(cx, |model, cx| {
            if model.active_repository_index().is_none() {
                model.select_repository_tab(0, cx);
            }
            // Con Settings a la vista, "Close tab" cerraría Settings.
            model.hide_preferences(cx);
            // Los de documentos necesitan documentos: dos abiertos y, para
            // recorrerlos, la vista de commits delante; "Close tab" cierra
            // el que se ve.
            if matches!(spec.id, "close-tab" | "next-document" | "previous-document")
                && let Some(path) = model.active_repository().map(|repo| repo.path.clone())
            {
                for file in ["README.md", "src/main.rs"] {
                    model.open_workspace_file(&path, &FileTabDescriptor::working(file), false, cx);
                }
                if spec.id != "close-tab" {
                    model.activate_workspace_tab(
                        &path,
                        mergeon_model::workspace_tabs::HOME_TAB,
                        cx,
                    );
                }
            }
        });
        cx.run_until_parked();
        let context = harness
            .model
            .read_with(cx, |model, _| CommandContext::read(model));
        assert!(spec.available(context), "{} disponible", spec.id);

        let before = observe(&harness, cx);
        cx.update(|cx| commands::run(spec.command, &harness.model, cx));
        wait_until(cx, spec.id, |cx| observe(&harness, cx) != before);

        if let Command::Shortcut(action) = spec.command {
            let expected = match action {
                ShortcutAction::Fetch => Some(DialogRequest::Fetch),
                ShortcutAction::Pull => Some(DialogRequest::Pull),
                ShortcutAction::Push => Some(DialogRequest::Push),
                ShortcutAction::ShowReflog => Some(DialogRequest::Reflog),
                ShortcutAction::ShowOperationsLog => Some(DialogRequest::OperationsLog),
                ShortcutAction::OpenRepository => Some(DialogRequest::Launcher),
                ShortcutAction::CloneFromUrl => Some(DialogRequest::CloneFromUrl),
                _ => None,
            };
            if let Some(expected) = expected {
                assert_eq!(
                    harness.events.borrow().last(),
                    Some(&AppEvent::ShowDialog(expected)),
                    "{}",
                    spec.id
                );
            }
        }
    }
}

#[gpui_kit::test]
async fn ctrl_shift_p_opens_the_palette_and_filters_with_prefixes(cx: &mut TestAppContext) {
    let harness = Harness::new(cx, true);
    let dir = repository("palette-keys");
    harness.open(&dir, cx).await;

    harness.open_palette(cx);
    let labels = harness.labels(cx);
    // Con la consulta vacía: comandos, repositorios y ramas, sin archivos.
    assert_eq!(labels[0], "Switch to branch…");
    assert!(labels.iter().any(|label| label == "Fetch…"));
    assert!(labels.iter().any(|label| label == "feature"));
    assert!(!labels.iter().any(|label| label == "README.md"));

    harness.type_text(cx, ">fetch");
    assert_eq!(
        harness.labels(cx).first().map(String::as_str),
        Some("Fetch…")
    );
    harness.in_window(cx, |window, _| {
        assert!(window.try_find(("palette-row", 0usize)).is_some());
    });

    // Flechas: ↓ y ↑ dan la vuelta; Esc cierra.
    harness.in_window(cx, |window, cx| {
        window.press("secondary-a", cx);
        window.input(">", cx);
    });
    let palette = harness.palette(cx).unwrap();
    let selected = |cx: &mut TestAppContext| cx.update(|cx| palette.read(cx).selected_index());
    assert_eq!(selected(cx), 0);
    harness.in_window(cx, |window, cx| {
        window.press("down", cx);
        window.press("down", cx);
    });
    assert_eq!(selected(cx), 2);
    harness.in_window(cx, |window, cx| {
        window.press("up", cx);
        window.press("up", cx);
        window.press("up", cx);
    });
    let count = cx.update(|cx| palette.read(cx).result_labels().len());
    assert_eq!(selected(cx), count - 1);
    harness.in_window(cx, |window, cx| window.press("escape", cx));
    assert!(harness.palette(cx).is_none());
    harness.in_window(cx, |window, cx| {
        assert_eq!(mergeon_ui::dialog::open_count(window, cx), 0);
    });

    // Enter ejecuta: "Settings".
    harness.open_palette(cx);
    harness.type_text(cx, "settings");
    harness.in_window(cx, |window, cx| window.press("enter", cx));
    assert!(harness.palette(cx).is_none());
    assert!(
        harness
            .model
            .read_with(cx, |model, _| model.show_preferences())
    );

    // "Switch to branch…" se queda abierta con `@`.
    harness.open_palette(cx);
    harness.type_text(cx, "switch to branch");
    harness.in_window(cx, |window, cx| window.press("enter", cx));
    let palette = harness.palette(cx).expect("sigue abierta");
    assert_eq!(cx.update(|cx| palette.read(cx).query(cx)), "@");
    let labels = harness.labels(cx);
    assert!(
        labels
            .iter()
            .all(|label| label == "main" || label == "feature")
    );
}

#[gpui_kit::test]
async fn switching_branch_from_the_palette_uses_the_checkout_flow(cx: &mut TestAppContext) {
    let harness = Harness::new(cx, true);
    let dir = repository("palette-branch");
    harness.open(&dir, cx).await;

    harness.open_palette(cx);
    harness.type_text(cx, "@feat");
    assert_eq!(harness.labels(cx), ["feature"]);
    harness.in_window(cx, |window, cx| window.press("enter", cx));
    let model = harness.model.clone();
    wait_until(cx, "checkout de feature", |cx| {
        model.read_with(cx, |model, _| {
            model
                .active_repository()
                .and_then(|repo| repo.current_branch.clone())
                == Some("feature".to_string())
        })
    });

    // Con cambios sin guardar, pasa por la confirmación del sidebar.
    std::fs::write(dir.path().join("README.md"), "cambio\n").unwrap();
    harness.open_palette(cx);
    harness.type_text(cx, "@main");
    harness.in_window(cx, |window, cx| window.press("enter", cx));
    let window = harness.window;
    wait_until(cx, "diálogo de checkout", |cx| {
        cx.update_window(window, |_, window, cx| {
            mergeon_ui::dialog::open_count(window, cx) > 0
        })
        .unwrap_or(false)
    });
    assert_eq!(
        model.read_with(cx, |model, _| model
            .active_repository()
            .and_then(|repo| repo.current_branch.clone())),
        Some("feature".to_string())
    );
}

#[gpui_kit::test]
async fn go_to_file_lists_ls_files_and_opens_a_file_tab(cx: &mut TestAppContext) {
    let harness = Harness::new(cx, true);
    let dir = repository("palette-file");
    harness.open(&dir, cx).await;

    harness.open_palette(cx);
    // `git ls-files` corre en segundo plano al abrir la paleta.
    let shell = harness.shell.clone().unwrap();
    let repo_path = harness.model.read_with(cx, |model, _| {
        model.active_repository().unwrap().path.clone()
    });
    wait_until(cx, "archivos", |cx| {
        cx.update(|cx| {
            shell
                .read(cx)
                .file_index()
                .read(cx)
                .get(&repo_path)
                .is_some_and(|list| list.files.len() == 2)
        })
    });
    harness.type_text(cx, "/main");
    assert_eq!(harness.labels(cx), ["src/main.rs"]);
    harness.in_window(cx, |window, cx| window.press("enter", cx));

    let path = dir.path().to_string_lossy().into_owned();
    let workspace = harness.model.read_with(cx, |model, _| {
        let repo = model.active_repository().unwrap();
        model.repository_workspace(&repo.path)
    });
    let head = String::from_utf8(
        Process::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&path)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert_eq!(
        workspace.active_tab,
        FileTabDescriptor::tree("src/main.rs", head.trim()).id()
    );
}

/// La acción va al repositorio que estaba activo al abrir la paleta.
#[gpui_kit::test]
async fn the_palette_runs_on_the_repository_active_when_it_opened(cx: &mut TestAppContext) {
    let harness = Harness::new(cx, true);
    let first = repository("palette-first");
    let second = repository("palette-second");
    harness.open(&first, cx).await;
    harness.open(&second, cx).await;
    harness
        .model
        .update(cx, |model, cx| model.select_repository_tab(0, cx));
    cx.run_until_parked();

    harness.open_palette(cx);
    harness.type_text(cx, ">show local changes");
    // Algo cambia la pestaña activa mientras la paleta está abierta.
    harness
        .model
        .update(cx, |model, cx| model.select_repository_tab(1, cx));
    cx.run_until_parked();
    harness.in_window(cx, |window, cx| window.press("enter", cx));

    harness.model.read_with(cx, |model, _| {
        assert_eq!(model.active_repository_index(), Some(0));
        assert_eq!(model.active_view(), WorkspaceSection::Changes);
    });
}

/// 5.000 ramas y 20.000 archivos: se filtra en segundo plano y la lista
/// solo construye las filas que se ven.
#[gpui_kit::test]
async fn thousands_of_branches_and_files_stay_virtual(cx: &mut TestAppContext) {
    let harness = Harness::new(cx, false);
    let entries: Vec<PaletteEntry> = (0..5_000)
        .map(|index| PaletteEntry {
            kind: PaletteKind::Branch,
            icon: mergeon_ui::Lucide::GitBranch,
            label: format!("team-{}/feature-{index}", index % 37).into(),
            context: "Local branch".into(),
            shortcut: None,
            enabled: true,
            action: PaletteAction::Branch(format!("feature-{index}")),
        })
        .collect();
    let files = FileList {
        files: Arc::new(
            (0..20_000)
                .map(|index| format!("src/module_{}/file_{index}.rs", index % 50))
                .collect(),
        ),
        head: Some("0".repeat(40)),
        truncated: false,
    };

    let palette = harness.in_window(cx, |window, cx| {
        let palette = cx.new(|cx| CommandPalette::new(entries, window, cx));
        palette.update(cx, |palette, cx| palette.set_files(files, false, cx));
        let input = palette.read(cx).input().clone();
        mergeon_ui::dialog::AppDialog::new()
            .flush_body()
            .body({
                let palette = palette.clone();
                move |_, _| palette.clone()
            })
            .open(window, cx);
        let handle = gpui_kit::Focusable::focus_handle(input.read(cx), cx);
        window.focus(&handle, cx);
        palette
    });

    let started = Instant::now();
    harness.in_window(cx, |window, cx| window.input("file1", cx));
    wait_until(cx, "ordenar", |cx| {
        cx.update(|cx| !palette.read(cx).is_ranking())
    });
    let elapsed = started.elapsed();
    let results = cx.update(|cx| palette.read(cx).result_labels().len());
    assert_eq!(results, RESULT_LIMIT);
    assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");

    harness.in_window(cx, |window, _| {
        let built = (0..RESULT_LIMIT)
            .filter(|index| window.try_find(("palette-row", *index)).is_some())
            .count();
        assert!(built > 0 && built <= 12, "{built} filas construidas");
    });
}

/// La regla de Ctrl+F: con el foco en un visor de diff gana la búsqueda del
/// visor; en cualquier otro sitio sigue siendo "Search commits".
#[gpui_kit::test]
async fn ctrl_f_searches_commits_unless_a_viewer_has_the_focus(cx: &mut TestAppContext) {
    let harness = Harness::new(cx, false);
    let dir = repository("palette-ctrl-f");
    harness.open(&dir, cx).await;
    let viewer = harness.in_window(cx, |window, cx| {
        let viewer = cx.new(|cx| {
            let mut viewer = mergeon_view_diff::DiffViewer::new(window, cx);
            viewer.set_diff("@@ -1 +1 @@\n-uno\n+dos\n", "a.txt", cx);
            viewer
        });
        let content = viewer.clone();
        mergeon_ui::dialog::AppDialog::new()
            .flush_body()
            .body(move |_, _| gpui_kit::div().h(gpui_kit::px(200.)).child(content.clone()))
            .open(window, cx);
        viewer
    });
    let searches = |harness: &Harness| {
        harness
            .events
            .borrow()
            .iter()
            .filter(|event| matches!(event, AppEvent::FocusCommitSearch))
            .count()
    };

    // El foco en el visor: su barra de búsqueda, y nada de buscar commits.
    harness.in_window(cx, |window, cx| {
        let handle = gpui_kit::Focusable::focus_handle(viewer.read(cx), cx);
        window.focus(&handle, cx);
    });
    harness.in_window(cx, |window, cx| window.press("secondary-f", cx));
    assert!(cx.update(|cx| viewer.read(cx).find_status(cx).open));
    assert_eq!(searches(&harness), 0);

    // Sin diálogos y con el foco fuera del visor: "Search commits".
    harness.in_window(cx, |window, cx| {
        mergeon_ui::dialog::close_all(window, cx);
        window.blur(cx);
    });
    harness.in_window(cx, |window, cx| window.press("secondary-f", cx));
    assert_eq!(searches(&harness), 1);
}

impl Harness {
    /// Escribe la consulta de la paleta abierta (sin pasar por el teclado: un
    /// selector empieza con un prefijo ya escrito).
    fn set_query(&self, cx: &mut TestAppContext, text: &str) {
        let palette = self.palette(cx).expect("paleta abierta");
        let text = text.to_string();
        self.in_window(cx, move |window, cx| {
            palette.update(cx, |palette, cx| palette.set_query(&text, window, cx));
        });
        let palette = self.palette(cx).expect("paleta abierta");
        wait_until(cx, "resultados", |cx| {
            cx.update(|cx| !palette.read(cx).is_ranking())
        });
    }

    /// Ejecuta un comando de la paleta por su etiqueta, como con Enter.
    fn run_command(&self, cx: &mut TestAppContext, label: &str) {
        self.open_palette(cx);
        self.set_query(cx, &format!(">{label}"));
        assert_eq!(
            self.labels(cx).first().map(String::as_str),
            Some(label),
            "{label} primero"
        );
        self.in_window(cx, |window, cx| window.press("enter", cx));
    }

    fn dialog_titles(&self, cx: &mut TestAppContext) -> Vec<String> {
        self.in_window(cx, |window, cx| {
            mergeon_ui::dialog::open_titles(window, cx)
                .iter()
                .map(ToString::to_string)
                .collect()
        })
    }
}

/// "File history…" reabre la paleta con los archivos y abre la pestaña de
/// historial del elegido (desde `HEAD`).
#[gpui_kit::test]
async fn file_history_from_the_palette_picks_a_file(cx: &mut TestAppContext) {
    let harness = Harness::new(cx, true);
    let dir = repository("palette-history");
    harness.open(&dir, cx).await;

    harness.run_command(cx, "File history…");
    let palette = harness.palette(cx).expect("el selector de archivos");
    assert_eq!(cx.update(|cx| palette.read(cx).query(cx)), "/");
    wait_until(cx, "archivos", |cx| harness.labels(cx).len() == 2);
    harness.set_query(cx, "/main");
    assert_eq!(harness.labels(cx), ["src/main.rs"]);
    harness.in_window(cx, |window, cx| window.press("enter", cx));

    let workspace = harness.model.read_with(cx, |model, _| {
        let repo = model.active_repository().unwrap();
        model.repository_workspace(&repo.path)
    });
    assert_eq!(
        workspace.active_tab,
        FileTabDescriptor::history("src/main.rs", None).id()
    );
    assert!(harness.palette(cx).is_none());
}

/// "Compare with current branch…": las demás ramas; la elegida abre la
/// comparación con la actual.
#[gpui_kit::test]
async fn compare_from_the_palette_picks_a_branch(cx: &mut TestAppContext) {
    let harness = Harness::new(cx, true);
    let dir = repository("palette-compare");
    harness.open(&dir, cx).await;

    harness.run_command(cx, "Compare with current branch…");
    assert!(harness.palette(cx).is_some(), "el selector de ramas");
    // La actual (main) no se ofrece.
    assert_eq!(harness.labels(cx), ["feature"]);
    harness.in_window(cx, |window, cx| window.press("enter", cx));
    wait_until(cx, "la comparación", |cx| {
        harness.dialog_titles(cx) == ["Compare"]
    });
    assert!(harness.palette(cx).is_none());
}

/// "Stash selected files…" lleva al diálogo de stash solo con lo marcado.
#[gpui_kit::test]
async fn stash_selected_from_the_palette_takes_the_checked_files(cx: &mut TestAppContext) {
    let harness = Harness::new(cx, true);
    let dir = repository("palette-stash");
    std::fs::write(dir.path().join("README.md"), "cambio\n").unwrap();
    std::fs::write(dir.path().join("src/main.rs"), "fn main() { }\n").unwrap();
    git(dir.path(), &["add", "src/main.rs"]);
    harness.open(&dir, cx).await;
    let model = harness.model.clone();
    wait_until(cx, "lo preparado", |cx| {
        model.read_with(cx, |model, _| CommandContext::read(model).has_staged)
    });

    harness.run_command(cx, "Stash selected files…");
    wait_until(cx, "el diálogo de stash", |_| {
        harness.events.borrow().iter().any(|event| {
            *event
                == AppEvent::ShowDialog(DialogRequest::Stash {
                    paths: vec!["src/main.rs".to_string()],
                })
        })
    });
}

/// "Fixup into…": los commits de la caja de commit; el elegido recibe un
/// `fixup!` con lo preparado.
#[gpui_kit::test]
async fn fixup_from_the_palette_picks_a_commit(cx: &mut TestAppContext) {
    let harness = Harness::new(cx, true);
    let dir = repository("palette-fixup");
    // El commit lo hace la app: necesita una identidad en el repositorio.
    git(dir.path(), &["config", "user.name", "Test"]);
    git(dir.path(), &["config", "user.email", "test@example.com"]);
    git(dir.path(), &["config", "commit.gpgsign", "false"]);
    std::fs::write(dir.path().join("README.md"), "arreglo\n").unwrap();
    git(dir.path(), &["add", "README.md"]);
    let repo_id = harness.open(&dir, cx).await;
    let shell = harness.shell.clone().unwrap();
    wait_until(cx, "los commits de Fixup into…", |cx| {
        harness.in_window(cx, |_, _| ());
        cx.update(|cx| {
            shell.read(cx).workspace(&repo_id).is_some_and(|workspace| {
                workspace.read(cx).changes().read(cx).fixup_targets().len() == 1
            })
        })
    });

    harness.run_command(cx, "Fixup into…");
    let labels = harness.labels(cx);
    assert_eq!(labels.len(), 1);
    assert!(labels[0].ends_with(" first"), "{labels:?}");
    harness.in_window(cx, |window, cx| window.press("enter", cx));
    wait_until(cx, "el commit fixup!", |_| {
        let output = Process::new("git")
            .args(["log", "-1", "--format=%s"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout).trim() == "fixup! first"
    });
    assert!(harness.palette(cx).is_none());
}

/// El selector de "Compare…" sigue en el repositorio de la paleta.
#[gpui_kit::test]
async fn pickers_keep_the_repository_of_the_palette(cx: &mut TestAppContext) {
    let harness = Harness::new(cx, true);
    let first = repository("palette-pick-a");
    let second = repository("palette-pick-b");
    git(second.path(), &["branch", "only-here"]);
    harness.open(&first, cx).await;
    let second_id = harness.open(&second, cx).await;
    let shell = harness.shell.clone().unwrap();
    let target = harness.model.read_with(cx, |model, _| {
        let repo = model.active_repository().unwrap();
        PaletteTarget {
            repo_id: repo.id.clone(),
            repo_path: repo.path.clone(),
        }
    });
    assert_eq!(target.repo_id, second_id);
    // Entretanto se pasó al primero: el selector es del segundo.
    harness
        .model
        .update(cx, |model, cx| model.select_repository_tab(0, cx));
    harness.in_window(cx, |window, cx| {
        shell.update(cx, |shell, cx| {
            shell.run_palette_action(
                PaletteAction::Command(Command::Pick(Picker::CompareBranch)),
                Some(target),
                window,
                cx,
            );
        })
    });
    let model = harness.model.clone();
    wait_until(cx, "ramas del segundo", |cx| {
        harness.palette(cx).is_some()
            && harness.labels(cx).contains(&"only-here".to_string())
            && model.read_with(cx, |model, _| model.active_repository_index() == Some(1))
    });
}

/// "Clean up merged branches…" y "New worktree…" abren los diálogos del
/// sidebar sobre el repositorio activo.
#[gpui_kit::test]
async fn sidebar_commands_from_the_palette_open_their_dialogs(cx: &mut TestAppContext) {
    let harness = Harness::new(cx, true);
    let dir = repository("palette-sidebar");
    harness.open(&dir, cx).await;

    harness.run_command(cx, "Clean up merged branches…");
    wait_until(cx, "el diálogo de limpieza", |cx| {
        harness.dialog_titles(cx) == ["Clean up merged branches"]
    });
    harness.in_window(cx, mergeon_ui::dialog::close_all);

    harness.run_command(cx, "New worktree…");
    wait_until(cx, "el diálogo de worktree", |cx| {
        harness.dialog_titles(cx) == ["New Worktree"]
    });
}

/// Un GitHub de prueba con el PR #7 de la rama `feature` (del propio
/// repositorio) y la cuenta activa.
fn github_backend() -> TestBackend {
    use mergeon_app::testing::FakeGitHubTransport;
    let store = TestBackend::memory_store();
    let account = serde_json::from_value(serde_json::json!({
        "id": "7",
        "username": "ana",
        "token": "ghp_test",
        "isPersonalAccessToken": true
    }))
    .unwrap();
    store.save_github_account(&account).unwrap();
    store.set_active_github_account_id(Some("7")).unwrap();
    let backend = TestBackend::new().with_store(store);
    backend.github.respond_with(|request| {
        let path = request
            .url
            .split("api.github.com")
            .nth(1)
            .unwrap_or_default()
            .split('?')
            .next()
            .unwrap_or_default();
        match path {
            "/repos/acme/demo/pulls" => FakeGitHubTransport::json(
                200,
                serde_json::json!([{
                    "number": 7,
                    "title": "Add the feature",
                    "html_url": "https://github.com/acme/demo/pull/7",
                    "draft": false,
                    "user": { "login": "acme" },
                    "head": { "ref": "feature", "sha": "sha-7",
                              "repo": { "full_name": "acme/demo", "owner": { "login": "acme" } } },
                    "base": { "ref": "main", "repo": { "full_name": "acme/demo" } },
                    "requested_reviewers": []
                }]),
            ),
            path if path.contains("/commits/") => FakeGitHubTransport::json(
                200,
                serde_json::json!({ "check_runs": [], "statuses": [] }),
            ),
            path if path.ends_with("/reviews") => {
                FakeGitHubTransport::json(200, serde_json::json!([]))
            }
            _ => FakeGitHubTransport::json(404, serde_json::json!({ "message": "Not Found" })),
        }
    });
    backend
}

/// "Open pull request of current branch" abre el PR de la rama en el
/// navegador; "Checkout pull request…" ofrece los PRs y desprotege el
/// elegido.
#[gpui_kit::test]
async fn pull_request_commands_from_the_palette(cx: &mut TestAppContext) {
    let harness = Harness::with_backend(cx, true, github_backend());
    let dir = repository("palette-pulls");
    // `origin` local con `feature` publicada, conocido por GitHub como
    // `acme/demo` (la URL de push).
    let origin = tempfile::Builder::new()
        .prefix("palette-pulls-origin")
        .tempdir()
        .unwrap();
    git(origin.path(), &["init", "-q", "--bare"]);
    let origin_path = origin.path().to_string_lossy().into_owned();
    git(dir.path(), &["remote", "add", "origin", &origin_path]);
    git(dir.path(), &["push", "-q", "origin", "main", "feature"]);
    git(
        dir.path(),
        &[
            "remote",
            "set-url",
            "--push",
            "origin",
            "https://github.com/acme/demo.git",
        ],
    );
    // Solo en el remoto: el checkout del PR la trae y la sigue (la local
    // cambia de nombre: `open` espera dos ramas).
    git(dir.path(), &["branch", "-m", "feature", "work"]);
    git(dir.path(), &["branch", "-D", "-r", "origin/feature"]);
    harness
        .model
        .update(cx, |model, cx| model.load_github_accounts_from_storage(cx))
        .await;
    harness.open(&dir, cx).await;
    let model = harness.model.clone();
    wait_until(cx, "los PRs", |cx| {
        model.read_with(cx, |model, _| CommandContext::read(model).has_pull_requests)
    });
    // En `main` no hay PR propio.
    assert!(!harness.model.read_with(cx, |model, _| {
        CommandContext::read(model).has_branch_pull_request
    }));

    harness.run_command(cx, "Checkout pull request…");
    assert_eq!(harness.labels(cx), ["#7 Add the feature"]);
    harness.in_window(cx, |window, cx| window.press("enter", cx));
    // Como "Checkout PR branch" del sidebar: trae `origin/feature` y pide
    // seguirla.
    wait_until(cx, "el diálogo de seguimiento", |cx| {
        harness.dialog_titles(cx) == ["Track Remote Branch"]
    });
    harness.in_window(cx, |window, cx| window.click("checkout-submit", cx));
    wait_until(cx, "el checkout de feature", |cx| {
        model.read_with(cx, |model, _| {
            model
                .active_repository()
                .and_then(|repo| repo.current_branch.clone())
                == Some("feature".to_string())
        })
    });

    wait_until(cx, "el PR de la rama", |cx| {
        model.read_with(cx, |model, _| {
            CommandContext::read(model).has_branch_pull_request
        })
    });
    harness.run_command(cx, "Open pull request of current branch");
    wait_until(cx, "el navegador", |_| {
        harness
            .launcher
            .launched_args()
            .iter()
            .any(|arg| arg == "https://github.com/acme/demo/pull/7")
    });
}

/// "Undo last Git operation" y "Redo Git operation": apagados sin nada en la
/// pila; con algo, lo mismo que Ctrl+Z / Ctrl+Shift+Z.
#[gpui_kit::test]
async fn undo_and_redo_commands_follow_the_history(cx: &mut TestAppContext) {
    let harness = Harness::new(cx, false);
    let dir = repository("palette-undo");
    harness.open(&dir, cx).await;
    let find = |id: &str| COMMANDS.iter().find(|spec| spec.id == id).unwrap();
    let context = harness
        .model
        .read_with(cx, |model, _| CommandContext::read(model));
    assert!(!find("undo").available(context));
    assert!(!find("redo").available(context));

    // El commit lo hace la app: necesita identidad en el repositorio.
    git(dir.path(), &["config", "user.name", "Test"]);
    git(dir.path(), &["config", "user.email", "test@example.com"]);
    git(dir.path(), &["config", "commit.gpgsign", "false"]);
    std::fs::write(dir.path().join("a.txt"), "a\n").unwrap();
    git(dir.path(), &["add", "a.txt"]);
    let path = harness.model.read_with(cx, |model, _| {
        model.active_repository().unwrap().path.clone()
    });
    let result = harness
        .model
        .update(cx, |model, cx| {
            model.commit(&path, "From the palette", Default::default(), true, cx)
        })
        .await;
    assert!(result.success(), "{}", result.message());
    let model = harness.model.clone();
    wait_until(cx, "algo que deshacer", |cx| {
        model.read_with(cx, |model, _| CommandContext::read(model).can_undo)
    });

    cx.update(|cx| commands::run(find("undo").command, &harness.model, cx));
    wait_until(cx, "deshecho", |cx| {
        model.read_with(cx, |model, _| {
            let context = CommandContext::read(model);
            context.can_redo && !context.can_undo
        })
    });
    cx.update(|cx| commands::run(find("redo").command, &harness.model, cx));
    wait_until(cx, "rehecho", |cx| {
        model.read_with(cx, |model, _| {
            let context = CommandContext::read(model);
            context.can_undo && !context.can_redo
        })
    });
}

// ---------------------------------------------------------------------------
// Espacio de trabajo y barra de título (Bloque C)
// ---------------------------------------------------------------------------

/// Pestaña activa y documentos (ruta, vista previa) del repositorio activo.
fn documents(harness: &Harness, cx: &mut TestAppContext) -> (String, Vec<(String, bool)>) {
    harness.model.read_with(cx, |model, _| {
        let repo = model.active_repository().expect("repositorio activo");
        let workspace = model.repository_workspace(&repo.path);
        (
            workspace.active_tab.clone(),
            workspace
                .tabs
                .iter()
                .map(|tab| (tab.path.clone(), tab.preview))
                .collect(),
        )
    })
}

/// Ruta canónica del repositorio activo (en Windows difiere de `dir.path()`).
fn active_repo_path(harness: &Harness, cx: &mut TestAppContext) -> String {
    harness.model.read_with(cx, |model, _| {
        model
            .active_repository()
            .expect("repositorio activo")
            .path
            .clone()
    })
}

/// El sidebar no se mueve al pasar de Commits a Local Changes ni al abrir un
/// documento, y la barra de documentos (32px) solo existe con alguno abierto.
#[gpui_kit::test]
async fn the_sidebar_stays_put_and_the_document_bar_only_exists_with_documents(
    cx: &mut TestAppContext,
) {
    use mergeon_model::workspace_tabs::{CHANGES_TAB, HOME_TAB};

    let harness = Harness::new(cx, true);
    let dir = repository("layout");
    harness.open(&dir, cx).await;
    let path = active_repo_path(&harness, cx);

    let left = |harness: &Harness, cx: &mut TestAppContext| {
        harness.in_window(cx, |window, _| window.find("sidebar").bounds())
    };
    let has_bar = |harness: &Harness, cx: &mut TestAppContext| {
        harness.in_window(cx, |window, _| window.try_find("workspace-tabs").is_some())
    };
    let commits = left(&harness, cx);
    assert!(!has_bar(&harness, cx), "sin documentos no hay barra");
    assert!(harness.in_window(cx, |window, _| {
        window.try_find("workspace-view-commits").is_some()
            && window.try_find("workspace-view-changes").is_some()
    }));

    harness.in_window(cx, |window, cx| window.click("workspace-view-changes", cx));
    assert_eq!(documents(&harness, cx).0, CHANGES_TAB);
    assert_eq!(
        left(&harness, cx),
        commits,
        "Local Changes no mueve el sidebar"
    );

    harness.model.update(cx, |model, cx| {
        model.open_workspace_file(&path, &FileTabDescriptor::working("README.md"), false, cx);
    });
    assert!(has_bar(&harness, cx));
    let bar = harness.in_window(cx, |window, _| window.find("workspace-tabs").bounds());
    assert_eq!(
        f32::from(bar.size.height),
        crate::workspace::DOCUMENT_BAR_HEIGHT
    );
    assert_eq!(
        left(&harness, cx),
        commits,
        "un documento no mueve el sidebar"
    );
    assert!(harness.in_window(cx, |window, _| {
        window.try_find("file-tab-breadcrumbs").is_some()
            && window.try_find("file-tab-open-in-editor").is_some()
            && window.try_find("toggle-file-list").is_some()
    }));

    harness.in_window(cx, |window, cx| window.click("workspace-view-commits", cx));
    assert_eq!(documents(&harness, cx).0, HOME_TAB);
    assert!(
        has_bar(&harness, cx),
        "la barra sigue mientras haya documentos"
    );

    harness.model.update(cx, |model, cx| {
        model.close_all_workspace_tabs(&path, cx);
    });
    assert!(!has_bar(&harness, cx));
}

/// Desde Commits los archivos van a la pestaña de vista previa; un doble
/// clic en ella la deja abierta.
#[gpui_kit::test]
async fn double_clicking_the_preview_tab_keeps_it_open(cx: &mut TestAppContext) {
    let harness = Harness::new(cx, true);
    let dir = repository("preview");
    harness.open(&dir, cx).await;
    let path = active_repo_path(&harness, cx);
    let readme = FileTabDescriptor::working("README.md");

    harness.model.update(cx, |model, cx| {
        model.open_workspace_file(&path, &readme, false, cx);
    });
    assert_eq!(documents(&harness, cx).1, [("README.md".to_string(), true)]);

    let tab = format!("file-tab-{}", readme.id());
    harness.in_window(cx, |window, cx| window.double_click(tab.clone(), cx));
    assert_eq!(
        documents(&harness, cx).1,
        [("README.md".to_string(), false)]
    );
}

/// `Ctrl+W` en Commits o Local Changes ya no cierra el repositorio; con un
/// documento delante, lo cierra a él.
#[gpui_kit::test]
async fn ctrl_w_only_closes_documents(cx: &mut TestAppContext) {
    let harness = Harness::new(cx, true);
    let dir = repository("ctrlw");
    harness.open(&dir, cx).await;
    let path = active_repo_path(&harness, cx);
    let open = |harness: &Harness, cx: &mut TestAppContext| {
        harness
            .model
            .read_with(cx, |model, _| model.open_repositories().len())
    };

    harness.in_window(cx, |window, cx| window.press("secondary-w", cx));
    cx.run_until_parked();
    assert_eq!(open(&harness, cx), 1);

    harness.model.update(cx, |model, cx| {
        model.activate_workspace_tab(&path, mergeon_model::workspace_tabs::CHANGES_TAB, cx);
        model.open_workspace_file(&path, &FileTabDescriptor::working("README.md"), false, cx);
    });
    assert_eq!(documents(&harness, cx).1.len(), 1);
    harness.in_window(cx, |window, cx| window.press("secondary-w", cx));
    cx.run_until_parked();
    assert_eq!(open(&harness, cx), 1);
    assert!(documents(&harness, cx).1.is_empty());
}

/// La rama de la barra abre la paleta con solo las ramas.
#[gpui_kit::test]
async fn the_branch_button_opens_the_branch_picker(cx: &mut TestAppContext) {
    let harness = Harness::new(cx, true);
    let dir = repository("branch-picker");
    harness.open(&dir, cx).await;

    harness.in_window(cx, |window, cx| window.click("toolbar-branch-button", cx));
    assert!(harness.palette(cx).is_some(), "se abrió la paleta");
    let palette = harness.palette(cx).unwrap();
    wait_until(cx, "resultados", |cx| {
        cx.update(|cx| !palette.read(cx).is_ranking())
    });
    let labels = harness.labels(cx);
    assert!(labels.iter().any(|label| label == "feature"), "{labels:?}");
    assert!(!labels.iter().any(|label| label == "Fetch"), "{labels:?}");
}

/// Sin barra de actividad: abrir repositorio, el log y Settings están en la
/// barra de título, y "Reload application" queda solo en la paleta.
#[gpui_kit::test]
async fn the_title_bar_holds_open_the_log_and_settings(cx: &mut TestAppContext) {
    let harness = Harness::new(cx, true);
    for id in [
        "rail-open-repository",
        "rail-reload",
        "rail-operations-log",
        "rail-settings",
    ] {
        assert!(
            !harness.in_window(cx, |window, _| window.try_find(id).is_some()),
            "{id}"
        );
    }

    harness.in_window(cx, |window, cx| window.click("titlebar-settings", cx));
    assert!(
        harness
            .model
            .read_with(cx, |model, _| model.show_preferences())
    );

    harness.in_window(cx, |window, cx| window.click("titlebar-operations-log", cx));
    assert_eq!(
        harness.events.borrow().last(),
        Some(&AppEvent::ShowDialog(DialogRequest::OperationsLog))
    );
    assert!(
        COMMANDS
            .iter()
            .any(|spec| spec.label == "Reload application")
    );
}

#[gpui_kit::test]
async fn direct_mode_exposes_options_commands_and_custom_shortcuts_in_palette(
    cx: &mut TestAppContext,
) {
    let harness = Harness::new(cx, true);
    let dir = repository("palette-direct-shortcuts");
    harness.open(&dir, cx).await;

    // Enable direct mode and rebind Fetch to `f5`.
    harness
        .model
        .update(cx, |model, cx| model.set_skip_action_dialogs(true, cx))
        .await;
    harness
        .model
        .update(cx, |model, cx| {
            model.set_keyboard_shortcut(ShortcutAction::Fetch, "f5", cx)
        })
        .await;
    cx.run_until_parked();

    harness.open_palette(cx);
    let labels = harness.labels(cx);
    assert!(labels.iter().any(|label| label == "Fetch"), "{labels:?}");
    assert!(
        labels.iter().any(|label| label == "Fetch (options)…"),
        "{labels:?}"
    );
    assert!(
        labels.iter().any(|label| label == "Pull (options)…"),
        "{labels:?}"
    );
    assert!(
        labels.iter().any(|label| label == "Push (options)…"),
        "{labels:?}"
    );

    // Selecting "Fetch (options)…" opens the Fetch dialog even in direct mode.
    harness.set_query(cx, ">Fetch (options)");
    assert_eq!(
        harness.labels(cx).first().map(String::as_str),
        Some("Fetch (options)…")
    );
    harness.in_window(cx, |window, cx| window.press("enter", cx));
    wait_until(cx, "diálogo Fetch", |_| {
        harness
            .events
            .borrow()
            .contains(&AppEvent::ShowDialog(DialogRequest::Fetch))
    });

    // Live rebinding via AppEvent::ShortcutsChanged: pressing `f5` now triggers Fetch!
    harness.in_window(cx, |window, cx| {
        mergeon_ui::dialog::close_all(window, cx);
        window.blur(cx);
    });
    harness.events.borrow_mut().clear();
    harness.in_window(cx, |window, cx| window.press("f5", cx));
    // Repo has no remote, so direct Fetch falls back to DialogRequest::Fetch.
    wait_until(cx, "atajo personalizado f5", |_| {
        harness
            .events
            .borrow()
            .contains(&AppEvent::ShowDialog(DialogRequest::Fetch))
    });
}
