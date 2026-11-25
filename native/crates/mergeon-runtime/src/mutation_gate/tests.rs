//! Port de `electron/services/repository-mutation-gate.test.js`, más los casos
//! propios de la versión con hilos.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;

use mergeon_gitcmd::{current_operation_context, operation_control_code};
use serde_json::json;

use super::*;

/// Margen para lo que debe ocurrir enseguida; solo se agota si algo falla.
const WAIT: Duration = Duration::from_secs(10);

/// Equivalente a `createDeferred()`: una tarea se queda esperando hasta que
/// la prueba la libera.
#[derive(Clone, Default)]
struct Deferred {
    state: Arc<(Mutex<bool>, Condvar)>,
}

impl Deferred {
    fn resolve(&self) {
        *lock(&self.state.0) = true;
        self.state.1.notify_all();
    }

    fn wait(&self) {
        let mut resolved = lock(&self.state.0);
        while !*resolved {
            resolved = self
                .state
                .1
                .wait(resolved)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }
}

#[derive(Clone, Default)]
struct Order(Arc<Mutex<Vec<&'static str>>>);

impl Order {
    fn push(&self, step: &'static str) {
        lock(&self.0).push(step);
    }

    fn snapshot(&self) -> Vec<&'static str> {
        lock(&self.0).clone()
    }
}

#[derive(Clone, Default)]
struct Events(Arc<Mutex<Vec<OperationEvent>>>);

impl Events {
    fn gate(&self) -> RepositoryMutationGate {
        let events = self.0.clone();
        RepositoryMutationGate::builder()
            .on_event(move |event| lock(&events).push(event))
            .build()
    }

    fn snapshot(&self) -> Vec<OperationEvent> {
        lock(&self.0).clone()
    }

    fn phases(&self, operation_id: &str) -> Vec<OperationPhase> {
        self.snapshot()
            .iter()
            .filter(|event| event.operation_id == operation_id)
            .map(|event| event.phase)
            .collect()
    }
}

#[derive(Default)]
struct RecordingWatcher {
    begun: Mutex<Vec<PathBuf>>,
    ended: Mutex<Vec<PathBuf>>,
}

impl MutationWatcher for RecordingWatcher {
    fn begin_mutation(&self, repo_path: &Path) {
        lock(&self.begun).push(repo_path.to_path_buf());
    }

