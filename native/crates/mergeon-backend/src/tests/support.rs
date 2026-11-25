//! Fixtures compartidas por los tests: un backend sin efectos fuera del
//! directorio temporal y repositorios git reales.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use mergeon_gitcmd::Git;
use mergeon_github::{GitHubClient, GitHubConfig};
use mergeon_runtime::{OperationEvent, OperationPhase, StatusChange};
use mergeon_store::{PlainTextCipher, Store, StoreOptions};

use crate::shell::{LaunchError, ProcessLauncher, Shell, ShellPlatform};
use crate::{Backend, BackendEvent, BackendOptions};

/// Margen para todo lo que en un test "tiene que acabar pasando".
pub(crate) const PATIENCE: Duration = Duration::from_secs(20);

/// Los eventos que emite el backend, en orden.
#[derive(Default)]
pub(crate) struct Events {
    list: Mutex<Vec<BackendEvent>>,
    changed: Condvar,
}

impl Events {
    pub fn push(&self, event: BackendEvent) {
        self.list.lock().unwrap().push(event);
        self.changed.notify_all();
    }

    /// Todos los eventos recibidos, en orden.
    pub fn all(&self) -> Vec<BackendEvent> {
        self.list.lock().unwrap().clone()
    }

    pub fn operations(&self) -> Vec<OperationEvent> {
        self.list
            .lock()
            .unwrap()
            .iter()
            .filter_map(|event| match event {
                BackendEvent::Operation(event) => Some(event.clone()),
                _ => None,
            })
            .collect()
    }

    pub fn status_changes(&self) -> Vec<StatusChange> {
        self.list
            .lock()
            .unwrap()
            .iter()
            .filter_map(|event| match event {
                BackendEvent::StatusChanged(change) => Some(change.clone()),
                _ => None,
            })
            .collect()
    }

    /// Fases por las que pasó una operación.
    pub fn phases_of(&self, operation_id: &str) -> Vec<OperationPhase> {
        self.operations()
            .into_iter()
            .filter(|event| event.operation_id == operation_id)
            .map(|event| event.phase)
            .collect()
    }

    /// Espera hasta que `find` encuentre algo entre los eventos recibidos.
    pub fn wait_for<T>(&self, what: &str, find: impl Fn(&[BackendEvent]) -> Option<T>) -> T {
        let deadline = Instant::now() + PATIENCE;
        let mut list = self.list.lock().unwrap();
        loop {
            if let Some(found) = find(&list) {
                return found;
            }
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .unwrap_or_else(|| panic!("timed out waiting for {what}"));
            list = self.changed.wait_timeout(list, remaining).unwrap().0;
        }
    }

    /// Espera el evento de `operation` en la fase `phase`.
    pub fn wait_for_operation(&self, operation: &str, phase: OperationPhase) -> OperationEvent {
        self.wait_for(&format!("{operation} {}", phase.as_str()), |events| {
            events.iter().find_map(|event| match event {
                BackendEvent::Operation(event)
                    if event.operation == operation && event.phase == phase =>
                {
                    Some(event.clone())
                }
                _ => None,
            })
        })
    }
}

/// Un proceso que el backend habría lanzado.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Launched {
    pub command: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub detached: bool,
}

/// Lanzador que no lanza nada: registra lo pedido y responde según la lista
/// de comandos "instalados", como el `fakeSpawn` del test original.
#[derive(Default)]
pub(crate) struct FakeLauncher {
    /// Comandos que "existen" para `which` y ejecutables que arrancan. Todo
    /// lo demás falla con ENOENT, como haría el sistema.
    available: Mutex<Vec<String>>,
    /// Código de salida de los comandos que se esperan (`run`).
    exit_codes: Mutex<Vec<(String, i32)>>,
    /// Si está, todo lanzamiento falla con este error.
    failure: Mutex<Option<LaunchError>>,
    calls: Mutex<Vec<Launched>>,
}

impl FakeLauncher {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn add(&self, command: &str) -> &Self {
        self.available.lock().unwrap().push(command.to_string());
        self
    }

    pub fn exit_with(&self, command: &str, code: i32) -> &Self {
        self.exit_codes
            .lock()
            .unwrap()
            .push((command.to_string(), code));
        self
    }

    pub fn fail_with(&self, error: LaunchError) {
        *self.failure.lock().unwrap() = Some(error);
    }

    fn is_available(&self, command: &str) -> bool {
        self.available
            .lock()
            .unwrap()
            .iter()
            .any(|candidate| candidate == command)
    }

