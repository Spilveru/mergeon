//! Port de `electron/ipc/git-handlers.test.js`: el envoltorio común de las
//! mutaciones (`registerRepoMutationHandler`).
//!
//! Allí las operaciones son mocks cuya promesa el test resuelve a mano; aquí
//! son operaciones reales sobre repos temporales y, donde el caso necesita
//! una tarea que se quede en vuelo o que "lance", una tarea propia encolada
//! por el mismo envoltorio (`run_repo_mutation`).

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use mergeon_git_ops::{CommitOptions, MutationResult, Outcome, PushOptions};
use mergeon_gitcmd::{ControlCode, current_operation_context};
use mergeon_runtime::{CancelPhase, CancelReason, OperationPhase};
use serde_json::json;

use super::support::{PATIENCE, TestBackend, TestRepo, wait_until};
use crate::git::{MutationConfig, push_config};
use crate::mutation::{Mutation, MutationBody, Thrown};
use crate::{BackendEvent, FailureCode};

/// La configuración del canal `git:stage` para `file_path`.
fn stage_config(repo_path: &Path, file_path: &str) -> MutationConfig {
    MutationConfig::new(
        "stage",
        "Failed to stage file",
        json!([repo_path.to_string_lossy(), file_path]),
    )
    .extra(json!({ "filePath": file_path, "filesAffected": [file_path] }))
}

/// El extremo del test de un stage retenido: avisa cuando el stage ya está
/// hecho y lo suelta cuando el test quiere.
struct Held {
    staged: mpsc::Receiver<()>,
    release: mpsc::Sender<()>,
}

impl Held {
    /// Espera a que la tarea haya hecho su stage y esté retenida.
    fn wait_until_staged(&self) {
        self.staged
            .recv_timeout(PATIENCE)
            .expect("the held stage to run");
    }

    fn release(&self) {
        self.release.send(()).expect("the held stage to be waiting");
    }
}

/// El extremo de la tarea.
struct Hold {
    staged: mpsc::Sender<()>,
    released: mpsc::Receiver<()>,
}

fn hold() -> (Held, Hold) {
    let (staged_sender, staged) = mpsc::channel();
    let (release, released) = mpsc::channel();
    (
        Held { staged, release },
        Hold {
            staged: staged_sender,
            released,
        },
    )
}

/// Encola un stage real que, una vez hecho, no devuelve su resultado hasta
/// que el test lo suelta: la promesa pendiente del mock original.
fn held_stage(
    test: &TestBackend,
    repo_path: &Path,
    file_path: &'static str,
    hold: Hold,
) -> Mutation<MutationResult> {
    let repo = repo_path.to_path_buf();
    test.git()
        .run_repo_mutation(repo_path, stage_config(repo_path, file_path), move || {
            let result = mergeon_git_ops::stage_operation(&repo, file_path);
            let _ = hold.staged.send(());
            let _ = hold.released.recv_timeout(PATIENCE);
            Ok(result)
        })
}

#[test]
fn serializes_mutations_on_the_same_repository_through_the_mutation_gate() {
    let test = TestBackend::new();
    let repo = TestRepo::new();
    repo.write("src/app.js", "export const value = 1\n");
    let (latch, stage_hold) = hold();

    std::thread::scope(|scope| {
        let stage = scope.spawn(|| held_stage(&test, &repo.path, "src/app.js", stage_hold));
        latch.wait_until_staged();
        let commit = scope.spawn(|| {
            test.git()
                .commit(&repo.path, "msg", &CommitOptions::default())
        });

        // El commit no arranca mientras el stage del mismo repo está en vuelo.
        test.events
            .wait_for_operation("commit", OperationPhase::Queued);
        let queue = test.git().get_operation_queue(&repo.path);
        assert_eq!(queue.active.unwrap().operation, "stage");
        assert_eq!(
            queue
                .queued
                .iter()
                .map(|operation| operation.operation.as_str())
                .collect::<Vec<_>>(),
            ["commit"]
        );
        assert!(
            !test.events.operations().iter().any(|event| {
                event.operation == "commit" && event.phase != OperationPhase::Queued
            })
        );
        assert_eq!(repo.log_subjects(), ["Initial commit"]);

        latch.release();
        let stage = stage.join().unwrap();
        let commit = commit.join().unwrap();

        assert!(stage.success());
        assert_eq!(stage.operation(), Some("stage"));
        assert!(commit.success(), "{}", commit.message());
        assert_eq!(commit.operation(), Some("commit"));
        // El commit corrió después del stage: se llevó lo preparado.
        assert_eq!(repo.log_subjects(), ["msg", "Initial commit"]);
    });
}

