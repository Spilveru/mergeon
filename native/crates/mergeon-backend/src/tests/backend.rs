//! Construcción del [`Backend`] y lo que cablea: eventos, watcher enlazado a
//! la cola, cifrado de tokens, actualizador y cierre.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use mergeon_git_ops::MutationResult;
use mergeon_runtime::{
    OperationPhase, StatusWatcherOptions, WatchBackend, WatchError, WatchHandle, WatchMode,
    WatchReason, WatchSink,
};
use mergeon_store::{GitHubAccount, RecentRepository, StoreError};
use serde_json::{Value, json};

use super::support::{PATIENCE, TestBackend, TestRepo};
use crate::git::MutationConfig;
use crate::mutation::Mutation;
use crate::updater::{UpdaterBridge, UpdaterEventSink};
use crate::{Backend, BackendError, BackendEvent, BackendOptions, KeychainCipher, MemoryKeyStore};

#[test]
fn the_backend_is_shared_between_threads() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Backend>();
    assert_send_sync::<BackendEvent>();

    let test = TestBackend::new();
    let repo = TestRepo::new();
    let backend = Arc::new(test.backend);
    let workers: Vec<_> = (0..4)
        .map(|index| {
            let backend = backend.clone();
            let path = repo.path.clone();
            std::thread::spawn(move || {
                std::fs::write(path.join(format!("file-{index}.txt")), "x\n").unwrap();
                backend
                    .git()
                    .stage(&path, &format!("file-{index}.txt"))
                    .success()
            })
        })
        .collect();

    for worker in workers {
        assert!(worker.join().unwrap());
    }
    let status = backend.git().get_status(&repo.path).unwrap();
    assert_eq!(status.staged.len(), 4);
}

#[test]
fn works_without_an_event_receiver() {
    let repo = TestRepo::new();
    repo.write("a.txt", "a\n");
    let backend = Backend::new(BackendOptions::new().store(mergeon_store::Store::in_memory(
        mergeon_store::StoreOptions::new(Arc::new(mergeon_store::PlainTextCipher)),
    )))
    .unwrap();

    let staged = backend.git().stage(&repo.path, "a.txt");
    assert!(staged.success());
    assert!(staged.operation_id().is_some());
    assert!(backend.git().watch_status(&repo.path).success);
    assert!(backend.git().unwatch_status().success);
}

