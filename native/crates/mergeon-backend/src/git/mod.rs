//! Canales `git:*` (port de `electron/ipc/git-handlers.js`).
//!
//! - `reads`: lecturas; delegan en `mergeon-git-query` y devuelven `Result`.
//! - `mutations`: los canales registrados con `registerRepoMutationHandler`
//!   cuya operación vive en `mergeon-git-ops`.
//! - `handlers`: los canales con lógica propia (`initRepository`, `clone`,
//!   `addToIgnore`, `setupTracking`).
//! - Aquí: la cola de mutaciones, el watcher de estado y el envoltorio común
//!   de todas las mutaciones.

mod credentials;
mod exec;
mod handlers;
mod mutations;
mod reads;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use mergeon_git_ops::undo::{RefCapture, RefProbe};
use mergeon_gitcmd::{ControlCode, operation_control_code};
use mergeon_runtime::{
    CancelResult, MutationError, MutationHandle, MutationOptions, OperationQueue, OutcomeReport,
    RepositoryMutationGate, RepositoryStatusWatcher, StatusWatcherOptions, WatchMode,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::events::{BackendEvent, EventCallback};
use crate::mutation::{
    FailureShape, Mutation, MutationBody, StructuredResult, Thrown, build_mutation_request_key,
    create_control_failure, create_generic_operation_failure, create_operation_id,
    create_structured_handler_error, get_mutation_timeout,
};

#[cfg(test)]
pub(crate) use mutations::push_config;

pub use handlers::{
    AddToIgnoreCode, AddToIgnoreResult, CloneCode, CloneResult, HandlerOperation,
    InitRepositoryCode, InitRepositoryResult, SetupTrackingCode, SetupTrackingResult,
};

/// Respuesta `{ success: true }` de los canales que no devuelven nada más.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuccessResult {
    pub success: bool,
}

impl SuccessResult {
    pub(crate) fn ok() -> Self {
        Self { success: true }
    }
}

/// Respuesta de `git:watchStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchStatusResult {
    pub success: bool,
    /// `fs-watch` si la vigilancia nativa arrancó; `poll` si hubo que sondear.
    pub mode: WatchMode,
}

/// Configuración de un canal de mutación (el `config` de
/// `registerRepoMutationHandler`).
pub(crate) struct MutationConfig {
    pub operation: &'static str,
    pub message_prefix: String,
    pub request_key: String,
    pub timeout: Option<Duration>,
    pub failure: FailureShape,
    pub extra: Map<String, Value>,
    /// Lo que se lee antes y después de la operación, dentro de su turno en
    /// la cola, para poder deshacerla (`Mutation::capture`).
    pub capture: Option<RefProbe>,
}

impl MutationConfig {
    /// Canal con la clave de dedupe por defecto: la operación y todos los
    /// argumentos del canal (`args`, con la ruta del repositorio primero).
    pub fn new(operation: &'static str, message_prefix: impl Into<String>, args: Value) -> Self {
        Self {
            operation,
            message_prefix: message_prefix.into(),
            request_key: build_mutation_request_key(operation, &args),
            timeout: None,
            failure: FailureShape::Generic,
            extra: Map::new(),
            capture: None,
        }
    }

    /// Lee HEAD (y lo que pida `probe`) antes y después de la operación.
    pub fn capture(mut self, probe: RefProbe) -> Self {
        self.capture = Some(probe);
        self
    }

    /// `config.requestKey`: clave propia del canal.
    pub fn request_key(mut self, args: Value) -> Self {
        self.request_key = build_mutation_request_key(self.operation, &args);
        self
    }

    /// `config.timeoutMs`.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// `failure: createStructuredHandlerError`.
    pub fn structured_failure(mut self) -> Self {
        self.failure = FailureShape::Structured;
        self
    }

    /// `config.extra(...args)`: campos que acompañan a un fallo del handler.
    pub fn extra(mut self, extra: Value) -> Self {
        if let Value::Object(extra) = extra {
            self.extra = extra;
        }
        self
    }

    /// `createControlFailure` con un error de control.
    fn control_failure<T>(&self, code: ControlCode, operation_id: Option<String>) -> Mutation<T> {
        Mutation::failed(
            create_control_failure(self.operation, code, self.extra.clone()),
            operation_id,
        )
    }