    fn end_mutation(&self, repo_path: &Path) {
        lock(&self.ended).push(repo_path.to_path_buf());
    }
}

fn options(operation: &str, operation_id: &str) -> MutationOptions {
    MutationOptions::new()
        .operation(operation)
        .operation_id(operation_id)
}

/// Una tarea que solo vuelve cuando la cola aborta su operación, como un
/// proceso git al que matan. Devuelve el código de control que vio.
fn wait_for_abort() -> Option<ControlCode> {
    let context = current_operation_context().expect("operation context");
    let deadline = Instant::now() + WAIT;
    while !context.is_cancelled() {
        assert!(Instant::now() < deadline, "the operation was never aborted");
        std::thread::sleep(Duration::from_millis(1));
    }
    operation_control_code(None)
}

#[test]
fn serializes_tasks_for_the_same_repository() {
    let gate = RepositoryMutationGate::new();
    let order = Order::default();
    let first = Deferred::default();
    let (started, first_started) = mpsc::channel();

    let first_run = gate.enqueue("/repo/a", None, MutationOptions::new(), {
        let (order, first) = (order.clone(), first.clone());
        move || {
            order.push("first-start");
            started.send(()).unwrap();
            first.wait();
            order.push("first-end");
            "first"
        }
    });
    let second_run = gate.enqueue("/repo/a", None, MutationOptions::new(), {
        let order = order.clone();
        move || {
            order.push("second-start");
            "second"
        }
    });

    // La segunda tarea no arranca mientras la primera está en vuelo.
    first_started.recv_timeout(WAIT).unwrap();
    assert_eq!(order.snapshot(), ["first-start"]);
    assert!(!second_run.is_finished());

    first.resolve();
    assert_eq!(first_run.wait(), Ok("first"));
    assert_eq!(second_run.wait(), Ok("second"));
    assert_eq!(
        order.snapshot(),
        ["first-start", "first-end", "second-start"]
    );
}

#[test]
fn runs_tasks_for_different_repositories_in_parallel() {
    let gate = RepositoryMutationGate::new();
    let order = Order::default();
    let blocker = Deferred::default();

    let slow = gate.enqueue("/repo/a", None, MutationOptions::new(), {
        let (order, blocker) = (order.clone(), blocker.clone());
        move || {
            order.push("a-start");
            blocker.wait();
            order.push("a-end");
        }
    });
    let fast = gate.run_exclusive("/repo/b", None, MutationOptions::new(), {
        let order = order.clone();
        move || order.push("b-start")
    });

    assert_eq!(fast, Ok(()));
    assert!(order.snapshot().contains(&"b-start"));
    assert!(!order.snapshot().contains(&"a-end"));

    blocker.resolve();
    assert_eq!(slow.wait(), Ok(()));
}

#[test]
fn coalesces_identical_requests_but_keeps_distinct_request_keys_queued() {
    let gate = RepositoryMutationGate::new();
    let blocker = Deferred::default();
    let order = Order::default();
    let (started, fetch_started) = mpsc::channel();

    let first = gate.enqueue("/repo/a", Some("fetch:origin"), MutationOptions::new(), {
        let (order, blocker) = (order.clone(), blocker.clone());
        move || {
            order.push("fetch-start");
            started.send(()).unwrap();
            blocker.wait();
            order.push("fetch-end");
            "fetch"
        }
    });
    let duplicate = gate.enqueue("/repo/a", Some("fetch:origin"), MutationOptions::new(), {
        let order = order.clone();
        move || {
            order.push("duplicate-start");
            "duplicate"
        }
    });
    let different = gate.enqueue("/repo/a", Some("pull:origin"), MutationOptions::new(), {
        let order = order.clone();
        move || {
            order.push("pull-start");
            "pull"
        }
    });

    assert!(duplicate.shares_operation_with(&first));
    assert!(duplicate.is_coalesced());
    assert!(!first.is_coalesced());
    assert!(!different.shares_operation_with(&first));
    fetch_started.recv_timeout(WAIT).unwrap();
    assert_eq!(order.snapshot(), ["fetch-start"]);

    blocker.resolve();
    assert_eq!(first.wait(), Ok("fetch"));
    assert_eq!(duplicate.wait(), Ok("fetch"));
    assert_eq!(different.wait(), Ok("pull"));
    assert_eq!(order.snapshot(), ["fetch-start", "fetch-end", "pull-start"]);
}

#[test]
fn does_not_poison_the_queue_when_a_task_throws() {
    let gate = RepositoryMutationGate::new();

    let failing = gate.enqueue(
        "/repo/a",
        None,
        MutationOptions::new(),
        || -> &'static str { panic!("boom") },
    );
    let following = gate.enqueue("/repo/a", None, MutationOptions::new(), || "ok");

    let error = failing.wait().unwrap_err();
    assert_eq!(error.to_string(), "boom");
    assert_eq!(error.control_code(), None);
    assert_eq!(error.code(), "GIT_ERROR");
    assert_eq!(following.wait(), Ok("ok"));
}

#[test]
fn calls_watcher_begin_and_end_mutation_around_each_task_even_on_throw() {
    let watcher = Arc::new(RecordingWatcher::default());
    let gate = RepositoryMutationGate::builder()
        .watcher(watcher.clone())
        .build();

    assert_eq!(
        gate.run_exclusive("/repo/a", None, MutationOptions::new(), || "ok"),
        Ok("ok")
    );
    let error = gate
        .run_exclusive("/repo/a", None, MutationOptions::new(), || -> () {
            panic!("boom")
        })
        .unwrap_err();
    assert_eq!(error.to_string(), "boom");

    let repo = PathBuf::from("/repo/a");
    assert_eq!(*lock(&watcher.begun), [repo.clone(), repo.clone()]);
    assert_eq!(*lock(&watcher.ended), [repo.clone(), repo]);
}

#[test]
fn skips_the_watcher_for_tasks_that_opt_out() {
    let watcher = Arc::new(RecordingWatcher::default());
    let gate = RepositoryMutationGate::builder()
        .watcher(watcher.clone())
        .build();

    let cloned = gate.run_exclusive(
        "/repo/a",
        None,
        MutationOptions::new().watch(false),
        || "cloned",
    );

    assert_eq!(cloned, Ok("cloned"));
    assert!(lock(&watcher.begun).is_empty());
    assert!(lock(&watcher.ended).is_empty());
}

#[test]
fn begins_the_mutation_only_when_the_queued_task_actually_starts() {
    let watcher = Arc::new(RecordingWatcher::default());
    let gate = RepositoryMutationGate::builder()
        .watcher(watcher.clone())
        .build();
    let first = Deferred::default();
    let (started, first_started) = mpsc::channel();

    let first_run = gate.enqueue("/repo/a", None, MutationOptions::new(), {
        let first = first.clone();
        move || {
            started.send(()).unwrap();
            first.wait();
        }
    });
    let second_run = gate.enqueue("/repo/a", None, MutationOptions::new(), || "second");

    first_started.recv_timeout(WAIT).unwrap();
    assert_eq!(lock(&watcher.begun).len(), 1);

    first.resolve();
    assert_eq!(first_run.wait(), Ok(()));
    assert_eq!(second_run.wait(), Ok("second"));
    assert_eq!(lock(&watcher.begun).len(), 2);
    assert_eq!(lock(&watcher.ended).len(), 2);
}

#[test]
fn normalizes_equivalent_paths_to_the_same_queue() {
    let gate = RepositoryMutationGate::new();
    let order = Order::default();
    let first = Deferred::default();
    let (started, first_started) = mpsc::channel();

    let first_run = gate.enqueue("/repo/a", None, MutationOptions::new(), {
        let (order, first) = (order.clone(), first.clone());
        move || {
            order.push("first-start");
            started.send(()).unwrap();
            first.wait();
        }
    });
    let second_run = gate.enqueue("/repo/a/", None, MutationOptions::new(), {
        let order = order.clone();
        move || order.push("second-start")
    });

    first_started.recv_timeout(WAIT).unwrap();
    assert_eq!(order.snapshot(), ["first-start"]);

    first.resolve();
    assert_eq!(first_run.wait(), Ok(()));
    assert_eq!(second_run.wait(), Ok(()));
    assert_eq!(order.snapshot(), ["first-start", "second-start"]);
}

#[test]
fn uses_the_injected_key_normalizer() {
    let gate = RepositoryMutationGate::builder()
        .normalize_key(|_repo_path| "single-queue".to_string())
        .build();
    let blocker = Deferred::default();
    let (started, first_started) = mpsc::channel();

    let first = gate.enqueue("/repo/a", None, options("fetch", "fetch-1"), {
        let blocker = blocker.clone();
        move || {
            started.send(()).unwrap();
            blocker.wait();
        }
    });
    let second = gate.enqueue("/repo/b", None, options("pull", "pull-1"), || ());

    first_started.recv_timeout(WAIT).unwrap();
    // Con una sola clave, el otro repositorio espera en la misma cola.
    let queue = gate.get_queue("/anything");
    assert_eq!(
        queue.active.and_then(|active| active.operation_id),
        Some("fetch-1".to_string())
    );
    assert_eq!(queue.queued.len(), 1);

    blocker.resolve();
    assert_eq!(first.wait(), Ok(()));
    assert_eq!(second.wait(), Ok(()));
}

#[test]
fn emits_queue_state_with_operation_ids_and_removes_completed_entries() {
    let events = Events::default();
    let gate = events.gate();

    let result = gate.run_exclusive(
        "/repo/a",
        Some("stage:file.txt"),
        options("stage", "stage-1"),
        || "ok",
    );
    assert_eq!(result, Ok("ok"));

    let emitted = events.snapshot();
    assert_eq!(
        emitted.iter().map(|event| event.phase).collect::<Vec<_>>(),
        [
            OperationPhase::Queued,
            OperationPhase::Running,
            OperationPhase::Completed
        ]
    );
    assert_eq!(emitted[0].operation_id, "stage-1");
    assert_eq!(emitted[0].operation, "stage");
    assert_eq!(emitted[0].repo_path, "/repo/a");
    assert_eq!(emitted[0].position, 1);
    assert_eq!(emitted[0].queue_length, 1);
    assert_eq!(emitted[0].active_operation_id, None);
    assert_eq!(emitted[0].operations.len(), 1);
    assert_eq!(emitted[0].operations[0].phase, OperationPhase::Queued);
    assert_eq!(emitted[0].operations[0].position, 1);
    assert!(!emitted[0].operations[0].cancellable);

    assert_eq!(emitted[1].position, 0);
    assert_eq!(emitted[1].queue_length, 0);
    assert_eq!(emitted[1].active_operation_id.as_deref(), Some("stage-1"));
    assert!(emitted[1].operations[0].cancellable);
    assert!(emitted[1].operations[0].started_at.is_some());

    assert_eq!(emitted[2].active_operation_id, None);
    assert!(emitted[2].operations.is_empty());
    assert_eq!(emitted[2].error, None);

    assert_eq!(gate.get_queue("/repo/a"), OperationQueue::default());
}

#[test]
fn operations_without_an_id_emit_no_events() {
    let events = Events::default();
    let gate = events.gate();

    assert_eq!(
        gate.run_exclusive("/repo/a", Some("stage"), MutationOptions::new(), || "ok"),
        Ok("ok")
    );

    assert!(events.snapshot().is_empty());
}

#[test]
fn cancels_a_queued_request_without_blocking_the_next_operation() {
    let first = Deferred::default();
    let gate = RepositoryMutationGate::new();
    let (started, first_started) = mpsc::channel();
    let queued_ran = Arc::new(AtomicBool::new(false));

    let first_run = gate.enqueue("/repo/a", Some("first"), options("fetch", "fetch-1"), {
        let first = first.clone();
        move || {
            started.send(()).unwrap();
            first.wait();
            "first"
        }
    });
    let queued_run = gate.enqueue("/repo/a", Some("second"), options("pull", "pull-1"), {
        let queued_ran = queued_ran.clone();
        move || {
            queued_ran.store(true, Ordering::SeqCst);
            "should-not-run"
        }
    });
    let following_run = gate.enqueue(
        "/repo/a",
        Some("third"),
        options("push", "push-1"),
        || "next",
    );

    first_started.recv_timeout(WAIT).unwrap();
    assert_eq!(
        gate.cancel("pull-1"),
        CancelResult {
            ok: true,
            phase: Some(CancelPhase::Queued),
            reason: None
        }
    );
    // Cancelada en cola ya no cuenta, pero sigue sin poder cancelarse otra vez.
    assert_eq!(gate.get_queue("/repo/a").queued.len(), 1);
    assert_eq!(
        gate.cancel("pull-1").reason,
        Some(CancelReason::NotCancellable)
    );

    first.resolve();
    assert_eq!(first_run.wait(), Ok("first"));
    assert_eq!(
        queued_run.wait(),
        Err(MutationError::Control {
            code: ControlCode::Cancelled,
            operation_id: Some("pull-1".to_string())
        })
    );
    assert_eq!(following_run.wait(), Ok("next"));
    assert!(!queued_ran.load(Ordering::SeqCst));
}

#[test]
fn a_request_cancelled_in_the_queue_no_longer_coalesces() {
    let events = Events::default();
    let gate = events.gate();
    let blocker = Deferred::default();
    let (started, first_started) = mpsc::channel();

    let first = gate.enqueue("/repo/a", Some("fetch"), options("fetch", "fetch-1"), {
        let blocker = blocker.clone();
        move || {
            started.send(()).unwrap();
            blocker.wait();
        }
    });
    let queued = gate.enqueue("/repo/a", Some("pull"), options("pull", "pull-1"), || "old");
    first_started.recv_timeout(WAIT).unwrap();
    assert!(gate.cancel("pull-1").ok);

    let retried = gate.enqueue("/repo/a", Some("pull"), options("pull", "pull-2"), || "new");
    assert!(!retried.is_coalesced());
    assert_eq!(retried.operation_id(), Some("pull-2"));

    blocker.resolve();
    assert_eq!(first.wait(), Ok(()));
    assert_eq!(
        queued.wait().unwrap_err().control_code(),
        Some(ControlCode::Cancelled)
    );
    assert_eq!(retried.wait(), Ok("new"));

    // La cancelación en cola se avisa una sola vez, en el momento de cancelar.
    assert_eq!(
        events.phases("pull-1"),
        [OperationPhase::Queued, OperationPhase::Cancelled]
    );
    let cancelled = events
        .snapshot()
        .into_iter()
        .find(|event| event.operation_id == "pull-1" && event.phase == OperationPhase::Cancelled)
        .unwrap();
    assert_eq!(cancelled.error, None);
    assert_eq!(cancelled.position, 0);
    assert_eq!(cancelled.active_operation_id.as_deref(), Some("fetch-1"));
}

#[test]
fn aborts_the_running_task_when_cancelled() {
    let events = Events::default();
    let gate = events.gate();
    let (started, task_started) = mpsc::channel();

    let run = gate.enqueue(
        "/repo/a",
        Some("fetch:origin"),
        options("fetch", "fetch-1"),
        move || {
            started.send(()).unwrap();
            wait_for_abort().map(ControlCode::as_str)
        },
    );

    task_started.recv_timeout(WAIT).unwrap();
    assert_eq!(
        gate.cancel("fetch-1"),
        CancelResult {
            ok: true,
            phase: Some(CancelPhase::Running),
            reason: None
        }
    );

    // La tarea vuelve por sí misma: su valor se entrega tal cual.
    assert_eq!(run.wait(), Ok(Some("CANCELLED")));
    assert_eq!(
        events.phases("fetch-1"),
        [
            OperationPhase::Queued,
            OperationPhase::Running,
            OperationPhase::Cancelling,
            OperationPhase::Cancelled
        ]
    );
    let emitted = events.snapshot();
    let cancelling = &emitted[2];
    assert_eq!(cancelling.active_operation_id.as_deref(), Some("fetch-1"));
    assert_eq!(cancelling.operations[0].phase, OperationPhase::Cancelling);
    assert!(!cancelling.operations[0].cancellable);
    assert_eq!(gate.cancel("fetch-1").reason, Some(CancelReason::NotFound));
}

#[test]
fn aborts_a_task_and_marks_it_as_timed_out() {
    let events = Events::default();
    let gate = events.gate();

    let run = gate.run_exclusive(
        "/repo/a",
        Some("fetch:origin"),
        options("fetch", "fetch-1").timeout(Duration::from_millis(5)),
        || wait_for_abort().map(ControlCode::as_str),
    );

    assert_eq!(run, Ok(Some("TIMEOUT")));
    assert_eq!(gate.get_queue("/repo/a"), OperationQueue::default());
    assert_eq!(
        events.phases("fetch-1"),
        [
            OperationPhase::Queued,
            OperationPhase::Running,
            OperationPhase::Cancelling,
            OperationPhase::Timeout
        ]
    );
}

#[test]
fn a_task_that_fails_after_being_aborted_reports_the_control_error() {
    let gate = RepositoryMutationGate::new();

    let error = gate
        .run_exclusive(
            "/repo/a",
            None,
            options("fetch", "fetch-1").timeout(Duration::from_millis(5)),
            || -> () {
                wait_for_abort();
                panic!("git was killed")
            },
        )
        .unwrap_err();

    assert_eq!(
        error,
        MutationError::Control {
            code: ControlCode::Timeout,
            operation_id: Some("fetch-1".to_string())
        }
    );
    assert_eq!(error.to_string(), "Git operation timed out.");
    assert_eq!(error.code(), "TIMEOUT");
}

#[test]
fn uses_the_default_timeout_of_the_gate() {
    let gate = RepositoryMutationGate::builder()
        .default_timeout(Duration::from_millis(5))
        .build();

    let run = gate.run_exclusive("/repo/a", None, MutationOptions::new(), || {
        wait_for_abort().map(ControlCode::as_str)
    });

    assert_eq!(run, Ok(Some("TIMEOUT")));
}

#[test]
fn the_operation_context_carries_the_id_and_the_timeout() {
    let gate = RepositoryMutationGate::new();

    let seen = gate.run_exclusive(
        "/repo/a",
        None,
        options("fetch", "fetch-1").timeout(Duration::from_secs(7)),
        || {
            let context = current_operation_context().expect("operation context");
            (
                context.operation_id().map(str::to_string),
                context.timeout(),
            )
        },
    );
    assert_eq!(
        seen,
        Ok((Some("fetch-1".to_string()), Duration::from_secs(7)))
    );

    // Sin timeout propio, git conserva su límite de inactividad local.
    let unlimited = gate.run_exclusive(
        "/repo/a",
        None,
        MutationOptions::new().timeout(Duration::ZERO),
        || current_operation_context().map(|context| context.timeout()),
    );
    assert_eq!(unlimited, Ok(Some(LOCAL_TIMEOUT)));
    assert!(current_operation_context().is_none());
}

impl MutationOutcome for (Option<String>, Duration) {}
impl MutationOutcome for Duration {}

#[test]
fn reports_the_queue_while_operations_wait() {
    let gate = RepositoryMutationGate::new();
    let blocker = Deferred::default();
    let (started, first_started) = mpsc::channel();

    let first = gate.enqueue("/repo/a", None, options("fetch", "fetch-1"), {
        let blocker = blocker.clone();
        move || {
            started.send(()).unwrap();
            blocker.wait();
        }
    });
    let second = gate.enqueue("/repo/a", None, options("pull", "pull-1"), || ());
    let third = gate.enqueue("/repo/a", None, MutationOptions::new(), || ());
    first_started.recv_timeout(WAIT).unwrap();

    let queue = gate.get_queue("/repo/a/");
    let active = queue.active.expect("active operation");
    assert_eq!(active.operation_id.as_deref(), Some("fetch-1"));
    assert_eq!(active.operation, "fetch");
    assert_eq!(active.phase, OperationPhase::Running);
    assert_eq!(active.position, 0);
    assert!(active.cancellable);
    assert!(active.started_at.is_some());

    assert_eq!(queue.queued.len(), 2);
    assert_eq!(queue.queued[0].operation_id.as_deref(), Some("pull-1"));
    assert_eq!(queue.queued[0].position, 1);
    assert_eq!(queue.queued[0].phase, OperationPhase::Queued);
    assert!(!queue.queued[0].cancellable);
    assert_eq!(queue.queued[0].started_at, None);
    assert_eq!(queue.queued[1].operation_id, None);
    assert_eq!(queue.queued[1].operation, "git");
    assert_eq!(queue.queued[1].position, 2);

    assert_eq!(gate.get_queue("/repo/b"), OperationQueue::default());

    blocker.resolve();
    for run in [first, second, third] {
        assert_eq!(run.wait(), Ok(()));
    }
    assert_eq!(gate.get_queue("/repo/a"), OperationQueue::default());
}

#[test]
fn cancelling_an_unknown_operation_is_reported_as_not_found() {
    let gate = RepositoryMutationGate::new();

    assert_eq!(
        gate.cancel("missing"),
        CancelResult {
            ok: false,
            phase: None,
            reason: Some(CancelReason::NotFound)
        }
    );
    assert_eq!(gate.cancel("").reason, Some(CancelReason::NotFound));
}

#[test]
fn a_failed_result_ends_as_failed_with_its_error() {
    let events = Events::default();
    let gate = events.gate();

    let result = gate.run_exclusive(
        "/repo/a",
        None,
        options("push", "push-1"),
        || json!({ "success": false, "code": "PUSH_REJECTED", "message": "Push was rejected." }),
    );
    assert_eq!(result.unwrap()["code"], "PUSH_REJECTED");

    let failed = events.snapshot().pop().unwrap();
    assert_eq!(failed.phase, OperationPhase::Failed);
    assert_eq!(
        failed.error,
        Some(OperationEventError {
            code: "PUSH_REJECTED".to_string(),
            message: "Push was rejected.".to_string()
        })
    );

    // Sin `code`, el evento lleva el genérico.
    gate.run_exclusive(
        "/repo/a",
        None,
        options("push", "push-2"),
        || json!({ "success": false, "message": "Nope." }),
    )
    .unwrap();
    assert_eq!(
        events.snapshot().pop().unwrap().error.unwrap().code,
        "GIT_ERROR"
    );
}

#[test]
fn a_result_with_a_control_code_sets_the_final_phase() {
    let events = Events::default();
    let gate = events.gate();

    gate.run_exclusive("/repo/a", None, options("fetch", "fetch-1"), || {
        json!({ "success": false, "code": "TIMEOUT", "message": "Git operation timed out and was stopped." })
    })
    .unwrap();
    gate.run_exclusive("/repo/a", None, options("fetch", "fetch-2"), || {
        json!({ "success": false, "code": "CANCELLED", "message": "Git operation was cancelled." })
    })
    .unwrap();

    assert_eq!(
        events.phases("fetch-1").last(),
        Some(&OperationPhase::Timeout)
    );
    assert_eq!(
        events.phases("fetch-2").last(),
        Some(&OperationPhase::Cancelled)
    );
}

#[test]
fn an_error_result_and_a_panic_end_as_failed() {
    let events = Events::default();
    let gate = events.gate();

    let result: Result<Result<(), String>, _> =
        gate.run_exclusive("/repo/a", None, options("stage", "stage-1"), || {
            Err("index is locked".to_string())
        });
    assert_eq!(result, Ok(Err("index is locked".to_string())));
    let failed = events.snapshot().pop().unwrap();
    assert_eq!(failed.phase, OperationPhase::Failed);
    assert_eq!(
        failed.error,
        Some(OperationEventError {
            code: "GIT_ERROR".to_string(),
            message: "index is locked".to_string()
        })
    );

    let error = gate
        .run_exclusive("/repo/a", None, options("stage", "stage-2"), || -> () {
            panic!("boom")
        })
        .unwrap_err();
    assert_eq!(
        error,
        MutationError::Panicked {
            message: "boom".to_string(),
            operation_id: Some("stage-2".to_string())
        }
    );
    let failed = events.snapshot().pop().unwrap();
    assert_eq!(failed.phase, OperationPhase::Failed);
    assert_eq!(failed.error.unwrap().message, "boom");
}

#[derive(Debug, Clone, PartialEq)]
struct ForeignResult {
    success: bool,
    code: &'static str,
    message: &'static str,
}

#[test]
fn reads_foreign_results_through_the_report_closure() {
    let events = Events::default();
    let gate = events.gate();
    let rejected = ForeignResult {
        success: false,
        code: "MERGE_CONFLICT",
        message: "Merge has conflicts.",
    };

    let result = gate.run_exclusive_with_report(
        "/repo/a",
        Some("merge:main"),
        options("merge", "merge-1"),
        {
            let rejected = rejected.clone();
            move || rejected
        },
        |result: &ForeignResult| {
            OutcomeReport::from_fields(result.success, Some(result.code), Some(result.message))
        },
    );

    assert_eq!(result, Ok(rejected));
    let failed = events.snapshot().pop().unwrap();
    assert_eq!(failed.phase, OperationPhase::Failed);
    assert_eq!(failed.error.unwrap().code, "MERGE_CONFLICT");
}

#[test]
fn requests_with_the_same_key_but_another_result_type_do_not_coalesce() {
    let gate = RepositoryMutationGate::new();
    let blocker = Deferred::default();
    let (started, first_started) = mpsc::channel();

    let text = gate.enqueue("/repo/a", Some("same"), MutationOptions::new(), {
        let blocker = blocker.clone();
        move || {
            started.send(()).unwrap();
            blocker.wait();
            "text"
        }
    });
    let flag = gate.enqueue("/repo/a", Some("same"), MutationOptions::new(), || true);
    first_started.recv_timeout(WAIT).unwrap();
    assert!(!flag.is_coalesced());

    blocker.resolve();
    assert_eq!(text.wait(), Ok("text"));
    assert_eq!(flag.wait(), Ok(true));
}

#[test]
fn a_handle_can_be_polled_and_waited_from_several_threads() {
    let gate = RepositoryMutationGate::new();
    let blocker = Deferred::default();

    let run = gate.enqueue("/repo/a", None, MutationOptions::new(), {
        let blocker = blocker.clone();
        move || {
            blocker.wait();
            "done"
        }
    });
    assert_eq!(run.try_result(), None);
    assert_eq!(run.wait_timeout(Duration::from_millis(5)), None);

    let waiters: Vec<_> = (0..4)
        .map(|_| {
            let run = run.clone();
            std::thread::spawn(move || run.wait())
        })
        .collect();
    blocker.resolve();

    for waiter in waiters {
        assert_eq!(waiter.join().unwrap(), Ok("done"));
    }
    assert_eq!(run.wait_timeout(WAIT), Some(Ok("done")));
    assert_eq!(run.try_result(), Some(Ok("done")));
}

#[test]
fn event_delivery_never_affects_the_operation() {
    let delivered = Arc::new(AtomicUsize::new(0));
    let gate = RepositoryMutationGate::builder()
        .on_event({
            let delivered = delivered.clone();
            move |_event| {
                delivered.fetch_add(1, Ordering::SeqCst);
                panic!("renderer is gone");
            }
        })
        .build();

    assert_eq!(
        gate.run_exclusive("/repo/a", None, options("stage", "stage-1"), || "ok"),
        Ok("ok")
    );
    assert_eq!(delivered.load(Ordering::SeqCst), 3);
}

#[test]
fn the_event_receiver_can_call_back_into_the_gate() {
    let slot: Arc<Mutex<Option<Arc<RepositoryMutationGate>>>> = Arc::default();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let gate = Arc::new(
        RepositoryMutationGate::builder()
            .on_event({
                let (slot, seen) = (slot.clone(), seen.clone());
                move |event| {
                    let gate = lock(&slot).clone();
                    if let Some(gate) = gate {
                        // Consultar y cancelar desde el receptor no se bloquea; la
                        // foto coherente del momento es la que trae el evento.
                        let _live_queue = gate.get_queue(&event.repo_path);
                        let cancelled = event.phase == OperationPhase::Running
                            && gate.cancel(&event.operation_id).ok;
                        lock(&seen).push((
                            event.phase,
                            event.active_operation_id.is_some(),
                            cancelled,
                        ));
                    }
                }
            })
            .build(),
    );
    *lock(&slot) = Some(gate.clone());

    let run = gate.run_exclusive("/repo/a", None, options("fetch", "fetch-1"), || {
        wait_for_abort().map(ControlCode::as_str)
    });
    assert_eq!(run, Ok(Some("CANCELLED")));
    assert_eq!(
        *lock(&seen),
        [
            (OperationPhase::Queued, false, false),
            (OperationPhase::Running, true, true),
            (OperationPhase::Cancelling, true, false),
            (OperationPhase::Cancelled, false, false),
        ]
    );

    // Rompe el ciclo gate → receptor → gate para que la cola se suelte.
    *lock(&slot) = None;
}

#[test]
fn dropping_the_gate_cancels_pending_work_and_joins_its_threads() {
    let events = Events::default();
    let gate = events.gate();
    let (started, task_started) = mpsc::channel();
    let finished = Arc::new(AtomicBool::new(false));

    let running = gate.enqueue("/repo/a", None, options("fetch", "fetch-1"), {
        let finished = finished.clone();
        move || {
            started.send(()).unwrap();
            let code = wait_for_abort().map(ControlCode::as_str);
            finished.store(true, Ordering::SeqCst);
            code
        }
    });
    let queued = gate.enqueue("/repo/a", None, options("pull", "pull-1"), || "never");
    task_started.recv_timeout(WAIT).unwrap();

    drop(gate);

    // Al volver de `drop` los hilos de trabajo ya terminaron.
    assert!(finished.load(Ordering::SeqCst));
    assert_eq!(running.try_result(), Some(Ok(Some("CANCELLED"))));
    assert_eq!(
        queued.try_result(),
        Some(Err(MutationError::Control {
            code: ControlCode::Cancelled,
            operation_id: Some("pull-1".to_string())
        }))
    );
    assert_eq!(
        events.phases("fetch-1").last(),
        Some(&OperationPhase::Cancelled)
    );
    assert_eq!(
        events.phases("pull-1"),
        [OperationPhase::Queued, OperationPhase::Cancelled]
    );
}

#[test]
fn many_callers_on_many_repositories_all_get_their_own_result() {
    let gate = Arc::new(RepositoryMutationGate::new());
    let running = Arc::new([const { AtomicUsize::new(0) }; 4]);

    let callers: Vec<_> = (0..32usize)
        .map(|index| {
            let (gate, running) = (gate.clone(), running.clone());
            std::thread::spawn(move || {
                let repo = index % 4;
                let result = gate.run_exclusive(
                    format!("/repo/{repo}"),
                    None,
                    MutationOptions::new(),
                    move || {
                        // Nunca dos tareas del mismo repositorio a la vez.
                        let concurrent = running[repo].fetch_add(1, Ordering::SeqCst);
                        std::thread::yield_now();
                        running[repo].fetch_sub(1, Ordering::SeqCst);
                        concurrent == 0
                    },
                );
                assert_eq!(result, Ok(true));
            })
        })
        .collect();

    for caller in callers {
        caller.join().unwrap();
    }
    for repo in 0..4 {
        assert_eq!(
            gate.get_queue(format!("/repo/{repo}")),
            OperationQueue::default()
        );
    }
}

#[test]
fn serializes_payloads_with_the_original_field_names() {
    let events = Events::default();
    let gate = events.gate();
    gate.run_exclusive(
        "/repo/a",
        None,
        options("stage", "stage-1"),
        || json!({ "success": false, "code": "LOCKED", "message": "Index is locked." }),
    )
    .unwrap();

    let emitted = events.snapshot();
    let queued = serde_json::to_value(&emitted[0]).unwrap();
    let queued_at = queued["operations"][0]["queuedAt"].clone();
    assert!(queued_at.as_str().is_some_and(|at| at.ends_with('Z')));
    assert_eq!(
        queued,
        json!({
            "operationId": "stage-1",
            "repoPath": "/repo/a",
            "operation": "stage",
            "phase": "queued",
            "position": 1,
            "queueLength": 1,
            "activeOperationId": null,
            "operations": [{
                "operationId": "stage-1",
                "operation": "stage",
                "phase": "queued",
                "position": 1,
                "cancellable": false,
                "startedAt": null,
                "queuedAt": queued_at
            }],
            "error": null
        })
    );
    assert_eq!(
        serde_json::to_value(&emitted[2]).unwrap()["error"],
        json!({ "code": "LOCKED", "message": "Index is locked." })
    );

    assert_eq!(
        serde_json::to_value(CancelResult::cancelled(CancelPhase::Queued)).unwrap(),
        json!({ "ok": true, "phase": "queued" })
    );
    assert_eq!(
        serde_json::to_value(CancelResult::rejected(CancelReason::NotCancellable)).unwrap(),
        json!({ "ok": false, "reason": "not-cancellable" })
    );
    assert_eq!(
        serde_json::to_value(OperationQueue::default()).unwrap(),
        json!({ "active": null, "queued": [] })
    );
    for phase in [
        OperationPhase::Queued,
        OperationPhase::Running,
        OperationPhase::Cancelling,
        OperationPhase::Cancelled,
        OperationPhase::Timeout,
        OperationPhase::Failed,
        OperationPhase::Completed,
    ] {
        assert_eq!(serde_json::to_value(phase).unwrap(), phase.as_str());
    }
}

#[test]
fn the_gate_is_shareable_between_threads() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<RepositoryMutationGate>();
    assert_send_sync::<MutationHandle<String>>();
}

