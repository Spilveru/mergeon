//! Cola de mutaciones Git por repositorio.
//!
//! Port de `electron/services/repository-mutation-gate.js`.
//!
//! Serializa las mutaciones Git por repositorio y mantiene una cola observable.
//! Solicitudes con la misma `request_key` comparten el mismo resultado;
//! operaciones distintas se ejecutan en orden sobre el mismo `.git` y en
//! paralelo en repos diferentes.
//!
//! Allí cada tarea es una promesa encadenada; aquí es un closure síncrono que
//! corre en un hilo de trabajo por repositorio, dentro de
//! [`mergeon_gitcmd::run_with_operation_context`], de modo que todo `Git` creado
//! en la tarea hereda la cancelación y el timeout de la operación.

use std::any::{Any, TypeId};
use std::collections::{HashMap, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::{JoinHandle, ThreadId};
use std::time::{Duration, Instant, SystemTime};

use mergeon_gitcmd::{
    ControlCode, GitProgress, LOCAL_TIMEOUT, OperationContext, ProgressThrottle,
    run_with_operation_context,
};
use serde::{Deserialize, Serialize};

use crate::util::{iso_timestamp, lock, resolve_path};

#[cfg(test)]
mod tests;

/// Timeout de una operación cuando ni la solicitud ni la cola fijan otro.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_millis(120_000);

const GENERIC_ERROR_CODE: &str = "GIT_ERROR";

/// Fase de una operación, tal como viaja en `git:operation-event`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OperationPhase {
    Queued,
    Running,
    Cancelling,
    Cancelled,
    Timeout,
    Failed,
    Completed,
}

impl OperationPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            OperationPhase::Queued => "queued",
            OperationPhase::Running => "running",
            OperationPhase::Cancelling => "cancelling",
            OperationPhase::Cancelled => "cancelled",
            OperationPhase::Timeout => "timeout",
            OperationPhase::Failed => "failed",
            OperationPhase::Completed => "completed",
        }
    }

    /// Fases finales: la operación ya salió de la cola.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            OperationPhase::Cancelled
                | OperationPhase::Timeout
                | OperationPhase::Failed
                | OperationPhase::Completed
        )
    }
}

/// Una operación dentro del estado de la cola.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationSummary {
    pub operation_id: Option<String>,
    pub operation: String,
    pub phase: OperationPhase,
    /// 0 para la operación activa; 1.. para las que esperan.
    pub position: usize,
    pub cancellable: bool,
    pub started_at: Option<String>,
    pub queued_at: String,
}

/// `error` de un evento de operación.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationEventError {
    pub code: String,
    pub message: String,
}

/// Payload de `git:operation-event`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationEvent {
    pub operation_id: String,
    pub repo_path: String,
    pub operation: String,
    pub phase: OperationPhase,
    pub position: usize,
    pub queue_length: usize,
    pub active_operation_id: Option<String>,
    pub operations: Vec<OperationSummary>,
    pub error: Option<OperationEventError>,
}

/// Progreso de una operación en curso (`git:operation-progress`, sin
/// original): lo que git informa con `--progress` en fetch, pull, push y
/// clone, ya limitado a unas 10 actualizaciones por segundo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationProgress {
    pub operation_id: String,
    pub repo_path: String,
    pub operation: String,
    pub progress: GitProgress,
}

/// Estado de la cola de un repositorio (`git:getOperationQueue`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationQueue {
    pub active: Option<OperationSummary>,
    pub queued: Vec<OperationSummary>,
}

/// En qué fase estaba la operación que se pidió cancelar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CancelPhase {
    Queued,
    Running,
}

impl CancelPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            CancelPhase::Queued => "queued",
            CancelPhase::Running => "running",
        }
    }
}

/// Por qué no se pudo cancelar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CancelReason {
    #[serde(rename = "not-found")]
    NotFound,
    #[serde(rename = "not-cancellable")]
    NotCancellable,
}

impl CancelReason {
    pub fn as_str(self) -> &'static str {
        match self {
            CancelReason::NotFound => "not-found",
            CancelReason::NotCancellable => "not-cancellable",
        }
    }
}

/// Respuesta de `git:cancelOperation`: `{ ok: true, phase }` o `{ ok: false, reason }`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelResult {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<CancelPhase>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<CancelReason>,
}

impl CancelResult {
    fn cancelled(phase: CancelPhase) -> Self {
        Self {
            ok: true,
            phase: Some(phase),
            reason: None,
        }
    }

    fn rejected(reason: CancelReason) -> Self {
        Self {
            ok: false,
            phase: None,
            reason: Some(reason),
        }
    }
}

/// Error con el que termina una solicitud cuya tarea no devolvió un valor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MutationError {
    /// `GitOperationControlError`: la operación se canceló (en cola o en curso)
    /// o venció su timeout.
    Control {
        code: ControlCode,
        operation_id: Option<String>,
    },
    /// La tarea entró en pánico (una tarea JS que lanza una excepción).
    Panicked {
        message: String,
        operation_id: Option<String>,
    },
}

impl MutationError {
    /// `getGitOperationControlCode(error)`.
    pub fn control_code(&self) -> Option<ControlCode> {
        match self {
            MutationError::Control { code, .. } => Some(*code),
            MutationError::Panicked { .. } => None,
        }
    }

    pub fn operation_id(&self) -> Option<&str> {
        match self {
            MutationError::Control { operation_id, .. }
            | MutationError::Panicked { operation_id, .. } => operation_id.as_deref(),
        }
    }

    /// `error.code || 'GIT_ERROR'`.
    pub fn code(&self) -> &'static str {
        self.control_code()
            .map_or(GENERIC_ERROR_CODE, ControlCode::as_str)
    }
}

