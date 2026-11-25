//! Watcher de estado del repositorio activo.
//!
//! Port de `electron/services/repository-status-watch.js`. Vigila el working
//! tree y lo que mueve HEAD dentro de `.git`, y avisa con debounce
//! (`git:status-changed`). También `.git/config`, las refs de tags y ramas
//! remotas y el stash, con motivos propios (`fs-watch-config`,
//! `fs-watch-refs`, `fs-watch-stash`) que se avisan aparte para que la app
//! relea remotos, tags y stashes. Si la vigilancia nativa no arranca o falla, cae a
//! un sondeo periódico que se pausa cuando la ventana pierde el foco.
//!
//! Allí los temporizadores son `setTimeout`/`setInterval`; aquí son plazos que
//! dispara un hilo propio, o la llamada a
//! [`RepositoryStatusWatcher::run_due_timers`] cuando el reloj es manual.

mod backend;
mod classify;
mod fingerprint;
mod ignore_rules;

#[cfg(test)]
mod tests;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, PoisonError, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::mutation_gate::MutationWatcher;
use crate::util::{lock, resolve_path};

pub use backend::{NotifyBackend, WatchBackend, WatchError, WatchHandle, WatchSink, WatchStrategy};
pub use classify::{
    WatchPathKind, WatchReason, classify_watch_path, is_packed_refs_path, should_ignore_watch_path,
    should_prune_watch_path,
};
pub use fingerprint::{
    MetadataFingerprint, get_head_fingerprint, get_metadata_fingerprint, get_refs_fingerprint,
};

use classify::merge_queued_reason;

pub const DEFAULT_DEBOUNCE: Duration = Duration::from_millis(400);
pub const DEFAULT_SUPPRESS_TRAILING: Duration = Duration::from_millis(600);
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(2000);

/// Payload de `git:status-changed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusChange {
    pub repo_path: String,
    pub reason: WatchReason,
}

/// Modo en que quedó el watcher (`mode` de la respuesta de `git:watchStatus`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WatchMode {
    #[serde(rename = "fs-watch")]
    FsWatch,
    #[serde(rename = "poll")]
    Poll,
    #[serde(rename = "stopped")]
    Stopped,
}

impl WatchMode {
    pub fn as_str(self) -> &'static str {
        match self {
            WatchMode::FsWatch => "fs-watch",
            WatchMode::Poll => "poll",
            WatchMode::Stopped => "stopped",
        }
    }
}

/// Reloj que solo avanza cuando se le pide, para probar los temporizadores
/// sin esperas reales.
#[derive(Debug, Clone, Default)]
pub struct ManualClock {
    now: Arc<Mutex<Duration>>,
}

impl ManualClock {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn now(&self) -> Duration {
        *lock(&self.now)
    }

    pub fn advance(&self, by: Duration) {
        *lock(&self.now) += by;
    }
}

/// Quién mueve el tiempo del watcher.
#[derive(Debug, Clone, Default)]
pub enum WatchTimers {
    /// Reloj del sistema y un hilo propio que dispara los plazos.
    #[default]
    System,
    /// Reloj manual y sin hilo: los plazos vencidos se disparan al llamar a
    /// [`RepositoryStatusWatcher::run_due_timers`].
    Manual(ManualClock),
}

/// Lectura de la huella de HEAD (`getHeadFingerprintImpl`).
pub type HeadFingerprintFn = Arc<dyn Fn(&Path) -> Option<String> + Send + Sync>;

/// Lectura de la huella de tags, ramas remotas y configuración, para el modo
/// poll ([`get_metadata_fingerprint`]).
pub type MetadataFingerprintFn = Arc<dyn Fn(&Path) -> Option<MetadataFingerprint> + Send + Sync>;

/// Configuración del watcher (`createRepositoryStatusWatcher({...})`).
#[derive(Clone)]
pub struct StatusWatcherOptions {
    /// Calma que debe haber tras el último cambio antes de avisar.
    pub debounce: Duration,
    /// Ventana tras una mutación propia en la que aún se descartan eventos.
    pub suppress_trailing: Duration,
    /// Cadencia del sondeo en modo poll.
    pub poll_interval: Duration,
    pub backend: Arc<dyn WatchBackend>,
    pub head_fingerprint: HeadFingerprintFn,
    pub metadata_fingerprint: MetadataFingerprintFn,
    pub timers: WatchTimers,
}