#[test]
fn mutations_on_different_repositories_do_not_wait_for_each_other() {
    let test = TestBackend::new();
    let busy = TestRepo::new();
    let other = TestRepo::new();
    busy.write("a.txt", "a\n");
    other.write("b.txt", "b\n");
    let (latch, stage_hold) = hold();

    std::thread::scope(|scope| {
        let held = scope.spawn(|| held_stage(&test, &busy.path, "a.txt", stage_hold));
        latch.wait_until_staged();

        // Otro repositorio tiene su propia cola.
        let staged = test.git().stage(&other.path, "b.txt");
        assert!(staged.success(), "{}", staged.message());
        assert!(test.git().get_operation_queue(&busy.path).active.is_some());

        latch.release();
        assert!(held.join().unwrap().success());
    });
}

#[test]
fn publishes_operation_lifecycle_events_and_returns_the_operation_id() {
    let test = TestBackend::new();
    let repo = TestRepo::new();
    repo.write("src/app.js", "export const value = 1\n");

    let result = test.git().stage(&repo.path, "src/app.js");

    assert!(result.success(), "{}", result.message());
    assert_eq!(result.outcome(), Some(Outcome::Completed));
    let operation_id = result.operation_id().expect("operation id");
    assert!(operation_id.starts_with("stage-"));
    assert_eq!(result.to_json()["operationId"], operation_id);

    let events = test.events.operations();
    assert_eq!(
        events.iter().map(|event| event.phase).collect::<Vec<_>>(),
        [
            OperationPhase::Queued,
            OperationPhase::Running,
            OperationPhase::Completed
        ]
    );
    for event in &events {
        assert_eq!(event.operation_id, operation_id);
        assert_eq!(event.operation, "stage");
        assert_eq!(event.repo_path, repo.path.to_string_lossy());
        assert_eq!(event.error, None);
    }
    assert_eq!(
        BackendEvent::Operation(events[0].clone()).channel(),
        "git:operation-event"
    );
}

#[test]
fn serves_the_operation_queue() {
    let test = TestBackend::new();

    let queue = test.git().get_operation_queue("/tmp/demo");

    assert_eq!(queue.active, None);
    assert!(queue.queued.is_empty());
    assert_eq!(
        serde_json::to_value(&queue).unwrap(),
        json!({ "active": null, "queued": [] })
    );
}

#[test]
fn turns_a_cancelled_in_flight_mutation_into_a_structured_cancelled_result() {
    let test = TestBackend::new();
    let repo = TestRepo::new();
    repo.write("src/app.js", "export const value = 1\n");
    let (latch, stage_hold) = hold();

    std::thread::scope(|scope| {
        let stage = scope.spawn(|| held_stage(&test, &repo.path, "src/app.js", stage_hold));
        latch.wait_until_staged();
        let running_event = test
            .events
            .wait_for_operation("stage", OperationPhase::Running);

        let cancelled = test.git().cancel_operation(&running_event.operation_id);
        assert!(cancelled.ok);
        assert_eq!(cancelled.phase, Some(CancelPhase::Running));
        assert_eq!(
            serde_json::to_value(cancelled).unwrap(),
            json!({ "ok": true, "phase": "running" })
        );
        latch.release();

        // La tarea devolvió un resultado con éxito: el handler lo pisa.
        let result = stage.join().unwrap();
        assert!(!result.success());
        assert_eq!(result.outcome(), Some(Outcome::Cancelled));
        assert_eq!(result.code(), "CANCELLED");
        assert_eq!(result.message(), "Git operation was cancelled.");
        assert_eq!(result.operation(), Some("stage"));
        assert_eq!(
            result.operation_id(),
            Some(running_event.operation_id.as_str())
        );
        assert!(matches!(result.body, MutationBody::Controlled { .. }));

        // El resto de campos del resultado original se conserva.
        let json = result.to_json();
        assert_eq!(json["success"], false);
        assert_eq!(json["outcome"], "cancelled");
        assert_eq!(json["code"], "CANCELLED");
        assert_eq!(json["operationId"], running_event.operation_id);
        assert_eq!(json["filePath"], "src/app.js");
        assert_eq!(json["status"]["staged"], json!(["src/app.js"]));

        assert_eq!(
            test.events.phases_of(&running_event.operation_id),
            [
                OperationPhase::Queued,
                OperationPhase::Running,
                OperationPhase::Cancelling,
                OperationPhase::Cancelled
            ]
        );
    });

    // Ya no hay nada que cancelar.
    let cancelled = test.git().cancel_operation("stage-unknown");
    assert!(!cancelled.ok);
    assert_eq!(cancelled.reason, Some(CancelReason::NotFound));
    assert_eq!(
        serde_json::to_value(test.git().cancel_operation("")).unwrap(),
        json!({ "ok": false, "reason": "not-found" })
    );
}