fn receiving(percent: u8) -> GitProgress {
    GitProgress {
        phase: "Receiving objects".into(),
        percent: Some(percent),
        processed: u64::from(percent),
        total: Some(100),
        ..Default::default()
    }
}

#[test]
fn progress_reaches_the_listener_throttled_and_before_the_final_event() {
    #[derive(Debug, PartialEq)]
    enum Seen {
        Progress(String, String, Option<u8>),
        Phase(OperationPhase),
    }
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (events, progress) = (seen.clone(), seen.clone());
    let gate = RepositoryMutationGate::builder()
        .on_event(move |event| lock(&events).push(Seen::Phase(event.phase)))
        .on_progress(move |update| {
            lock(&progress).push(Seen::Progress(
                update.operation_id,
                update.operation,
                update.progress.percent,
            ))
        })
        .build();

    gate.run_exclusive(
        "/repo/progress",
        None,
        MutationOptions::new()
            .operation("fetch")
            .operation_id("fetch-1"),
        || {
            let context = current_operation_context().expect("context");
            // Una ráfaga en la misma fase: solo pasa la primera.
            for percent in 0..50 {
                context.report_progress(receiving(percent));
            }
            // Un cambio de fase pasa siempre.
            context.report_progress(GitProgress {
                phase: "Resolving deltas".into(),
                percent: Some(10),
                ..Default::default()
            });
        },
    )
    .unwrap();

    let seen = lock(&seen);
    let fetch = |percent| Seen::Progress("fetch-1".into(), "fetch".into(), Some(percent));
    assert_eq!(
        *seen,
        [
            Seen::Phase(OperationPhase::Queued),
            Seen::Phase(OperationPhase::Running),
            fetch(0),
            fetch(10),
            Seen::Phase(OperationPhase::Completed),
        ]
    );
}

#[test]
fn operations_without_id_or_listener_have_no_progress_sink() {
    let published = Arc::new(AtomicUsize::new(0));
    let counter = published.clone();
    let gate = RepositoryMutationGate::builder()
        .on_progress(move |_| {
            counter.fetch_add(1, Ordering::SeqCst);
        })
        .build();
    let has_sink = gate
        .run_exclusive("/repo/progress", None, MutationOptions::new(), || {
            current_operation_context()
                .expect("context")
                .progress_sink()
                .is_some()
        })
        .unwrap();
    assert!(!has_sink);

    let plain = RepositoryMutationGate::builder().build();
    let has_sink = plain
        .run_exclusive(
            "/repo/progress",
            None,
            MutationOptions::new().operation_id("op-2"),
            || {
                current_operation_context()
                    .expect("context")
                    .progress_sink()
                    .is_some()
            },
        )
        .unwrap();
    assert!(!has_sink);
    assert_eq!(published.load(Ordering::SeqCst), 0);
}