    /// `createControlFailure` sin código de control: el fallo del canal.
    fn fallback_failure<T>(&self, error_message: &str) -> Mutation<T> {
        let failure = match self.failure {
            FailureShape::Generic => create_generic_operation_failure(
                self.operation,
                &self.message_prefix,
                error_message,
                self.extra.clone(),
            ),
            FailureShape::Structured => create_structured_handler_error(
                self.operation,
                &self.message_prefix,
                error_message,
                self.extra.clone(),
            ),
        };
        Mutation::failed(failure, None)
    }
}

/// Lo que devuelve la tarea de una mutación dentro de la cola: su resultado
/// ya envuelto, o el error que en el original rechazaba la promesa.
type TaskOutcome<T> = Result<Mutation<T>, Thrown>;

/// Dentro de la tarea: aplica al resultado el control de la cola y le pone el
/// `operationId`.
fn finish_task<T: StructuredResult>(
    operation: &'static str,
    operation_id: String,
    outcome: Result<T, Thrown>,
    capture: Option<RefCapture>,
) -> TaskOutcome<T> {
    // Cancelación o timeout que la cola pidió mientras la tarea corría.
    let control_code = operation_control_code(None);

    match outcome {
        Ok(result) => {
            let body = match control_code {
                Some(code) if result.code() != code.as_str() => MutationBody::Controlled {
                    result,
                    operation: operation.to_string(),
                    code,
                },
                _ => MutationBody::Completed(result),
            };
            Ok(Mutation {
                operation_id: Some(operation_id),
                body,
                capture,
            })
        }
        Err(mut thrown) => {
            thrown.control = control_code.or(thrown.control);
            thrown.operation_id = Some(operation_id);
            Err(thrown)
        }
    }
}

/// Lo que la cola lee del resultado para decidir la fase final y el `error`
/// del evento (`success`, `code` y `message`, o el error lanzado).
fn report_task<T: StructuredResult>(outcome: &TaskOutcome<T>) -> OutcomeReport {
    match outcome {
        Ok(mutation) => OutcomeReport::from_fields(
            mutation.success(),
            Some(mutation.code()),
            Some(mutation.message()),
        ),
        Err(thrown) => match thrown.control {
            // `GitOperationControlError`.
            Some(code) => OutcomeReport {
                failed: true,
                code: Some(code.as_str().to_string()),
                message: Some(code.message().to_string()),
            },
            None => OutcomeReport {
                failed: true,
                code: thrown.code.clone(),
                message: Some(thrown.message.clone()),
            },
        },
    }
}

/// Una mutación ya encolada: la promesa que el handler original esperaba.
pub(crate) struct PendingMutation<T> {
    handle: MutationHandle<TaskOutcome<T>>,
    config: MutationConfig,
}

impl<T: Clone> PendingMutation<T> {
    /// La solicitud se unió a otra idéntica que ya estaba en la cola.
    #[cfg(test)]
    pub fn is_coalesced(&self) -> bool {
        self.handle.is_coalesced()
    }

    /// Espera el resultado. Lo que en el original era el `catch` del handler:
    /// si la cola rechazó la solicitud o la tarea lanzó, el resultado es el
    /// fallo estructurado del canal.
    pub fn wait(self) -> Mutation<T> {
        let config = self.config;
        match self.handle.wait() {
            Ok(Ok(mutation)) => mutation,
            Ok(Err(thrown)) => match thrown.control {
                Some(code) => config.control_failure(code, thrown.operation_id),
                None => config.fallback_failure(&thrown.message),
            },
            Err(MutationError::Control { code, operation_id }) => {
                config.control_failure(code, operation_id)
            }
            Err(MutationError::Panicked { message, .. }) => config.fallback_failure(&message),
        }
    }
}

/// `window.api.git`: un método por canal `git:*`.
pub struct GitApi {
    // La cola va antes que el watcher: al soltarla espera a sus hilos de
    // trabajo, que todavía pueden avisar al watcher.
    gate: RepositoryMutationGate,
    watcher: Arc<RepositoryStatusWatcher>,
    events: EventCallback,
}

impl std::fmt::Debug for GitApi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitApi")
            .field("gate", &self.gate)
            .field("watcher", &self.watcher)
            .finish_non_exhaustive()
    }
}

