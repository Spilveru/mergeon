//! Fixtures de los tests del sidebar: un `AppModel` sobre un backend sin
//! efectos fuera del directorio temporal, repositorios git reales y el sidebar
//! montado en una ventana sin pantalla.

#![allow(dead_code)]

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, Bounds, Context, Entity, IntoElement,
    ParentElement as _, Render, Styled as _, Subscription, TestAppContext, Window, WindowBounds,
    WindowOptions, div, point, px, size,
};
use mergeon_app::testing::{RecordingUiHost, TestBackend};
use mergeon_app::{AddRepository, AppModel, OpenRepository};
use mergeon_gitcmd::Git;
use mergeon_view_sidebar::{MenuEntry, Sidebar, SidebarEvent, SidebarResizer};

/// Margen para todo lo que en un test "tiene que acabar pasando".
const PATIENCE: Duration = Duration::from_secs(30);

pub fn git_in(dir: &Path, args: &[&str]) -> String {
    match Git::new(dir).raw(args) {
        Ok(output) => output,
        Err(error) => panic!("git {args:?} failed: {}", error.message),
    }
}

fn configure_identity(repo_path: &Path) {
    for (key, value) in [
        ("user.name", "Test User"),
        ("user.email", "test@example.com"),
        ("commit.gpgsign", "false"),
        ("tag.gpgsign", "false"),
        ("core.autocrlf", "false"),
    ] {
        git_in(repo_path, &["config", key, value]);
    }
}

/// Un repositorio con identidad propia y rama `main`, sin depender de la
/// configuración global de la máquina.
pub struct TestRepo {
    root: tempfile::TempDir,
    pub path: PathBuf,
}

impl TestRepo {
    /// Repositorio con un commit base en `main`.
    pub fn new() -> Self {
        let root = tempfile::Builder::new()
            .prefix("mergeon-sidebar-")
            .tempdir()
            .expect("temp dir");
        let base = mergeon_gitcmd::paths::canonicalize(root.path()).expect("canonical temp dir");
        let path = base.join("demo");
        std::fs::create_dir(&path).expect("repo dir");
        git_in(&path, &["init", "-b", "main"]);
        configure_identity(&path);
        let repo = Self { root, path };
        repo.commit_file("README.md", "# Demo\n", "Initial commit");
        repo
    }

    /// `main` con checkout y una rama `feature` con un commit propio, como el
    /// repositorio de `Sidebar.test.js`.
    pub fn with_feature() -> Self {
        let repo = Self::new();
        repo.git(&["checkout", "-b", "feature"]);
        repo.commit_file("feature.txt", "feature\n", "Add feature");
        repo.git(&["checkout", "main"]);
        repo
    }

    /// La ruta como la maneja el modelo (y la reciben las vistas): con `/`
    /// también en Windows. `path` queda con los separadores del sistema.
    pub fn path_string(&self) -> String {
        self.path.to_string_lossy().replace('\\', "/")
    }

    /// El `id` que el modelo da al repositorio (su ruta normalizada).
    pub fn id(&self) -> String {
        mergeon_model::repositories::normalize_repository_path(&self.path_string())
    }

    pub fn root(&self) -> PathBuf {
        mergeon_gitcmd::paths::canonicalize(self.root.path()).expect("canonical temp dir")
    }

    pub fn git(&self, args: &[&str]) -> String {
        git_in(&self.path, args)
    }

    pub fn write(&self, name: &str, content: &str) {
        std::fs::write(self.path.join(name), content).expect("write file");
    }

    pub fn head(&self) -> String {
        self.git(&["rev-parse", "HEAD"]).trim().to_string()
    }

    pub fn rev_parse(&self, reference: &str) -> String {
        self.git(&["rev-parse", reference]).trim().to_string()
    }

    pub fn current_branch(&self) -> String {
        self.git(&["rev-parse", "--abbrev-ref", "HEAD"])
            .trim()
            .to_string()
    }

    pub fn commit_file(&self, name: &str, content: &str, message: &str) -> String {
        self.write(name, content);
        self.git(&["add", "--", name]);
        self.git(&["commit", "-m", message]);
        self.head()
    }

    pub fn branches(&self) -> Vec<String> {
        self.git(&["branch", "--format=%(refname:short)"])
            .lines()
            .map(str::to_string)
            .collect()
    }

    pub fn tags(&self) -> Vec<String> {
        self.git(&["tag"]).lines().map(str::to_string).collect()
    }

