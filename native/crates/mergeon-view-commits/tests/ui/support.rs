//! Fixtures de los tests de interfaz: el modelo sobre un backend sin efectos
//! fuera del temporal, una ventana sin pantalla que pinta la vista bajo prueba
//! y repositorios git reales.

use std::path::PathBuf;
use std::process::Command;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AnyView, AnyWindowHandle, App, AppContext as _, Bounds, Context, ElementId, Entity,
    IntoElement, ParentElement as _, Pixels, Render, Styled as _, TestAppContext, Window,
    WindowBounds, WindowOptions, div, point, px, size,
};
use mergeon_app::testing::{RecordingLauncher, RecordingUiHost, TestBackend};
use mergeon_app::{AddRepository, AppModel};
use mergeon_backend::Store;
use mergeon_ui::dialog;

/// Margen para todo lo que en un test "tiene que acabar pasando".
const PATIENCE: Duration = Duration::from_secs(30);

/// La vista que pinta la ventana de prueba.
pub struct Host {
    content: Option<AnyView>,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().flex().size_full().children(self.content.clone())
    }
}

/// El modelo y la ventana con todo lo que un test necesita mirar.
pub struct TestApp {
    pub model: Entity<AppModel>,
    pub host: Rc<RecordingUiHost>,
    pub window: AnyWindowHandle,
    pub root: Entity<Host>,
    /// Lo que el backend habría lanzado (el editor externo, por ejemplo).
    pub launcher: Arc<RecordingLauncher>,
}

impl TestApp {
    pub fn new(cx: &mut TestAppContext) -> Self {
        Self::with_store(1200., TestBackend::memory_store(), cx)
    }

    pub fn with_width(width: f32, cx: &mut TestAppContext) -> Self {
        Self::with_store(width, TestBackend::memory_store(), cx)
    }

    /// Ventana de `width` × `height`.
    pub fn with_size(width: f32, height: f32, cx: &mut TestAppContext) -> Self {
        Self::build(width, height, TestBackend::memory_store(), cx)
    }

    /// Ventana de `width` × 800 sobre un store ya sembrado.
    pub fn with_store(width: f32, store: Store, cx: &mut TestAppContext) -> Self {
        Self::build(width, 800., store, cx)
    }