impl std::fmt::Display for MutationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MutationError::Control { code, .. } => f.write_str(code.message()),
            MutationError::Panicked { message, .. } => f.write_str(message),
        }
    }
}

impl std::error::Error for MutationError {}

/// Lo que la cola necesita saber del resultado de una tarea para decidir la
/// fase final y el `error` del evento: en el original se leen `success`, `code`
/// y `message` del objeto de resultado.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OutcomeReport {
    /// `result.success === false`.
    pub failed: bool,
    /// `result.code`. `CANCELLED` / `TIMEOUT` marcan la fase final aunque la
    /// cola no haya pedido el control.
    pub code: Option<String>,
    /// `result.message`.
    pub message: Option<String>,
}

impl OutcomeReport {
    /// Resultado sin nada que informar: la operación termina `completed`.
    pub fn success() -> Self {
        Self::default()
    }

    /// Resultado fallido sin código propio (el evento lleva `GIT_ERROR`).
    pub fn failure(message: impl Into<String>) -> Self {
        Self {
            failed: true,
            code: None,
            message: Some(message.into()),
        }
    }

    /// A partir de los campos del resultado estructurado de una mutación.
    pub fn from_fields(success: bool, code: Option<&str>, message: Option<&str>) -> Self {
        Self {
            failed: !success,
            code: code.filter(|code| !code.is_empty()).map(str::to_string),
            message: message.map(str::to_string),
        }
    }

    fn control_code(&self) -> Option<ControlCode> {
        match self.code.as_deref() {
            Some("CANCELLED") => Some(ControlCode::Cancelled),
            Some("TIMEOUT") => Some(ControlCode::Timeout),
            _ => None,
        }
    }
}

/// Resultado de una tarea que la cola sabe interpretar. La implementación por
/// defecto informa éxito; para tipos de otros crates está
/// [`RepositoryMutationGate::enqueue_with_report`].
pub trait MutationOutcome {
    fn outcome_report(&self) -> OutcomeReport {
        OutcomeReport::success()
    }
}

impl MutationOutcome for () {}
impl MutationOutcome for bool {}
impl MutationOutcome for String {}
impl MutationOutcome for &'static str {}

impl MutationOutcome for serde_json::Value {
    fn outcome_report(&self) -> OutcomeReport {
        let failed = self.get("success").and_then(serde_json::Value::as_bool) == Some(false);
        let text = |field: &str| {
            self.get(field)
                .and_then(serde_json::Value::as_str)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        };
        OutcomeReport {
            failed,
            code: text("code"),
            // `result.message || String(result)`.
            message: text("message").or_else(|| failed.then(|| self.to_string())),
        }
    }
}

impl<T: MutationOutcome> MutationOutcome for Option<T> {
    fn outcome_report(&self) -> OutcomeReport {
        self.as_ref()
            .map_or_else(OutcomeReport::success, T::outcome_report)
    }
}

/// Un `Err` equivale a una tarea que lanza: la operación termina `failed`.
impl<T: MutationOutcome, E: std::fmt::Display> MutationOutcome for Result<T, E> {
    fn outcome_report(&self) -> OutcomeReport {
        match self {
            Ok(value) => value.outcome_report(),
            Err(error) => OutcomeReport::failure(error.to_string()),
        }
    }
}

impl<T: MutationOutcome + ?Sized> MutationOutcome for Arc<T> {
    fn outcome_report(&self) -> OutcomeReport {
        (**self).outcome_report()
    }
}

impl<T: MutationOutcome + ?Sized> MutationOutcome for Box<T> {
    fn outcome_report(&self) -> OutcomeReport {
        (**self).outcome_report()
    }
}

/// Quien suprime sus propios avisos mientras corre una mutación de la app
/// (el watcher de estado). Se llama alrededor de cada tarea con `watch`.
pub trait MutationWatcher: Send + Sync {
    fn begin_mutation(&self, repo_path: &Path);
    fn end_mutation(&self, repo_path: &Path);
}

/// Opciones de una solicitud (`options` de `runExclusive`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MutationOptions {
    /// Identifica la operación para cancelarla y en los eventos. Sin
    /// identificador la operación no emite eventos ni se puede cancelar.
    pub operation_id: Option<String>,
    /// Nombre de la operación (`fetch`, `stage`...). Por defecto `git`.
    pub operation: Option<String>,
    /// Tiempo máximo de la tarea desde que arranca. `None`: el de la cola.
    /// `Duration::ZERO`: sin límite propio (queda el de inactividad de git).
    pub timeout: Option<Duration>,
    /// Avisar al watcher alrededor de la tarea. Por defecto sí.
    pub watch: bool,
}

impl Default for MutationOptions {
    fn default() -> Self {
        Self {
            operation_id: None,
            operation: None,
            timeout: None,
            watch: true,
        }
    }
}

impl MutationOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn operation(mut self, operation: impl Into<String>) -> Self {
        self.operation = Some(operation.into());
        self
    }

    pub fn operation_id(mut self, operation_id: impl Into<String>) -> Self {
        self.operation_id = Some(operation_id.into());
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    pub fn watch(mut self, watch: bool) -> Self {
        self.watch = watch;
        self
    }
}

type EventCallback = Arc<dyn Fn(OperationEvent) + Send + Sync>;
type ProgressCallback = Arc<dyn Fn(OperationProgress) + Send + Sync>;
type KeyNormalizer = Arc<dyn Fn(&Path) -> String + Send + Sync>;

