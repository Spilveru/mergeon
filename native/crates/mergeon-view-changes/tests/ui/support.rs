//! Fixtures de los tests de interfaz: el modelo sobre un backend sin efectos
//! fuera del temporal, una ventana sin pantalla con la vista de cambios (como
//! la monta la cáscara) y repositorios git reales.

use std::cell::RefCell;
use std::path::PathBuf;
use std::process::Command;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AnyElement, AnyWindowHandle, App, AppContext as _, Bounds, Context, ElementId, Entity,
    InputEvent as _, IntoElement, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent,
    ParentElement as _, Render, Styled as _, Subscription, TestAppContext, Window, WindowBounds,
    WindowOptions, div, point, px, size,
};
use mergeon_app::testing::{RecordingLauncher, RecordingUiHost, TestBackend};
use mergeon_app::{AddRepository, AppModel};
use mergeon_model::workspace_tabs::FileTabDescriptor;
use mergeon_ui::dialog;
use mergeon_view_changes::{ChangesMode, ChangesView, ChangesViewEvent};

/// Margen para todo lo que en un test "tiene que acabar pasando".
const PATIENCE: Duration = Duration::from_secs(20);

type Extra = Box<dyn Fn(&mut Window, &mut Context<Shell>) -> AnyElement>;

/// Lo que hará la cáscara: la vista de cambios ocupando la ventana. `extra`
/// deja a un test dibujar en su lugar una pieza suelta.
pub struct Shell {
    view: Option<Entity<ChangesView>>,
    extra: Option<Extra>,
    events: Rc<RefCell<Vec<ChangesViewEvent>>>,
    _subscription: Option<Subscription>,
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let extra = self.extra.as_ref().map(|extra| extra(window, cx));
        div()
            .size_full()
            .flex()
            .flex_col()
            .children(self.view.clone())
            .children(extra)
    }
}

/// El modelo y la ventana con todo lo que un test necesita mirar.
pub struct TestApp {
    pub model: Entity<AppModel>,
    pub host: Rc<RecordingUiHost>,
    pub window: AnyWindowHandle,
    pub shell: Entity<Shell>,
    /// Lo que el backend habría lanzado (editor externo, explorador…).
    pub launcher: Arc<RecordingLauncher>,
    /// Lo que emitió la vista, en orden.
    pub events: Rc<RefCell<Vec<ChangesViewEvent>>>,
}

impl TestApp {
    pub fn new(cx: &mut TestAppContext) -> Self {
        Self::with_size(1200., 800., cx)
    }

