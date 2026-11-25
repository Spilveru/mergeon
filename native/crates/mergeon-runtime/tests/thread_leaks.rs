//! Soltar la cola y el watcher no deja hilos vivos.
//!
//! Va en un binario de pruebas propio, con una sola prueba, para poder contar
//! los hilos del proceso sin que otras pruebas en paralelo muevan la cuenta.

#![cfg(target_os = "linux")]

use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use mergeon_gitcmd::current_operation_context;
use mergeon_runtime::{
    MutationOptions, RepositoryMutationGate, RepositoryStatusWatcher, StatusWatcherOptions,
    WatchMode,
};

const WAIT: Duration = Duration::from_secs(10);

fn thread_count() -> usize {
    std::fs::read_dir("/proc/self/task").unwrap().count()
}

/// El hilo interno de `notify` termina solo, poco después de cerrarse.
fn wait_for_thread_count(expected: usize) {
    let deadline = Instant::now() + WAIT;
    while thread_count() != expected {
        assert!(
            Instant::now() < deadline,
            "{} threads are still alive, expected {expected}",
            thread_count()
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn exercise(repo: &Path) {
    let watcher = Arc::new(RepositoryStatusWatcher::with_options(
        StatusWatcherOptions {
            debounce: Duration::from_millis(5),
            ..StatusWatcherOptions::default()
        },
    ));
    let gate = RepositoryMutationGate::builder()
        .watcher(watcher.clone())
        .on_event(|_event| {})
        .build();

    let (changes, received) = mpsc::channel();
    let mode = watcher.start(repo, move |change| {
        let _ = changes.send(change);
    });
    assert_eq!(mode, WatchMode::FsWatch);

    std::fs::write(repo.join("file.txt"), "x").unwrap();
    received.recv_timeout(WAIT).unwrap();

    // Trabajo terminado, con timeout (arranca el hilo de plazos) y en dos colas.
    for name in ["a", "b"] {
        let done = gate.run_exclusive(
            repo.join(name),
            None,
            MutationOptions::new()
                .operation_id(format!("done-{name}"))
                .timeout(Duration::from_secs(30)),
            || true,
        );
        assert_eq!(done, Ok(true));
    }

    // Y trabajo a medias al soltar: una tarea en curso y otra en cola.
    let (started, task_started) = mpsc::channel();
    let running = gate.enqueue(
        repo,
        None,
        MutationOptions::new().operation_id("running"),
        move || {
            started.send(()).unwrap();
            let context = current_operation_context().unwrap();
            while !context.is_cancelled() {
                std::thread::sleep(Duration::from_millis(1));
            }
        },
    );
    let queued = gate.enqueue(repo, None, MutationOptions::new(), || ());
    task_started.recv_timeout(WAIT).unwrap();

    drop(gate);
    assert_eq!(running.try_result(), Some(Ok(())));
    assert!(queued.try_result().is_some_and(|result| result.is_err()));
    drop(watcher);
}

#[test]
fn dropping_the_gate_and_the_watcher_leaves_no_threads_behind() {
    let repo = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(repo.path().join(".git/refs/heads")).unwrap();
    std::fs::create_dir_all(repo.path().join("src/nested")).unwrap();

    let baseline = thread_count();
    for _ in 0..3 {
        exercise(repo.path());
        wait_for_thread_count(baseline);
    }
}