/// Configuración de la cola (`createRepositoryMutationGate({...})`).
pub struct MutationGateBuilder {
    watcher: Option<Arc<dyn MutationWatcher>>,
    normalize_key: Option<KeyNormalizer>,
    on_event: Option<EventCallback>,
    on_progress: Option<ProgressCallback>,
    default_timeout: Duration,
}

impl MutationGateBuilder {
    /// Watcher al que avisar de cada mutación.
    pub fn watcher(mut self, watcher: Arc<dyn MutationWatcher>) -> Self {
        self.watcher = Some(watcher);
        self
    }

    /// Cómo se obtiene la clave de cola de una ruta. Por defecto, la ruta
    /// absoluta normalizada (`path.resolve`).
    pub fn normalize_key(
        mut self,
        normalize_key: impl Fn(&Path) -> String + Send + Sync + 'static,
    ) -> Self {
        self.normalize_key = Some(Arc::new(normalize_key));
        self
    }

    /// Receptor de los eventos de operación. Se llama desde el hilo que
    /// provoca el evento, en orden y sin locks de la cola tomados; no debe
    /// bloquearse esperando a otra operación de la misma cola.
    pub fn on_event(mut self, on_event: impl Fn(OperationEvent) + Send + Sync + 'static) -> Self {
        self.on_event = Some(Arc::new(on_event));
        self
    }

    /// Receptor del progreso de las operaciones con identificador. Se llama
    /// desde el hilo que lee la salida de git, como mucho unas 10 veces por
    /// segundo por operación (los cambios de fase pasan siempre) y siempre
    /// antes del evento final de la operación.
    pub fn on_progress(
        mut self,
        on_progress: impl Fn(OperationProgress) + Send + Sync + 'static,
    ) -> Self {
        self.on_progress = Some(Arc::new(on_progress));
        self
    }

    pub fn default_timeout(mut self, default_timeout: Duration) -> Self {
        self.default_timeout = default_timeout;
        self
    }

    pub fn build(self) -> RepositoryMutationGate {
        RepositoryMutationGate {
            inner: Arc::new(Inner {
                state: Mutex::new(State::default()),
                timer_wake: Condvar::new(),
                watcher: self.watcher,
                normalize_key: self.normalize_key,
                on_event: self.on_event,
                on_progress: self.on_progress,
                default_timeout: self.default_timeout,
                pending_events: Mutex::new(VecDeque::new()),
                delivery: Mutex::new(()),
                delivering_thread: Mutex::new(None),
            }),
        }
    }
}

/// Resultado compartido de una solicitud.
struct Slot<T> {
    result: Mutex<Option<Result<T, MutationError>>>,
    ready: Condvar,
}

impl<T> Slot<T> {
    fn new() -> Self {
        Self {
            result: Mutex::new(None),
            ready: Condvar::new(),
        }
    }

    fn fill(&self, result: Result<T, MutationError>) {
        let mut slot = lock(&self.result);
        if slot.is_none() {
            *slot = Some(result);
        }
        drop(slot);
        self.ready.notify_all();
    }
}

/// Lo que la cola puede hacer con un resultado sin conocer su tipo.
trait ErasedSlot: Send + Sync {
    fn reject(&self, error: MutationError);
}

impl<T: Send> ErasedSlot for Slot<T> {
    fn reject(&self, error: MutationError) {
        self.fill(Err(error));
    }
}

/// Solicitud encolada. Equivale a la promesa que devuelve `runExclusive`: se
/// puede esperar desde varios hilos y las solicitudes idénticas la comparten.
pub struct MutationHandle<T> {
    slot: Arc<Slot<T>>,
    operation_id: Option<String>,
    coalesced: bool,
}

impl<T> Clone for MutationHandle<T> {
    fn clone(&self) -> Self {
        Self {
            slot: self.slot.clone(),
            operation_id: self.operation_id.clone(),
            coalesced: self.coalesced,
        }
    }
}

impl<T> std::fmt::Debug for MutationHandle<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MutationHandle")
            .field("operation_id", &self.operation_id)
            .field("coalesced", &self.coalesced)
            .field("finished", &self.is_finished())
            .finish()
    }
}

impl<T> MutationHandle<T> {
    /// Identificador de la operación que resolverá esta solicitud. En una
    /// solicitud deduplicada es el de la operación original.
    pub fn operation_id(&self) -> Option<&str> {
        self.operation_id.as_deref()
    }

    /// La solicitud se unió a otra idéntica que ya estaba en la cola.
    pub fn is_coalesced(&self) -> bool {
        self.coalesced
    }

    /// Ambas solicitudes esperan la misma operación.
    pub fn shares_operation_with(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.slot, &other.slot)
    }

    pub fn is_finished(&self) -> bool {
        lock(&self.slot.result).is_some()
    }
}

