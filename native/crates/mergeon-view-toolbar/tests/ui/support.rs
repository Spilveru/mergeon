//! Fixtures de los tests de interfaz: el modelo sobre un backend sin efectos
//! fuera del temporal, una ventana sin pantalla con la barra y su franja de
//! estado (como las monta la cáscara) y repositorios git reales.

use std::path::PathBuf;
use std::process::Command;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AnyElement, AnyWindowHandle, App, AppContext as _, Bounds, Context, ElementId, Entity,
    IntoElement, ParentElement as _, Render, Styled as _, Subscription, TestAppContext, Window,
    WindowBounds, WindowOptions, div, point, px, size,
};
use mergeon_app::testing::{RecordingLauncher, RecordingUiHost, TestBackend};
use mergeon_app::{AddRepository, AppEvent, AppModel};
use mergeon_ui::dialog;
use mergeon_view_toolbar::{
    OperationStatusSlot, StatusBarView, ToolbarView, handle_dialog_request,
};

/// Margen para todo lo que en un test "tiene que acabar pasando".
const PATIENCE: Duration = Duration::from_secs(20);

type Extra = Box<dyn Fn(&mut Window, &mut Context<Shell>) -> AnyElement>;

/// Lo que hará la cáscara: la barra, la franja de estado y la delegación de
/// los diálogos que piden los flujos de `AppModel`. `extra` deja a un test
/// dibujar además una pieza suelta.
pub struct Shell {
    pub toolbar: Entity<ToolbarView>,
    pub status: Entity<OperationStatusSlot>,
    pub status_bar: Entity<StatusBarView>,
    extra: Option<Extra>,
    _subscription: Subscription,
}

impl Shell {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let model = AppModel::global(cx);
        let subscription = cx.subscribe_in(&model, window, |_, _, event: &AppEvent, window, cx| {
            if let AppEvent::ShowDialog(request) = event {
                handle_dialog_request(request, window, cx);
            }
        });
        Self {
            toolbar: cx.new(|cx| ToolbarView::new(window, cx)),
            status: cx.new(|cx| OperationStatusSlot::new(window, cx)),
            status_bar: cx.new(|cx| StatusBarView::new(window, cx)),
            extra: None,
            _subscription: subscription,
        }
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let extra = self.extra.as_ref().map(|extra| extra(window, cx));
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(self.toolbar.clone())
            .child(self.status.clone())
            .children(extra)
            .child(self.status_bar.clone())
    }
}

/// El modelo y la ventana con todo lo que un test necesita mirar.
pub struct TestApp {
    pub model: Entity<AppModel>,
    pub host: Rc<RecordingUiHost>,
    pub launcher: Arc<RecordingLauncher>,
    pub window: AnyWindowHandle,
    pub shell: Entity<Shell>,
}

impl TestApp {
    /// Ventana ancha: la barra en su disposición de una fila.
    pub fn new(cx: &mut TestAppContext) -> Self {
        Self::with_width(1200., cx)
    }

    pub fn with_width(width: f32, cx: &mut TestAppContext) -> Self {
        Self::with_backend(TestBackend::new(), width, cx)
    }