#[test]
fn a_mutation_cancelled_while_queued_never_runs_and_reports_cancelled() {
    let test = TestBackend::new();
    let repo = TestRepo::new();
    repo.write("a.txt", "a\n");
    repo.write("b.txt", "b\n");
    let (latch, stage_hold) = hold();

    std::thread::scope(|scope| {
        let held = scope.spawn(|| held_stage(&test, &repo.path, "a.txt", stage_hold));
        latch.wait_until_staged();

        let waiting = scope.spawn(|| test.git().discard_changes(&repo.path, "b.txt"));
        let queued_event = test
            .events
            .wait_for_operation("discard", OperationPhase::Queued);

        let cancelled = test.git().cancel_operation(&queued_event.operation_id);
        assert_eq!(
            serde_json::to_value(cancelled).unwrap(),
            json!({ "ok": true, "phase": "queued" })
        );
        latch.release();
        assert!(held.join().unwrap().success());

        let result = waiting.join().unwrap();
        assert_eq!(
            result.to_json(),
            json!({
                "success": false,
                "operation": "discard",
                "outcome": "cancelled",
                "code": "CANCELLED",
                "message": "Git operation was cancelled.",
                "status": null,
                "operationState": null,
                "conflictState": null,
                "branch": null,
                "remote": null,
                "tag": null,
                "stashRef": null,
                "commit": null,
                "mode": null,
                "filePath": "b.txt",
                "filesAffected": ["b.txt"],
                "operationId": queued_event.operation_id
            })
        );
        assert_eq!(result.failure().unwrap().code, FailureCode::Cancelled);
        assert_eq!(
            test.events.phases_of(&queued_event.operation_id),
            [OperationPhase::Queued, OperationPhase::Cancelled]
        );
    });

    // La operación descartada no llegó a tocar el archivo.
    assert_eq!(repo.read("b.txt"), "b\n");
}

#[test]
fn returns_the_exact_generic_failure_contract_when_a_mutation_operation_throws() {
    let test = TestBackend::new();
    let repo = TestRepo::new();

    // La validación de la ruta lanza antes de llegar a la operación.
    let result = test.git().stage(&repo.path, "../outside.js");

    assert_eq!(
        result.to_json(),
        json!({
            "success": false,
            "operation": "stage",
            "outcome": "failed",
            "code": "IPC_ERROR",
            "message": "Failed to stage file: The file path must stay inside the repository.",
            "status": null,
            "operationState": null,
            "conflictState": null,
            "branch": null,
            "remote": null,
            "tag": null,
            "stashRef": null,
            "commit": null,
            "mode": null,
            "filePath": "../outside.js",
            "filesAffected": ["../outside.js"]
        })
    );
    assert_eq!(result.operation_id(), None);
    assert_eq!(result.result(), None);

    // La cola sí vio fallar la operación.
    let events = test.events.operations();
    assert_eq!(
        events.iter().map(|event| event.phase).collect::<Vec<_>>(),
        [
            OperationPhase::Queued,
            OperationPhase::Running,
            OperationPhase::Failed
        ]
    );
    let error = events[2].error.clone().expect("event error");
    assert_eq!(error.code, "GIT_ERROR");
    assert_eq!(
        error.message,
        "The file path must stay inside the repository."
    );
}

#[test]
fn a_task_that_fails_without_a_message_names_the_operation() {
    let test = TestBackend::new();
    let repo = TestRepo::new();

    let result: Mutation<MutationResult> =
        test.git()
            .run_repo_mutation(&repo.path, stage_config(&repo.path, "src/app.js"), || {
                Err(Thrown::new(""))
            });

    assert_eq!(result.message(), "Failed to stage file: stage failed");
    assert_eq!(result.code(), "IPC_ERROR");
}