impl<T: Clone> MutationHandle<T> {
    /// Bloquea hasta que la operación termina.
    pub fn wait(&self) -> Result<T, MutationError> {
        let mut result = lock(&self.slot.result);
        loop {
            if let Some(result) = result.as_ref() {
                return result.clone();
            }
            result = self
                .slot
                .ready
                .wait(result)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// Como [`MutationHandle::wait`], con un máximo de espera. `None` si la
    /// operación sigue en la cola al vencer.
    pub fn wait_timeout(&self, timeout: Duration) -> Option<Result<T, MutationError>> {
        let deadline = Instant::now() + timeout;
        let mut result = lock(&self.slot.result);
        loop {
            if let Some(result) = result.as_ref() {
                return Some(result.clone());
            }
            let remaining = deadline.checked_duration_since(Instant::now())?;
            if remaining.is_zero() {
                return None;
            }
            result = self
                .slot
                .ready
                .wait_timeout(result, remaining)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// El resultado si la operación ya terminó.
    pub fn try_result(&self) -> Option<Result<T, MutationError>> {
        lock(&self.slot.result).clone()
    }
}

enum JobResult {
    Finished {
        report: OutcomeReport,
        publish: Box<dyn FnOnce() + Send>,
    },
    Panicked(String),
}

type Job = Box<dyn FnOnce() -> JobResult + Send>;

/// Solicitudes idénticas: misma clave y mismo tipo de resultado.
type RequestKey = (String, TypeId);

struct ActiveRequest {
    entry_id: u64,
    slot: Arc<dyn Any + Send + Sync>,
}

struct Entry {
    id: u64,
    repo_path: PathBuf,
    request_key: Option<RequestKey>,
    operation_id: Option<String>,
    operation: String,
    phase: OperationPhase,
    queued_at: String,
    started_at: Option<String>,
    timeout: Duration,
    deadline: Option<Instant>,
    watch: bool,
    control_code: Option<ControlCode>,
    cancellation_event_sent: bool,
    context: OperationContext,
    slot: Arc<dyn ErasedSlot>,
}

#[derive(Default)]
struct Queue {
    /// En orden de llegada: la operación en curso, las que esperan y las
    /// canceladas en cola que aún no llegaron a su turno.
    entries: Vec<Entry>,
    active: HashMap<RequestKey, ActiveRequest>,
    jobs: VecDeque<(u64, Job)>,
    worker_running: bool,
}

#[derive(Default)]
struct State {
    queues: HashMap<String, Queue>,
    /// operationId → (clave de cola, id de entrada).
    operations: HashMap<String, (String, u64)>,
    next_entry_id: u64,
    shutdown: bool,
    workers: Vec<JoinHandle<()>>,
    timer: Option<JoinHandle<()>>,
}

struct Inner {
    state: Mutex<State>,
    timer_wake: Condvar,
    watcher: Option<Arc<dyn MutationWatcher>>,
    normalize_key: Option<KeyNormalizer>,
    on_event: Option<EventCallback>,
    on_progress: Option<ProgressCallback>,
    default_timeout: Duration,
    /// Los eventos se construyen con el estado tomado y se entregan después,
    /// sin locks, en el mismo orden.
    pending_events: Mutex<VecDeque<OperationEvent>>,
    delivery: Mutex<()>,
    delivering_thread: Mutex<Option<ThreadId>>,
}

fn to_operation_summary(entry: &Entry, position: usize) -> OperationSummary {
    OperationSummary {
        operation_id: entry.operation_id.clone(),
        operation: entry.operation.clone(),
        phase: entry.phase,
        position,
        cancellable: entry.phase == OperationPhase::Running,
        started_at: entry.started_at.clone(),
        queued_at: entry.queued_at.clone(),
    }
}

fn is_active_phase(phase: OperationPhase) -> bool {
    matches!(phase, OperationPhase::Running | OperationPhase::Cancelling)
}

fn final_phase_for(control_code: ControlCode) -> OperationPhase {
    match control_code {
        ControlCode::Cancelled => OperationPhase::Cancelled,
        ControlCode::Timeout => OperationPhase::Timeout,
    }
}

fn panic_message(payload: Box<dyn Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&'static str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "Repository mutation task panicked.".to_string()
    }
}

impl Inner {
    fn queue_key(&self, repo_path: &Path) -> String {
        match &self.normalize_key {
            Some(normalize_key) => normalize_key(repo_path),
            None => resolve_path(repo_path).to_string_lossy().into_owned(),
        }
    }

    /// `safeEmit`: deja listo el evento de `entry` con el estado de su cola.
    /// `entries` no incluye a las operaciones ya finalizadas.
    fn emit(
        &self,
        entries: &[Entry],
        entry: &Entry,
        phase: OperationPhase,
        error: Option<OperationEventError>,
    ) {
        if self.on_event.is_none() {
            return;
        }
        let Some(operation_id) = entry.operation_id.clone() else {
            return;
        };

        let active = entries
            .iter()
            .find(|candidate| is_active_phase(candidate.phase));
        let queued: Vec<&Entry> = entries
            .iter()
            .filter(|candidate| candidate.phase == OperationPhase::Queued)
            .collect();
        let position = if phase == OperationPhase::Queued {
            queued
                .iter()
                .position(|candidate| candidate.id == entry.id)
                .map_or(0, |index| index + 1)
        } else {
            0
        };

        let mut operations = Vec::with_capacity(queued.len() + 1);
        operations.extend(active.map(|active| to_operation_summary(active, 0)));
        operations.extend(
            queued
                .iter()
                .enumerate()
                .map(|(index, candidate)| to_operation_summary(candidate, index + 1)),
        );

        lock(&self.pending_events).push_back(OperationEvent {
            operation_id,
            repo_path: entry.repo_path.to_string_lossy().into_owned(),
            operation: entry.operation.clone(),
            phase,
            position,
            queue_length: queued.len(),
            active_operation_id: active.and_then(|active| active.operation_id.clone()),
            operations,
            error,
        });
    }

    /// Entrega los eventos pendientes. Al volver, todo evento encolado antes
    /// de la llamada ya fue entregado (por este hilo o por otro), salvo que la
    /// llamada venga del propio receptor: entonces lo entrega el bucle que ya
    /// está corriendo en este hilo.
    fn flush_events(&self) {
        let Some(on_event) = &self.on_event else {
            return;
        };
        let current = std::thread::current().id();
        if *lock(&self.delivering_thread) == Some(current) {
            return;
        }

        let _delivery = lock(&self.delivery);
        *lock(&self.delivering_thread) = Some(current);
        loop {
            let event = lock(&self.pending_events).pop_front();
            let Some(event) = event else {
                break;
            };
            // UI event delivery must never affect the Git operation itself.
            let _ = catch_unwind(AssertUnwindSafe(|| on_event(event)));
        }
        *lock(&self.delivering_thread) = None;
    }

    /// `removeEntry`: saca la operación de la cola y emite su fase final.
    fn remove_entry(
        &self,
        state: &mut State,
        key: &str,
        id: u64,
        final_phase: OperationPhase,
        error: Option<OperationEventError>,
    ) -> Option<Entry> {
        let queue = state.queues.get_mut(key)?;
        let index = queue.entries.iter().position(|entry| entry.id == id)?;
        let mut entry = queue.entries.remove(index);
        entry.phase = final_phase;
        entry.deadline = None;

        if let Some(request_key) = &entry.request_key
            && queue
                .active
                .get(request_key)
                .is_some_and(|active| active.entry_id == id)
        {
            queue.active.remove(request_key);
        }
        if let Some(operation_id) = &entry.operation_id
            && state
                .operations
                .get(operation_id)
                .is_some_and(|(_, entry_id)| *entry_id == id)
        {
            state.operations.remove(operation_id);
        }

        if !entry.cancellation_event_sent {
            self.emit(&queue.entries, &entry, final_phase, error);
        }
        Some(entry)
    }

    /// Cancela una entrada concreta (`cancel` del original, ya localizada).
    fn cancel_entry(&self, state: &mut State, key: &str, id: u64) -> CancelResult {
        let Some(queue) = state.queues.get_mut(key) else {
            return CancelResult::rejected(CancelReason::NotFound);
        };
        let Some(index) = queue.entries.iter().position(|entry| entry.id == id) else {
            return CancelResult::rejected(CancelReason::NotFound);
        };

        match queue.entries[index].phase {
            OperationPhase::Queued => {
                let entry = &mut queue.entries[index];
                entry.control_code = Some(ControlCode::Cancelled);
                entry.phase = OperationPhase::Cancelled;
                entry.cancellation_event_sent = true;
                if let Some(request_key) = &entry.request_key
                    && queue
                        .active
                        .get(request_key)
                        .is_some_and(|active| active.entry_id == id)
                {
                    queue.active.remove(request_key);
                }
                self.emit(
                    &queue.entries,
                    &queue.entries[index],
                    OperationPhase::Cancelled,
                    None,
                );
                CancelResult::cancelled(CancelPhase::Queued)
            }
            OperationPhase::Running => {
                let entry = &mut queue.entries[index];
                entry.control_code = Some(ControlCode::Cancelled);
                entry.phase = OperationPhase::Cancelling;
                entry.context.cancel();
                self.emit(
                    &queue.entries,
                    &queue.entries[index],
                    OperationPhase::Cancelling,
                    None,
                );
                CancelResult::cancelled(CancelPhase::Running)
            }
            _ => CancelResult::rejected(CancelReason::NotCancellable),
        }
    }

    /// Vence el timeout de una operación en curso.
    fn expire_entry(&self, state: &mut State, key: &str, id: u64) {
        let Some(queue) = state.queues.get_mut(key) else {
            return;
        };
        let Some(index) = queue.entries.iter().position(|entry| entry.id == id) else {
            return;
        };
        let entry = &mut queue.entries[index];
        if entry.phase != OperationPhase::Running {
            return;
        }

        entry.deadline = None;
        entry.control_code = Some(ControlCode::Timeout);
        entry.phase = OperationPhase::Cancelling;
        // El primer código gana: hay que fijar TIMEOUT antes de abortar.
        entry.context.set_control_code(ControlCode::Timeout);
        entry.context.cancel();
        self.emit(
            &queue.entries,
            &queue.entries[index],
            OperationPhase::Cancelling,
            None,
        );
    }

    fn notify_watcher(&self, repo_path: &Path, begin: bool) {
        let Some(watcher) = &self.watcher else {
            return;
        };
        // Un fallo del watcher no debe dejar la cola sin su hilo de trabajo.
        let _ = catch_unwind(AssertUnwindSafe(|| {
            if begin {
                watcher.begin_mutation(repo_path);
            } else {
                watcher.end_mutation(repo_path);
            }
        }));
    }
}

fn ensure_timer(inner: &Arc<Inner>, state: &mut State) {
    if state.timer.is_some() {
        return;
    }
    let shared = inner.clone();
    match std::thread::Builder::new()
        .name("mergeon-mutation-timeout".into())
        .spawn(move || timer_loop(&shared))
    {
        Ok(handle) => state.timer = Some(handle),
        Err(error) => log::error!("Failed to start the mutation timeout thread: {error}"),
    }
}

fn timer_loop(inner: &Inner) {
    let mut state = lock(&inner.state);
    loop {
        if state.shutdown {
            return;
        }

        let now = Instant::now();
        let mut due = Vec::new();
        let mut nearest: Option<Instant> = None;
        for (key, queue) in &state.queues {
            for entry in &queue.entries {
                if entry.phase != OperationPhase::Running {
                    continue;
                }
                match entry.deadline {
                    Some(deadline) if deadline <= now => due.push((key.clone(), entry.id)),
                    Some(deadline) => {
                        nearest = Some(nearest.map_or(deadline, |nearest| nearest.min(deadline)));
                    }
                    None => {}
                }
            }
        }

        if !due.is_empty() {
            for (key, id) in due {
                inner.expire_entry(&mut state, &key, id);
            }
            drop(state);
            inner.flush_events();
            state = lock(&inner.state);
            continue;
        }

        state = match nearest {
            Some(deadline) => {
                inner
                    .timer_wake
                    .wait_timeout(state, deadline.saturating_duration_since(now))
                    .unwrap_or_else(PoisonError::into_inner)
                    .0
            }
            None => inner
                .timer_wake
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner),
        };
    }
}

fn worker_loop(inner: &Arc<Inner>, key: &str) {
    loop {
        let (id, job) = {
            let mut state = lock(&inner.state);
            let Some(queue) = state.queues.get_mut(key) else {
                return;
            };
            match queue.jobs.pop_front() {
                Some(next) => next,
                None => {
                    // `cleanupQueue`: una cola sin trabajo no se conserva.
                    queue.worker_running = false;
                    if queue.entries.is_empty() && queue.active.is_empty() {
                        state.queues.remove(key);
                    }
                    return;
                }
            }
        };
        run_job(inner, key, id, job);
    }
}

struct RunningJob {
    repo_path: PathBuf,
    watch: bool,
    context: OperationContext,
    operation_id: Option<String>,
}

fn run_job(inner: &Arc<Inner>, key: &str, id: u64, job: Job) {
    // Arranque: la operación deja de esperar, salvo que la cancelaran en cola.
    let started = {
        let mut state = lock(&inner.state);
        let Some(index) = state
            .queues
            .get(key)
            .and_then(|queue| queue.entries.iter().position(|entry| entry.id == id))
        else {
            return;
        };
        let Some(queue) = state.queues.get_mut(key) else {
            return;
        };

        if queue.entries[index].control_code == Some(ControlCode::Cancelled) {
            let error = MutationError::Control {
                code: ControlCode::Cancelled,
                operation_id: queue.entries[index].operation_id.clone(),
            };
            let removed = inner.remove_entry(&mut state, key, id, OperationPhase::Cancelled, None);
            Err((removed, error))
        } else {
            let entry = &mut queue.entries[index];
            entry.phase = OperationPhase::Running;
            entry.started_at = Some(iso_timestamp(SystemTime::now()));
            let has_deadline = !entry.timeout.is_zero();
            if has_deadline {
                entry.deadline = Some(Instant::now() + entry.timeout);
            }
            let running = RunningJob {
                repo_path: entry.repo_path.clone(),
                watch: entry.watch,
                context: entry.context.clone(),
                operation_id: entry.operation_id.clone(),
            };
            inner.emit(
                &queue.entries,
                &queue.entries[index],
                OperationPhase::Running,
                None,
            );
            if has_deadline {
                ensure_timer(inner, &mut state);
            }
            Ok(running)
        }
    };

    let running = match started {
        Ok(running) => running,
        Err((removed, error)) => {
            inner.flush_events();
            // La tarea no llega a ejecutarse; se suelta fuera del lock.
            drop(job);
            if let Some(entry) = removed {
                entry.slot.reject(error);
            }
            return;
        }
    };
    inner.timer_wake.notify_all();
    inner.flush_events();

    if running.watch {
        inner.notify_watcher(&running.repo_path, true);
    }
    let result = run_with_operation_context(running.context.clone(), job);
    if running.watch {
        inner.notify_watcher(&running.repo_path, false);
    }

    // Cierre: fase final, evento y, solo después, el resultado a quien espera.
    let publish: Box<dyn FnOnce() + Send> = {
        let mut state = lock(&inner.state);
        let entry_control_code = state
            .queues
            .get(key)
            .and_then(|queue| queue.entries.iter().find(|entry| entry.id == id))
            .and_then(|entry| entry.control_code);
        // El ejecutor de git también fija TIMEOUT en el contexto cuando mata
        // un proceso por inactividad (el plugin `timeout` de simple-git).
        let control_code = entry_control_code.or_else(|| running.context.control_code());

        match result {
            JobResult::Finished { report, publish } => {
                let control_code = control_code.or_else(|| report.control_code());
                let final_phase = match control_code {
                    Some(code) => final_phase_for(code),
                    None if report.failed => OperationPhase::Failed,
                    None => OperationPhase::Completed,
                };
                let error = report.failed.then(|| OperationEventError {
                    code: report
                        .code
                        .unwrap_or_else(|| GENERIC_ERROR_CODE.to_string()),
                    message: report.message.unwrap_or_default(),
                });
                inner.remove_entry(&mut state, key, id, final_phase, error);
                publish
            }
            JobResult::Panicked(message) => {
                let (final_phase, error) = match control_code {
                    Some(code) => (
                        final_phase_for(code),
                        MutationError::Control {
                            code,
                            operation_id: running.operation_id.clone(),
                        },
                    ),
                    None => (
                        OperationPhase::Failed,
                        MutationError::Panicked {
                            message,
                            operation_id: running.operation_id.clone(),
                        },
                    ),
                };
                let event_error = OperationEventError {
                    code: error.code().to_string(),
                    message: error.to_string(),
                };
                let removed =
                    inner.remove_entry(&mut state, key, id, final_phase, Some(event_error));
                Box::new(move || {
                    if let Some(entry) = removed {
                        entry.slot.reject(error);
                    }
                })
            }
        }
    };
    inner.flush_events();
    publish();
}

/// Cola de mutaciones. Se comparte entre hilos por referencia (`Arc`); al
/// soltarla cancela lo que quede y espera a sus hilos.
pub struct RepositoryMutationGate {
    inner: Arc<Inner>,
}

impl Default for RepositoryMutationGate {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for RepositoryMutationGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = lock(&self.inner.state);
        f.debug_struct("RepositoryMutationGate")
            .field("queues", &state.queues.len())
            .field("operations", &state.operations.len())
            .finish()
    }
}

impl RepositoryMutationGate {
    /// Cola sin watcher ni receptor de eventos.
    pub fn new() -> Self {
        Self::builder().build()
    }