    pub fn with_size(width: f32, height: f32, cx: &mut TestAppContext) -> Self {
        // Los eventos del backend (cola de mutaciones) llegan desde sus
        // propios hilos: el planificador de tests tiene que admitirlo.
        cx.executor().allow_parking();

        let mut test_backend = TestBackend::new();
        let (backend, events) = test_backend.build();
        let host = RecordingUiHost::new();
        let model = cx.update(|cx| AppModel::init(backend, events, cx));
        model.update(cx, |model, cx| model.set_ui_host(host.clone(), cx));
        let recorded = Rc::new(RefCell::new(Vec::new()));

        let (window, shell) = cx.update(|cx| {
            mergeon_ui::init(cx);
            mergeon_view_diff::init(cx);
            mergeon_view_changes::init(cx);
            // Las animaciones de entrada usan el reloj real; sin movimiento
            // quedan resueltas en el primer frame.
            cx.set_reduce_motion(true);
            let recorded = recorded.clone();
            let (window, shell) = gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: point(px(0.), px(0.)),
                        size: size(px(width), px(height)),
                    })),
                    ..Default::default()
                },
                cx,
                move |_, cx| {
                    cx.new(|_| Shell {
                        view: None,
                        extra: None,
                        events: recorded,
                        _subscription: None,
                    })
                },
            )
            .expect("no se pudo abrir la ventana de prueba");
            (window, shell)
        });

        Self {
            model,
            host,
            window,
            shell,
            launcher: test_backend.launcher.clone(),
            events: recorded,
        }
    }

    /// Abre un repositorio como lo hace la app y espera a su primera lectura.
    pub async fn open(&self, repo: &TestRepo, cx: &mut TestAppContext) -> String {
        self.model
            .update(cx, |model, cx| {
                model.add_repository(&repo.path_string(), AddRepository::default(), cx)
            })
            .await
            .expect("add repository");
        self.model
            .update(cx, |model, cx| model.hydrate_opened_repository(cx))
            .await;
        cx.run_until_parked();
        self.model.read_with(cx, |model, _| {
            model
                .active_repository()
                .map(|repository| repository.id.clone())
                .expect("the repository is open")
        })
    }

    /// Monta la vista de cambios del repositorio en la ventana.
    pub fn mount(
        &self,
        cx: &mut TestAppContext,
        repo_id: &str,
        mode: ChangesMode,
    ) -> Entity<ChangesView> {
        let repo_id = repo_id.to_string();
        let shell = self.shell.clone();
        let view = cx
            .update_window(self.window, |_, window, cx| {
                let view = cx.new(|cx| ChangesView::new(repo_id, mode, window, cx));
                shell.update(cx, |shell, cx| {
                    let events = shell.events.clone();
                    shell._subscription = Some(cx.subscribe(
                        &view,
                        move |_, _, event: &ChangesViewEvent, _| {
                            events.borrow_mut().push(event.clone());
                        },
                    ));
                    shell.view = Some(view.clone());
                    cx.notify();
                });
                view
            })
            .expect("la ventana de prueba se cerró");
        cx.run_until_parked();
        view
    }

    /// Abre la pestaña de un archivo en el espacio de trabajo y monta su vista.
    pub fn mount_tab(
        &self,
        cx: &mut TestAppContext,
        repo_id: &str,
        descriptor: FileTabDescriptor,
    ) -> Entity<ChangesView> {
        let path = self.repo_path(cx, repo_id);
        self.model.update(cx, |model, cx| {
            model.open_workspace_file(&path, &descriptor, false, cx);
        });
        self.mount(
            cx,
            repo_id,
            ChangesMode::FileTab {
                tab_id: descriptor.id(),
            },
        )
    }

    pub fn repo_path(&self, cx: &mut TestAppContext, repo_id: &str) -> String {
        self.model.read_with(cx, |model, _| {
            model
                .repository(repo_id)
                .map(|repository| repository.path.clone())
                .expect("the repository is open")
        })
    }

    /// Ejecuta `f` dentro de la ventana, con un frame recién dibujado.
    pub fn in_window<R>(
        &self,
        cx: &mut TestAppContext,
        f: impl FnOnce(&mut Window, &mut App) -> R,
    ) -> R {
        cx.run_until_parked();
        let result = cx
            .update_window(self.window, |_, window, cx| {
                window.render_frame(cx);
                f(window, cx)
            })
            .expect("la ventana de prueba se cerró");
        cx.run_until_parked();
        result
    }

    pub fn click(&self, cx: &mut TestAppContext, id: impl Into<ElementId>) {
        let id = id.into();
        self.in_window(cx, |window, cx| window.click(id, cx));
    }

    pub fn double_click(&self, cx: &mut TestAppContext, id: impl Into<ElementId>) {
        let id = id.into();
        self.in_window(cx, |window, cx| window.double_click(id, cx));
    }

    pub fn right_click(&self, cx: &mut TestAppContext, id: impl Into<ElementId>) {
        let id = id.into();
        self.in_window(cx, |window, cx| window.right_click(id, cx));
    }

    /// Clic en la entrada `index` (contando separadores) del menú abierto.
    pub fn click_menu_item(&self, cx: &mut TestAppContext, index: usize) {
        self.in_window(cx, |window, cx| {
            window.within("popup-menu").click(index, cx);
        });
    }

    /// Títulos de los diálogos abiertos, de abajo hacia arriba.
    pub fn dialogs(&self, cx: &mut TestAppContext) -> Vec<String> {
        self.in_window(cx, |window, cx| {
            dialog::open_titles(window, cx)
                .iter()
                .map(|title| title.to_string())
                .collect()
        })
    }

    /// Etiqueta accesible de un elemento, si está en pantalla.
    pub fn label(&self, cx: &mut TestAppContext, id: impl Into<ElementId>) -> Option<String> {
        let id = id.into();
        self.in_window(cx, |window, _| {
            window
                .try_find(id)
                .and_then(|element| element.label().map(str::to_string))
        })
    }

    pub fn exists(&self, cx: &mut TestAppContext, id: impl Into<ElementId>) -> bool {
        let id = id.into();
        self.in_window(cx, |window, _| window.try_find(id).is_some())
    }

    /// Clic con teclas modificadoras (`Shift+clic`, `Ctrl+clic`…) en el centro
    /// del elemento.
    pub fn click_with(
        &self,
        cx: &mut TestAppContext,
        id: impl Into<ElementId>,
        modifiers: Modifiers,
    ) {
        let id = id.into();
        self.in_window(cx, |window, cx| {
            let position = window.find(id).bounds().center();
            window.dispatch_event(
                MouseDownEvent {
                    button: MouseButton::Left,
                    position,
                    modifiers,
                    click_count: 1,
                    first_mouse: false,
                }
                .to_platform_input(),
                cx,
            );
            window.render_frame(cx);
            window.dispatch_event(
                MouseUpEvent {
                    button: MouseButton::Left,
                    position,
                    modifiers,
                    click_count: 1,
                }
                .to_platform_input(),
                cx,
            );
            window.render_frame(cx);
        });
    }

    /// Texto que pinta una fila de la lista (su etiqueta, dentro de su id).
    pub fn row_label(&self, cx: &mut TestAppContext, row: &str) -> Option<String> {
        let row = gpui_kit::SharedString::from(row.to_string());
        self.in_window(cx, |window, _| {
            window
                .within(row)
                .try_find("tree-label")
                .and_then(|label| label.label().map(str::to_string))
        })
    }

    /// Clic sin dejar correr lo que lance: para mirar el estado mientras una
    /// mutación está en curso (`in_window` espera a los hilos de fondo).
    pub fn click_now(&self, cx: &mut TestAppContext, id: impl Into<ElementId>) {
        let id = id.into();
        cx.update_window(self.window, |_, window, cx| {
            window.render_frame(cx);
            window.click(id, cx);
        })
        .expect("la ventana de prueba se cerró");
    }

    /// Una tecla sin dejar correr lo que lance.
    pub fn press_now(&self, cx: &mut TestAppContext, key: &str) {
        cx.update_window(self.window, |_, window, cx| window.press(key, cx))
            .expect("la ventana de prueba se cerró");
    }

    /// Dibuja en la ventana una pieza suelta en lugar de la vista.
    pub fn show(
        &self,
        cx: &mut TestAppContext,
        extra: impl Fn(&mut Window, &mut Context<Shell>) -> AnyElement + 'static,
    ) {
        self.shell.update(cx, |shell, cx| {
            shell.extra = Some(Box::new(extra));
            cx.notify();
        });
        cx.run_until_parked();
    }

    /// Espera a que `condition` se cumpla, dando tiempo a los hilos del
    /// backend a entregar sus eventos.
    pub fn wait_until(
        &self,
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
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Espera a que la vista no tenga nada en curso.
    pub fn settle(&self, cx: &mut TestAppContext, view: &Entity<ChangesView>) {
        let view = view.clone();
        self.wait_until(cx, "the view to settle", move |cx| {
            view.read_with(cx, |view, cx| {
                !view.is_loading_changes()
                    && !view.is_loading_diff()
                    && !view.is_interaction_locked()
                    && !view.is_resolving_conflict()
                    && !AppModel::global(cx).read(cx).git_is_loading()
            })
        });
        // Un último frame para lo que se haya pedido al terminar.
        self.in_window(cx, |_, _| {});
    }
}