    pub fn stash_count(&self) -> usize {
        self.git(&["stash", "list"]).lines().count()
    }

    pub fn remotes(&self) -> Vec<String> {
        self.git(&["remote"]).lines().map(str::to_string).collect()
    }

    /// Crea un remoto bare local, lo registra como `name` y devuelve su ruta.
    pub fn add_bare_remote(&self, name: &str) -> PathBuf {
        let remote = self.bare_repository(name);
        self.git(&["remote", "add", name, &remote.to_string_lossy()]);
        remote
    }

    /// Un repositorio bare junto al de trabajo, sin registrarlo como remoto.
    pub fn bare_repository(&self, name: &str) -> PathBuf {
        let remote = self.root().join(format!("{name}.git"));
        std::fs::create_dir(&remote).expect("bare dir");
        git_in(&remote, &["init", "--bare", "-b", "main"]);
        remote
    }

    /// Un "GitHub" local: `origin` bare con `main` publicado y con tracking.
    pub fn publish_to_origin(&self) -> PathBuf {
        let origin = self.add_bare_remote("origin");
        self.git(&["push", "-u", "origin", "main"]);
        origin
    }

    /// Publica en `origin` una rama que no existe en local.
    pub fn publish_remote_only_branch(&self, name: &str) {
        self.git(&["branch", name]);
        self.git(&["push", "origin", name]);
        self.git(&["branch", "-D", name]);
    }

    /// Crea de una vez muchas refs (`refs/heads/...`, `refs/tags/...`,
    /// `refs/remotes/...`) apuntando a HEAD, con `git update-ref --stdin`.
    pub fn create_refs(&self, refs: impl IntoIterator<Item = String>) {
        use std::io::Write as _;
        let head = self.head();
        let input: String = refs
            .into_iter()
            .map(|reference| format!("create {reference} {head}\n"))
            .collect();
        let mut child = std::process::Command::new("git")
            .args(["update-ref", "--stdin"])
            .current_dir(&self.path)
            .stdin(std::process::Stdio::piped())
            .spawn()
            .expect("git update-ref");
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(input.as_bytes())
            .expect("write refs");
        assert!(child.wait().expect("git update-ref").success());
    }

    /// Deja cambios sin commitear en un archivo ya seguido.
    pub fn dirty(&self) {
        self.write("README.md", "# Demo\n\nlocal change\n");
    }
}

/// Vista de prueba: el sidebar y su divisor, como en la app.
struct Host {
    sidebar: Entity<Sidebar>,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .size_full()
            .child(self.sidebar.clone())
            .child(SidebarResizer::new(&self.sidebar))
            .child(div().flex_1())
    }
}

/// Qué secciones arrancan desplegadas (`settings.sidebarSections`).
pub fn sections(expanded: &[&str]) -> serde_json::Value {
    let has = |name: &str| expanded.contains(&name);
    serde_json::json!({
        "starred": has("starred"),
        "branches": has("branches"),
        "remotes": has("remotes"),
        "tags": has("tags"),
        "stashes": has("stashes"),
        "submodules": has("submodules"),
    })
}

/// Las secciones por defecto: Starred y Branches.
pub fn default_sections() -> serde_json::Value {
    sections(&["starred", "branches"])
}

pub struct Harness {
    pub model: Entity<AppModel>,
    pub host: Rc<RecordingUiHost>,
    pub window: AnyWindowHandle,
    pub sidebar: Entity<Sidebar>,
    pub repo_id: String,
    events: Rc<RefCell<Vec<SidebarEvent>>>,
    _subscription: Subscription,
}

impl Harness {
    /// Abre `repo` con el sidebar montado y espera a su primera lectura.
    pub async fn open(repo: &TestRepo, cx: &mut TestAppContext) -> Self {
        Self::open_with(repo, default_sections(), cx).await
    }

    pub async fn open_with(
        repo: &TestRepo,
        sidebar_sections: serde_json::Value,
        cx: &mut TestAppContext,
    ) -> Self {
        let mut harness = Self::mount(Some(sidebar_sections), cx);
        harness.repo_id = repo.id();
        harness.open_repository(repo, cx).await;
        harness
    }

    /// Monta el sidebar sin abrir ningún repositorio.
    pub fn mount(sidebar_sections: Option<serde_json::Value>, cx: &mut TestAppContext) -> Self {
        let store = TestBackend::memory_store();
        if let Some(sidebar_sections) = sidebar_sections {
            store
                .update_settings_json(&serde_json::json!({ "sidebarSections": sidebar_sections }))
                .expect("seed settings");
        }
        Self::mount_with(TestBackend::new().with_store(store), cx)
    }