#[test]
fn a_task_that_panics_is_reported_as_a_generic_failure() {
    let test = TestBackend::new();
    let repo = TestRepo::new();

    let result: Mutation<MutationResult> =
        test.git()
            .run_repo_mutation(&repo.path, stage_config(&repo.path, "src/app.js"), || {
                panic!("index locked")
            });

    assert_eq!(result.message(), "Failed to stage file: index locked");
    assert_eq!(result.code(), "IPC_ERROR");
    assert_eq!(
        result.failure().unwrap().field("filePath"),
        Some(&json!("src/app.js"))
    );

    // La cola sigue sirviendo después de un pánico.
    repo.write("ok.txt", "ok\n");
    assert!(test.git().stage(&repo.path, "ok.txt").success());
}

#[test]
fn push_reports_its_defaults_on_handler_failures() {
    let test = TestBackend::new();
    let repo = TestRepo::new();
    let options = PushOptions {
        branch: Some("main".to_string()),
        ..Default::default()
    };

    let result: Mutation<MutationResult> =
        test.git()
            .run_repo_mutation(&repo.path, push_config(&repo.path, &options), || {
                Err(Thrown::new("network down"))
            });

    // `createStructuredHandlerError`: solo el `extra` del canal, sin los
    // campos nulos del fallo genérico.
    assert_eq!(
        result.to_json(),
        json!({
            "success": false,
            "operation": "push",
            "outcome": "failed",
            "code": "IPC_ERROR",
            "message": "Failed to push: network down",
            "conflictFiles": [],
            "operationState": null,
            "branch": "main",
            "remote": "origin",
            "upstreamSet": false
        })
    );

    let without_branch = test.git().run_repo_mutation::<MutationResult, _>(
        &repo.path,
        push_config(
            &repo.path,
            &PushOptions {
                remote: "upstream".to_string(),
                ..Default::default()
            },
        ),
        || Err(Thrown::new("network down")),
    );
    assert_eq!(without_branch.to_json()["branch"], json!(null));
    assert_eq!(without_branch.to_json()["remote"], "upstream");
}

#[test]
fn identical_requests_share_one_operation_and_its_result() {
    let test = TestBackend::new();
    let repo = TestRepo::new();
    repo.write("a.txt", "a\n");
    repo.write("b.txt", "b\n");
    repo.write("blocker.txt", "x\n");
    let (latch, stage_hold) = hold();
    let duplicate_ran = Arc::new(AtomicBool::new(false));

    std::thread::scope(|scope| {
        let held = scope.spawn(|| held_stage(&test, &repo.path, "blocker.txt", stage_hold));
        latch.wait_until_staged();

        let first = scope.spawn(|| test.git().stage(&repo.path, "a.txt"));
        wait_until("the first request to queue", || {
            test.git().get_operation_queue(&repo.path).queued.len() == 1
        });

        // Misma operación, mismos argumentos: se une a la que ya espera y su
        // tarea no llega a ejecutarse.
        let duplicate =
            test.git()
                .enqueue_repo_mutation(&repo.path, stage_config(&repo.path, "a.txt"), {
                    let duplicate_ran = duplicate_ran.clone();
                    let repo_path = repo.path.clone();
                    move || {
                        duplicate_ran.store(true, Ordering::SeqCst);
                        Ok(mergeon_git_ops::stage_operation(&repo_path, "a.txt"))
                    }
                });
        assert!(duplicate.is_coalesced());
        assert_eq!(test.git().get_operation_queue(&repo.path).queued.len(), 1);

        // Otros argumentos: operación aparte.
        let different = scope.spawn(|| test.git().stage(&repo.path, "b.txt"));
        wait_until("the different request to queue", || {
            test.git().get_operation_queue(&repo.path).queued.len() == 2
        });

        latch.release();
        assert!(held.join().unwrap().success());
        let first = first.join().unwrap();
        let duplicate = duplicate.wait();
        let different = different.join().unwrap();

        assert!(first.success(), "{}", first.message());
        assert_eq!(first, duplicate);
        assert_eq!(first.operation_id(), duplicate.operation_id());
        assert!(different.success());
        assert_ne!(first.operation_id(), different.operation_id());
    });
    assert!(!duplicate_ran.load(Ordering::SeqCst));

    // Tres operaciones en la cola para cuatro solicitudes.
    let queued = test
        .events
        .operations()
        .into_iter()
        .filter(|event| event.phase == OperationPhase::Queued)
        .count();
    assert_eq!(queued, 3);

    // Terminada la operación, la misma solicitud vuelve a ejecutarse.
    let again = test.git().stage(&repo.path, "a.txt");
    assert_eq!(again.code(), "NOTHING_TO_STAGE");
    assert_ne!(again.operation_id(), None);
}

