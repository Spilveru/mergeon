//! Fixtures compartidas por los tests: el modelo sobre un backend sin efectos
//! fuera del directorio temporal y repositorios git reales.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::{Entity, Subscription, TestAppContext};
use mergeon_backend::Backend;
use mergeon_gitcmd::Git;
use mergeon_model::repositories::AddRepositoryOptions;

use crate::testing::{FakeGitHubTransport, RecordingLauncher, RecordingUiHost, TestBackend};
use crate::{AddRepository, AppEvent, AppModel};

/// Margen para todo lo que en un test "tiene que acabar pasando".
const PATIENCE: Duration = Duration::from_secs(20);

/// El modelo con todo lo que un test necesita mirar.
pub(crate) struct TestApp {
    pub model: Entity<AppModel>,
    pub host: Rc<RecordingUiHost>,
    pub launcher: Arc<RecordingLauncher>,
    pub github: Arc<FakeGitHubTransport>,
    pub backend: Arc<Backend>,
    events: Rc<RefCell<Vec<AppEvent>>>,
    _subscription: Subscription,
}

impl TestApp {
    pub fn new(cx: &mut TestAppContext) -> Self {
        Self::with_backend(TestBackend::new(), cx)
    }

    pub fn with_backend(mut test_backend: TestBackend, cx: &mut TestAppContext) -> Self {
        // Los eventos del backend (cola de mutaciones, actualizador) llegan
        // desde sus propios hilos: el planificador de tests tiene que
        // admitirlo.
        cx.executor().allow_parking();

        let (backend, events) = test_backend.build();
        let host = RecordingUiHost::new();
        let model = cx.update(|cx| AppModel::init(backend.clone(), events, cx));
        model.update(cx, |model, cx| model.set_ui_host(host.clone(), cx));

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
            events: recorded,
            _subscription: subscription,
        }
    }

    /// Cuenta los avisos del modelo a sus observadores (cada uno hace que
    /// las vistas que lo leen repinten la ventana) mientras viva la
    /// suscripción.
    pub fn count_notifications(&self, cx: &mut TestAppContext) -> NotificationCounter {
        let count = Rc::new(std::cell::Cell::new(0));
        let subscription = cx.update(|cx| {
            let count = count.clone();
            cx.observe(&self.model, move |_, _| count.set(count.get() + 1))
        });
        NotificationCounter {
            count,
            _subscription: subscription,
        }
    }

    /// Eventos emitidos por el modelo, en orden.
    pub fn events(&self) -> Vec<AppEvent> {
        self.events.borrow().clone()
    }

    pub fn clear_events(&self) {
        self.events.borrow_mut().clear();
    }

    /// Operaciones de las mutaciones registradas (`lastMutationOperation`),
    /// en orden.
    pub fn recorded_mutations(&self) -> Vec<String> {
        self.events()
            .into_iter()
            .filter_map(|event| match event {
                AppEvent::MutationRecorded { operation, .. } => operation,
                _ => None,
            })
            .collect()
    }

    /// Abre un repositorio como lo hace el store (`addRepository`) y espera a
    /// que termine su primera lectura.
    pub async fn open(&self, repo: &TestRepo, cx: &mut TestAppContext) -> String {
        self.model
            .update(cx, |model, cx| {
                model.add_repository(&repo.path_string(), AddRepository::default(), cx)
            })
            .await
            .expect("add repository");
        cx.run_until_parked();
        repo.id()
    }

    /// Deja un repositorio abierto y activo sin leer nada de él, como el
    /// `seedRepository` de los tests originales.
    pub fn seed(&self, repo: &TestRepo, cx: &mut TestAppContext) -> String {
        self.model.update(cx, |model, cx| {
            model
                .repositories
                .add_repository(&repo.path_string(), AddRepositoryOptions::default())
                .expect("seed repository");
            cx.notify();
        });
        repo.id()
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
}

/// Avisos del modelo contados por [`TestApp::count_notifications`].
pub(crate) struct NotificationCounter {
    count: Rc<std::cell::Cell<usize>>,
    _subscription: Subscription,
}

impl NotificationCounter {
    pub fn get(&self) -> usize {
        self.count.get()
    }

    pub fn reset(&self) {
        self.count.set(0);
    }
}

pub(crate) fn git_in(dir: &Path, args: &[&str]) -> String {
    match Git::new(dir).raw(args) {
        Ok(output) => output,
        Err(error) => panic!("git {args:?} failed: {}", error.message),
    }
}

fn configure_identity(repo_path: &Path, name: &str) {
    for (key, value) in [
        ("user.name", name),
        ("user.email", "test@example.com"),
        ("commit.gpgsign", "false"),
        ("tag.gpgsign", "false"),
        ("core.autocrlf", "false"),
    ] {
        git_in(repo_path, &["config", key, value]);
    }
}