#[test]
fn exposes_the_store_and_the_github_client() {
    let test = TestBackend::new();

    let recent = test
        .backend
        .storage()
        .add_recent_repository(&RecentRepository {
            id: "1".to_string(),
            name: "demo".to_string(),
            path: "/tmp/demo".to_string(),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(recent.len(), 1);
    assert_eq!(
        test.backend.storage().get_recent_repositories()[0].path,
        "/tmp/demo"
    );
    assert!(!test.backend.github().client_id().is_empty());
    assert!(!test.backend.updater().is_available());
    assert_eq!(test.backend.updater().get_state(), None);
}

// --- git:watchStatus / unwatchStatus / setStatusWatchWindowFocus ------------

/// Vigilancia de mentira: el test decide cuándo "cambia" un archivo.
#[derive(Default)]
struct ManualWatchBackend {
    sink: Mutex<Option<WatchSink>>,
    fail: bool,
}

impl ManualWatchBackend {
    fn touch(&self, relative_path: &str) {
        let sink = self.sink.lock().unwrap().clone().expect("watching");
        sink.event(relative_path);
    }
}

impl WatchBackend for ManualWatchBackend {
    fn watch(
        &self,
        _repo_path: &Path,
        sink: WatchSink,
    ) -> Result<Box<dyn WatchHandle>, WatchError> {
        if self.fail {
            return Err(WatchError::new("no watches left"));
        }
        *self.sink.lock().unwrap() = Some(sink);
        Ok(Box::new(()))
    }
}

const DEBOUNCE: Duration = Duration::from_millis(15);
const SUPPRESS_TRAILING: Duration = Duration::from_millis(30);
/// Bastante más que el debounce: si un aviso fuera a salir, ya habría salido.
const QUIET: Duration = Duration::from_millis(150);

fn backend_watching_with(watch: Arc<ManualWatchBackend>) -> TestBackend {
    TestBackend::with_options(|options| {
        options.watcher_options(StatusWatcherOptions {
            debounce: DEBOUNCE,
            suppress_trailing: SUPPRESS_TRAILING,
            backend: watch,
            ..Default::default()
        })
    })
}

fn wait_for_status_changes(test: &TestBackend, count: usize) {
    test.events
        .wait_for(&format!("{count} status change(s)"), |events| {
            let seen = events
                .iter()
                .filter(|event| matches!(event, BackendEvent::StatusChanged(_)))
                .count();
            (seen >= count).then_some(())
        });
}

#[test]
fn watch_status_delivers_status_changes_as_backend_events() {
    let watch = Arc::new(ManualWatchBackend::default());
    let test = backend_watching_with(watch.clone());
    let repo = TestRepo::new();

    let watching = test.git().watch_status(&repo.path);
    assert_eq!(watching.mode, WatchMode::FsWatch);
    assert_eq!(
        serde_json::to_value(watching).unwrap(),
        json!({ "success": true, "mode": "fs-watch" })
    );

    watch.touch("src/app.js");
    wait_for_status_changes(&test, 1);
    let change = test.events.status_changes().remove(0);
    assert_eq!(change.repo_path, repo.path.to_string_lossy());
    assert_eq!(change.reason, WatchReason::FsWatchWorkingTree);
    assert_eq!(
        BackendEvent::StatusChanged(change.clone()).channel(),
        "git:status-changed"
    );
    assert_eq!(
        serde_json::to_value(&change).unwrap(),
        json!({ "repoPath": repo.path.to_string_lossy(), "reason": "fs-watch-working-tree" })
    );

    watch.touch(".git/HEAD");
    wait_for_status_changes(&test, 2);
    assert_eq!(
        test.events.status_changes()[1].reason,
        WatchReason::FsWatchHead
    );

    assert_eq!(
        serde_json::to_value(test.git().set_status_watch_window_focus(false)).unwrap(),
        json!({ "success": true })
    );
    assert_eq!(
        serde_json::to_value(test.git().unwatch_status()).unwrap(),
        json!({ "success": true })
    );

    // Tras dejar de vigilar, lo que llegue de la vigilancia anterior se descarta.
    watch.touch("src/app.js");
    std::thread::sleep(QUIET);
    assert_eq!(test.events.status_changes().len(), 2);
}

#[test]
fn the_queue_silences_the_watcher_while_one_of_its_mutations_runs() {
    let watch = Arc::new(ManualWatchBackend::default());
    let test = backend_watching_with(watch.clone());
    let repo = TestRepo::new();
    repo.write("a.txt", "a\n");
    test.git().watch_status(&repo.path);
    let (started, running) = mpsc::channel::<()>();
    let (release, released) = mpsc::channel::<()>();

    std::thread::scope(|scope| {
        let mutation = scope.spawn(|| {
            let path = repo.path.clone();
            test.git().run_repo_mutation(
                &repo.path,
                MutationConfig::new("stage", "Failed to stage file", json!(["a.txt"])),
                move || {
                    let result = mergeon_git_ops::stage_operation(&path, "a.txt");
                    let _ = started.send(());
                    let _ = released.recv_timeout(PATIENCE);
                    Ok(result)
                },
            )
        });
        // La tarea ya corre: la cola avisó antes al watcher.
        running.recv_timeout(PATIENCE).expect("the mutation to run");
        assert_eq!(
            test.events.operations().last().unwrap().phase,
            OperationPhase::Running
        );

        // Los cambios que provoca la propia mutación no se anuncian.
        watch.touch("a.txt");
        watch.touch(".git/index");
        std::thread::sleep(QUIET);
        assert!(test.events.status_changes().is_empty());

        release.send(()).unwrap();
        let result: Mutation<MutationResult> = mutation.join().unwrap();
        assert!(result.success());
    });

    // Pasada la ventana de cola, los cambios externos vuelven a anunciarse.
    std::thread::sleep(SUPPRESS_TRAILING * 3);
    watch.touch("b.txt");
    wait_for_status_changes(&test, 1);
}

#[test]
fn watch_status_falls_back_to_polling_when_native_watching_fails() {
    let watch = Arc::new(ManualWatchBackend {
        fail: true,
        ..Default::default()
    });
    let test = backend_watching_with(watch);
    let repo = TestRepo::new();

    let watching = test.git().watch_status(&repo.path);

    assert!(watching.success);
    assert_eq!(watching.mode, WatchMode::Poll);
    assert_eq!(
        serde_json::to_value(watching).unwrap(),
        json!({ "success": true, "mode": "poll" })
    );
    test.git().unwatch_status();
}

#[test]
fn the_real_watcher_notices_a_change_in_the_working_tree() {
    let test = TestBackend::new();
    let repo = TestRepo::new();

    assert!(test.git().watch_status(&repo.path).success);
    repo.write("notes.txt", "hello\n");

    wait_for_status_changes(&test, 1);
    assert_eq!(
        test.events.status_changes()[0].repo_path,
        repo.path.to_string_lossy()
    );
    test.git().unwatch_status();
}

#[test]
fn shutdown_and_drop_stop_the_watcher() {
    let watch = Arc::new(ManualWatchBackend::default());
    let test = backend_watching_with(watch.clone());
    let repo = TestRepo::new();
    repo.write("a.txt", "a\n");
    test.git().watch_status(&repo.path);

    test.backend.shutdown();
    watch.touch("a.txt");
    std::thread::sleep(QUIET);
    assert!(test.events.status_changes().is_empty());

    // La cola sigue viva hasta soltar el backend.
    assert!(test.git().stage(&repo.path, "a.txt").success());

    let events = test.events.clone();
    drop(test);
    watch.touch("a.txt");
    std::thread::sleep(QUIET);
    assert!(events.status_changes().is_empty());
}

// --- Configuración persistente y cifrado de tokens --------------------------

fn account(token: &str) -> GitHubAccount {
    GitHubAccount {
        id: "42".to_string(),
        username: "octocat".to_string(),
        token: token.to_string(),
        refresh_token: "ghr_refresh".to_string(),
        ..Default::default()
    }
}

fn backend_at(path: &Path, keys: &Arc<MemoryKeyStore>) -> Backend {
    Backend::new(
        BackendOptions::new()
            .store_path(path)
            .cipher(Arc::new(
                KeychainCipher::with_store(keys.clone()).with_retry_interval(Duration::ZERO),
            ))
            .without_electron_import(),
    )
    .unwrap()
}

#[test]
fn tokens_are_encrypted_on_disk_with_the_keyring_master_key() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("settings.json");
    let keys = Arc::new(MemoryKeyStore::new());

    {
        let backend = backend_at(&path, &keys);
        assert_eq!(backend.storage().path(), Some(path.as_path()));
        let saved = backend
            .storage()
            .save_github_account(&account("gho_secret_token"))
            .unwrap();
        assert_eq!(saved.token, "gho_secret_token");
    }

    let on_disk = std::fs::read_to_string(&path).unwrap();
    assert!(!on_disk.contains("gho_secret_token"));
    assert!(!on_disk.contains("ghr_refresh"));
    let document: Value = serde_json::from_str(&on_disk).unwrap();
    let stored = &document["githubAccounts"][0];
    assert!(stored["tokenCipher"].as_str().unwrap().starts_with("safe:"));
    assert!(
        stored["refreshTokenCipher"]
            .as_str()
            .unwrap()
            .starts_with("safe:")
    );

    // Otro arranque, misma clave en el llavero: los tokens vuelven.
    let reopened = backend_at(&path, &keys);
    let accounts = reopened.storage().get_github_accounts().unwrap();
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].token, "gho_secret_token");
    assert_eq!(accounts[0].refresh_token, "ghr_refresh");
}