impl Default for StatusWatcherOptions {
    fn default() -> Self {
        Self {
            debounce: DEFAULT_DEBOUNCE,
            suppress_trailing: DEFAULT_SUPPRESS_TRAILING,
            poll_interval: DEFAULT_POLL_INTERVAL,
            backend: Arc::new(NotifyBackend::new()),
            head_fingerprint: Arc::new(get_head_fingerprint),
            metadata_fingerprint: Arc::new(get_metadata_fingerprint),
            timers: WatchTimers::System,
        }
    }
}

type ChangeCallback = Arc<dyn Fn(StatusChange) + Send + Sync>;

struct State {
    /// Cambia en cada `start`/`stop`: lo que llegue de una vigilancia anterior
    /// se descarta.
    generation: u64,
    current_repo_path: Option<PathBuf>,
    emit_change: Option<ChangeCallback>,
    focused: bool,
    queued_reason: Option<WatchReason>,
    /// Cambios de metadatos pendientes (tags/remotas, config): se avisan
    /// aparte del de working tree / HEAD, uno por clase.
    queued_metadata: Vec<WatchReason>,
    last_head_fingerprint: Option<String>,
    last_metadata_fingerprint: Option<MetadataFingerprint>,
    // Supresión de eventos auto-infligidos: las mutaciones propias ya devuelven
    // el status embebido en su resultado, así que sus eventos de FS son ruido.
    mutation_depth: u32,
    suppress_until: Duration,
    debounce_deadline: Option<Duration>,
    poll_deadline: Option<Duration>,
    fs_watcher: Option<Box<dyn WatchHandle>>,
    shutdown: bool,
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
    /// `start` y `stop` no se intercalan entre sí.
    lifecycle: Mutex<()>,
    debounce: Duration,
    suppress_trailing: Duration,
    poll_interval: Duration,
    backend: Arc<dyn WatchBackend>,
    head_fingerprint: HeadFingerprintFn,
    metadata_fingerprint: MetadataFingerprintFn,
    manual_clock: Option<ManualClock>,
    origin: Instant,
}

enum DueTimer {
    Debounce {
        emit_change: Option<ChangeCallback>,
        changes: Vec<StatusChange>,
    },
    Poll {
        deadline: Duration,
        target: Option<(PathBuf, u64)>,
    },
}

impl Shared {
    fn now(&self) -> Duration {
        match &self.manual_clock {
            Some(clock) => clock.now(),
            None => self.origin.elapsed(),
        }
    }

    /// Programa el aviso con debounce. `now` es el instante del evento: para
    /// un temporizador, el de su plazo.
    fn queue_emit(&self, state: &mut State, reason: WatchReason, now: Duration) {
        if state.current_repo_path.is_none() || state.emit_change.is_none() {
            return;
        }

        // Descarta eventos durante una mutación propia y en una corta ventana de
        // cola tras terminarla (los writes de git llegan con retardo por fs.watch).
        // Trade-off: un cambio EXTERNO dentro de esa ventana (~600ms anclados a una
        // acción explícita del usuario) se pierde puntualmente; en modo poll el
        // siguiente tick lo recupera y en fs-watch cualquier evento posterior
        // re-dispara. Lo concurrente con la mutación ya viene en su status embebido.
        if state.mutation_depth > 0 || now < state.suppress_until {
            return;
        }

        if reason.is_metadata() {
            // Uno por clase (refs / config); el primero que llega se queda.
            let same_kind = |queued: &WatchReason| {
                queued.is_refs_change() == reason.is_refs_change()
                    && queued.is_config_change() == reason.is_config_change()
                    && queued.is_stash_change() == reason.is_stash_change()
            };
            if !state.queued_metadata.iter().any(same_kind) {
                state.queued_metadata.push(reason);
            }
        } else {
            state.queued_reason = Some(merge_queued_reason(state.queued_reason, reason));
        }
        state.debounce_deadline = Some(now + self.debounce);
    }

    fn start_polling(&self, state: &mut State, now: Duration) {
        if state.poll_deadline.is_none() {
            state.poll_deadline = Some(now + self.poll_interval);
        }
    }