/// Un repo con identidad propia y rama `main`, sin depender de la
/// configuración global de la máquina.
pub(crate) struct TestRepo {
    root: tempfile::TempDir,
    pub path: PathBuf,
}

impl TestRepo {
    /// Directorio temporal vacío, sin repositorio.
    pub fn plain_directory() -> Self {
        let root = tempfile::Builder::new()
            .prefix("mergeon-app-")
            .tempdir()
            .unwrap();
        // La ruta real: en macOS el temporal cuelga de un symlink.
        let base = mergeon_gitcmd::paths::canonicalize(root.path()).unwrap();
        let path = base.join("repo");
        std::fs::create_dir(&path).unwrap();
        Self { root, path }
    }

    /// Repositorio vacío (sin commits).
    pub fn empty() -> Self {
        let repo = Self::plain_directory();
        git_in(&repo.path, &["init", "-b", "main"]);
        configure_identity(&repo.path, "Test User");
        repo
    }

    /// Repositorio con un commit base en `main`.
    pub fn new() -> Self {
        let repo = Self::empty();
        repo.commit_file("README.md", "# Demo\n", "Initial commit");
        repo
    }

    /// La ruta como la recibe el modelo de sus vistas: con `/` también en
    /// Windows (las funciones del modelo comparan con las rutas que él da).
    /// `path` queda con los separadores del sistema.
    pub fn path_string(&self) -> String {
        self.path.to_string_lossy().replace('\\', "/")
    }

    /// El `id` que el store da al repositorio (su ruta normalizada).
    pub fn id(&self) -> String {
        mergeon_model::repositories::normalize_repository_path(&self.path_string())
    }

    /// Directorio que contiene al repo (para remotos bare y otros clones).
    pub fn root(&self) -> PathBuf {
        mergeon_gitcmd::paths::canonicalize(self.root.path()).unwrap()
    }

    pub fn git(&self, args: &[&str]) -> String {
        git_in(&self.path, args)
    }

    pub fn write(&self, name: &str, content: &str) {
        let path = self.path.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }

    pub fn head(&self) -> String {
        self.git(&["rev-parse", "HEAD"]).trim().to_string()
    }

    pub fn current_branch(&self) -> String {
        self.git(&["rev-parse", "--abbrev-ref", "HEAD"])
            .trim()
            .to_string()
    }

    /// Escribe `name`, lo añade y hace commit. Devuelve el hash completo.
    pub fn commit_file(&self, name: &str, content: &str, message: &str) -> String {
        self.write(name, content);
        self.git(&["add", "--", name]);
        self.git(&["commit", "-m", message]);
        self.head()
    }

    /// Asuntos de los commits de HEAD, del más reciente al más antiguo.
    pub fn log_subjects(&self) -> Vec<String> {
        self.git(&["log", "--pretty=%s"])
            .trim()
            .split('\n')
            .map(str::to_string)
            .collect()
    }

    /// Crea un remoto bare local, lo registra como `name` y devuelve su ruta.
    pub fn add_bare_remote(&self, name: &str) -> PathBuf {
        let remote = self.root().join(format!("{name}.git"));
        std::fs::create_dir(&remote).unwrap();
        git_in(&remote, &["init", "--bare", "-b", "main"]);
        self.git(&["remote", "add", name, &remote.to_string_lossy()]);
        remote
    }

    /// Un "GitHub" local: `origin` bare con `main` publicado y con tracking.
    pub fn publish_to_origin(&self) -> PathBuf {
        let origin = self.add_bare_remote("origin");
        self.git(&["push", "-u", "origin", "main"]);
        origin
    }

    /// Otro clon del remoto, con su propia identidad, para generar cambios
    /// desde fuera.
    pub fn colleague(&self, origin: &Path) -> Colleague {
        let path = self.root().join("colleague");
        git_in(
            &self.root(),
            &["clone", &origin.to_string_lossy(), &path.to_string_lossy()],
        );
        configure_identity(&path, "Colleague");
        Colleague { path }
    }

    /// Rompe el repositorio: a partir de aquí toda lectura de git falla.
    pub fn break_repository(&self) {
        std::fs::remove_dir_all(self.path.join(".git")).unwrap();
    }
}

pub(crate) struct Colleague {
    pub path: PathBuf,
}

impl Colleague {
    /// Hace commit de un archivo y lo publica en `main`.
    pub fn commit_and_push(&self, name: &str, content: &str, message: &str) -> String {
        std::fs::write(self.path.join(name), content).unwrap();
        git_in(&self.path, &["add", "--", name]);
        git_in(&self.path, &["commit", "-m", message]);
        git_in(&self.path, &["push", "origin", "main"]);
        git_in(&self.path, &["rev-parse", "HEAD"])
            .trim()
            .to_string()
    }
}