#[test]
fn without_a_keyring_tokens_are_not_saved_in_plain_text_silently() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("settings.json");
    let keys = Arc::new(MemoryKeyStore::new());
    keys.set_unavailable(true);

    let backend = backend_at(&path, &keys);
    let error = backend
        .storage()
        .save_github_account(&account("gho_secret_token"))
        .unwrap_err();
    assert!(matches!(
        error,
        mergeon_store::StoreError::KeyringUnavailable
    ));

    let on_disk = std::fs::read_to_string(&path).unwrap();
    assert!(!on_disk.contains("gho_secret_token"));
    assert_eq!(keys.secret(), None);
}

#[test]
fn a_settings_file_that_cannot_be_read_fails_the_construction() {
    let directory = tempfile::tempdir().unwrap();

    // La ruta existe pero es un directorio.
    let error = Backend::new(BackendOptions::new().store_path(directory.path())).unwrap_err();

    let BackendError::Store(store_error) = &error;
    assert!(matches!(store_error, StoreError::Io { .. }), "{error}");
    assert!(
        error
            .to_string()
            .starts_with("Failed to open the settings store: ")
    );
}

// --- Actualizador -----------------------------------------------------------

#[derive(Default)]
struct FakeUpdater {
    sink: Mutex<Option<UpdaterEventSink>>,
    checks: AtomicUsize,
    shutdowns: AtomicUsize,
}