    fn build(width: f32, height: f32, store: Store, cx: &mut TestAppContext) -> Self {
        // Los eventos del backend llegan desde sus propios hilos.
        cx.executor().allow_parking();

        let mut test_backend = TestBackend::new().with_store(store);
        let (backend, events) = test_backend.build();
        let host = RecordingUiHost::new();
        let model = cx.update(|cx| AppModel::init(backend, events, cx));
        model.update(cx, |model, cx| model.set_ui_host(host.clone(), cx));

        let (window, root) = cx.update(|cx| {
            mergeon_ui::init(cx);
            mergeon_view_commits::init(cx);
            cx.set_reduce_motion(true);
            let (window, root) = gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: point(px(0.), px(0.)),
                        size: size(px(width), px(height)),
                    })),
                    ..Default::default()
                },
                cx,
                |_, cx| cx.new(|_| Host { content: None }),
            )
            .expect("no se pudo abrir la ventana de prueba");
            (window, root)
        });

        Self {
            model,
            host,
            window,
            root,
            launcher: test_backend.launcher.clone(),
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

    /// Crea una entidad con la ventana a mano y la pinta.
    pub fn mount<V: Render>(
        &self,
        cx: &mut TestAppContext,
        build: impl FnOnce(&mut Window, &mut Context<V>) -> V,
    ) -> Entity<V> {
        let root = self.root.clone();
        cx.update_window(self.window, |_, window, cx| {
            let view = cx.new(|cx| build(window, cx));
            root.update(cx, |root, cx| {
                root.content = Some(view.clone().into());
                cx.notify();
            });
            view
        })
        .expect("la ventana de prueba se cerró")
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

    /// Clic izquierdo con modificadores (Ctrl/⌘+clic, Shift+clic): los
    /// clics de `TestWindowExt` van siempre sin ellos.
    pub fn click_with(
        &self,
        cx: &mut TestAppContext,
        id: impl Into<ElementId>,
        modifiers: gpui_kit::Modifiers,
    ) {
        use gpui_kit::{
            InputEvent as _, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
        };
        let id = id.into();
        self.in_window(cx, |window, cx| {
            let position = window.find(id).bounds().center();
            window.dispatch_event(
                MouseMoveEvent {
                    position,
                    pressed_button: None,
                    modifiers,
                }
                .to_platform_input(),
                cx,
            );
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
        });
    }

    pub fn exists(&self, cx: &mut TestAppContext, id: impl Into<ElementId>) -> bool {
        let id = id.into();
        self.in_window(cx, |window, _| window.try_find(id).is_some())
    }

    /// Dónde quedó pintado un elemento (en píxeles de la ventana).
    pub fn bounds(&self, cx: &mut TestAppContext, id: impl Into<ElementId>) -> Bounds<Pixels> {
        let id = id.into();
        self.in_window(cx, |window, _| window.find(id).bounds())
    }

    pub fn label(&self, cx: &mut TestAppContext, id: impl Into<ElementId>) -> Option<String> {
        let id = id.into();
        self.in_window(cx, |window, _| {
            window
                .try_find(id)
                .and_then(|element| element.label().map(str::to_string))
        })
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

    pub fn wait_for_dialog(&self, cx: &mut TestAppContext, title: &str) {
        self.wait_until(cx, &format!("the `{title}` dialog"), |app, cx| {
            app.dialogs(cx).iter().any(|open| open == title)
        });
    }

    /// Clic en la entrada `index` del menú abierto (los separadores cuentan).
    pub fn click_menu_item(&self, cx: &mut TestAppContext, index: usize) {
        self.in_window(cx, move |window, cx| {
            assert!(window.try_find("popup-menu").is_some(), "no menu is open");
            window.within("popup-menu").click(index, cx);
        });
    }

    /// Cuántas entradas tiene el menú abierto (separadores y cabecera
    /// incluidos).
    pub fn menu_len(&self, cx: &mut TestAppContext) -> usize {
        self.in_window(cx, |window, _| {
            let menu = window.within("popup-menu");
            (0usize..64)
                .take_while(|index| menu.try_find(*index).is_some())
                .count()
        })
    }

    /// El texto copiado al portapapeles.
    pub fn clipboard(&self, cx: &mut TestAppContext) -> Option<String> {
        cx.read_from_clipboard().and_then(|item| item.text())
    }

    pub fn menu_is_open(&self, cx: &mut TestAppContext) -> bool {
        self.in_window(cx, |window, _| window.try_find("popup-menu").is_some())
    }

    /// Espera a que `condition` se cumpla, dando tiempo a los hilos del
    /// backend a entregar sus resultados.
    pub fn wait_until(
        &self,
        cx: &mut TestAppContext,
        what: &str,
        mut condition: impl FnMut(&Self, &mut TestAppContext) -> bool,
    ) {
        let deadline = Instant::now() + PATIENCE;
        loop {
            cx.run_until_parked();
            if condition(self, cx) {
                return;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Como [`Self::wait_until`], leyendo una entidad.
    pub fn wait_for<V: 'static>(
        &self,
        cx: &mut TestAppContext,
        entity: &Entity<V>,
        what: &str,
        mut condition: impl FnMut(&V, &App) -> bool,
    ) {
        let entity = entity.clone();
        self.wait_until(cx, what, move |_, cx| {
            entity.read_with(cx, |value, cx| condition(value, cx))
        });
    }

    /// Deja pasar un rato sin que deba ocurrir nada (para comprobar que algo
    /// no se repite).
    pub fn settle(&self, cx: &mut TestAppContext, millis: u64) {
        let deadline = Instant::now() + Duration::from_millis(millis);
        while Instant::now() < deadline {
            cx.run_until_parked();
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
        Self::named("demo")
    }

    pub fn named(name: &str) -> Self {
        let root = tempfile::Builder::new()
            .prefix("mergeon-view-commits-")
            .tempdir()
            .expect("temporary directory");
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

    pub fn commit_file(&self, file: &str, content: &str, message: &str) {
        self.write(file, content);
        self.git(&["add", "--all"]);
        self.git(&["commit", "-q", "-m", message]);
    }

    pub fn head(&self) -> String {
        self.git(&["rev-parse", "HEAD"])
    }

    pub fn rev(&self, reference: &str) -> String {
        self.git(&["rev-parse", reference])
    }

    pub fn short(&self, reference: &str) -> String {
        self.git(&["rev-parse", "--short", reference])
    }

    pub fn current_branch(&self) -> String {
        self.try_git(&["symbolic-ref", "--short", "-q", "HEAD"])
            .unwrap_or_else(|_| "HEAD".to_string())
    }

    /// `count` commits encima de la rama actual, sin pasar por el índice
    /// (rápido aunque sean miles).
    pub fn many_commits(&self, count: usize, prefix: &str) {
        let mut stream = String::new();
        let parent = self.head();
        for index in 0..count {
            let message = format!("{prefix} {index}");
            stream.push_str(&format!(
                "commit refs/heads/main\nmark :{}\ncommitter Test User <test@example.com> {} +0000\ndata {}\n{}\n",
                index + 1,
                1_780_000_000 + index,
                message.len(),
                message
            ));
            if index == 0 {
                stream.push_str(&format!("from {parent}\n"));
            }
            stream.push('\n');
        }
        let mut child = Command::new("git")
            .args(["fast-import", "--quiet"])
            .current_dir(&self.path)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .stdin(std::process::Stdio::piped())
            .spawn()
            .expect("git fast-import");
        use std::io::Write as _;
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(stream.as_bytes())
            .expect("write stream");
        assert!(child.wait().expect("fast-import").success());
        self.git(&["reset", "-q", "--hard", "main"]);
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

    /// Un compañero publica `message` en `origin/main` y el repositorio lo
    /// trae con fetch (queda por detrás de su upstream).
    pub fn colleague_pushes(&self, message: &str) {
        let colleague = self.root.path().join("colleague");
        if !colleague.exists() {
            let cloned = Command::new("git")
                .args(["clone", "-q"])
                .arg(self.origin_path())
                .arg(&colleague)
                .status()
                .expect("git clone");
            assert!(cloned.success());
        }
        let run = |args: &[&str]| {
            let status = Command::new("git")
                .args(args)
                .current_dir(&colleague)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_AUTHOR_NAME", "Luis Vega")
                .env("GIT_AUTHOR_EMAIL", "luis@example.com")
                .env("GIT_COMMITTER_NAME", "Luis Vega")
                .env("GIT_COMMITTER_EMAIL", "luis@example.com")
                .status()
                .expect("git");
            assert!(status.success(), "git {args:?}");
        };
        run(&["commit", "-q", "--allow-empty", "-m", message]);
        run(&["push", "-q", "origin", "main"]);
        self.git(&["fetch", "-q", "origin"]);
    }
}

/// Posición de una entrada en el menú de un commit, contando separadores y
/// la cabecera (hash · asunto).
pub fn commit_menu_index(label: &str) -> usize {
    let commit = mergeon_model::commit_view_utils::CommitRow {
        parents: "parent".to_string(),
        ..Default::default()
    };
    let mut index = 1;
    for (group_index, group) in mergeon_view_commits::commit_menu(&commit, "main", true)
        .into_iter()
        .enumerate()
    {
        if group_index > 0 {
            index += 1;
        }
        for entry in group {
            if entry.label == label {
                return index;
            }
            index += 1;
        }
    }
    panic!("`{label}` is not in the commit menu");
}

type SceneRender = Box<dyn Fn(&mut Window, &mut Context<Scene>) -> gpui_kit::AnyElement>;

/// Vista de prueba que pinta lo que le pida el test en cada frame (para los
/// componentes sin estado propio).
pub struct Scene {
    render: SceneRender,
}

impl Scene {
    pub fn new(
        render: impl Fn(&mut Window, &mut Context<Scene>) -> gpui_kit::AnyElement + 'static,
    ) -> Self {
        Self {
            render: Box::new(render),
        }
    }
}

impl Render for Scene {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child((self.render)(window, cx))
    }
}