#[test]
fn two_callers_of_the_same_channel_share_the_result() {
    // Lo mismo, solo con la API pública: dos hilos piden el mismo fetch
    // mientras la cola está ocupada.
    let test = TestBackend::new();
    let repo = TestRepo::new();
    repo.publish_to_origin();
    repo.write("blocker.txt", "x\n");
    let (latch, stage_hold) = hold();

    std::thread::scope(|scope| {
        let held = scope.spawn(|| held_stage(&test, &repo.path, "blocker.txt", stage_hold));
        latch.wait_until_staged();

        let (calling, about_to_call) = mpsc::channel();
        let (git, repo_path) = (test.git(), &repo.path);
        let callers: Vec<_> = (0..3)
            .map(|_| {
                let calling = calling.clone();
                scope.spawn(move || {
                    calling.send(()).unwrap();
                    git.fetch(repo_path, &Default::default())
                })
            })
            .collect();
        for _ in 0..3 {
            about_to_call.recv_timeout(PATIENCE).unwrap();
        }
        wait_until("the fetch to queue", || {
            test.git().get_operation_queue(&repo.path).queued.len() == 1
        });
        // Margen para que los tres, ya en marcha, hayan llegado a la cola: una
        // solicitud que se une a otra no deja rastro en ella.
        std::thread::sleep(Duration::from_millis(250));
        assert_eq!(test.git().get_operation_queue(&repo.path).queued.len(), 1);

        latch.release();
        assert!(held.join().unwrap().success());
        let results: Vec<_> = callers
            .into_iter()
            .map(|caller| caller.join().unwrap())
            .collect();
        assert!(results[0].success(), "{}", results[0].message());
        assert_eq!(results[0], results[1]);
        assert_eq!(results[0], results[2]);
    });
}

#[test]
fn a_mutation_that_outlives_its_timeout_is_stopped_and_reports_timeout() {
    let test = TestBackend::new();
    let repo = TestRepo::new();
    repo.write("src/app.js", "export const value = 1\n");
    // El resultado con éxito que la tarea devolverá demasiado tarde.
    let late_result = mergeon_git_ops::stage_operation(&repo.path, "src/app.js");
    assert!(late_result.success);

    let result: Mutation<MutationResult> = test.git().run_repo_mutation(
        &repo.path,
        stage_config(&repo.path, "src/app.js").timeout(Duration::from_millis(40)),
        move || {
            // Una tarea que no termina hasta que la cola la detiene.
            let context = current_operation_context().expect("operation context");
            while !context.is_cancelled() {
                std::thread::sleep(Duration::from_millis(2));
            }
            Ok(late_result)
        },
    );

    assert!(!result.success());
    assert_eq!(result.outcome(), Some(Outcome::Timeout));
    assert_eq!(result.code(), "TIMEOUT");
    assert_eq!(result.message(), "Git operation timed out and was stopped.");
    assert_eq!(result.handler_control_code(), Some(ControlCode::Timeout));
    assert!(result.is_timeout());
    let operation_id = result.operation_id().expect("operation id").to_string();
    assert_eq!(
        test.events.phases_of(&operation_id),
        [
            OperationPhase::Queued,
            OperationPhase::Running,
            OperationPhase::Cancelling,
            OperationPhase::Timeout
        ]
    );
}

#[test]
fn a_task_that_throws_after_its_timeout_becomes_a_timeout_failure_with_its_operation_id() {
    let test = TestBackend::new();
    let repo = TestRepo::new();

    let result: Mutation<MutationResult> = test.git().run_repo_mutation(
        &repo.path,
        stage_config(&repo.path, "src/app.js").timeout(Duration::from_millis(40)),
        || {
            let context = current_operation_context().expect("operation context");
            while !context.is_cancelled() {
                std::thread::sleep(Duration::from_millis(2));
            }
            Err(Thrown::new("the process was killed"))
        },
    );

    let failure = result.failure().expect("handler failure");
    assert_eq!(failure.code, FailureCode::Timeout);
    assert_eq!(failure.outcome, Outcome::Timeout);
    assert_eq!(result.message(), "Git operation timed out and was stopped.");
    // Siempre el fallo genérico, con el `extra` del canal.
    assert_eq!(failure.field("filePath"), Some(&json!("src/app.js")));
    assert_eq!(failure.field("status"), Some(&json!(null)));

    let operation_id = result.operation_id().expect("operation id");
    assert!(operation_id.starts_with("stage-"));
    assert_eq!(result.to_json()["operationId"], operation_id);
    let events = test.events.operations();
    let last = events.last().unwrap();
    assert_eq!(last.phase, OperationPhase::Timeout);
    assert_eq!(last.operation_id, operation_id);
    assert_eq!(last.error.as_ref().unwrap().code, "TIMEOUT");
    assert_eq!(
        last.error.as_ref().unwrap().message,
        "Git operation timed out."
    );
}