    fn handle_fs_event(&self, generation: u64, relative_path: &Path) {
        let mut state = lock(&self.state);
        if state.generation != generation {
            return;
        }

        let relative_path = relative_path.to_string_lossy();
        let reason = match classify_watch_path(&relative_path) {
            WatchPathKind::Ignore => return,
            WatchPathKind::Head => WatchReason::FsWatchHead,
            WatchPathKind::WorkingTree => WatchReason::FsWatchWorkingTree,
            WatchPathKind::Refs => WatchReason::FsWatchRefs,
            WatchPathKind::Config => WatchReason::FsWatchConfig,
            WatchPathKind::Stash => WatchReason::FsWatchStash,
        };
        let now = self.now();
        self.queue_emit(&mut state, reason, now);
        if is_packed_refs_path(&relative_path) {
            self.queue_emit(&mut state, WatchReason::FsWatchRefs, now);
        }
        drop(state);
        self.wake.notify_all();
    }

    /// `handleWatcherFailure`: sin vigilancia nativa, se sondea.
    fn handle_watcher_failure(&self, generation: u64, error: &WatchError) {
        let fs_watcher = {
            let mut state = lock(&self.state);
            if state.generation != generation {
                return;
            }
            log::error!("Repository status watcher failed, falling back to polling: {error}");
            let now = self.now();
            self.start_polling(&mut state, now);
            state.fs_watcher.take()
        };
        self.wake.notify_all();
        // Cerrar la vigilancia puede esperar a su hilo: fuera del lock.
        drop(fs_watcher);
    }

    fn sink(self: &Arc<Self>, generation: u64) -> WatchSink {
        // Referencias débiles: un backend que siga vivo no retiene al watcher.
        let events: Weak<Shared> = Arc::downgrade(self);
        let errors = events.clone();
        WatchSink::new(
            move |relative_path| {
                if let Some(shared) = events.upgrade() {
                    shared.handle_fs_event(generation, relative_path);
                }
            },
            move |error| {
                if let Some(shared) = errors.upgrade() {
                    shared.handle_watcher_failure(generation, &error);
                }
            },
        )
    }

    /// Saca el siguiente temporizador vencido, si lo hay.
    fn take_due_timer(&self) -> Option<DueTimer> {
        let mut state = lock(&self.state);
        if state.shutdown {
            return None;
        }

        let now = self.now();
        let poll_due = state.poll_deadline.filter(|deadline| *deadline <= now);
        let debounce_due = state.debounce_deadline.filter(|deadline| *deadline <= now);

        match (poll_due, debounce_due) {
            // A igualdad de plazo va antes el sondeo: su intervalo se creó primero.
            (Some(poll), Some(debounce)) if debounce < poll => Some(self.take_debounce(&mut state)),
            (Some(poll), _) => Some(self.take_poll(&mut state, poll, now)),
            (None, Some(_)) => Some(self.take_debounce(&mut state)),
            (None, None) => None,
        }
    }

    fn take_debounce(&self, state: &mut State) -> DueTimer {
        state.debounce_deadline = None;
        let reasons: Vec<WatchReason> = state
            .queued_reason
            .take()
            .into_iter()
            .chain(std::mem::take(&mut state.queued_metadata))
            .collect();
        let changes = match &state.current_repo_path {
            Some(repo_path) => reasons
                .into_iter()
                .map(|reason| StatusChange {
                    repo_path: repo_path.to_string_lossy().into_owned(),
                    reason,
                })
                .collect(),
            None => Vec::new(),
        };
        DueTimer::Debounce {
            emit_change: state.emit_change.clone(),
            changes,
        }
    }

    fn take_poll(&self, state: &mut State, deadline: Duration, now: Duration) -> DueTimer {
        // Tras una suspensión larga no se recuperan los ticks perdidos de golpe.
        let interval = self.poll_interval.max(Duration::from_millis(1));
        let mut next = deadline + interval;
        if next <= now {
            next = now + interval;
        }
        state.poll_deadline = Some(next);

        let target = match &state.current_repo_path {
            Some(repo_path) if state.focused => Some((repo_path.clone(), state.generation)),
            _ => None,
        };
        DueTimer::Poll { deadline, target }
    }