    pub fn builder() -> MutationGateBuilder {
        MutationGateBuilder {
            watcher: None,
            normalize_key: None,
            on_event: None,
            on_progress: None,
            default_timeout: DEFAULT_TIMEOUT,
        }
    }

    /// `runExclusive`: encola `task` para `repo_path` y bloquea hasta tener su
    /// resultado. Con `request_key`, una solicitud idéntica que ya esté en la
    /// cola se reutiliza y `task` no llega a ejecutarse.
    ///
    /// No debe llamarse desde una tarea del mismo repositorio: esperaría a que
    /// termine la tarea que la está llamando.
    pub fn run_exclusive<T, F>(
        &self,
        repo_path: impl AsRef<Path>,
        request_key: Option<&str>,
        options: MutationOptions,
        task: F,
    ) -> Result<T, MutationError>
    where
        T: MutationOutcome + Clone + Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        self.enqueue(repo_path, request_key, options, task).wait()
    }

    /// Como [`RepositoryMutationGate::run_exclusive`], leyendo el resultado
    /// con `report` en vez de con [`MutationOutcome`].
    pub fn run_exclusive_with_report<T, F, R>(
        &self,
        repo_path: impl AsRef<Path>,
        request_key: Option<&str>,
        options: MutationOptions,
        task: F,
        report: R,
    ) -> Result<T, MutationError>
    where
        T: Clone + Send + 'static,
        F: FnOnce() -> T + Send + 'static,
        R: FnOnce(&T) -> OutcomeReport + Send + 'static,
    {
        self.enqueue_with_report(repo_path, request_key, options, task, report)
            .wait()
    }