impl UpdaterBridge for FakeUpdater {
    fn get_state(&self) -> Value {
        json!({ "status": "idle", "checks": self.checks.load(Ordering::SeqCst) })
    }

    fn check_for_updates(&self, manual: bool) {
        self.checks.fetch_add(1, Ordering::SeqCst);
        if let Some(sink) = self.sink.lock().unwrap().as_ref() {
            sink(json!({ "status": "checking", "manual": manual }));
            sink(self.get_state());
        }
    }

    fn install_update(&self) -> Value {
        json!({ "ok": false, "reason": "not-ready" })
    }

    fn subscribe(&self, sink: UpdaterEventSink) {
        *self.sink.lock().unwrap() = Some(sink);
    }

    fn shutdown(&self) {
        self.shutdowns.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn a_plugged_updater_answers_the_updater_channels_and_publishes_its_events() {
    let updater = Arc::new(FakeUpdater::default());
    let bridge = updater.clone();
    let test = TestBackend::with_options(move |options| options.updater(bridge));

    assert!(test.backend.updater().is_available());
    assert_eq!(
        test.backend.updater().get_state(),
        Some(json!({ "status": "idle", "checks": 0 }))
    );
    assert_eq!(
        test.backend.updater().check(true),
        Some(json!({ "status": "idle", "checks": 1 }))
    );
    assert_eq!(
        test.backend.updater().install(),
        json!({ "ok": false, "reason": "not-ready" })
    );

    let updater_events: Vec<Value> = test.events.wait_for("updater events", |events| {
        let states: Vec<Value> = events
            .iter()
            .filter_map(|event| match event {
                BackendEvent::Updater(state) => Some(state.clone()),
                _ => None,
            })
            .collect();
        (states.len() == 2).then_some(states)
    });
    assert_eq!(
        updater_events,
        [
            json!({ "status": "checking", "manual": true }),
            json!({ "status": "idle", "checks": 1 })
        ]
    );
    assert_eq!(
        BackendEvent::Updater(Value::Null).channel(),
        "updater:event"
    );

    drop(test);
    assert_eq!(updater.shutdowns.load(Ordering::SeqCst), 1);
}