/// Un repositorio `demo` con identidad propia y rama `main`, sin depender de
/// la configuración global de la máquina.
pub struct TestRepo {
    root: tempfile::TempDir,
    pub path: PathBuf,
}

impl TestRepo {
    /// Repositorio vacío (sin commits).
    pub fn empty() -> Self {
        Self::named("demo")
    }

    fn named(name: &str) -> Self {
        let root = tempfile::Builder::new()
            .prefix("mergeon-view-changes-")
            .tempdir()
            .expect("temporary directory");
        // La ruta real: en macOS el temporal cuelga de un symlink.
        let base = mergeon_gitcmd::paths::canonicalize(root.path())
            .expect("canonical temporary directory");
        let path = base.join(name);
        std::fs::create_dir(&path).expect("repository directory");
        let repo = Self { root, path };
        repo.git(&["init", "-q", "-b", "main"]);
        for (key, value) in [
            ("user.name", "Test User"),
            ("user.email", "test@example.com"),
            ("commit.gpgsign", "false"),
            ("tag.gpgsign", "false"),
            ("core.autocrlf", "false"),
        ] {
            repo.git(&["config", key, value]);
        }
        repo
    }

    /// Repositorio con un commit base en `main`.
    pub fn new() -> Self {
        let repo = Self::empty();
        repo.commit_file("README.md", "# Demo\n", "Initial commit");
        repo
    }