    /// Encola `task` sin esperar: devuelve la solicitud para esperarla después.
    pub fn enqueue<T, F>(
        &self,
        repo_path: impl AsRef<Path>,
        request_key: Option<&str>,
        options: MutationOptions,
        task: F,
    ) -> MutationHandle<T>
    where
        T: MutationOutcome + Clone + Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        self.enqueue_with_report(repo_path, request_key, options, task, T::outcome_report)
    }

    /// Como [`RepositoryMutationGate::enqueue`], para resultados que no
    /// implementan [`MutationOutcome`] (los tipos de otros crates): `report`
    /// dice a la cola cómo leer `success`, `code` y `message`.
    pub fn enqueue_with_report<T, F, R>(
        &self,
        repo_path: impl AsRef<Path>,
        request_key: Option<&str>,
        options: MutationOptions,
        task: F,
        report: R,
    ) -> MutationHandle<T>
    where
        T: Clone + Send + 'static,
        F: FnOnce() -> T + Send + 'static,
        R: FnOnce(&T) -> OutcomeReport + Send + 'static,
    {
        let inner = &self.inner;
        let repo_path = repo_path.as_ref();
        let key = inner.queue_key(repo_path);
        let request_key: Option<RequestKey> = request_key
            .filter(|request_key| !request_key.is_empty())
            .map(|request_key| (request_key.to_string(), TypeId::of::<T>()));
        let operation_id = options.operation_id.filter(|id| !id.is_empty());
        let slot = Arc::new(Slot::<T>::new());

        let mut state = lock(&inner.state);

        if state.shutdown {
            drop(state);
            slot.fill(Err(MutationError::Control {
                code: ControlCode::Cancelled,
                operation_id: operation_id.clone(),
            }));
            return MutationHandle {
                slot,
                operation_id,
                coalesced: false,
            };
        }

        if let Some(request_key) = &request_key {
            let existing = state.queues.get(&key).and_then(|queue| {
                let active = queue.active.get(request_key)?;
                let slot = active.slot.clone().downcast::<Slot<T>>().ok()?;
                let operation_id = queue
                    .entries
                    .iter()
                    .find(|entry| entry.id == active.entry_id)
                    .and_then(|entry| entry.operation_id.clone());
                Some((slot, operation_id))
            });
            if let Some((slot, operation_id)) = existing {
                drop(state);
                return MutationHandle {
                    slot,
                    operation_id,
                    coalesced: true,
                };
            }
        }

        let id = state.next_entry_id;
        state.next_entry_id += 1;

        let timeout = options.timeout.unwrap_or(inner.default_timeout);
        let context = OperationContext::new(
            operation_id.clone(),
            // `operationContext.timeoutMs || GIT_OPERATION_TIMEOUTS.local`.
            if timeout.is_zero() {
                LOCAL_TIMEOUT
            } else {
                timeout
            },
        );
        if let (Some(on_progress), Some(operation_id)) = (&inner.on_progress, &operation_id) {
            let on_progress = on_progress.clone();
            let throttle = Mutex::new(ProgressThrottle::default());
            let operation_id = operation_id.clone();
            let repo_path = repo_path.to_string_lossy().into_owned();
            let operation = options
                .operation
                .clone()
                .filter(|operation| !operation.is_empty())
                .unwrap_or_else(|| "git".to_string());
            context.set_progress_sink(Arc::new(move |progress: GitProgress| {
                if !lock(&throttle).admit(&progress, Instant::now()) {
                    return;
                }
                // Igual que los eventos: la UI no puede tumbar la operación.
                let _ = catch_unwind(AssertUnwindSafe(|| {
                    on_progress(OperationProgress {
                        operation_id: operation_id.clone(),
                        repo_path: repo_path.clone(),
                        operation: operation.clone(),
                        progress,
                    })
                }));
            }));
        }

        let job_slot = slot.clone();
        let job: Job = Box::new(move || match catch_unwind(AssertUnwindSafe(task)) {
            Ok(value) => {
                let report = catch_unwind(AssertUnwindSafe(|| report(&value)))
                    .unwrap_or_else(|_| OutcomeReport::success());
                JobResult::Finished {
                    report,
                    publish: Box::new(move || job_slot.fill(Ok(value))),
                }
            }
            Err(payload) => JobResult::Panicked(panic_message(payload)),
        });

        if let Some(operation_id) = &operation_id {
            state
                .operations
                .insert(operation_id.clone(), (key.clone(), id));
        }

        let needs_worker = {
            let queue = state.queues.entry(key.clone()).or_default();
            queue.entries.push(Entry {
                id,
                repo_path: repo_path.to_path_buf(),
                request_key: request_key.clone(),
                operation_id: operation_id.clone(),
                operation: options
                    .operation
                    .filter(|operation| !operation.is_empty())
                    .unwrap_or_else(|| "git".to_string()),
                phase: OperationPhase::Queued,
                queued_at: iso_timestamp(SystemTime::now()),
                started_at: None,
                timeout,
                deadline: None,
                watch: options.watch,
                control_code: None,
                cancellation_event_sent: false,
                context,
                slot: slot.clone(),
            });
            if let Some(entry) = queue.entries.last() {
                inner.emit(&queue.entries, entry, OperationPhase::Queued, None);
            }
            queue.jobs.push_back((id, job));
            if let Some(request_key) = request_key {
                queue.active.insert(
                    request_key,
                    ActiveRequest {
                        entry_id: id,
                        slot: slot.clone(),
                    },
                );
            }
            !std::mem::replace(&mut queue.worker_running, true)
        };

        let mut spawn_failure = None;
        if needs_worker {
            state.workers.retain(|worker| !worker.is_finished());
            let worker_inner = inner.clone();
            let worker_key = key.clone();
            match std::thread::Builder::new()
                .name("mergeon-mutation-worker".into())
                .spawn(move || worker_loop(&worker_inner, &worker_key))
            {
                Ok(handle) => state.workers.push(handle),
                Err(error) => {
                    // Sin hilo no hay quien ejecute la tarea: se rechaza aquí.
                    let error = MutationError::Panicked {
                        message: format!("Failed to start the repository mutation worker: {error}"),
                        operation_id: operation_id.clone(),
                    };
                    let job = state.queues.get_mut(&key).and_then(|queue| {
                        queue.worker_running = false;
                        queue.jobs.pop_back()
                    });
                    let event_error = OperationEventError {
                        code: error.code().to_string(),
                        message: error.to_string(),
                    };
                    inner.remove_entry(
                        &mut state,
                        &key,
                        id,
                        OperationPhase::Failed,
                        Some(event_error),
                    );
                    if state
                        .queues
                        .get(&key)
                        .is_some_and(|queue| queue.entries.is_empty() && queue.active.is_empty())
                    {
                        state.queues.remove(&key);
                    }
                    spawn_failure = Some((job, error));
                }
            }
        }
        drop(state);

        inner.flush_events();
        if let Some((job, error)) = spawn_failure {
            drop(job);
            slot.fill(Err(error));
        }

        MutationHandle {
            slot,
            operation_id,
            coalesced: false,
        }
    }

    /// `git:cancelOperation`. Una operación en cola queda descartada (su
    /// solicitud termina con `CANCELLED` cuando le llega el turno); una en
    /// curso recibe la orden de abortar y termina cuando su tarea vuelve.
    pub fn cancel(&self, operation_id: &str) -> CancelResult {
        let result = {
            let mut state = lock(&self.inner.state);
            match state.operations.get(operation_id).cloned() {
                Some((key, id)) => self.inner.cancel_entry(&mut state, &key, id),
                None => CancelResult::rejected(CancelReason::NotFound),
            }
        };
        self.inner.flush_events();
        result
    }

    /// `git:getOperationQueue`.
    pub fn get_queue(&self, repo_path: impl AsRef<Path>) -> OperationQueue {
        let key = self.inner.queue_key(repo_path.as_ref());
        let state = lock(&self.inner.state);
        let Some(queue) = state.queues.get(&key) else {
            return OperationQueue::default();
        };

        OperationQueue {
            active: queue
                .entries
                .iter()
                .find(|entry| is_active_phase(entry.phase))
                .map(|entry| to_operation_summary(entry, 0)),
            queued: queue
                .entries
                .iter()
                .filter(|entry| entry.phase == OperationPhase::Queued)
                .enumerate()
                .map(|(index, entry)| to_operation_summary(entry, index + 1))
                .collect(),
        }
    }
}

impl Drop for RepositoryMutationGate {
    /// Cancela lo que esté en cola o en curso y espera a los hilos de trabajo.
    /// Una tarea que no atienda la cancelación retiene el `drop` hasta volver.
    fn drop(&mut self) {
        let inner = &self.inner;
        let threads: Vec<JoinHandle<()>> = {
            let mut state = lock(&inner.state);
            state.shutdown = true;
            let pending: Vec<(String, u64)> = state
                .queues
                .iter()
                .flat_map(|(key, queue)| queue.entries.iter().map(|entry| (key.clone(), entry.id)))
                .collect();
            for (key, id) in pending {
                inner.cancel_entry(&mut state, &key, id);
            }
            let timer = state.timer.take();
            state.workers.drain(..).chain(timer).collect()
        };
        inner.timer_wake.notify_all();
        inner.flush_events();

        let current = std::thread::current().id();
        for thread in threads {
            // Soltar la cola desde una de sus tareas no puede esperarse a sí misma.
            if thread.thread().id() != current {
                let _ = thread.join();
            }
        }
    }
}
