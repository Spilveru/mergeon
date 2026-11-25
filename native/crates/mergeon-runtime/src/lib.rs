//! Cola de mutaciones por repositorio y watcher de estado.
//!
//! - [`RepositoryMutationGate`]: port de
//!   `electron/services/repository-mutation-gate.js`. Serializa las mutaciones
//!   del mismo repositorio, deduplica solicitudes idénticas, permite cancelar,
//!   aplica timeout y emite los eventos de `git:operation-event`.
//! - [`RepositoryStatusWatcher`]: port de
//!   `electron/services/repository-status-watch.js`. Avisa (`git:status-changed`)
//!   cuando cambia el working tree o lo que mueve HEAD.
//!
//! Todo es síncrono: la concurrencia interna usa `std::thread` y los avisos
//! llegan por callbacks.
//!
//! ```no_run
//! use std::sync::Arc;
//! use mergeon_runtime::{MutationOptions, RepositoryMutationGate, RepositoryStatusWatcher};
//!
//! // Un watcher y una cola para toda la app, como en `git-handlers.js`.
//! let watcher = Arc::new(RepositoryStatusWatcher::new());
//! let gate = RepositoryMutationGate::builder()
//!     .watcher(watcher.clone())
//!     .on_event(|event| println!("git:operation-event {}", event.phase.as_str()))
//!     .build();
//!
//! // git:watchStatus / git:setStatusWatchWindowFocus / git:unwatchStatus
//! let mode = watcher.start("/path/to/repo", |change| {
//!     println!("git:status-changed {} {}", change.repo_path, change.reason.as_str());
//! });
//! println!("watching in {} mode", mode.as_str());
//! watcher.set_focused(false);
//!
//! // Una mutación: bloquea hasta tener el resultado de la tarea.
//! let result = gate.run_exclusive(
//!     "/path/to/repo",
//!     Some("stage:README.md"),
//!     MutationOptions::new()
//!         .operation("stage")
//!         .operation_id("stage-1")
//!         .timeout(mergeon_gitcmd::LOCAL_TIMEOUT),
//!     || serde_json::json!({ "success": true, "operation": "stage", "code": "OK" }),
//! );
//! assert!(result.is_ok());
//!
//! // git:cancelOperation / git:getOperationQueue
//! let cancelled = gate.cancel("stage-1");
//! let queue = gate.get_queue("/path/to/repo");
//! assert!(!cancelled.ok && queue.active.is_none());
//! watcher.stop();
//! ```

mod mutation_gate;
mod status_watch;
mod util;

pub use mutation_gate::{
    CancelPhase, CancelReason, CancelResult, DEFAULT_TIMEOUT, MutationError, MutationGateBuilder,
    MutationHandle, MutationOptions, MutationOutcome, MutationWatcher, OperationEvent,
    OperationEventError, OperationPhase, OperationProgress, OperationQueue, OperationSummary,
    OutcomeReport, RepositoryMutationGate,
};
pub use status_watch::{
    DEFAULT_DEBOUNCE, DEFAULT_POLL_INTERVAL, DEFAULT_SUPPRESS_TRAILING, HeadFingerprintFn,
    ManualClock, NotifyBackend, RepositoryStatusWatcher, StatusChange, StatusWatcherOptions,
    WatchBackend, WatchError, WatchHandle, WatchMode, WatchPathKind, WatchReason, WatchSink,
    WatchStrategy, WatchTimers, classify_watch_path, get_head_fingerprint, get_refs_fingerprint,
    should_ignore_watch_path, should_prune_watch_path,
};