    /// Sobre un backend de prueba ya preparado (store sembrado, GitHub
    /// guionizado...).
    pub fn with_backend(
        mut test_backend: TestBackend,
        width: f32,
        cx: &mut TestAppContext,
    ) -> Self {
        // Los eventos del backend (cola de mutaciones) llegan desde sus
        // propios hilos: el planificador de tests tiene que admitirlo.
        cx.executor().allow_parking();

        let (backend, events) = test_backend.build();
        let host = RecordingUiHost::new();
        let model = cx.update(|cx| AppModel::init(backend, events, cx));
        model.update(cx, |model, cx| model.set_ui_host(host.clone(), cx));

        let (window, shell) = cx.update(|cx| {
            mergeon_ui::init(cx);
            mergeon_view_toolbar::init(cx);
            // Las animaciones de entrada usan el reloj real; sin movimiento
            // quedan resueltas en el primer frame.
            cx.set_reduce_motion(true);
            let (window, shell) = gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: point(px(0.), px(0.)),
                        size: size(px(width), px(800.)),
                    })),
                    ..Default::default()
                },
                cx,
                |window, cx| cx.new(|cx| Shell::new(window, cx)),
            )
            .expect("no se pudo abrir la ventana de prueba");
            (window, shell)
        });

        Self {
            model,
            host,
            launcher: test_backend.launcher.clone(),
            window,
            shell,
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

    /// Clic en un control de la ventana.
    pub fn click(&self, cx: &mut TestAppContext, id: impl Into<ElementId>) {
        let id = id.into();
        self.in_window(cx, |window, cx| window.click(id, cx));
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

    /// Espera a que se abra un diálogo con ese título (los flujos que leen
    /// del repositorio antes de pedirlo tardan un momento).
    pub fn wait_for_dialog(&self, cx: &mut TestAppContext, title: &str) {
        let deadline = Instant::now() + PATIENCE;
        while !self.dialogs(cx).iter().any(|open| open == title) {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for the `{title}` dialog"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Espera a que no quede ningún diálogo abierto.
    pub fn wait_for_no_dialogs(&self, cx: &mut TestAppContext) {
        let deadline = Instant::now() + PATIENCE;
        while !self.dialogs(cx).is_empty() {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for the dialogs to close"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Espera a que un elemento esté en pantalla.
    pub fn wait_for_element(&self, cx: &mut TestAppContext, id: &'static str) {
        let deadline = Instant::now() + PATIENCE;
        while !self.exists(cx, id) {
            assert!(Instant::now() < deadline, "timed out waiting for `{id}`");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Espera a que un elemento tenga esa etiqueta accesible.
    pub fn wait_for_label(&self, cx: &mut TestAppContext, id: &'static str, label: &str) {
        let deadline = Instant::now() + PATIENCE;
        while self.label(cx, id).as_deref() != Some(label) {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for `{id}` to read `{label}`"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
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

    /// Dibuja además una pieza suelta bajo la barra.
    pub fn show(
        &self,
        cx: &mut TestAppContext,
        extra: impl Fn(&mut Window, &mut Context<Shell>) -> AnyElement + 'static,
    ) {
        self.shell.update(cx, |shell, cx| {
            shell.extra = Some(Box::new(extra));
            cx.notify();
        });
    }

    /// Espera a que `condition` se cumpla, dando tiempo a los hilos del
    /// backend a entregar sus eventos.
    pub fn wait_for(
        &self,
        cx: &mut TestAppContext,
        what: &str,
        mut condition: impl FnMut(&AppModel) -> bool,
    ) {
        let deadline = Instant::now() + PATIENCE;
        loop {
            cx.run_until_parked();
            if self.model.read_with(cx, |model, _| condition(model)) {
                return;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Como [`Self::wait_for`], sobre cualquier condición.
    pub fn wait_until(
        &self,
        cx: &mut TestAppContext,
        what: &str,
        mut condition: impl FnMut() -> bool,
    ) {
        let deadline = Instant::now() + PATIENCE;
        loop {
            cx.run_until_parked();
            if condition() {
                return;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
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
        let root = tempfile::Builder::new()
            .prefix("mergeon-view-toolbar-")
            .tempdir()
            .expect("temporary directory");
        // La ruta real: en macOS el temporal cuelga de un symlink.
        let base = mergeon_gitcmd::paths::canonicalize(root.path())
            .expect("canonical temporary directory");
        let path = base.join("demo");
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
        std::fs::write(self.path.join(file), content).expect("write file");
    }

    pub fn commit_file(&self, file: &str, content: &str, message: &str) {
        self.write(file, content);
        self.git(&["add", "--all"]);
        self.git(&["commit", "-q", "-m", message]);
    }

    pub fn head_short(&self) -> String {
        self.git(&["rev-parse", "--short", "HEAD"])
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

    /// Resuelve el conflicto de [`Self::with_merge_conflict`].
    pub fn resolve_conflict(self) -> Self {
        self.write("notes.txt", "from main and feature\n");
        self.git(&["add", "notes.txt"]);
        self
    }
}