impl GitApi {
    /// Watcher y cola enlazados, como al cargar `git-handlers.js`: la cola
    /// avisa al watcher del inicio y el fin de cada mutación y publica los
    /// eventos de operación.
    pub(crate) fn new(events: EventCallback, watcher_options: StatusWatcherOptions) -> Self {
        let watcher = Arc::new(RepositoryStatusWatcher::with_options(watcher_options));
        let operation_events = events.clone();
        let progress_events = events.clone();
        let gate = RepositoryMutationGate::builder()
            .watcher(watcher.clone())
            .on_event(move |event| operation_events(BackendEvent::Operation(event)))
            .on_progress(move |progress| progress_events(BackendEvent::OperationProgress(progress)))
            .build();

        Self {
            gate,
            watcher,
            events,
        }
    }

    /// `stopGitRepositoryStatusWatcher`.
    pub(crate) fn stop_status_watcher(&self) {
        self.watcher.stop();
    }

    /// `registerRepoMutationHandler`: serializa `run` por repositorio a través
    /// de la cola (evita carreras de `index.lock`) con supresión de los avisos
    /// del watcher que la propia mutación provoca, y traduce a un resultado
    /// estructurado todo lo que no sea un resultado: nunca falla.
    pub(crate) fn run_repo_mutation<T, F>(
        &self,
        repo_path: &Path,
        config: MutationConfig,
        run: F,
    ) -> Mutation<T>
    where
        T: StructuredResult + Clone + Send + 'static,
        F: FnOnce() -> Result<T, Thrown> + Send + 'static,
    {
        self.enqueue_repo_mutation(repo_path, config, run).wait()
    }

    /// La primera mitad de [`GitApi::run_repo_mutation`]: deja la solicitud en
    /// la cola (o la une a una idéntica que ya esté) sin esperar su resultado.
    pub(crate) fn enqueue_repo_mutation<T, F>(
        &self,
        repo_path: &Path,
        config: MutationConfig,
        run: F,
    ) -> PendingMutation<T>
    where
        T: StructuredResult + Clone + Send + 'static,
        F: FnOnce() -> Result<T, Thrown> + Send + 'static,
    {
        let operation = config.operation;
        let operation_id = create_operation_id(operation);
        let options = MutationOptions::new()
            .operation(operation)
            .operation_id(operation_id.clone())
            .timeout(get_mutation_timeout(operation, config.timeout))
            .watch(true);

        let probe = config.capture.clone();
        let repo = repo_path.to_path_buf();
        let handle = self.gate.enqueue_with_report(
            repo_path,
            Some(&config.request_key),
            options,
            move || {
                // Antes y después, en el mismo turno: nada se cuela entre la
                // lectura y la operación.
                let before = probe.as_ref().map(|probe| {
                    let snapshot = mergeon_git_ops::undo::read_ref_snapshot(&repo, probe);
                    let discard = probe
                        .discard
                        .as_ref()
                        .map(|discard| mergeon_git_ops::undo::back_up_discard(&repo, discard));
                    (snapshot, discard)
                });
                let outcome = run();
                let capture = before
                    .zip(probe.as_ref())
                    .map(|((before, discard), probe)| RefCapture {
                        before,
                        after: mergeon_git_ops::undo::read_ref_snapshot(&repo, probe),
                        discard,
                    });
                finish_task(operation, operation_id, outcome, capture)
            },
            report_task::<T>,
        );

        PendingMutation { handle, config }
    }

    /// Canal `git:watchStatus`: vigila `repo_path` (y deja de vigilar el
    /// anterior). Los avisos llegan como [`BackendEvent::StatusChanged`].
    pub fn watch_status(&self, repo_path: impl AsRef<Path>) -> WatchStatusResult {
        let events = self.events.clone();
        let mode = self.watcher.start(repo_path, move |change| {
            events(BackendEvent::StatusChanged(change));
        });

        WatchStatusResult {
            success: true,
            mode,
        }
    }

    /// Canal `git:unwatchStatus`.
    pub fn unwatch_status(&self) -> SuccessResult {
        self.watcher.stop();
        SuccessResult::ok()
    }

    /// Canal `git:setStatusWatchWindowFocus`: sin foco, el sondeo se pausa.
    pub fn set_status_watch_window_focus(&self, is_focused: bool) -> SuccessResult {
        self.watcher.set_focused(is_focused);
        SuccessResult::ok()
    }

    /// Canal `git:cancelOperation`.
    pub fn cancel_operation(&self, operation_id: &str) -> CancelResult {
        self.gate.cancel(operation_id)
    }

    /// Canal `git:getOperationQueue`.
    pub fn get_operation_queue(&self, repo_path: impl AsRef<Path>) -> OperationQueue {
        self.gate.get_queue(repo_path)
    }
}
