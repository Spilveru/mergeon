//! Vigilancia del sistema de archivos. El watcher de estado solo conoce el
//! contrato ([`WatchBackend`]); la implementación real usa el crate `notify`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;

use notify::event::ModifyKind;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use super::classify::prune_git_metadata;
use super::ignore_rules::IgnoreRules;
use crate::util::resolve_path;

/// Fallo al iniciar o mantener la vigilancia.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchError(String);

impl WatchError {
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl std::fmt::Display for WatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for WatchError {}

impl From<notify::Error> for WatchError {
    fn from(error: notify::Error) -> Self {
        Self(error.to_string())
    }
}

impl From<std::io::Error> for WatchError {
    fn from(error: std::io::Error) -> Self {
        Self(error.to_string())
    }
}

/// Por dónde un backend entrega lo que ve: el callback de `fs.watch` y su
/// `on('error')`.
#[derive(Clone)]
pub struct WatchSink {
    on_event: Arc<dyn Fn(&Path) + Send + Sync>,
    on_error: Arc<dyn Fn(WatchError) + Send + Sync>,
}

impl WatchSink {
    pub fn new(
        on_event: impl Fn(&Path) + Send + Sync + 'static,
        on_error: impl Fn(WatchError) + Send + Sync + 'static,
    ) -> Self {
        Self {
            on_event: Arc::new(on_event),
            on_error: Arc::new(on_error),
        }
    }

    /// Algo cambió en `relative_path` (relativa al repositorio; vacía si no
    /// se sabe qué).
    pub fn event(&self, relative_path: impl AsRef<Path>) {
        (self.on_event)(relative_path.as_ref());
    }

    /// La vigilancia dejó de ser fiable: el watcher pasa a modo poll.
    pub fn error(&self, error: impl Into<WatchError>) {
        (self.on_error)(error.into());
    }
}

/// Vigilancia en curso; se cierra al soltarla (`watcher.close()`).
pub trait WatchHandle: Send {}

impl<T: Send> WatchHandle for T {}

/// Cómo se vigila un repositorio (`watchImpl` / `linuxWatchImpl`).
pub trait WatchBackend: Send + Sync {
    /// Empieza a vigilar `repo_path`. Un `Err` manda al watcher a modo poll.
    ///
    /// La vigilancia devuelta puede soltarse desde dentro de una llamada a
    /// `sink.error`, así que su `Drop` no debe esperar al hilo que la llama.
    fn watch(&self, repo_path: &Path, sink: WatchSink) -> Result<Box<dyn WatchHandle>, WatchError>;
}

/// Cómo se reparte la vigilancia sobre el árbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WatchStrategy {
    /// Una sola vigilancia recursiva sobre la raíz.
    Recursive,
    /// Una vigilancia por directorio, sin entrar en los podados: lo de `.git`
    /// que no interesa (`should_prune_watch_path`) y lo que ignora el
    /// repositorio sin tener nada versionado (`.gitignore`, `node_modules`…).
    PrunedTree,
}

impl WatchStrategy {
    /// En Windows y macOS la vigilancia recursiva nativa es un solo handle
    /// (ReadDirectoryChangesW / FSEvents) y es lo más barato. En linux no hay
    /// equivalente: inotify necesita un watch por directorio y, sin podar,
    /// node_modules agota los watches del usuario.
    pub fn for_current_platform() -> Self {
        if cfg!(any(target_os = "macos", target_os = "windows")) {
            WatchStrategy::Recursive
        } else {
            WatchStrategy::PrunedTree
        }
    }
}

/// Backend por defecto: el watcher nativo de la plataforma vía `notify`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotifyBackend {
    strategy: WatchStrategy,
}

impl Default for NotifyBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl NotifyBackend {
    pub fn new() -> Self {
        Self::with_strategy(WatchStrategy::for_current_platform())
    }

    pub fn with_strategy(strategy: WatchStrategy) -> Self {
        Self { strategy }
    }

    pub fn strategy(&self) -> WatchStrategy {
        self.strategy
    }
}

enum Message {
    Notify(notify::Result<notify::Event>),
    Stop,
}

impl WatchBackend for NotifyBackend {
    fn watch(&self, repo_path: &Path, sink: WatchSink) -> Result<Box<dyn WatchHandle>, WatchError> {
        let root = resolve_path(repo_path);
        let (sender, receiver) = mpsc::channel();

        let event_sender = sender.clone();
        let mut watcher = notify::recommended_watcher(move |result| {
            // Si la bomba ya terminó no queda nadie a quien avisar.
            let _ = event_sender.send(Message::Notify(result));
        })?;

        let mut rules = IgnoreRules::load(&root);
        match self.strategy {
            WatchStrategy::Recursive => watcher.watch(&root, RecursiveMode::Recursive)?,
            WatchStrategy::PrunedTree => watch_tree(&mut watcher, &mut rules, &root, &root, true)?,
        }

        let pump = Pump {
            watcher,
            rules,
            strategy: self.strategy,
            // Algunos backends (FSEvents) informan la ruta real aunque se
            // haya pedido vigilar un symlink.
            // En Windows, sin el prefijo `\\?\` que no traen los eventos.
            canonical_root: mergeon_gitcmd::paths::canonicalize(&root)
                .ok()
                .filter(|canonical| *canonical != root),
            root,
            sink,
        };
        let thread = std::thread::Builder::new()
            .name("mergeon-status-watch-fs".into())
            .spawn(move || pump.run(&receiver))?;

        Ok(Box::new(NotifyHandle {
            sender,
            thread: Some(thread),
        }))
    }
}

