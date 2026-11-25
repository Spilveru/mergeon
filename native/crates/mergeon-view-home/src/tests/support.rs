//! Fixtures de los tests de interacción: el modelo real sobre un backend sin
//! efectos fuera de su directorio temporal, una ventana sin pantalla con la
//! misma raíz que la app y repositorios git reales.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use gpui_kit::test::{ElementSnapshot, TestWindowExt as _};
use gpui_kit::{
    AnyView, AnyWindowHandle, App, AppContext as _, Bounds, Context, ElementId, Entity,
    IntoElement, ParentElement as _, Render, SharedString, Styled as _, Subscription,
    TestAppContext, Window, WindowBounds, WindowOptions, div, point, px, size,
};
use mergeon_app::backend::Backend;
use mergeon_app::model_types::notify::NotifyType;
use mergeon_app::testing::{FakeGitHubTransport, RecordingLauncher, RecordingUiHost, TestBackend};
use mergeon_app::{AppEvent, AppModel};
use mergeon_gitcmd::Git;
use mergeon_store::{GitHubAccount, RecentRepository, Store};
use serde_json::Value;

/// Margen para todo lo que en un test "tiene que acabar pasando".
const PATIENCE: Duration = Duration::from_secs(20);

/// La vista que el test esté ejercitando.
pub(crate) struct Stage {
    content: Option<AnyView>,
}

impl Render for Stage {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().children(self.content.clone())
    }
}

/// Configuración global de git propia de los tests: ni lee ni toca la del
/// usuario. La identidad global es "Global User".
fn global_gitconfig() -> &'static PathBuf {
    static CONFIG: OnceLock<PathBuf> = OnceLock::new();
    CONFIG.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("mergeon-view-home-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("crear la carpeta de configuración");
        let path = dir.join("gitconfig");
        write_global_identity(&path);
        // SAFETY: se ejecuta una vez, antes de lanzar ningún git.
        unsafe {
            std::env::set_var("GIT_CONFIG_GLOBAL", &path);
            std::env::set_var("GIT_CONFIG_SYSTEM", "/dev/null");
            std::env::set_var("GIT_TERMINAL_PROMPT", "0");
        }
        path
    })
}