    fn record(&self, command: &str, args: &[String], cwd: Option<&Path>, detached: bool) {
        self.calls.lock().unwrap().push(Launched {
            command: command.to_string(),
            args: args.to_vec(),
            cwd: cwd.map(|cwd| cwd.to_string_lossy().into_owned()),
            detached,
        });
    }

    /// Lo lanzado, sin las consultas a `which`.
    pub fn launched(&self) -> Vec<Launched> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|call| {
                call.command != "which"
                    && !(call.command == "flatpak-spawn"
                        && call.args.get(1).map(String::as_str) == Some("which"))
            })
            .cloned()
            .collect()
    }
}

impl ProcessLauncher for FakeLauncher {
    fn spawn_detached(
        &self,
        command: &str,
        args: &[String],
        cwd: Option<&Path>,
    ) -> Result<(), LaunchError> {
        self.record(command, args, cwd, true);
        if let Some(error) = self.failure.lock().unwrap().clone() {
            return Err(error);
        }
        if self.is_available(command) {
            Ok(())
        } else {
            Err(LaunchError::not_found(command))
        }
    }

    fn run(
        &self,
        command: &str,
        args: &[String],
        cwd: Option<&Path>,
    ) -> Result<Option<i32>, LaunchError> {
        self.record(command, args, cwd, false);
        if let Some(error) = self.failure.lock().unwrap().clone() {
            return Err(error);
        }
        if command == "which" {
            return Ok(Some(if self.is_available(&args[0]) { 0 } else { 1 }));
        }
        if command == "flatpak-spawn" && args.get(1).map(String::as_str) == Some("which") {
            let host_command = format!("host:{}", args[2]);
            return Ok(Some(if self.is_available(&host_command) {
                0
            } else {
                1
            }));
        }
        if !self.is_available(command) {
            return Err(LaunchError::not_found(command));
        }
        let code = self
            .exit_codes
            .lock()
            .unwrap()
            .iter()
            .find(|(candidate, _)| candidate == command)
            .map_or(0, |(_, code)| *code);
        Ok(Some(code))
    }
}

/// Un [`Backend`] para tests: configuración en memoria, sin llavero, sin
/// lanzar procesos del sistema y con los eventos a la vista.
pub(crate) struct TestBackend {
    pub backend: Backend,
    pub events: Arc<Events>,
}

impl TestBackend {
    pub fn new() -> Self {
        Self::with_options(|options| options)
    }

    pub fn with_options(configure: impl FnOnce(BackendOptions) -> BackendOptions) -> Self {
        let events = Arc::new(Events::default());
        let sink = events.clone();
        let options = BackendOptions::new()
            .store(Store::in_memory(StoreOptions::new(Arc::new(
                PlainTextCipher,
            ))))
            .github(GitHubClient::new(GitHubConfig::default()))
            // Ningún test abre programas del sistema.
            .shell(Shell::with_launcher(
                FakeLauncher::new(),
                ShellPlatform::Linux,
            ))
            .on_event(move |event| sink.push(event));
        let backend = Backend::new(configure(options)).expect("backend");

        Self { backend, events }
    }

    pub fn git(&self) -> &crate::GitApi {
        self.backend.git()
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
    /// Repositorio vacío (sin commits).
    pub fn empty() -> Self {
        let root = tempfile::Builder::new()
            .prefix("mergeon-backend-")
            .tempdir()
            .unwrap();
        // La ruta real: en macOS el temporal cuelga de un symlink.
        let base = mergeon_gitcmd::paths::canonicalize(root.path()).unwrap();
        let path = base.join("repo");
        std::fs::create_dir(&path).unwrap();
        git_in(&path, &["init", "-b", "main"]);
        configure_identity(&path, "Test User");
        Self { root, path }
    }

    /// Repositorio con un commit base en `main`.
    pub fn new() -> Self {
        let repo = Self::empty();
        repo.commit_file("README.md", "# Demo\n", "Initial commit");
        repo
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

    pub fn read(&self, name: &str) -> String {
        std::fs::read_to_string(self.path.join(name)).unwrap()
    }

    pub fn head(&self) -> String {
        self.git(&["rev-parse", "HEAD"]).trim().to_string()
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

/// Espera a que `condition` se cumpla, consultándola cada poco.
pub(crate) fn wait_until(what: &str, condition: impl Fn() -> bool) {
    let deadline = Instant::now() + PATIENCE;
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(2));
    }
}