    /// Otro repositorio con un commit, con otro nombre de carpeta.
    pub fn other(name: &str) -> Self {
        let repo = Self::named(name);
        repo.commit_file("README.md", "# Other\n", "Initial commit");
        repo
    }

    /// La ruta como la maneja el modelo (y la reciben las vistas): con `/`
    /// también en Windows. `path` queda con los separadores del sistema.
    pub fn path_string(&self) -> String {
        self.path.to_string_lossy().replace('\\', "/")
    }

    pub fn git(&self, args: &[&str]) -> String {
        self.try_git(args)
            .unwrap_or_else(|error| panic!("git {args:?} failed: {error}"))
    }

    pub fn try_git(&self, args: &[&str]) -> Result<String, String> {
        let output = Command::new("git")
            .args(args)
            .current_dir(&self.path)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .map_err(|error| error.to_string())?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
        }
    }

    pub fn write(&self, file: &str, content: &str) {
        let path = self.path.join(file);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("parent directory");
        }
        std::fs::write(path, content).expect("write file");
    }

    pub fn read(&self, file: &str) -> String {
        std::fs::read_to_string(self.path.join(file)).unwrap_or_default()
    }

    pub fn commit_file(&self, file: &str, content: &str, message: &str) {
        self.write(file, content);
        self.git(&["add", "--all"]);
        self.git(&["commit", "-q", "-m", message]);
    }

    pub fn head(&self) -> String {
        self.git(&["rev-parse", "HEAD"])
    }

    /// Remoto bare `origin` junto al repositorio, con `main` ya publicado y
    /// seguido.
    pub fn with_origin(self) -> Self {
        let origin = self.origin_path();
        let created = Command::new("git")
            .args(["init", "-q", "--bare", "-b", "main"])
            .arg(&origin)
            .status()
            .expect("git init --bare");
        assert!(created.success());
        self.git(&["remote", "add", "origin", &origin.to_string_lossy()]);
        self.git(&["push", "-q", "-u", "origin", "main"]);
        self
    }

    pub fn origin_path(&self) -> PathBuf {
        self.root.path().join("origin.git")
    }

    /// El commit al que apunta una rama del remoto bare.
    pub fn origin_ref(&self, branch: &str) -> Option<String> {
        let output = Command::new("git")
            .args(["rev-parse", "--verify", "-q", branch])
            .current_dir(self.origin_path())
            .output()
            .ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// Deja un merge a medias con `notes.txt` en conflicto.
    pub fn with_merge_conflict(self) -> Self {
        self.commit_file("notes.txt", "base\n", "Add notes");
        self.git(&["checkout", "-q", "-b", "feature"]);
        self.commit_file("notes.txt", "from feature\n", "Notes on feature");
        self.git(&["checkout", "-q", "main"]);
        self.commit_file("notes.txt", "from main\n", "Notes on main");
        assert!(
            self.try_git(&["merge", "feature"]).is_err(),
            "the merge should stop on a conflict"
        );
        self
    }
}