/// Escribe la configuración global de partida. Se sustituye de una vez (con
/// un renombrado) para que un git de otro test nunca la lea a medias.
fn write_global_identity(path: &Path) {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let staging = path.with_extension(format!(
        "{}.tmp",
        NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    std::fs::write(
        &staging,
        "[user]\n\tname = Global User\n\temail = global@example.com\n[init]\n\tdefaultBranch = main\n",
    )
    .expect("escribir gitconfig");
    std::fs::rename(&staging, path).expect("instalar gitconfig");
}

/// Deja la configuración global como al empezar y la reserva: los tests que
/// la leen o la escriben no se pisan entre sí.
pub(crate) fn reset_global_identity() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    let guard = LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    write_global_identity(global_gitconfig());
    guard
}

pub(crate) fn git_in(dir: &Path, args: &[&str]) -> String {
    global_gitconfig();
    match Git::new(dir).raw(args) {
        Ok(output) => output,
        Err(error) => panic!("git {args:?} failed: {}", error.message),
    }
}

/// Una carpeta temporal donde crear repositorios agrupados por carpeta, como
/// los ve el inicio (`<raíz>/<grupo>/<nombre>`).
pub(crate) struct Workspace {
    _root: tempfile::TempDir,
    pub base: PathBuf,
}

impl Workspace {
    pub fn new() -> Self {
        global_gitconfig();
        let root = tempfile::Builder::new()
            .prefix("mergeon-view-home-")
            .tempdir()
            .expect("crear carpeta temporal");
        // La ruta real: en macOS el temporal cuelga de un symlink.
        let base = mergeon_gitcmd::paths::canonicalize(root.path()).expect("ruta real");
        Self { _root: root, base }
    }

    /// La ruta como la guardan el modelo y los recientes: con `/` también en
    /// Windows (los ids de las filas y los nombres salen de ella).
    pub fn path(&self, relative: &str) -> String {
        self.base
            .join(relative)
            .to_string_lossy()
            .replace('\\', "/")
    }

    /// Carpeta vacía, sin repositorio.
    pub fn directory(&self, relative: &str) -> String {
        std::fs::create_dir_all(self.base.join(relative)).expect("crear carpeta");
        self.path(relative)
    }

    /// Repositorio con un commit en `main`. `readme` es el contenido de su
    /// README.md, si lo lleva.
    pub fn repository(&self, relative: &str, readme: Option<&str>) -> String {
        let dir = self.base.join(relative);
        std::fs::create_dir_all(&dir).expect("crear repositorio");
        git_in(&dir, &["init", "-b", "main"]);
        for (key, value) in [("commit.gpgsign", "false"), ("core.autocrlf", "false")] {
            git_in(&dir, &["config", key, value]);
        }
        std::fs::write(dir.join("main.txt"), "content\n").expect("escribir archivo");
        if let Some(readme) = readme {
            std::fs::write(dir.join("README.md"), readme).expect("escribir README");
        }
        git_in(&dir, &["add", "--all"]);
        git_in(&dir, &["commit", "-m", "Initial commit"]);
        self.path(relative)
    }

    /// Un "GitHub" local: repositorio bare con `main` publicado. Devuelve su
    /// ruta, que sirve de URL de clonado.
    pub fn bare_remote(&self, name: &str) -> String {
        let source = self.repository(&format!("sources/{name}"), Some("# Remote\n"));
        let remote = self.base.join(format!("remotes/{name}.git"));
        std::fs::create_dir_all(&remote).expect("crear remoto");
        git_in(&remote, &["init", "--bare", "-b", "main"]);
        let remote = remote.to_string_lossy().into_owned();
        git_in(Path::new(&source), &["push", &remote, "main"]);
        remote
    }
}

pub(crate) fn recent(path: &str) -> RecentRepository {
    let name = path.rsplit('/').next().unwrap_or(path).to_string();
    RecentRepository {
        id: path.to_string(),
        name,
        path: path.to_string(),
        current_branch: "main".to_string(),
        ..Default::default()
    }
}

pub(crate) fn account(value: Value) -> GitHubAccount {
    serde_json::from_value(value).expect("cuenta de GitHub")
}

/// Un store en memoria con recientes (el primero es el más reciente) y
/// cuentas; la primera cuenta queda activa.
pub(crate) fn seeded_store(recents: &[&str], accounts: &[GitHubAccount]) -> Store {
    let store = TestBackend::memory_store();
    for path in recents.iter().rev() {
        store
            .add_recent_repository(&recent(path))
            .expect("sembrar recientes");
    }
    for account in accounts {
        store.save_github_account(account).expect("sembrar cuenta");
    }
    if let Some(first) = accounts.first() {
        store
            .set_active_github_account_id(Some(&first.id))
            .expect("sembrar cuenta activa");
    }
    store
}

/// El modelo, la ventana y todo lo que un test necesita mirar.
pub(crate) struct TestApp {
    pub model: Entity<AppModel>,
    pub host: Rc<RecordingUiHost>,
    pub launcher: Arc<RecordingLauncher>,
    pub github: Arc<FakeGitHubTransport>,
    pub backend: Arc<Backend>,
    pub window: AnyWindowHandle,
    stage: Entity<Stage>,
    events: Rc<RefCell<Vec<AppEvent>>>,
    _subscription: Subscription,
}

impl TestApp {
    pub fn new(cx: &mut TestAppContext) -> Self {
        Self::with_backend(TestBackend::new(), cx)
    }

    pub fn with_store(store: Store, cx: &mut TestAppContext) -> Self {
        Self::with_backend(TestBackend::new().with_store(store), cx)
    }

    pub fn with_backend(mut test_backend: TestBackend, cx: &mut TestAppContext) -> Self {
        // Los eventos del backend (cola de mutaciones, actualizador) llegan
        // desde sus propios hilos.
        cx.executor().allow_parking();
        global_gitconfig();

        let (backend, events) = test_backend.build();
        let host = RecordingUiHost::new();
        let (model, window, stage) = cx.update(|cx| {
            mergeon_ui::init(cx);
            // Las animaciones de entrada usan el reloj real; sin movimiento
            // quedan resueltas en el primer frame.
            cx.set_reduce_motion(true);
            crate::init(cx);
            // Ningún test sale a la red a por un avatar.
            crate::set_avatar_fetcher(Arc::new(|_| Err("offline".to_string())), cx);

            let model = AppModel::init(backend.clone(), events, cx);
            model.update(cx, |model, cx| model.set_ui_host(host.clone(), cx));

            let (window, stage) = gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: point(px(0.), px(0.)),
                        size: size(px(1200.), px(800.)),
                    })),
                    ..Default::default()
                },
                cx,
                |_, cx| cx.new(|_| Stage { content: None }),
            )
            .expect("no se pudo abrir la ventana de prueba");
            (model, window, stage)
        });

        let recorded = Rc::new(RefCell::new(Vec::new()));
        let subscription = cx.update(|cx| {
            let recorded = recorded.clone();
            cx.subscribe(&model, move |_, event: &AppEvent, _| {
                recorded.borrow_mut().push(event.clone());
            })
        });

        Self {
            model,
            host,
            launcher: test_backend.launcher.clone(),
            github: test_backend.github.clone(),
            backend,
            window,
            stage,
            events: recorded,
            _subscription: subscription,
        }
    }

    /// El `onMounted` de `App.vue`: carga recientes, cuentas y ajustes.
    pub fn start(&self, cx: &mut TestAppContext) {
        self.model.update(cx, |model, cx| model.start(cx)).detach();
        self.settle(cx);
    }

    /// Abre un repositorio como lo hace el store y espera a su primera lectura.
    pub fn open_repository(&self, path: &str, cx: &mut TestAppContext) {
        let path = path.to_string();
        self.model
            .update(cx, |model, cx| {
                model.add_repository(&path, mergeon_app::AddRepository::default(), cx)
            })
            .detach();
        self.settle(cx);
    }

    /// Monta una vista en la ventana.
    pub fn show<V: Render>(
        &self,
        cx: &mut TestAppContext,
        build: impl FnOnce(&mut Window, &mut Context<V>) -> V,
    ) -> Entity<V> {
        let stage = self.stage.clone();
        let view = cx
            .update_window(self.window, |_, window, cx| {
                let view = cx.new(|cx| build(window, cx));
                stage.update(cx, |stage, cx| {
                    stage.content = Some(view.clone().into());
                    cx.notify();
                });
                view
            })
            .expect("la ventana de prueba se cerró");
        self.settle(cx);
        view
    }

    /// Pone en la ventana una vista ya creada (o nada): lo que estaba deja
    /// de pintarse, como el inicio cuando se abre un repositorio.
    pub fn put(&self, cx: &mut TestAppContext, view: Option<AnyView>) {
        self.stage.update(cx, |stage, cx| {
            stage.content = view;
            cx.notify();
        });
        self.settle(cx);
    }

    /// Deja que terminen los trabajos pendientes.
    pub fn settle(&self, cx: &mut TestAppContext) {
        cx.run_until_parked();
    }

    /// Ejecuta `f` dentro de la ventana, con un frame ya dibujado. No deja
    /// correr los trabajos pendientes: lo que `f` lance sigue en vuelo al
    /// volver, hasta el siguiente [`TestApp::settle`].
    pub fn in_window<R>(
        &self,
        cx: &mut TestAppContext,
        f: impl FnOnce(&mut Window, &mut App) -> R,
    ) -> R {
        cx.update_window(self.window, |_, window, cx| {
            window.render_frame(cx);
            f(window, cx)
        })
        .expect("la ventana de prueba se cerró")
    }

    /// Clic real sobre el elemento `id`, y deja terminar lo que desencadene.
    pub fn click(&self, cx: &mut TestAppContext, id: impl Into<ElementId>) {
        let id = id.into();
        self.in_window(cx, |window, cx| window.click(id, cx));
        self.settle(cx);
    }

    /// Clic sobre el elemento `id` de dentro de `scope` (ids que se repiten,
    /// como las opciones de dos grupos de radio).
    pub fn click_within(
        &self,
        cx: &mut TestAppContext,
        scope: impl Into<ElementId>,
        id: impl Into<ElementId>,
    ) {
        let (scope, id) = (scope.into(), id.into());
        self.in_window(cx, |window, cx| window.within(scope).click(id, cx));
        self.settle(cx);
    }

    /// Doble clic sobre el elemento `id`.
    pub fn double_click(&self, cx: &mut TestAppContext, id: impl Into<ElementId>) {
        let id = id.into();
        self.in_window(cx, |window, cx| window.double_click(id, cx));
        self.settle(cx);
    }

    /// El elemento `id` del último frame, si existe.
    pub fn find(
        &self,
        cx: &mut TestAppContext,
        id: impl Into<ElementId>,
    ) -> Option<ElementSnapshot> {
        let id = id.into();
        self.in_window(cx, |window, _| window.try_find(id))
    }

    pub fn exists(&self, cx: &mut TestAppContext, id: impl Into<ElementId>) -> bool {
        self.find(cx, id).is_some()
    }

    /// Texto accesible del elemento `id`.
    pub fn label(&self, cx: &mut TestAppContext, id: impl Into<ElementId>) -> Option<String> {
        self.find(cx, id)
            .and_then(|snapshot| snapshot.label().map(str::to_string))
    }

    /// Títulos de los diálogos abiertos, de abajo hacia arriba.
    pub fn dialog_titles(&self, cx: &mut TestAppContext) -> Vec<String> {
        self.in_window(cx, |window, cx| {
            mergeon_ui::dialog::open_titles(window, cx)
                .iter()
                .map(|title| title.to_string())
                .collect()
        })
    }

    /// Eventos emitidos por el modelo, en orden.
    pub fn events(&self) -> Vec<AppEvent> {
        self.events.borrow().clone()
    }

    pub fn has_toast(&self, kind: NotifyType, message: &str) -> bool {
        self.host.has_message(kind, message)
    }

    /// Espera a que `condition` se cumpla, dando tiempo a los hilos del
    /// backend a entregar sus eventos.
    pub fn wait_for(
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
}

/// Id de la fila de un repositorio en el inicio.
pub(crate) fn repo_row(path: &str) -> SharedString {
    SharedString::from(format!("repo:{path}"))
}