    /// Dispara, en orden de plazo, todos los temporizadores vencidos.
    fn run_due_timers(&self) {
        while let Some(timer) = self.take_due_timer() {
            match timer {
                DueTimer::Debounce {
                    emit_change,
                    changes,
                } => {
                    let Some(emit_change) = emit_change else {
                        continue;
                    };
                    for change in changes {
                        // Un fallo de quien recibe el aviso no debe parar los
                        // temporizadores.
                        if catch_unwind(AssertUnwindSafe(|| emit_change(change))).is_err() {
                            log::error!("Repository status change handler panicked");
                        }
                    }
                }
                DueTimer::Poll { deadline, target } => {
                    let Some((repo_path, generation)) = target else {
                        continue;
                    };
                    // Lee el disco: sin el estado tomado.
                    let next_head_fingerprint = (self.head_fingerprint)(&repo_path);
                    let next_metadata_fingerprint = (self.metadata_fingerprint)(&repo_path);

                    let mut state = lock(&self.state);
                    if state.generation != generation || state.poll_deadline.is_none() {
                        continue;
                    }
                    let head_changed = next_head_fingerprint != state.last_head_fingerprint;
                    state.last_head_fingerprint = next_head_fingerprint;

                    let previous_metadata = state.last_metadata_fingerprint.take();
                    if let (Some(previous), Some(next)) =
                        (&previous_metadata, &next_metadata_fingerprint)
                    {
                        if previous.refs != next.refs {
                            self.queue_emit(&mut state, WatchReason::PollRefs, deadline);
                        }
                        if previous.config != next.config {
                            self.queue_emit(&mut state, WatchReason::PollConfig, deadline);
                        }
                        if previous.stash != next.stash {
                            self.queue_emit(&mut state, WatchReason::PollStash, deadline);
                        }
                    }
                    state.last_metadata_fingerprint = next_metadata_fingerprint;

                    // Emite en cada tick a propósito: en modo fallback (la vigilancia
                    // nativa no está disponible) es la única forma de detectar cambios
                    // del working-tree. La supresión de mutaciones propias evita el
                    // ruido durante operaciones de la app.
                    let reason = if head_changed {
                        WatchReason::PollHead
                    } else {
                        WatchReason::PollWorkingTree
                    };
                    self.queue_emit(&mut state, reason, deadline);
                }
            }
        }
    }

    /// Detiene todo y devuelve la vigilancia para cerrarla fuera del lock.
    fn reset(&self, state: &mut State) -> Option<Box<dyn WatchHandle>> {
        state.generation += 1;
        state.debounce_deadline = None;
        state.poll_deadline = None;
        state.current_repo_path = None;
        state.emit_change = None;
        state.queued_reason = None;
        state.queued_metadata.clear();
        state.last_head_fingerprint = None;
        state.last_metadata_fingerprint = None;
        state.mutation_depth = 0;
        state.suppress_until = Duration::ZERO;
        state.fs_watcher.take()
    }

    fn stop(&self) {
        let fs_watcher = self.reset(&mut lock(&self.state));
        self.wake.notify_all();
        drop(fs_watcher);
    }

    fn is_current_repo(state: &State, repo_path: &Path) -> bool {
        if repo_path.as_os_str().is_empty() {
            return false;
        }
        state
            .current_repo_path
            .as_deref()
            .is_some_and(|current| resolve_path(repo_path) == resolve_path(current))
    }
}

fn timer_loop(shared: &Shared) {
    loop {
        shared.run_due_timers();

        let state = lock(&shared.state);
        if state.shutdown {
            return;
        }
        let next = match (state.poll_deadline, state.debounce_deadline) {
            (Some(poll), Some(debounce)) => Some(poll.min(debounce)),
            (poll, debounce) => poll.or(debounce),
        };
        let now = shared.now();
        match next {
            Some(deadline) if deadline <= now => {}
            Some(deadline) => {
                let _state = shared
                    .wake
                    .wait_timeout(state, deadline - now)
                    .unwrap_or_else(PoisonError::into_inner);
            }
            None => {
                let _state = shared
                    .wake
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner);
            }
        }
    }
}

/// Watcher de estado de un repositorio a la vez. Se comparte entre hilos por
/// referencia (`Arc`); al soltarlo cierra la vigilancia y espera a su hilo.
pub struct RepositoryStatusWatcher {
    shared: Arc<Shared>,
    timer_thread: Option<JoinHandle<()>>,
}