/// Con git de verdad: un commit detenido en su hook se cancela matando el
/// proceso, y es la propia operación la que informa `CANCELLED`.
#[cfg(unix)]
#[test]
fn cancelling_a_running_git_process_stops_it() {
    use std::os::unix::fs::PermissionsExt;

    let test = TestBackend::new();
    let repo = TestRepo::new();
    let marker = repo.root().join("hook-started");
    let hook = repo.path.join(".git/hooks/pre-commit");
    // El hook suelta la salida de git antes de dormir: así matar a git cierra
    // sus tuberías aunque el hook siga vivo unos segundos.
    std::fs::write(
        &hook,
        format!(
            "#!/bin/sh\ntouch '{}'\nexec >/dev/null 2>&1\nsleep 5\n",
            marker.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    repo.write("a.txt", "a\n");
    repo.git(&["add", "a.txt"]);

    std::thread::scope(|scope| {
        let commit = scope.spawn(|| {
            test.git()
                .commit(&repo.path, "never lands", &CommitOptions::default())
        });
        let running = test
            .events
            .wait_for_operation("commit", OperationPhase::Running);
        wait_until("the pre-commit hook to start", || marker.exists());

        assert!(test.git().cancel_operation(&running.operation_id).ok);
        let result = commit.join().unwrap();

        assert!(!result.success());
        assert_eq!(result.code(), "CANCELLED");
        assert_eq!(result.outcome(), Some(Outcome::Cancelled));
        assert_eq!(result.operation_id(), Some(running.operation_id.as_str()));
        // El resultado ya venía cancelado de la operación: no hubo que pisarlo.
        assert!(matches!(result.body, MutationBody::Completed(_)));
    });

    assert_eq!(repo.log_subjects(), ["Initial commit"]);
    assert_eq!(
        test.events.operations().last().unwrap().phase,
        OperationPhase::Cancelled
    );
}

/// Cada canal registrado con `registerRepoMutationHandler` entra en la cola
/// con su nombre de operación y, si la cola lo rechaza, responde con el fallo
/// genérico más el `extra` de ese canal. Se comprueban todos de una vez:
/// encolados detrás de una operación retenida y cancelados antes de arrancar.
#[test]
fn every_mutation_channel_queues_with_its_operation_name_and_reports_its_extra() {
    use serde_json::Value;

    let test = TestBackend::new();
    let repo = TestRepo::new();
    repo.write("blocker.txt", "x\n");
    let (latch, stage_hold) = hold();
    let git = test.git();
    let path = repo.path.as_path();

    let file_extra = json!({ "filePath": "a.txt", "filesAffected": ["a.txt"] });
    let sequence_extra = json!({
        "conflictFiles": [],
        "operationState": null,
        "branch": null,
        "remote": null,
        "upstreamSet": false
    });
    type Call<'a> = Box<dyn FnOnce() -> Value + Send + 'a>;
    let calls: Vec<(&str, Value, Call)> = vec![
        (
            "init-repository",
            json!({}),
            Box::new(|| git.init_repository(path).to_json()),
        ),
        (
            "add-remote",
            json!({ "remote": "up" }),
            Box::new(|| {
                git.add_remote(path, "up", "https://example.com/a.git")
                    .to_json()
            }),
        ),
        (
            "remove-remote",
            json!({ "remote": "up" }),
            Box::new(|| git.remove_remote(path, "up").to_json()),
        ),
        (
            "rename-remote",
            json!({ "remote": "up" }),
            Box::new(|| git.rename_remote(path, "up", "down").to_json()),
        ),
        (
            "set-remote-url",
            json!({ "remote": "up" }),
            Box::new(|| {
                git.set_remote_url(path, "up", "https://example.com/b.git")
                    .to_json()
            }),
        ),
        (
            "submodule-update",
            json!({}),
            Box::new(|| git.submodule_update(path, &Default::default()).to_json()),
        ),
        (
            "fetch",
            json!({}),
            Box::new(|| git.fetch(path, &Default::default()).to_json()),
        ),
        (
            "pull",
            json!({}),
            Box::new(|| git.pull(path, &Default::default()).to_json()),
        ),
        (
            "push",
            json!({
                "conflictFiles": [],
                "operationState": null,
                "branch": "main",
                "remote": "origin",
                "upstreamSet": false
            }),
            Box::new(|| {
                git.push(
                    path,
                    &PushOptions {
                        branch: Some("main".to_string()),
                        ..Default::default()
                    },
                )
                .to_json()
            }),
        ),
        (
            "checkout",
            json!({ "branch": "dev" }),
            Box::new(|| git.checkout(path, "dev").to_json()),
        ),
        (
            "checkout-remote-branch",
            json!({ "remote": "origin/dev" }),
            Box::new(|| git.checkout_remote_branch(path, "origin/dev").to_json()),
        ),
        (
            "create-branch",
            json!({ "branch": "new" }),
            Box::new(|| git.create_branch(path, "new", None, true).to_json()),
        ),
        (
            "rename-branch",
            json!({ "branch": "newer" }),
            Box::new(|| git.rename_branch(path, "old", "newer").to_json()),
        ),
        (
            "delete-branch",
            json!({ "branch": "old" }),
            Box::new(|| git.delete_branch(path, "old").to_json()),
        ),
        (
            "delete-remote-branch",
            json!({ "branch": "old", "remote": "origin" }),
            Box::new(|| git.delete_remote_branch(path, "origin", "old").to_json()),
        ),
        (
            "fast-forward-branch",
            json!({ "branch": "main", "remote": "origin/main" }),
            Box::new(|| {
                git.fast_forward_branch(path, "main", "origin/main")
                    .to_json()
            }),
        ),
        (
            "apply-patch",
            json!({}),
            Box::new(|| git.apply_patch(path, "diff", &Default::default()).to_json()),
        ),
        (
            "stage",
            file_extra.clone(),
            Box::new(|| git.stage(path, "a.txt").to_json()),
        ),
        (
            "stage-all",
            json!({}),
            Box::new(|| git.stage_all(path).to_json()),
        ),
        (
            "unstage",
            file_extra.clone(),
            Box::new(|| git.unstage(path, "a.txt").to_json()),
        ),
        (
            "unstage-all",
            json!({}),
            Box::new(|| git.unstage_all(path).to_json()),
        ),
        (
            "discard",
            file_extra.clone(),
            Box::new(|| git.discard_changes(path, "a.txt").to_json()),
        ),
        (
            "discard-all",
            json!({}),
            Box::new(|| git.discard_all(path, None).to_json()),
        ),
        (
            "commit",
            json!({}),
            Box::new(|| git.commit(path, "msg", &Default::default()).to_json()),
        ),
        (
            "merge",
            json!({
                "conflictFiles": [],
                "operationState": null,
                "branch": "main",
                "remote": null,
                "upstreamSet": false
            }),
            Box::new(|| {
                git.merge(
                    path,
                    "feature",
                    &mergeon_git_ops::MergeOptions {
                        target_branch: Some("main".to_string()),
                        ..Default::default()
                    },
                )
                .to_json()
            }),
        ),
        (
            "rebase",
            sequence_extra.clone(),
            Box::new(|| git.rebase(path, "feature", &Default::default()).to_json()),
        ),
        (
            "stash",
            json!({}),
            Box::new(|| git.stash(path, Some("wip")).to_json()),
        ),
        (
            "apply-stash",
            json!({ "stashRef": "stash@{0}" }),
            Box::new(|| git.apply_stash(path, "stash@{0}").to_json()),
        ),
        (
            "pop-stash",
            json!({ "stashRef": "stash@{0}" }),
            Box::new(|| git.pop_stash(path, "stash@{0}").to_json()),
        ),
        (
            "drop-stash",
            json!({ "stashRef": "stash@{0}" }),
            Box::new(|| git.drop_stash(path, "stash@{0}").to_json()),
        ),
        (
            "stash-branch",
            json!({ "stashRef": "stash@{0}", "branch": "from-stash" }),
            Box::new(|| git.stash_branch(path, "stash@{0}", "from-stash").to_json()),
        ),
        (
            "create-tag",
            json!({ "tag": "v1" }),
            Box::new(|| git.create_tag(path, "v1", None, None).to_json()),
        ),
        (
            "delete-tag",
            json!({ "tag": "v1" }),
            Box::new(|| git.delete_tag(path, "v1").to_json()),
        ),
        (
            "push-tag",
            json!({ "tag": "v1" }),
            Box::new(|| git.push_tag(path, "v1", &Default::default()).to_json()),
        ),
        (
            "delete-remote-tag",
            json!({ "tag": "v1" }),
            Box::new(|| {
                git.delete_remote_tag(path, "v1", &Default::default())
                    .to_json()
            }),
        ),
        (
            "checkout-tag",
            json!({ "tag": "v1" }),
            Box::new(|| git.checkout_tag(path, "v1").to_json()),
        ),
        (
            "setup-tracking",
            json!({ "branch": "main" }),
            Box::new(|| git.setup_tracking(path, Some("main")).to_json()),
        ),
        (
            "reset",
            json!({ "commit": "abc123", "mode": "hard" }),
            Box::new(|| git.reset(path, "abc123", "hard").to_json()),
        ),
        (
            "revert",
            sequence_extra.clone(),
            Box::new(|| git.revert(path, "abc123").to_json()),
        ),
        (
            "cherry-pick",
            sequence_extra.clone(),
            Box::new(|| git.cherry_pick(path, "abc123").to_json()),
        ),
        (
            "add-to-ignore",
            json!({ "filePath": "dist" }),
            Box::new(|| git.add_to_ignore(path, "dist").to_json()),
        ),
        (
            "resolve-conflict",
            json!({ "filePath": "a.txt" }),
            Box::new(|| git.resolve_conflict(path, "a.txt", "resolved\n").to_json()),
        ),
        (
            "interactive-rebase",
            json!({}),
            Box::new(|| git.interactive_rebase(path, &Default::default()).to_json()),
        ),
        (
            "rebase-action",
            json!({ "action": "drop", "commit": "abc123" }),
            Box::new(|| {
                git.rebase_action(path, "drop", "abc123", &Default::default())
                    .to_json()
            }),
        ),
        (
            "sequence-action",
            json!({ "action": "abort" }),
            Box::new(|| git.sequence_action(path, "abort").to_json()),
        ),
    ];
    let expected: Vec<(&str, Value)> = calls
        .iter()
        .map(|(operation, extra, _)| (*operation, extra.clone()))
        .collect();

    let results: Vec<Value> = std::thread::scope(|scope| {
        let held = scope.spawn(|| held_stage(&test, path, "blocker.txt", stage_hold));
        latch.wait_until_staged();

        let pending: Vec<_> = calls
            .into_iter()
            .map(|(_, _, call)| scope.spawn(call))
            .collect();
        wait_until("every channel to queue", || {
            git.get_operation_queue(path).queued.len() == expected.len()
        });

        // Todas esperan su turno: ninguna llega a ejecutarse.
        for operation in git.get_operation_queue(path).queued {
            let cancelled = git.cancel_operation(&operation.operation_id.unwrap());
            assert!(cancelled.ok);
        }
        latch.release();
        assert!(held.join().unwrap().success());

        pending
            .into_iter()
            .map(|call| call.join().unwrap())
            .collect()
    });

    for ((operation, extra), result) in expected.iter().zip(&results) {
        let mut wanted = json!({
            "success": false,
            "operation": operation,
            "outcome": "cancelled",
            "code": "CANCELLED",
            "message": "Git operation was cancelled.",
            "status": null,
            "operationState": null,
            "conflictState": null,
            "branch": null,
            "remote": null,
            "tag": null,
            "stashRef": null,
            "commit": null,
            "mode": null,
            "filePath": null,
            "filesAffected": [],
            "operationId": result["operationId"]
        });
        for (key, value) in extra.as_object().unwrap() {
            wanted[key.as_str()] = value.clone();
        }
        assert_eq!(result, &wanted, "{operation}");
        let operation_id = result["operationId"].as_str().expect("operation id");
        assert!(
            operation_id.starts_with(&format!("{operation}-")),
            "{operation_id}"
        );
        assert_eq!(
            test.events.phases_of(operation_id),
            [OperationPhase::Queued, OperationPhase::Cancelled],
            "{operation}"
        );
    }

    // Nada de lo cancelado tocó el repositorio.
    assert_eq!(repo.log_subjects(), ["Initial commit"]);
    assert!(!repo.path.join(".gitignore").exists());
    assert_eq!(repo.git(&["branch", "--list"]).trim(), "* main");
    assert_eq!(repo.git(&["tag"]).trim(), "");
}