    /// Monta el sidebar sobre un backend de prueba ya preparado (store
    /// sembrado, GitHub guionizado...).
    pub fn mount_with(mut test_backend: TestBackend, cx: &mut TestAppContext) -> Self {
        // Los eventos del backend llegan desde sus propios hilos.
        cx.executor().allow_parking();

        let (backend, backend_events) = test_backend.build();
        let host = RecordingUiHost::new();

        let (model, window, sidebar) = cx.update(|cx| {
            mergeon_ui::init(cx);
            mergeon_view_sidebar::init(cx);
            // Las animaciones de entrada usan el reloj real; sin movimiento
            // quedan resueltas en el primer frame.
            cx.set_reduce_motion(true);

            let model = AppModel::init(backend, backend_events, cx);
            model.update(cx, |model, cx| model.set_ui_host(host.clone(), cx));

            let (window, view) = gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: point(px(0.), px(0.)),
                        size: size(px(1200.), px(900.)),
                    })),
                    ..Default::default()
                },
                cx,
                |window, cx| {
                    cx.new(|cx| Host {
                        sidebar: cx.new(|cx| Sidebar::new(window, cx)),
                    })
                },
            )
            .expect("no se pudo abrir la ventana de prueba");
            let sidebar = view.read(cx).sidebar.clone();
            (model, window, sidebar)
        });

        let events = Rc::new(RefCell::new(Vec::new()));
        let subscription = cx.update(|cx| {
            let events = events.clone();
            cx.subscribe(&sidebar, move |_, event: &SidebarEvent, _| {
                events.borrow_mut().push(event.clone());
            })
        });
        cx.run_until_parked();

        Self {
            model,
            host,
            window,
            sidebar,
            repo_id: String::new(),
            events,
            _subscription: subscription,
        }
    }

    /// Abre un repositorio como lo hace la app y espera a que termine su
    /// hidratación (estado, ramas, remotos y destacadas).
    pub async fn open_repository(&self, repo: &TestRepo, cx: &mut TestAppContext) {
        self.model
            .update(cx, |model, cx| {
                model.add_repository(&repo.path_string(), AddRepository::default(), cx)
            })
            .await
            .expect("add repository");
        let repo_id = repo.id();
        self.wait(cx, "the opened repository to hydrate", |model| {
            model
                .repository(repo_id.as_str())
                .is_some_and(|repository| {
                    repository.sidebar_loaded.branches
                        && repository.sidebar_loaded.remotes
                        // Plegadas o no, se leen para el número de su cabecera.
                        && repository.sidebar_loaded.tags
                        && repository.sidebar_loaded.stashes
                        && repository.sidebar_loaded.submodules
                        && repository.last_mutation_operation.as_deref()
                            == Some("hydrate-opened-repository")
                })
        });
    }

    pub fn events(&self) -> Vec<SidebarEvent> {
        self.events.borrow().clone()
    }

    /// Espera a que `condition` se cumpla sobre el modelo, dando tiempo a los
    /// hilos del backend a entregar sus resultados.
    pub fn wait(
        &self,
        cx: &mut TestAppContext,
        what: &str,
        mut condition: impl FnMut(&AppModel) -> bool,
    ) {
        let deadline = Instant::now() + PATIENCE;
        loop {
            cx.run_until_parked();
            if self.model.read_with(cx, |model, _| condition(model)) {
                cx.run_until_parked();
                return;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Espera a que `condition` se cumpla, leyendo las entidades que haga falta.
    pub fn wait_app(
        &self,
        cx: &mut TestAppContext,
        what: &str,
        mut condition: impl FnMut(&App) -> bool,
    ) {
        let deadline = Instant::now() + PATIENCE;
        loop {
            cx.run_until_parked();
            if cx.read(|cx| condition(cx)) {
                cx.run_until_parked();
                return;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Espera a que `condition` se cumpla sobre el repositorio activo.
    pub fn wait_repo(
        &self,
        cx: &mut TestAppContext,
        what: &str,
        mut condition: impl FnMut(&OpenRepository) -> bool,
    ) {
        self.wait(cx, what, |model| {
            model.active_repository().is_some_and(&mut condition)
        });
    }

    /// Espera a que se cumpla una condición que se mira desde la ventana
    /// (elementos pintados, diálogos abiertos...).
    pub fn wait_ui(
        &self,
        cx: &mut TestAppContext,
        what: &str,
        mut condition: impl FnMut(&mut Window, &mut App) -> bool,
    ) {
        let deadline = Instant::now() + PATIENCE;
        loop {
            cx.run_until_parked();
            if self.in_window(cx, |window, cx| condition(window, cx)) {
                return;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Deja que termine todo lo que esté en marcha: ninguna operación del
    /// repositorio en vuelo durante un rato.
    pub fn settle(&self, cx: &mut TestAppContext) {
        let deadline = Instant::now() + PATIENCE;
        let mut quiet_since = Instant::now();
        loop {
            cx.run_until_parked();
            let busy = self.model.read_with(cx, |model, _| {
                model.git_is_loading()
                    || model.active_repository().is_some_and(|repository| {
                        let flags = repository.sidebar_loading;
                        flags.branches
                            || flags.remotes
                            || flags.tags
                            || flags.stashes
                            || flags.submodules
                            || flags.starred_branches
                    })
            });
            if busy {
                quiet_since = Instant::now();
            } else if quiet_since.elapsed() > Duration::from_millis(120) {
                return;
            }
            assert!(Instant::now() < deadline, "timed out waiting for quiet");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Ejecuta `f` dentro de la ventana, con un frame ya dibujado.
    pub fn in_window<R>(
        &self,
        cx: &mut TestAppContext,
        f: impl FnOnce(&mut Window, &mut App) -> R,
    ) -> R {
        let result = cx
            .update_window(self.window, |_, window, cx| {
                window.render_frame(cx);
                f(window, cx)
            })
            .expect("la ventana de prueba se cerró");
        cx.run_until_parked();
        // Parte de lo que desencadena un clic (el `on_click` de un botón, el
        // cierre de un diálogo) se resuelve al dibujar el frame siguiente.
        cx.update_window(self.window, |_, window, cx| window.render_frame(cx))
            .expect("la ventana de prueba se cerró");
        cx.run_until_parked();
        result
    }

    pub fn click(&self, cx: &mut TestAppContext, id: &str) {
        let id = id.to_string();
        self.in_window(cx, move |window, cx| window.click(id, cx));
    }

    pub fn double_click(&self, cx: &mut TestAppContext, id: &str) {
        let id = id.to_string();
        self.in_window(cx, move |window, cx| window.double_click(id, cx));
    }

    pub fn right_click(&self, cx: &mut TestAppContext, id: &str) {
        let id = id.to_string();
        self.in_window(cx, move |window, cx| window.right_click(id, cx));
    }

    pub fn hover(&self, cx: &mut TestAppContext, id: &str) {
        let id = id.to_string();
        self.in_window(cx, move |window, cx| window.hover(id, cx));
    }

    /// Hay un elemento con ese id en el último frame.
    pub fn exists(&self, cx: &mut TestAppContext, id: &str) -> bool {
        let id = id.to_string();
        self.in_window(cx, move |window, _| window.try_find(id).is_some())
    }

    /// Clic derecho sobre `row` y clic en la entrada `label` del menú que se
    /// abre. `entries` es la descripción del menú (para saber qué posición
    /// ocupa la entrada).
    pub fn choose_menu_entry(
        &self,
        cx: &mut TestAppContext,
        row: &str,
        entries: &[MenuEntry],
        label: &str,
    ) {
        let index = mergeon_view_sidebar::menus::entry_index(entries, label)
            .unwrap_or_else(|| panic!("no menu entry `{label}`"));
        self.right_click(cx, row);
        self.in_window(cx, move |window, cx| {
            window.within("popup-menu").click(index, cx);
        });
    }

    /// Lecturas arrancadas de una sección del repositorio.
    pub fn reads(&self, cx: &mut TestAppContext, section: &str) -> usize {
        self.model.read_with(cx, |model, _| {
            model.started_section_reads(&self.repo_id, section)
        })
    }

    pub fn repo<R>(&self, cx: &mut TestAppContext, read: impl FnOnce(&OpenRepository) -> R) -> R {
        self.model.read_with(cx, |model, _| {
            read(model.active_repository().expect("active repository"))
        })
    }

    pub fn dialog_titles(&self, cx: &mut TestAppContext) -> Vec<String> {
        self.in_window(cx, |window, cx| {
            mergeon_ui::dialog::open_titles(window, cx)
                .into_iter()
                .map(|title| title.to_string())
                .collect()
        })
    }
}