impl Default for RepositoryStatusWatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for RepositoryStatusWatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = lock(&self.shared.state);
        f.debug_struct("RepositoryStatusWatcher")
            .field("repo_path", &state.current_repo_path)
            .field("focused", &state.focused)
            .finish()
    }
}

impl RepositoryStatusWatcher {
    /// Watcher con la vigilancia nativa de la plataforma y el reloj del sistema.
    pub fn new() -> Self {
        Self::with_options(StatusWatcherOptions::default())
    }

    pub fn with_options(options: StatusWatcherOptions) -> Self {
        let manual_clock = match options.timers {
            WatchTimers::System => None,
            WatchTimers::Manual(clock) => Some(clock),
        };
        let spawn_timer_thread = manual_clock.is_none();

        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                generation: 0,
                current_repo_path: None,
                emit_change: None,
                focused: true,
                queued_reason: None,
                queued_metadata: Vec::new(),
                last_head_fingerprint: None,
                last_metadata_fingerprint: None,
                mutation_depth: 0,
                suppress_until: Duration::ZERO,
                debounce_deadline: None,
                poll_deadline: None,
                fs_watcher: None,
                shutdown: false,
            }),
            wake: Condvar::new(),
            lifecycle: Mutex::new(()),
            debounce: options.debounce,
            suppress_trailing: options.suppress_trailing,
            poll_interval: options.poll_interval,
            backend: options.backend,
            head_fingerprint: options.head_fingerprint,
            metadata_fingerprint: options.metadata_fingerprint,
            manual_clock,
            origin: Instant::now(),
        });

        let timer_thread = if spawn_timer_thread {
            let timer_shared = shared.clone();
            match std::thread::Builder::new()
                .name("mergeon-status-watch-timer".into())
                .spawn(move || timer_loop(&timer_shared))
            {
                Ok(thread) => Some(thread),
                Err(error) => {
                    log::error!("Failed to start the status watcher timer thread: {error}");
                    None
                }
            }
        } else {
            None
        };

        Self {
            shared,
            timer_thread,
        }
    }

    /// `git:watchStatus`: deja de vigilar lo anterior y vigila `repo_path`.
    /// `on_change` recibe cada aviso desde el hilo del watcher (o desde quien
    /// llame a [`RepositoryStatusWatcher::run_due_timers`]), sin locks tomados.
    /// Con una ruta vacía solo detiene.
    pub fn start(
        &self,
        repo_path: impl AsRef<Path>,
        on_change: impl Fn(StatusChange) + Send + Sync + 'static,
    ) -> WatchMode {
        let shared = &self.shared;
        let repo_path = repo_path.as_ref();
        let _lifecycle = lock(&shared.lifecycle);

        shared.stop();
        if repo_path.as_os_str().is_empty() {
            return WatchMode::Stopped;
        }

        let last_head_fingerprint = (shared.head_fingerprint)(repo_path);
        let last_metadata_fingerprint = (shared.metadata_fingerprint)(repo_path);
        let generation = {
            let mut state = lock(&shared.state);
            if state.shutdown {
                return WatchMode::Stopped;
            }
            state.generation += 1;
            state.current_repo_path = Some(repo_path.to_path_buf());
            state.emit_change = Some(Arc::new(on_change));
            state.last_head_fingerprint = last_head_fingerprint;
            state.last_metadata_fingerprint = last_metadata_fingerprint;
            state.generation
        };

        // En Windows y macOS la vigilancia recursiva nativa es un solo handle;
        // en linux hay que podar el árbol: ver `WatchStrategy`.
        match shared.backend.watch(repo_path, shared.sink(generation)) {
            Ok(fs_watcher) => {
                let mut state = lock(&shared.state);
                // Si ya falló mientras arrancaba, el sondeo está en marcha.
                if state.generation == generation && state.poll_deadline.is_none() {
                    state.fs_watcher = Some(fs_watcher);
                    WatchMode::FsWatch
                } else {
                    drop(state);
                    drop(fs_watcher);
                    WatchMode::Poll
                }
            }
            Err(error) => {
                shared.handle_watcher_failure(generation, &error);
                WatchMode::Poll
            }
        }
    }

    /// Como [`RepositoryStatusWatcher::start`], leyendo el estado en cada
    /// aviso: `read_status` se inyecta (el watcher no sabe leerlo) y
    /// `on_change` recibe el aviso junto con lo leído. La lectura corre en el
    /// hilo del watcher; mientras dura, el siguiente aviso espera.
    pub fn start_with_status<S>(
        &self,
        repo_path: impl AsRef<Path>,
        read_status: impl Fn(&Path) -> S + Send + Sync + 'static,
        on_change: impl Fn(StatusChange, S) + Send + Sync + 'static,
    ) -> WatchMode {
        let watched_path = repo_path.as_ref().to_path_buf();
        self.start(repo_path, move |change| {
            let status = read_status(&watched_path);
            on_change(change, status);
        })
    }

    /// `git:unwatchStatus`.
    pub fn stop(&self) {
        let _lifecycle = lock(&self.shared.lifecycle);
        self.shared.stop();
    }

    /// `git:setStatusWatchWindowFocus`: sin foco, el sondeo se pausa.
    pub fn set_focused(&self, is_focused: bool) {
        lock(&self.shared.state).focused = is_focused;
    }

    /// Modo actual: vigilancia nativa, sondeo o detenido.
    pub fn mode(&self) -> WatchMode {
        let state = lock(&self.shared.state);
        if state.current_repo_path.is_none() {
            WatchMode::Stopped
        } else if state.poll_deadline.is_some() {
            WatchMode::Poll
        } else {
            WatchMode::FsWatch
        }
    }

    /// Empieza una mutación propia sobre `repo_path`: sus eventos se descartan.
    pub fn begin_mutation(&self, repo_path: impl AsRef<Path>) {
        let mut state = lock(&self.shared.state);
        if !Shared::is_current_repo(&state, repo_path.as_ref()) {
            return;
        }

        state.mutation_depth += 1;
    }

    /// Termina una mutación propia. Al cerrarse la última empieza la ventana
    /// de cola en la que aún se descartan eventos.
    pub fn end_mutation(&self, repo_path: impl AsRef<Path>) {
        let shared = &self.shared;
        let refresh = {
            let mut state = lock(&shared.state);
            if !Shared::is_current_repo(&state, repo_path.as_ref()) {
                return;
            }

            state.mutation_depth = state.mutation_depth.saturating_sub(1);
            if state.mutation_depth > 0 {
                return;
            }
            state.suppress_until = shared.now() + shared.suppress_trailing;
            state
                .current_repo_path
                .clone()
                .map(|repo_path| (repo_path, state.generation))
        };

        // Refrescar el fingerprint evita que el poll fallback dispare un
        // `poll-head` fantasma por un checkout/commit hecho desde la app.
        if let Some((current_repo_path, generation)) = refresh {
            let head_fingerprint = (shared.head_fingerprint)(&current_repo_path);
            let metadata_fingerprint = (shared.metadata_fingerprint)(&current_repo_path);
            let mut state = lock(&shared.state);
            if state.generation == generation {
                state.last_head_fingerprint = head_fingerprint;
                state.last_metadata_fingerprint = metadata_fingerprint;
            }
        }
    }

    /// Dispara los temporizadores vencidos según el reloj. Con
    /// [`WatchTimers::System`] ya lo hace el hilo del watcher; con
    /// [`WatchTimers::Manual`] hay que llamarlo tras avanzar el reloj.
    pub fn run_due_timers(&self) {
        self.shared.run_due_timers();
    }
}

impl MutationWatcher for RepositoryStatusWatcher {
    fn begin_mutation(&self, repo_path: &Path) {
        RepositoryStatusWatcher::begin_mutation(self, repo_path);
    }

    fn end_mutation(&self, repo_path: &Path) {
        RepositoryStatusWatcher::end_mutation(self, repo_path);
    }
}

impl Drop for RepositoryStatusWatcher {
    fn drop(&mut self) {
        let fs_watcher = {
            let mut state = lock(&self.shared.state);
            state.shutdown = true;
            self.shared.reset(&mut state)
        };
        self.shared.wake.notify_all();
        drop(fs_watcher);

        if let Some(thread) = self.timer_thread.take() {
            // Soltado desde su propio aviso no puede esperarse a sí mismo.
            if thread.thread().id() != std::thread::current().id() {
                let _ = thread.join();
            }
        }
    }
}