struct NotifyHandle {
    sender: Sender<Message>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for NotifyHandle {
    fn drop(&mut self) {
        let _ = self.sender.send(Message::Stop);
        if let Some(thread) = self.thread.take() {
            // Soltada desde `sink.error`, la bomba es el hilo actual: termina
            // sola en cuanto lea el `Stop`.
            if thread.thread().id() != std::thread::current().id() {
                let _ = thread.join();
            }
        }
    }
}

fn to_relative_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Errores de `watch` que solo afectan a un directorio (desapareció mientras
/// se recorría, o no se puede leer) y no invalidan la vigilancia del resto.
fn is_skippable(error: &notify::Error) -> bool {
    match &error.kind {
        notify::ErrorKind::PathNotFound => true,
        notify::ErrorKind::Io(error) => matches!(
            error.kind(),
            std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
        ),
        _ => false,
    }
}

/// Vigila `directory` y sus subdirectorios no podados, uno a uno. Los enlaces
/// simbólicos no se siguen.
fn watch_tree(
    watcher: &mut RecommendedWatcher,
    rules: &mut IgnoreRules,
    root: &Path,
    directory: &Path,
    strict: bool,
) -> Result<(), WatchError> {
    let mut pending = vec![directory.to_path_buf()];
    let mut first = true;

    while let Some(current) = pending.pop() {
        let required = strict && first;
        first = false;

        match watcher.watch(&current, RecursiveMode::NonRecursive) {
            Ok(()) => {}
            Err(error) if !required && is_skippable(&error) => continue,
            Err(error) => return Err(error.into()),
        }

        let Ok(children) = std::fs::read_dir(&current) else {
            continue;
        };
        for child in children.flatten() {
            if !child.file_type().is_ok_and(|file_type| file_type.is_dir()) {
                continue;
            }
            let path = child.path();
            let relative = path.strip_prefix(root).unwrap_or(&path);
            if !should_prune_directory(rules, relative) {
                pending.push(path);
            }
        }
    }

    Ok(())
}

/// Si el recorrido no entra en el directorio `relative`.
fn should_prune_directory(rules: &mut IgnoreRules, relative: &Path) -> bool {
    prune_git_metadata(&to_relative_string(relative))
        .unwrap_or_else(|| rules.should_prune_directory(relative))
}

/// Hilo que traduce los eventos de `notify` al contrato del watcher. Es el
/// dueño del watcher nativo: añadir vigilancias desde el callback de `notify`
/// bloquearía su propio bucle de eventos.
struct Pump {
    watcher: RecommendedWatcher,
    rules: IgnoreRules,
    strategy: WatchStrategy,
    root: PathBuf,
    canonical_root: Option<PathBuf>,
    sink: WatchSink,
}

impl Pump {
    fn run(mut self, receiver: &Receiver<Message>) {
        while let Ok(message) = receiver.recv() {
            match message {
                Message::Stop => break,
                Message::Notify(Ok(event)) => self.handle_event(&event),
                Message::Notify(Err(error)) => self.sink.error(error),
            }
        }
    }

    fn relative(&self, path: &Path) -> PathBuf {
        path.strip_prefix(&self.root)
            .ok()
            .or_else(|| {
                self.canonical_root
                    .as_ref()
                    .and_then(|canonical_root| path.strip_prefix(canonical_root).ok())
            })
            .map(Path::to_path_buf)
            .unwrap_or_default()
    }

    fn handle_event(&mut self, event: &notify::Event) {
        // Abrir o leer un archivo no cambia el estado del repositorio.
        if matches!(event.kind, EventKind::Access(_)) {
            return;
        }

        // Sin ruta (p. ej. se desbordó la cola del kernel) no se sabe qué
        // cambió: cuenta como cambio del working tree.
        if event.paths.is_empty() {
            self.sink.event(Path::new(""));
            return;
        }

        let may_add_directory = matches!(
            event.kind,
            EventKind::Create(_) | EventKind::Modify(ModifyKind::Name(_))
        );

        for path in &event.paths {
            let relative = self.relative(path);

            if self.strategy == WatchStrategy::PrunedTree
                && prune_git_metadata(&to_relative_string(&relative)) == Some(true)
            {
                continue;
            }

            // Lo que el repositorio ignora no cambia su estado (y en la
            // vigilancia recursiva llega igual: hay que descartarlo aquí).
            let is_dir = std::fs::symlink_metadata(path)
                .ok()
                .map(|metadata| metadata.is_dir());
            if self.rules.should_skip_event(&relative, is_dir) {
                continue;
            }

            // Un `.gitignore` editado cambia qué se ignora desde ahora. Lo
            // que ya se podó no se empieza a vigilar hasta el próximo `watch`.
            if relative
                .file_name()
                .is_some_and(|name| name == ".gitignore")
            {
                self.rules
                    .reload_gitignore(relative.parent().unwrap_or(Path::new("")));
            }

            // Un directorio nuevo (creado o movido aquí) hay que vigilarlo.
            if self.strategy == WatchStrategy::PrunedTree
                && may_add_directory
                && is_dir == Some(true)
                && let Err(error) =
                    watch_tree(&mut self.watcher, &mut self.rules, &self.root, path, false)
            {
                self.sink.error(error);
            }

            self.sink.event(&relative);
        }
    }
}
