//! Port de `electron/services/repository-status-watch.test.js`, más las
//! pruebas de la vigilancia real con `notify`.

use std::collections::VecDeque;
use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};

use super::classify::merge_queued_reason;
use super::*;

/// Margen para lo que debe ocurrir enseguida; solo se agota si algo falla.
const WAIT: Duration = Duration::from_secs(10);

/// Sustituto de `watchImpl`: guarda el sink para disparar eventos a mano.
#[derive(Default)]
struct FakeBackend {
    failure: Option<&'static str>,
    watched: Mutex<Vec<(PathBuf, WatchSink)>>,
    closed: Arc<AtomicUsize>,
}

struct CloseCounter(Arc<AtomicUsize>);

impl Drop for CloseCounter {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

impl FakeBackend {
    fn failing(message: &'static str) -> Self {
        Self {
            failure: Some(message),
            ..Self::default()
        }
    }

    fn sink(&self) -> WatchSink {
        lock(&self.watched)
            .last()
            .map(|(_, sink)| sink.clone())
            .expect("the watcher never started the backend")
    }

    fn watched_paths(&self) -> Vec<PathBuf> {
        lock(&self.watched)
            .iter()
            .map(|(path, _)| path.clone())
            .collect()
    }

    fn closed(&self) -> usize {
        self.closed.load(Ordering::SeqCst)
    }
}

impl WatchBackend for FakeBackend {
    fn watch(&self, repo_path: &Path, sink: WatchSink) -> Result<Box<dyn WatchHandle>, WatchError> {
        lock(&self.watched).push((repo_path.to_path_buf(), sink));
        match self.failure {
            Some(message) => Err(WatchError::new(message)),
            None => Ok(Box::new(CloseCounter(self.closed.clone()))),
        }
    }
}

/// `vi.fn().mockReturnValueOnce(a).mockReturnValueOnce(b)`: devuelve la
/// secuencia y, agotada, `then`.
fn fingerprints(sequence: &[&str], then: Option<&str>) -> (HeadFingerprintFn, Arc<AtomicUsize>) {
    let pending: Mutex<VecDeque<String>> =
        Mutex::new(sequence.iter().map(|value| value.to_string()).collect());
    let then = then.map(str::to_string);
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let read: HeadFingerprintFn = Arc::new(move |_repo_path| {
        counter.fetch_add(1, Ordering::SeqCst);
        lock(&pending).pop_front().or_else(|| then.clone())
    });
    (read, calls)
}

struct Harness {
    watcher: RepositoryStatusWatcher,
    clock: ManualClock,
    backend: Arc<FakeBackend>,
    emitted: Arc<Mutex<Vec<StatusChange>>>,
}

impl Harness {
    fn new(backend: FakeBackend) -> Self {
        Self::with_fingerprint(backend, Arc::new(get_head_fingerprint))
    }

    fn with_fingerprint(backend: FakeBackend, head_fingerprint: HeadFingerprintFn) -> Self {
        Self::with_fingerprints(
            backend,
            head_fingerprint,
            Arc::new(|_| Some(MetadataFingerprint::default())),
        )
    }

    fn with_fingerprints(
        backend: FakeBackend,
        head_fingerprint: HeadFingerprintFn,
        metadata_fingerprint: MetadataFingerprintFn,
    ) -> Self {
        let backend = Arc::new(backend);
        let clock = ManualClock::new();
        let watcher = RepositoryStatusWatcher::with_options(StatusWatcherOptions {
            backend: backend.clone(),
            head_fingerprint,
            metadata_fingerprint,
            timers: WatchTimers::Manual(clock.clone()),
            ..StatusWatcherOptions::default()
        });
        Self {
            watcher,
            clock,
            backend,
            emitted: Arc::default(),
        }
    }

    /// `createFsWatchHarness()`: vigilancia nativa simulada sobre `/tmp/demo`.
    fn fs_watch() -> Self {
        let harness = Self::new(FakeBackend::default());
        assert_eq!(harness.start("/tmp/demo"), WatchMode::FsWatch);
        harness
    }

    fn start(&self, repo_path: &str) -> WatchMode {
        let emitted = self.emitted.clone();
        self.watcher
            .start(repo_path, move |change| lock(&emitted).push(change))
    }

    /// `vi.advanceTimersByTimeAsync(ms)`.
    fn advance(&self, millis: u64) {
        self.clock.advance(Duration::from_millis(millis));
        self.watcher.run_due_timers();
    }

    fn fire(&self, filename: &str) {
        self.backend.sink().event(filename);
    }

    fn emitted(&self) -> Vec<(String, WatchReason)> {
        lock(&self.emitted)
            .iter()
            .map(|change| (change.repo_path.clone(), change.reason))
            .collect()
    }
}

fn demo(reason: WatchReason) -> Vec<(String, WatchReason)> {
    vec![("/tmp/demo".to_string(), reason)]
}

#[test]
fn ignores_changes_inside_internal_directories() {
    assert!(should_ignore_watch_path(".git/index.lock"));
    assert!(!should_ignore_watch_path(".git/HEAD"));
    assert!(!should_ignore_watch_path(".git/refs/heads/main"));
    // `node_modules` y compañía los descarta la vigilancia, que sabe si hay
    // algo versionado dentro (ver `ignore_rules`), no la clasificación.
    assert!(!should_ignore_watch_path("node_modules/pkg/index.js"));
    assert!(!should_ignore_watch_path("src/App.vue"));
    assert_eq!(classify_watch_path(".git/packed-refs"), WatchPathKind::Head);
    assert_eq!(
        classify_watch_path("src/App.vue"),
        WatchPathKind::WorkingTree
    );
}

#[test]
fn classifies_nested_repositories_windows_separators_and_unknown_paths() {
    assert_eq!(classify_watch_path(""), WatchPathKind::WorkingTree);
    assert_eq!(classify_watch_path(".git"), WatchPathKind::Ignore);
    assert_eq!(classify_watch_path("sub/.git/HEAD"), WatchPathKind::Head);
    assert_eq!(
        classify_watch_path("sub/.git/refs/heads/feature/x"),
        WatchPathKind::Head
    );
    assert_eq!(
        classify_watch_path("sub/.git/index"),
        WatchPathKind::WorkingTree
    );
    assert_eq!(
        classify_watch_path(".git/index"),
        WatchPathKind::WorkingTree
    );
    assert_eq!(
        classify_watch_path(".git/index.lock"),
        WatchPathKind::Ignore
    );
    assert_eq!(
        classify_watch_path(".git\\refs\\heads\\main"),
        WatchPathKind::Head
    );
    assert_eq!(
        classify_watch_path("sub\\.git\\objects\\ab"),
        WatchPathKind::Ignore
    );
    assert_eq!(classify_watch_path("dist"), WatchPathKind::WorkingTree);
    assert_eq!(
        classify_watch_path("outline.md"),
        WatchPathKind::WorkingTree
    );
    assert_eq!(classify_watch_path("src/dist"), WatchPathKind::WorkingTree);
    assert_eq!(WatchPathKind::Head.as_str(), "head");
    assert_eq!(WatchPathKind::WorkingTree.as_str(), "working-tree");
    assert_eq!(WatchPathKind::Ignore.as_str(), "ignore");
}

#[test]
fn emits_a_debounced_fs_watch_event_for_tracked_and_untracked_file_changes() {
    let harness = Harness::fs_watch();

    harness.fire("src/App.vue");
    harness.fire("src/NewFile.vue");

    assert!(harness.emitted().is_empty());

    harness.advance(399);
    assert!(harness.emitted().is_empty());

    harness.advance(1);
    assert_eq!(harness.emitted(), demo(WatchReason::FsWatchWorkingTree));
}

#[test]
fn every_new_event_restarts_the_debounce() {
    let harness = Harness::fs_watch();

    harness.fire("src/App.vue");
    harness.advance(300);
    harness.fire("src/App.vue");
    harness.advance(300);
    assert!(harness.emitted().is_empty());

    harness.advance(100);
    assert_eq!(harness.emitted(), demo(WatchReason::FsWatchWorkingTree));
}

#[test]
fn emits_a_head_specific_fs_watch_event_for_relevant_git_metadata() {
    let harness = Harness::fs_watch();

    harness.fire(".git/HEAD");
    harness.fire(".git/refs/heads/main");

    harness.advance(400);
    assert_eq!(harness.emitted(), demo(WatchReason::FsWatchHead));
}

#[test]
fn a_head_change_wins_over_working_tree_changes_in_the_same_window() {
    let harness = Harness::fs_watch();

    harness.fire("src/App.vue");
    harness.fire(".git/HEAD");
    harness.fire("src/Other.vue");

    harness.advance(400);
    assert_eq!(harness.emitted(), demo(WatchReason::FsWatchHead));
}

#[test]
fn merges_queued_reasons_like_the_original() {
    use WatchReason::{FsWatchHead, FsWatchWorkingTree, PollHead, PollWorkingTree};

    assert_eq!(merge_queued_reason(None, PollHead), PollHead);
    assert_eq!(merge_queued_reason(Some(PollHead), PollHead), PollHead);
    assert_eq!(
        merge_queued_reason(Some(PollWorkingTree), PollHead),
        PollHead
    );
    assert_eq!(
        merge_queued_reason(Some(PollHead), FsWatchWorkingTree),
        FsWatchHead
    );
    assert_eq!(
        merge_queued_reason(Some(FsWatchWorkingTree), PollWorkingTree),
        FsWatchWorkingTree
    );
    assert_eq!(
        merge_queued_reason(Some(PollWorkingTree), PollWorkingTree),
        PollWorkingTree
    );
    assert_eq!(
        merge_queued_reason(Some(FsWatchHead), FsWatchWorkingTree),
        FsWatchHead
    );
    assert!(FsWatchHead.is_head_change() && PollHead.is_head_change());
    assert!(!FsWatchWorkingTree.is_head_change() && !PollWorkingTree.is_head_change());
}

#[test]
fn does_not_emit_for_ignored_fs_watch_paths() {
    let harness = Harness::fs_watch();

    harness.fire(".git/index.lock");
    harness.fire(".git/objects/ab/cdef");

    harness.advance(500);
    assert!(harness.emitted().is_empty());
}

#[test]
fn falls_back_to_polling_when_the_watcher_cannot_start() {
    let (head_fingerprint, _calls) = fingerprints(&["head-1", "head-2"], None);
    let harness = Harness::with_fingerprint(
        FakeBackend::failing("watch not supported"),
        head_fingerprint,
    );

    assert_eq!(harness.start("/tmp/demo"), WatchMode::Poll);
    assert_eq!(harness.watcher.mode(), WatchMode::Poll);

    harness.advance(2000);
    harness.advance(400);

    assert_eq!(harness.emitted(), demo(WatchReason::PollHead));
}

#[test]
fn prunes_what_should_not_even_be_watched() {
    assert!(!should_prune_watch_path(""));
    assert!(!should_prune_watch_path("src"));
    assert!(!should_prune_watch_path("src/App.vue"));
    assert!(should_prune_watch_path("node_modules"));
    assert!(should_prune_watch_path("packages/app/node_modules"));
    assert!(should_prune_watch_path("out"));
    // De .git solo interesa lo que mueve HEAD; hay que poder llegar hasta ahí.
    assert!(!should_prune_watch_path(".git"));
    assert!(!should_prune_watch_path(".git/HEAD"));
    assert!(!should_prune_watch_path(".git/packed-refs"));
    assert!(!should_prune_watch_path(".git/refs"));
    assert!(!should_prune_watch_path(".git/refs/heads"));
    assert!(!should_prune_watch_path(".git/refs/heads/feature/x"));
    assert!(should_prune_watch_path(".git/objects"));
    // Tags y ramas remotas también (remotos y tags creados en la terminal).
    assert!(!should_prune_watch_path(".git/refs/remotes"));
    assert!(!should_prune_watch_path(".git/refs/remotes/origin"));
    assert!(!should_prune_watch_path(".git/refs/tags"));
    assert!(!should_prune_watch_path(".git/refs/tags/release/v1"));
    assert!(!should_prune_watch_path(".git/config"));
    assert!(should_prune_watch_path(".git/refs/notes"));
    assert!(should_prune_watch_path(".git/logs/refs/remotes"));
    assert!(should_prune_watch_path(".git/logs/refs/heads"));
    // El reflog del stash (`git stash push` / `drop`) sí.
    assert!(!should_prune_watch_path(".git/logs"));
    assert!(!should_prune_watch_path(".git/logs/refs"));
    assert!(!should_prune_watch_path(".git/index"));
    assert!(should_prune_watch_path(".git/index.lock"));
    assert!(!should_prune_watch_path("sub/.git/HEAD"));
    assert!(should_prune_watch_path("sub/.git/logs/refs/heads"));
    assert!(should_prune_watch_path("sub\\.git\\objects"));
}

#[test]
fn starts_the_backend_on_the_repository_and_reports_fs_watch_mode() {
    let harness = Harness::new(FakeBackend::default());

    assert_eq!(harness.start("/tmp/demo"), WatchMode::FsWatch);
    assert_eq!(harness.watcher.mode(), WatchMode::FsWatch);
    assert_eq!(
        harness.backend.watched_paths(),
        [PathBuf::from("/tmp/demo")]
    );

    harness.fire("src/App.vue");
    harness.advance(400);

    assert_eq!(harness.emitted(), demo(WatchReason::FsWatchWorkingTree));
}

#[test]
fn prunes_the_tree_on_linux_and_watches_recursively_where_it_is_one_handle() {
    let expected = if cfg!(any(target_os = "macos", target_os = "windows")) {
        WatchStrategy::Recursive
    } else {
        WatchStrategy::PrunedTree
    };

    assert_eq!(WatchStrategy::for_current_platform(), expected);
    assert_eq!(NotifyBackend::new().strategy(), expected);
    assert_eq!(
        NotifyBackend::with_strategy(WatchStrategy::Recursive).strategy(),
        WatchStrategy::Recursive
    );
}

#[test]
fn falls_back_to_polling_when_the_watcher_runs_out_of_watches() {
    let (head_fingerprint, _calls) = fingerprints(&["head-1", "head-2"], None);
    let harness = Harness::with_fingerprint(
        FakeBackend::failing("ENOSPC: System limit for number of file watchers reached"),
        head_fingerprint,
    );

    assert_eq!(harness.start("/tmp/demo"), WatchMode::Poll);

    // El tick del sondeo y su debounce vencen dentro del mismo avance.
    harness.advance(2400);
    assert_eq!(harness.emitted(), demo(WatchReason::PollHead));
}

#[test]
fn falls_back_to_polling_when_the_watcher_fails_after_starting() {
    let (head_fingerprint, _calls) = fingerprints(&[], Some("same-head"));
    let harness = Harness::with_fingerprint(FakeBackend::default(), head_fingerprint);
    assert_eq!(harness.start("/tmp/demo"), WatchMode::FsWatch);

    harness.backend.sink().error(WatchError::new("EMFILE"));

    assert_eq!(harness.watcher.mode(), WatchMode::Poll);
    assert_eq!(harness.backend.closed(), 1);

    harness.advance(2400);
    assert_eq!(harness.emitted(), demo(WatchReason::PollWorkingTree));

    // Un segundo fallo no reinicia el intervalo del sondeo.
    harness.backend.sink().error(WatchError::new("EMFILE"));
    harness.advance(1600);
    assert_eq!(harness.emitted().len(), 1);
    harness.advance(400);
    assert_eq!(harness.emitted().len(), 2);
}

#[test]
fn emits_a_polling_working_tree_event_when_head_metadata_did_not_change() {
    let (head_fingerprint, _calls) = fingerprints(&[], Some("same-head"));
    let harness = Harness::with_fingerprint(
        FakeBackend::failing("watch not supported"),
        head_fingerprint,
    );
    harness.start("/tmp/demo");

    harness.advance(2000);
    harness.advance(400);

    assert_eq!(harness.emitted(), demo(WatchReason::PollWorkingTree));
}

#[test]
fn pauses_polling_while_the_window_is_unfocused() {
    let (head_fingerprint, calls) = fingerprints(&[], Some("same-head"));
    let harness = Harness::with_fingerprint(
        FakeBackend::failing("watch not supported"),
        head_fingerprint,
    );
    harness.start("/tmp/demo");
    harness.watcher.set_focused(false);

    harness.advance(3000);
    assert!(harness.emitted().is_empty());
    // Sin foco ni siquiera se lee el disco.
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    harness.watcher.set_focused(true);
    harness.advance(2000);
    harness.advance(400);

    assert_eq!(harness.emitted(), demo(WatchReason::PollWorkingTree));
}

#[test]
fn losing_focus_does_not_pause_the_native_watcher() {
    let harness = Harness::fs_watch();
    harness.watcher.set_focused(false);

    harness.fire("src/App.vue");
    harness.advance(400);

    assert_eq!(harness.emitted(), demo(WatchReason::FsWatchWorkingTree));
}

#[test]
fn discards_fs_watch_events_fired_while_an_in_app_mutation_is_running() {
    let harness = Harness::fs_watch();

    harness.watcher.begin_mutation("/tmp/demo");
    harness.fire("src/App.vue");
    harness.advance(1000);
    assert!(harness.emitted().is_empty());

    harness.watcher.end_mutation("/tmp/demo");
    // Pasada la ventana de cola, los eventos externos vuelven a emitirse.
    harness.advance(700);
    harness.fire("src/App.vue");
    harness.advance(400);
    assert_eq!(harness.emitted(), demo(WatchReason::FsWatchWorkingTree));
}

#[test]
fn discards_events_inside_the_trailing_window_after_a_mutation_ends() {
    let harness = Harness::fs_watch();

    harness.watcher.begin_mutation("/tmp/demo");
    harness.watcher.end_mutation("/tmp/demo");

    harness.advance(500);
    harness.fire("src/App.vue");
    harness.advance(1000);
    assert!(harness.emitted().is_empty());
}

#[test]
fn still_emits_a_pre_mutation_external_event_whose_debounce_was_already_armed() {
    let harness = Harness::fs_watch();

    harness.fire("src/App.vue");
    harness.watcher.begin_mutation("/tmp/demo");

    harness.advance(400);
    assert_eq!(harness.emitted(), demo(WatchReason::FsWatchWorkingTree));
}

#[test]
fn keeps_suppressing_while_nested_mutations_are_still_open() {
    let harness = Harness::fs_watch();

    harness.watcher.begin_mutation("/tmp/demo");
    harness.watcher.begin_mutation("/tmp/demo");
    harness.watcher.end_mutation("/tmp/demo");

    harness.fire("src/App.vue");
    harness.advance(1000);
    assert!(harness.emitted().is_empty());
}

#[test]
fn ignores_begin_and_end_mutation_calls_for_a_different_repository() {
    let harness = Harness::fs_watch();

    harness.watcher.begin_mutation("/tmp/other");
    harness.fire("src/App.vue");
    harness.advance(400);
    assert_eq!(harness.emitted(), demo(WatchReason::FsWatchWorkingTree));
}

#[test]
fn matches_the_mutated_repository_by_its_resolved_path() {
    let harness = Harness::fs_watch();

    harness.watcher.begin_mutation("/tmp/demo/sub/..");
    harness.fire("src/App.vue");
    harness.advance(1000);
    assert!(harness.emitted().is_empty());

    harness.watcher.end_mutation("/tmp/demo/");
    harness.advance(700);
    harness.fire("src/App.vue");
    harness.advance(400);
    assert_eq!(harness.emitted(), demo(WatchReason::FsWatchWorkingTree));
}

#[test]
fn clears_suppression_state_on_stop_and_start() {
    let harness = Harness::new(FakeBackend::default());

    harness.start("/tmp/demo");
    harness.watcher.begin_mutation("/tmp/demo");
    harness.watcher.stop();
    assert_eq!(harness.watcher.mode(), WatchMode::Stopped);
    assert_eq!(harness.backend.closed(), 1);

    harness.start("/tmp/demo");
    harness.fire("src/App.vue");
    harness.advance(400);
    assert_eq!(harness.emitted(), demo(WatchReason::FsWatchWorkingTree));
}

#[test]
fn refreshes_the_head_fingerprint_after_a_mutation_so_poll_mode_does_not_report_a_ghost_head_change()
 {
    // start: head-1 · refresh en end_mutation: head-2 · tick del poll: head-2 (sin cambio)
    let (head_fingerprint, _calls) = fingerprints(&["head-1"], Some("head-2"));
    let harness = Harness::with_fingerprint(
        FakeBackend::failing("watch not supported"),
        head_fingerprint,
    );
    harness.start("/tmp/demo");

    harness.watcher.begin_mutation("/tmp/demo");
    harness.watcher.end_mutation("/tmp/demo");

    harness.advance(2000);
    harness.advance(400);

    // El tick incondicional del fallback sigue emitiendo, pero NO como cambio de HEAD.
    assert_eq!(harness.emitted(), demo(WatchReason::PollWorkingTree));
}

#[test]
fn stopping_cancels_a_pending_notification_and_ignores_the_old_watcher() {
    let harness = Harness::fs_watch();
    let stale = harness.backend.sink();

    harness.fire("src/App.vue");
    harness.watcher.stop();
    harness.advance(1000);
    assert!(harness.emitted().is_empty());

    // Lo que aún llegue de la vigilancia anterior no cuenta para la nueva.
    harness.start("/tmp/other");
    stale.event("src/App.vue");
    stale.error(WatchError::new("late failure"));
    harness.advance(3000);
    assert!(harness.emitted().is_empty());
    assert_eq!(harness.watcher.mode(), WatchMode::FsWatch);
}

#[test]
fn starting_without_a_path_only_stops() {
    let harness = Harness::fs_watch();

    assert_eq!(harness.start(""), WatchMode::Stopped);

    assert_eq!(harness.watcher.mode(), WatchMode::Stopped);
    assert_eq!(harness.backend.closed(), 1);
    assert_eq!(harness.backend.watched_paths().len(), 1);
}

#[test]
fn dropping_the_watcher_closes_the_native_watcher() {
    let harness = Harness::fs_watch();
    let backend = harness.backend.clone();
    let stale = backend.sink();

    drop(harness);

    assert_eq!(backend.closed(), 1);
    // Un backend que sobreviva al watcher ya no tiene a quién avisar.
    stale.event("src/App.vue");
    stale.error(WatchError::new("late failure"));
}

#[test]
fn reads_the_status_with_the_injected_closure_on_every_notification() {
    let harness = Harness::new(FakeBackend::default());
    let reads = Arc::new(AtomicUsize::new(0));
    let seen = Arc::new(Mutex::new(Vec::new()));

    let mode = harness.watcher.start_with_status(
        "/tmp/demo",
        {
            let reads = reads.clone();
            move |repo_path: &Path| {
                let read = reads.fetch_add(1, Ordering::SeqCst) + 1;
                format!("{}#{read}", repo_path.display())
            }
        },
        {
            let seen = seen.clone();
            move |change, status| lock(&seen).push((change.reason, status))
        },
    );
    assert_eq!(mode, WatchMode::FsWatch);
    assert_eq!(reads.load(Ordering::SeqCst), 0);

    harness.fire("src/App.vue");
    harness.advance(400);
    harness.fire(".git/HEAD");
    harness.advance(400);

    assert_eq!(
        *lock(&seen),
        [
            (WatchReason::FsWatchWorkingTree, "/tmp/demo#1".to_string()),
            (WatchReason::FsWatchHead, "/tmp/demo#2".to_string()),
        ]
    );
}

#[test]
fn serializes_payloads_with_the_original_field_names() {
    let change = StatusChange {
        repo_path: "/tmp/demo".to_string(),
        reason: WatchReason::FsWatchWorkingTree,
    };
    assert_eq!(
        serde_json::to_value(&change).unwrap(),
        serde_json::json!({ "repoPath": "/tmp/demo", "reason": "fs-watch-working-tree" })
    );

    for reason in [
        WatchReason::FsWatchHead,
        WatchReason::FsWatchWorkingTree,
        WatchReason::PollHead,
        WatchReason::PollWorkingTree,
    ] {
        assert_eq!(serde_json::to_value(reason).unwrap(), reason.as_str());
    }
    for mode in [WatchMode::FsWatch, WatchMode::Poll, WatchMode::Stopped] {
        assert_eq!(serde_json::to_value(mode).unwrap(), mode.as_str());
    }
    assert_eq!(WatchMode::FsWatch.as_str(), "fs-watch");
}

#[test]
fn the_watcher_is_shareable_between_threads() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<RepositoryStatusWatcher>();
}

// --- Huella de HEAD -------------------------------------------------------

fn init_git_dir(root: &Path) {
    fs::create_dir_all(root.join(".git/refs/heads")).unwrap();
    fs::write(root.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
}

#[test]
fn the_head_fingerprint_follows_head_packed_refs_and_local_branches() {
    let repo = tempfile::tempdir().unwrap();
    init_git_dir(repo.path());

    let initial = get_head_fingerprint(repo.path()).unwrap();
    assert!(initial.contains("packed-refs:missing"));
    assert_eq!(get_head_fingerprint(repo.path()).unwrap(), initial);

    // Lo que no mueve HEAD no cambia la huella.
    fs::create_dir_all(repo.path().join(".git/refs/remotes/origin")).unwrap();
    fs::write(repo.path().join(".git/refs/remotes/origin/main"), "abc\n").unwrap();
    fs::write(repo.path().join(".git/index"), "index").unwrap();
    fs::write(repo.path().join("file.txt"), "working tree").unwrap();
    assert_eq!(get_head_fingerprint(repo.path()).unwrap(), initial);

    fs::create_dir_all(repo.path().join(".git/refs/heads/feature")).unwrap();
    fs::write(repo.path().join(".git/refs/heads/feature/x"), "abc\n").unwrap();
    let with_branch = get_head_fingerprint(repo.path()).unwrap();
    assert_ne!(with_branch, initial);

    fs::write(repo.path().join(".git/HEAD"), "ref: refs/heads/feature/x\n").unwrap();
    let after_checkout = get_head_fingerprint(repo.path()).unwrap();
    assert_ne!(after_checkout, with_branch);

    fs::write(repo.path().join(".git/packed-refs"), "# pack-refs\n").unwrap();
    assert_ne!(get_head_fingerprint(repo.path()).unwrap(), after_checkout);
}

#[test]
fn the_head_fingerprint_resolves_a_gitdir_file() {
    let root = tempfile::tempdir().unwrap();
    let git_dir = root.path().join("main/.git/worktrees/linked");
    fs::create_dir_all(&git_dir).unwrap();
    fs::write(git_dir.join("HEAD"), "ref: refs/heads/linked\n").unwrap();
    let worktree = root.path().join("linked");
    fs::create_dir_all(&worktree).unwrap();
    fs::write(
        worktree.join(".git"),
        "gitdir: ../main/.git/worktrees/linked\n",
    )
    .unwrap();

    assert_eq!(fingerprint::resolve_git_dir(&worktree), git_dir);
    let initial = get_head_fingerprint(&worktree).unwrap();
    let head = fingerprint::resolve_git_dir(&worktree).join("HEAD");
    assert!(initial.contains(&head.display().to_string()));

    fs::write(git_dir.join("HEAD"), "ref: refs/heads/another-branch\n").unwrap();
    assert_ne!(get_head_fingerprint(&worktree).unwrap(), initial);
}

#[test]
fn the_head_fingerprint_of_a_missing_repository_is_stable() {
    let root = tempfile::tempdir().unwrap();
    let missing = root.path().join("nope");

    assert_eq!(get_head_fingerprint(Path::new("")), None);
    assert_eq!(fingerprint::resolve_git_dir(&missing), missing.join(".git"));
    let fingerprint = get_head_fingerprint(&missing).unwrap();
    assert_eq!(fingerprint.matches(":missing").count(), 3);
    assert_eq!(get_head_fingerprint(&missing).unwrap(), fingerprint);
}

// --- Vigilancia real con `notify` -----------------------------------------

fn normalized(path: &Path) -> String {
    // En Windows las rutas relativas llegan con barras invertidas;
    // `classify_watch_path` ya las normaliza, aquí solo se comparan rutas.
    path.to_string_lossy().replace('\\', "/")
}

fn collecting_sink() -> (WatchSink, Receiver<Result<String, String>>) {
    let (sender, receiver) = mpsc::channel();
    let errors: Sender<Result<String, String>> = sender.clone();
    let sink = WatchSink::new(
        move |relative_path| {
            let _ = sender.send(Ok(normalized(relative_path)));
        },
        move |error| {
            let _ = errors.send(Err(error.to_string()));
        },
    );
    (sink, receiver)
}

/// Espera a que el backend informe `expected`; devuelve todo lo visto hasta ahí.
fn wait_for_path(receiver: &Receiver<Result<String, String>>, expected: &str) -> Vec<String> {
    let deadline = Instant::now() + WAIT;
    let mut seen = Vec::new();
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match receiver.recv_timeout(remaining) {
            Ok(Ok(path)) => {
                let found = path == expected;
                seen.push(path);
                if found {
                    return seen;
                }
            }
            Ok(Err(error)) => panic!("the watcher failed: {error}"),
            Err(_) => panic!("never saw {expected}; saw {seen:?}"),
        }
    }
}

fn watch_fixture() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("src")).unwrap();
    fs::create_dir_all(root.path().join("node_modules/pkg")).unwrap();
    fs::create_dir_all(root.path().join(".git/objects")).unwrap();
    fs::create_dir_all(root.path().join(".git/refs/heads")).unwrap();
    root
}

// La vigilancia podada necesita un backend barato por directorio (inotify).
#[cfg(target_os = "linux")]
#[test]
fn reports_working_tree_changes_with_repo_relative_paths_and_skips_pruned_directories() {
    let root = watch_fixture();
    let (sink, seen) = collecting_sink();
    let backend = NotifyBackend::with_strategy(WatchStrategy::PrunedTree);
    let watching = backend.watch(root.path(), sink).unwrap();

    fs::write(root.path().join("src/a.js"), "a").unwrap();
    fs::write(root.path().join("node_modules/pkg/index.js"), "x").unwrap();
    fs::write(root.path().join(".git/objects/ab"), "x").unwrap();
    fs::write(root.path().join(".git/index.lock"), "x").unwrap();
    fs::write(root.path().join(".git/index"), "x").unwrap();
    fs::write(root.path().join(".git/HEAD"), "ref: refs/heads/main").unwrap();
    // Lo último escrito: al verlo, todo lo anterior ya pasó por el backend.
    let seen = wait_for_path(&seen, ".git/HEAD");
    drop(watching);

    assert!(seen.contains(&"src/a.js".to_string()), "{seen:?}");
    assert!(!seen.iter().any(|path| path.starts_with("node_modules")));
    assert!(!seen.iter().any(|path| path.starts_with(".git/objects")));
    assert!(!seen.iter().any(|path| path == ".git/index.lock"));
    // `git add` / `git reset` de fuera de la app.
    assert!(seen.contains(&".git/index".to_string()), "{seen:?}");
}

#[cfg(target_os = "linux")]
#[test]
fn watches_directories_created_after_starting_but_not_pruned_ones() {
    let root = watch_fixture();
    let (sink, seen) = collecting_sink();
    let backend = NotifyBackend::with_strategy(WatchStrategy::PrunedTree);
    let watching = backend.watch(root.path(), sink).unwrap();

    // El aviso de un directorio nuevo llega cuando ya está vigilado.
    fs::create_dir(root.path().join("packages")).unwrap();
    wait_for_path(&seen, "packages");
    fs::create_dir(root.path().join("packages/app")).unwrap();
    wait_for_path(&seen, "packages/app");
    fs::create_dir(root.path().join("packages/app/dist")).unwrap();
    fs::write(root.path().join("packages/app/dist/bundle.js"), "x").unwrap();
    fs::write(root.path().join("packages/app/index.js"), "x").unwrap();
    let seen_in_packages = wait_for_path(&seen, "packages/app/index.js");
    assert!(
        !seen_in_packages.iter().any(|path| path.contains("dist")),
        "{seen_in_packages:?}"
    );

    // Una rama nueva dentro de un espacio de nombres nuevo también se ve.
    fs::create_dir(root.path().join(".git/refs/heads/feature")).unwrap();
    wait_for_path(&seen, ".git/refs/heads/feature");
    fs::write(root.path().join(".git/refs/heads/feature/x"), "abc").unwrap();
    wait_for_path(&seen, ".git/refs/heads/feature/x");

    // Un directorio movido deja de informar con su ruta vieja.
    fs::rename(root.path().join("src"), root.path().join("lib")).unwrap();
    wait_for_path(&seen, "lib");
    fs::write(root.path().join("lib/moved.js"), "x").unwrap();
    wait_for_path(&seen, "lib/moved.js");

    drop(watching);
}

#[test]
fn the_platform_watcher_reports_working_tree_and_head_changes() {
    let root = watch_fixture();
    let (sink, seen) = collecting_sink();
    let watching = NotifyBackend::new().watch(root.path(), sink).unwrap();

    fs::write(root.path().join("src/a.js"), "a").unwrap();
    wait_for_path(&seen, "src/a.js");
    fs::write(root.path().join(".git/HEAD"), "ref: refs/heads/main").unwrap();
    wait_for_path(&seen, ".git/HEAD");
    fs::write(root.path().join(".git/refs/heads/main"), "abc").unwrap();
    wait_for_path(&seen, ".git/refs/heads/main");

    drop(watching);
}

#[test]
fn the_platform_watcher_cannot_start_on_a_missing_directory() {
    let root = tempfile::tempdir().unwrap();
    let (sink, _seen) = collecting_sink();

    let result = NotifyBackend::new().watch(&root.path().join("missing"), sink);

    assert!(result.is_err());
}

/// Vigilancia real que además deja ver a la prueba qué rutas ya pasaron por
/// el watcher, para no depender de esperas.
struct TappedBackend {
    inner: NotifyBackend,
    tap: Sender<Result<String, String>>,
}

impl WatchBackend for TappedBackend {
    fn watch(&self, repo_path: &Path, sink: WatchSink) -> Result<Box<dyn WatchHandle>, WatchError> {
        let events = self.tap.clone();
        let errors = events.clone();
        let (forward_events, forward_errors) = (sink.clone(), sink);
        self.inner.watch(
            repo_path,
            WatchSink::new(
                move |relative_path| {
                    forward_events.event(relative_path);
                    let _ = events.send(Ok(normalized(relative_path)));
                },
                move |error| {
                    let _ = errors.send(Err(error.to_string()));
                    forward_errors.error(error);
                },
            ),
        )
    }
}

/// Barrera: crea una ruta que el backend informa pero el watcher ignora
/// (`<dir>/.git/refs`) y espera a verla. inotify entrega en orden, así que
/// al volver todo lo escrito antes ya pasó por el watcher.
#[cfg(target_os = "linux")]
fn sync_watcher(root: &Path, seen: &Receiver<Result<String, String>>, barrier: usize) {
    let relative = format!("sync-{barrier}/.git/refs");
    assert_eq!(classify_watch_path(&relative), WatchPathKind::Ignore);
    fs::create_dir(root.join(&relative)).unwrap();
    wait_for_path(seen, &relative);
}

// Solo en linux hay un orden de entrega que permita la barrera.
#[cfg(target_os = "linux")]
#[test]
fn watches_a_real_repository_and_suppresses_the_mutations_of_the_gate() {
    use crate::mutation_gate::{MutationOptions, RepositoryMutationGate};

    let root = watch_fixture();
    init_git_dir(root.path());
    for barrier in 1..=3 {
        fs::create_dir_all(root.path().join(format!("sync-{barrier}/.git"))).unwrap();
    }
    let repo = root.path().to_path_buf();

    let (tap, seen) = mpsc::channel();
    let clock = ManualClock::new();
    let watcher = Arc::new(RepositoryStatusWatcher::with_options(
        StatusWatcherOptions {
            backend: Arc::new(TappedBackend {
                inner: NotifyBackend::new(),
                tap,
            }),
            timers: WatchTimers::Manual(clock.clone()),
            ..StatusWatcherOptions::default()
        },
    ));
    let gate = RepositoryMutationGate::builder()
        .watcher(watcher.clone())
        .build();
    let emitted = Arc::new(Mutex::new(Vec::new()));
    let advance = |millis: u64| {
        clock.advance(Duration::from_millis(millis));
        watcher.run_due_timers();
    };

    let mode = watcher.start(&repo, {
        let emitted = emitted.clone();
        move |change| lock(&emitted).push(change.reason)
    });
    assert_eq!(mode, WatchMode::FsWatch);

    // Un cambio externo del working tree.
    fs::write(repo.join("src/external.js"), "x").unwrap();
    sync_watcher(&repo, &seen, 1);
    advance(400);
    assert_eq!(*lock(&emitted), [WatchReason::FsWatchWorkingTree]);

    // Una mutación de la app: lo que escribe no provoca avisos.
    let written = gate.run_exclusive(&repo, None, MutationOptions::new(), {
        let repo = repo.clone();
        move || fs::write(repo.join("src/staged.js"), "x").is_ok()
    });
    assert_eq!(written, Ok(true));
    sync_watcher(&repo, &seen, 2);
    advance(400);
    assert_eq!(lock(&emitted).len(), 1);

    // Pasada la ventana de cola, un checkout externo se avisa como cambio de HEAD.
    advance(700);
    fs::write(repo.join("src/after.js"), "x").unwrap();
    fs::write(repo.join(".git/HEAD"), "ref: refs/heads/other\n").unwrap();
    sync_watcher(&repo, &seen, 3);
    advance(400);
    assert_eq!(
        *lock(&emitted),
        [WatchReason::FsWatchWorkingTree, WatchReason::FsWatchHead]
    );

    // Detenido, deja de vigilar el directorio.
    watcher.stop();
    fs::write(repo.join("src/ignored.js"), "x").unwrap();
    advance(1000);
    assert_eq!(lock(&emitted).len(), 2);
}

#[test]
fn the_tapped_backend_forwards_to_the_watcher() {
    let root = watch_fixture();
    let (tap, seen) = mpsc::channel();
    let watcher = RepositoryStatusWatcher::with_options(StatusWatcherOptions {
        debounce: Duration::from_millis(10),
        backend: Arc::new(TappedBackend {
            inner: NotifyBackend::new(),
            tap,
        }),
        ..StatusWatcherOptions::default()
    });
    let (changes, received) = mpsc::channel();

    // De punta a punta con todo real: vigilancia nativa, reloj e hilo propios.
    let mode = watcher.start(root.path(), move |change| {
        let _ = changes.send(change);
    });
    assert_eq!(mode, WatchMode::FsWatch);

    fs::write(root.path().join("src/a.js"), "a").unwrap();
    wait_for_path(&seen, "src/a.js");
    let change = received.recv_timeout(WAIT).unwrap();
    assert_eq!(change.repo_path, root.path().to_string_lossy());
    assert_eq!(change.reason, WatchReason::FsWatchWorkingTree);
}

#[test]
fn the_timer_thread_delivers_notifications_on_its_own() {
    let (head_fingerprint, _calls) = fingerprints(&["head-1", "head-2"], Some("head-2"));
    let backend = Arc::new(FakeBackend::default());
    let watcher = RepositoryStatusWatcher::with_options(StatusWatcherOptions {
        debounce: Duration::from_millis(10),
        poll_interval: Duration::from_millis(15),
        backend: backend.clone(),
        head_fingerprint,
        ..StatusWatcherOptions::default()
    });
    let (changes, received) = mpsc::channel();

    let mode = watcher.start("/tmp/demo", move |change| {
        let _ = changes.send(change.reason);
    });
    assert_eq!(mode, WatchMode::FsWatch);

    backend.sink().event("src/App.vue");
    assert_eq!(
        received.recv_timeout(WAIT),
        Ok(WatchReason::FsWatchWorkingTree)
    );

    // Sin vigilancia nativa, el propio hilo sondea.
    backend.sink().error(WatchError::new("EMFILE"));
    assert_eq!(received.recv_timeout(WAIT), Ok(WatchReason::PollHead));
    assert_eq!(
        received.recv_timeout(WAIT),
        Ok(WatchReason::PollWorkingTree)
    );

    drop(watcher);
    // Soltado el watcher, su hilo ya no existe y el receptor se cerró con él.
    let end = loop {
        if let Err(end) = received.recv_timeout(WAIT) {
            break end;
        }
    };
    assert_eq!(end, mpsc::RecvTimeoutError::Disconnected);
}

// --- Remotos y tags hechos fuera de la app --------------------------------

#[test]
fn classifies_the_config_and_the_tag_and_remote_refs() {
    assert_eq!(classify_watch_path(".git/config"), WatchPathKind::Config);
    assert_eq!(
        classify_watch_path("sub/.git/config"),
        WatchPathKind::Config
    );
    assert_eq!(classify_watch_path(".git\\config"), WatchPathKind::Config);
    // El lock de `git config` no cuenta: sí el renombrado final.
    assert_eq!(
        classify_watch_path(".git/config.lock"),
        WatchPathKind::Ignore
    );
    assert_eq!(
        classify_watch_path(".git/config.worktree"),
        WatchPathKind::Ignore
    );
    assert_eq!(
        classify_watch_path(".git/refs/tags/v1"),
        WatchPathKind::Refs
    );
    assert_eq!(classify_watch_path(".git/refs/tags"), WatchPathKind::Refs);
    assert_eq!(
        classify_watch_path(".git/refs/remotes/origin/main"),
        WatchPathKind::Refs
    );
    assert_eq!(
        classify_watch_path(".git\\refs\\remotes\\upstream\\feature\\x"),
        WatchPathKind::Refs
    );
    assert_eq!(
        classify_watch_path(".git/refs/tags/v1.lock"),
        WatchPathKind::Ignore
    );
    assert_eq!(
        classify_watch_path(".git/refs/notes/commits"),
        WatchPathKind::Ignore
    );
    // Un archivo del working tree llamado igual no es la configuración.
    assert_eq!(classify_watch_path("config"), WatchPathKind::WorkingTree);
    assert_eq!(
        classify_watch_path("refs/tags/x"),
        WatchPathKind::WorkingTree
    );
    assert!(is_packed_refs_path(".git/packed-refs"));
    assert!(!is_packed_refs_path(".git/HEAD"));
    assert_eq!(WatchPathKind::Refs.as_str(), "refs");
    assert_eq!(WatchPathKind::Config.as_str(), "config");
    assert_eq!(WatchReason::FsWatchRefs.as_str(), "fs-watch-refs");
    assert_eq!(WatchReason::FsWatchConfig.as_str(), "fs-watch-config");
    assert_eq!(WatchReason::PollRefs.as_str(), "poll-refs");
    assert_eq!(WatchReason::PollConfig.as_str(), "poll-config");
    assert!(WatchReason::FsWatchRefs.is_metadata() && WatchReason::PollConfig.is_metadata());
    assert!(!WatchReason::FsWatchHead.is_metadata());
    assert!(!WatchReason::FsWatchRefs.is_head_change());
}

#[test]
fn reports_config_and_ref_changes_apart_from_the_working_tree() {
    let harness = Harness::fs_watch();

    harness.fire("src/App.vue");
    harness.fire(".git/config");
    harness.fire(".git/refs/tags/v1");
    harness.fire(".git/refs/remotes/origin/main");
    harness.fire(".git/config");
    harness.advance(399);
    assert!(harness.emitted().is_empty());
    harness.advance(1);

    assert_eq!(
        harness.emitted(),
        [
            ("/tmp/demo".to_string(), WatchReason::FsWatchWorkingTree),
            ("/tmp/demo".to_string(), WatchReason::FsWatchConfig),
            ("/tmp/demo".to_string(), WatchReason::FsWatchRefs),
        ]
    );
}

#[test]
fn a_metadata_change_alone_does_not_report_a_working_tree_change() {
    let harness = Harness::fs_watch();

    harness.fire(".git/refs/tags/v1");
    harness.advance(400);
    assert_eq!(harness.emitted(), demo(WatchReason::FsWatchRefs));
}

#[test]
fn packed_refs_counts_as_a_head_and_a_refs_change() {
    let harness = Harness::fs_watch();

    harness.fire(".git/packed-refs");
    harness.advance(400);
    assert_eq!(
        harness.emitted(),
        [
            ("/tmp/demo".to_string(), WatchReason::FsWatchHead),
            ("/tmp/demo".to_string(), WatchReason::FsWatchRefs),
        ]
    );
}

#[test]
fn the_app_own_mutations_do_not_report_config_or_ref_changes() {
    let harness = Harness::fs_watch();

    harness.watcher.begin_mutation("/tmp/demo");
    harness.fire(".git/config");
    harness.fire(".git/refs/tags/v1");
    harness.watcher.end_mutation("/tmp/demo");
    harness.fire(".git/refs/remotes/origin/main");
    harness.advance(1000);
    assert!(harness.emitted().is_empty());
}

#[test]
fn polling_reports_tag_remote_and_config_changes_from_the_metadata_fingerprint() {
    let (head_fingerprint, _calls) = fingerprints(&[], Some("head"));
    let sequence = Mutex::new(VecDeque::from([
        // Al empezar.
        MetadataFingerprint {
            refs: "r1".into(),
            config: "c1".into(),
            stash: String::new(),
        },
        // Primer tick: sin cambios.
        MetadataFingerprint {
            refs: "r1".into(),
            config: "c1".into(),
            stash: String::new(),
        },
        // Segundo: un tag nuevo.
        MetadataFingerprint {
            refs: "r2".into(),
            config: "c1".into(),
            stash: String::new(),
        },
        // Tercero: un remoto nuevo.
        MetadataFingerprint {
            refs: "r2".into(),
            config: "c2".into(),
            stash: String::new(),
        },
    ]));
    let metadata: MetadataFingerprintFn = Arc::new(move |_| {
        let mut sequence = lock(&sequence);
        let next = sequence.pop_front();
        match next {
            Some(next) if sequence.is_empty() => {
                sequence.push_back(next.clone());
                Some(next)
            }
            next => next,
        }
    });
    let harness = Harness::with_fingerprints(
        FakeBackend::failing("watch not supported"),
        head_fingerprint,
        metadata,
    );
    assert_eq!(harness.start("/tmp/demo"), WatchMode::Poll);

    harness.advance(2000);
    harness.advance(400);
    assert_eq!(harness.emitted(), demo(WatchReason::PollWorkingTree));

    harness.advance(1600);
    harness.advance(400);
    assert_eq!(
        harness.emitted()[1..],
        [
            ("/tmp/demo".to_string(), WatchReason::PollWorkingTree),
            ("/tmp/demo".to_string(), WatchReason::PollRefs),
        ]
    );

    harness.advance(1600);
    harness.advance(400);
    assert_eq!(
        harness.emitted()[3..],
        [
            ("/tmp/demo".to_string(), WatchReason::PollWorkingTree),
            ("/tmp/demo".to_string(), WatchReason::PollConfig),
        ]
    );
}

#[test]
fn serializes_the_new_reasons() {
    let change = StatusChange {
        repo_path: "/tmp/demo".into(),
        reason: WatchReason::FsWatchConfig,
    };
    assert_eq!(
        serde_json::to_value(&change).unwrap(),
        serde_json::json!({ "repoPath": "/tmp/demo", "reason": "fs-watch-config" })
    );
}

#[cfg(target_os = "linux")]
#[test]
fn the_pruned_tree_reports_the_config_and_new_tag_and_remote_refs() {
    let root = watch_fixture();
    let (sink, seen) = collecting_sink();
    let backend = NotifyBackend::with_strategy(WatchStrategy::PrunedTree);
    let watching = backend.watch(root.path(), sink).unwrap();

    fs::write(root.path().join(".git/config"), "[core]\n").unwrap();
    wait_for_path(&seen, ".git/config");
    // `refs/tags` y `refs/remotes/<remoto>` aparecen después de empezar.
    fs::create_dir_all(root.path().join(".git/refs/tags")).unwrap();
    wait_for_path(&seen, ".git/refs/tags");
    fs::write(root.path().join(".git/refs/tags/v1"), "abc").unwrap();
    wait_for_path(&seen, ".git/refs/tags/v1");
    fs::create_dir_all(root.path().join(".git/refs/remotes")).unwrap();
    wait_for_path(&seen, ".git/refs/remotes");
    fs::create_dir(root.path().join(".git/refs/remotes/upstream")).unwrap();
    wait_for_path(&seen, ".git/refs/remotes/upstream");
    fs::write(root.path().join(".git/refs/remotes/upstream/main"), "abc").unwrap();
    wait_for_path(&seen, ".git/refs/remotes/upstream/main");

    drop(watching);
}

/// `git remote add` y `git tag` de verdad, desde la terminal, con la
/// vigilancia nativa: llegan como `fs-watch-config` y `fs-watch-refs`.
#[cfg(target_os = "linux")]
#[test]
fn a_real_repository_reports_remotes_and_tags_added_from_the_terminal() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().to_path_buf();
    let git = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .args(["-c", "user.name=T", "-c", "user.email=t@example.com"])
            .args(["-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false"])
            .args(args)
            .current_dir(&repo)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .expect("git");
        assert!(status.success(), "git {args:?}");
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["commit", "-q", "--allow-empty", "-m", "first"]);

    let watcher = RepositoryStatusWatcher::with_options(StatusWatcherOptions {
        debounce: Duration::from_millis(20),
        ..StatusWatcherOptions::default()
    });
    let (changes, received) = mpsc::channel();
    assert_eq!(
        watcher.start(&repo, move |change| {
            let _ = changes.send(change.reason);
        }),
        WatchMode::FsWatch
    );

    let wait_for = |expected: WatchReason| {
        let deadline = Instant::now() + WAIT;
        let mut seen = Vec::new();
        while let Ok(reason) =
            received.recv_timeout(deadline.saturating_duration_since(Instant::now()))
        {
            seen.push(reason);
            if reason == expected {
                return;
            }
        }
        panic!("never saw {expected:?}; saw {seen:?}");
    };

    git(&[
        "remote",
        "add",
        "upstream",
        "https://example.com/acme/web.git",
    ]);
    wait_for(WatchReason::FsWatchConfig);
    git(&["tag", "v1"]);
    wait_for(WatchReason::FsWatchRefs);
    git(&["update-ref", "refs/remotes/upstream/main", "HEAD"]);
    wait_for(WatchReason::FsWatchRefs);
    git(&["remote", "remove", "upstream"]);
    wait_for(WatchReason::FsWatchConfig);
}

#[test]
fn classifies_the_stash_and_its_reflog() {
    assert_eq!(classify_watch_path(".git/refs/stash"), WatchPathKind::Stash);
    assert_eq!(
        classify_watch_path(".git/logs/refs/stash"),
        WatchPathKind::Stash
    );
    assert_eq!(
        classify_watch_path("sub/.git/logs/refs/stash"),
        WatchPathKind::Stash
    );
    assert_eq!(
        classify_watch_path(".git\\logs\\refs\\stash"),
        WatchPathKind::Stash
    );
    assert_eq!(
        classify_watch_path(".git/refs/stash.lock"),
        WatchPathKind::Ignore
    );
    // El resto de reflogs no cuenta.
    assert_eq!(classify_watch_path(".git/logs/HEAD"), WatchPathKind::Ignore);
    assert_eq!(
        classify_watch_path(".git/logs/refs/heads/main"),
        WatchPathKind::Ignore
    );
    assert_eq!(classify_watch_path("stash"), WatchPathKind::WorkingTree);
    assert_eq!(WatchPathKind::Stash.as_str(), "stash");
    assert_eq!(WatchReason::FsWatchStash.as_str(), "fs-watch-stash");
    assert_eq!(WatchReason::PollStash.as_str(), "poll-stash");
    assert!(WatchReason::FsWatchStash.is_metadata() && WatchReason::PollStash.is_stash_change());
    assert!(!WatchReason::FsWatchRefs.is_stash_change());
}

#[test]
fn reports_stash_changes_apart_from_tags_and_the_working_tree() {
    let harness = Harness::fs_watch();

    harness.fire("src/App.vue");
    harness.fire(".git/refs/stash");
    harness.fire(".git/logs/refs/stash");
    harness.fire(".git/refs/tags/v1");
    harness.advance(400);

    assert_eq!(
        harness.emitted(),
        [
            ("/tmp/demo".to_string(), WatchReason::FsWatchWorkingTree),
            ("/tmp/demo".to_string(), WatchReason::FsWatchStash),
            ("/tmp/demo".to_string(), WatchReason::FsWatchRefs),
        ]
    );
}

#[test]
fn polling_reports_stash_changes_from_the_metadata_fingerprint() {
    let (head_fingerprint, _calls) = fingerprints(&[], Some("head"));
    let calls = AtomicUsize::new(0);
    let metadata: MetadataFingerprintFn = Arc::new(move |_| {
        // Al empezar y en el primer tick, igual; después, un stash nuevo.
        let call = calls.fetch_add(1, Ordering::SeqCst);
        Some(MetadataFingerprint {
            stash: if call < 2 { "s1" } else { "s2" }.into(),
            ..MetadataFingerprint::default()
        })
    });
    let harness = Harness::with_fingerprints(
        FakeBackend::failing("watch not supported"),
        head_fingerprint,
        metadata,
    );
    assert_eq!(harness.start("/tmp/demo"), WatchMode::Poll);

    harness.advance(2000);
    harness.advance(400);
    assert_eq!(harness.emitted(), demo(WatchReason::PollWorkingTree));
    harness.advance(1600);
    harness.advance(400);
    assert_eq!(
        harness.emitted()[1..],
        [
            ("/tmp/demo".to_string(), WatchReason::PollWorkingTree),
            ("/tmp/demo".to_string(), WatchReason::PollStash),
        ]
    );
}

/// `git stash` y `git stash drop` de verdad, con la vigilancia nativa: llegan
/// como `fs-watch-stash` (también el primer stash, que crea `logs/refs`).
#[cfg(target_os = "linux")]
#[test]
fn a_real_repository_reports_stashes_made_and_dropped_in_the_terminal() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().to_path_buf();
    let git = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .args(["-c", "user.name=T", "-c", "user.email=t@example.com"])
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .current_dir(&repo)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .expect("git");
        assert!(status.success(), "git {args:?}");
    };
    git(&["init", "-q", "-b", "main"]);
    fs::write(repo.join("a.txt"), "a\n").unwrap();
    git(&["add", "a.txt"]);
    git(&["commit", "-q", "-m", "first"]);

    let watcher = RepositoryStatusWatcher::with_options(StatusWatcherOptions {
        debounce: Duration::from_millis(20),
        ..StatusWatcherOptions::default()
    });
    let (changes, received) = mpsc::channel();
    assert_eq!(
        watcher.start(&repo, move |change| {
            let _ = changes.send(change.reason);
        }),
        WatchMode::FsWatch
    );
    let wait_for = |expected: WatchReason| {
        let deadline = Instant::now() + WAIT;
        let mut seen = Vec::new();
        while let Ok(reason) =
            received.recv_timeout(deadline.saturating_duration_since(Instant::now()))
        {
            seen.push(reason);
            if reason == expected {
                return;
            }
        }
        panic!("never saw {expected:?}; saw {seen:?}");
    };

    fs::write(repo.join("a.txt"), "changed\n").unwrap();
    git(&["stash", "push", "-q", "-m", "one"]);
    wait_for(WatchReason::FsWatchStash);
    fs::write(repo.join("a.txt"), "changed again\n").unwrap();
    git(&["stash", "push", "-q", "-m", "two"]);
    wait_for(WatchReason::FsWatchStash);
    // Borrar el de abajo solo reescribe el reflog.
    git(&["stash", "drop", "-q", "stash@{1}"]);
    wait_for(WatchReason::FsWatchStash);
    git(&["stash", "drop", "-q"]);
    wait_for(WatchReason::FsWatchStash);
}

// --- Reglas de ignorado del repositorio (`.gitignore`) ---------------------

/// Árbol con `.gitignore` en la raíz y en un subdirectorio, más
/// `.git/info/exclude`. No hace falta un repositorio de verdad: lo versionado
/// lo dice la prueba.
fn ignore_fixture() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let path = root.path();
    for directory in [
        "src",
        "target/debug",
        ".venv/lib",
        "build",
        "sub/generated",
        "generated",
        "secret",
        "node_modules/pkg",
        "dist",
        "out",
        ".git/info",
    ] {
        fs::create_dir_all(path.join(directory)).unwrap();
    }
    fs::write(
        path.join(".gitignore"),
        "target/\n.venv/\nbuild/\n*.log\n!keep.log\n",
    )
    .unwrap();
    fs::write(path.join("sub/.gitignore"), "generated/\n").unwrap();
    fs::write(path.join(".git/info/exclude"), "secret/\n").unwrap();
    root
}

#[test]
fn ignore_rules_prune_ignored_directories_unless_they_hold_tracked_files() {
    let root = ignore_fixture();
    let mut rules =
        ignore_rules::IgnoreRules::with_tracked(root.path(), &["build/keep.txt", "dist/app.js"]);
    let mut prune = |path: &str| rules.should_prune_directory(Path::new(path));

    // Lo que ignora el `.gitignore` de la raíz, el de un subdirectorio y
    // `info/exclude`.
    assert!(prune("target"));
    assert!(prune(".venv"));
    assert!(prune("sub/generated"));
    assert!(
        !prune("generated"),
        "sub/.gitignore only applies inside sub"
    );
    assert!(prune("secret"));
    // El respaldo, si no tiene nada versionado.
    assert!(prune("node_modules"));
    assert!(prune("out"));
    // Ignorado o del respaldo pero con algo versionado dentro: se vigila.
    assert!(!prune("build"));
    assert!(!prune("dist"));
    // Lo normal y `.git` (que decide `classify`).
    assert!(!prune("src"));
    assert!(!prune(""));
    assert!(!prune(".git"));
}

#[test]
fn ignore_rules_skip_events_that_cannot_change_the_status() {
    let root = ignore_fixture();
    let mut rules =
        ignore_rules::IgnoreRules::with_tracked(root.path(), &["build/keep.txt", "dist/app.js"]);
    let mut skip =
        |path: &str, is_dir: Option<bool>| rules.should_skip_event(Path::new(path), is_dir);

    assert!(skip("target/debug/app.o", Some(false)));
    assert!(skip("target", Some(true)));
    assert!(skip("app.log", Some(false)));
    assert!(skip("node_modules/pkg/index.js", Some(false)));
    // Borrado: sin saber si era carpeta, vale cualquiera de las dos.
    assert!(skip("target", None));

    assert!(!skip("src/main.rs", Some(false)));
    assert!(!skip("keep.log", Some(false)));
    assert!(!skip("generated/a.txt", Some(false)));
    // Lo versionado dentro de algo ignorado sí cuenta (también si se borra)…
    assert!(!skip("build/keep.txt", Some(false)));
    assert!(!skip("build/keep.txt", None));
    // …pero lo demás de esa carpeta ignorada no.
    assert!(skip("build/other.o", Some(false)));
    // En una carpeta del respaldo versionada, un archivo nuevo es un cambio.
    assert!(!skip("dist/new.js", Some(false)));
    assert!(!skip(".git/HEAD", Some(false)));
    assert!(!skip("", None));
}

#[test]
fn ignore_rules_follow_a_gitignore_edited_after_loading() {
    let root = ignore_fixture();
    let mut rules = ignore_rules::IgnoreRules::with_tracked(root.path(), &[]);
    assert!(!rules.should_prune_directory(Path::new("generated")));

    fs::write(root.path().join(".gitignore"), "generated/\n").unwrap();
    rules.reload_gitignore(Path::new(""));

    assert!(rules.should_prune_directory(Path::new("generated")));
    assert!(!rules.should_prune_directory(Path::new("target")));
}

/// Repositorio de verdad con `target/` en `.gitignore` y un `dist/`
/// versionado: la vigilancia podada no entra en `target/` (ni avisa de lo
/// que pasa ahí) y sí vigila `dist/`, aunque esté en el respaldo.
#[cfg(target_os = "linux")]
#[test]
fn a_real_repository_does_not_watch_what_its_gitignore_ignores() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().to_path_buf();
    let git = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .args(["-c", "user.name=T", "-c", "user.email=t@example.com"])
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .current_dir(&repo)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .expect("git");
        assert!(status.success(), "git {args:?}");
    };
    git(&["init", "-q", "-b", "main"]);
    fs::write(repo.join(".gitignore"), "target/\n").unwrap();
    fs::create_dir_all(repo.join("dist")).unwrap();
    fs::write(repo.join("dist/app.js"), "a\n").unwrap();
    fs::create_dir_all(repo.join("src")).unwrap();
    fs::write(repo.join("src/main.rs"), "fn main() {}\n").unwrap();
    git(&["add", ".gitignore", "dist/app.js", "src/main.rs"]);
    git(&["commit", "-q", "-m", "first"]);
    fs::create_dir_all(repo.join("target/debug/deps")).unwrap();

    let (sink, seen) = collecting_sink();
    let backend = NotifyBackend::with_strategy(WatchStrategy::PrunedTree);
    let watching = backend.watch(&repo, sink).unwrap();

    fs::write(repo.join("target/debug/deps/lib.rlib"), "x").unwrap();
    fs::write(repo.join("target/debug/app"), "x").unwrap();
    fs::create_dir(repo.join("target/release")).unwrap();
    fs::write(repo.join("dist/app.js"), "b\n").unwrap();
    // Lo último escrito: al verlo, todo lo anterior ya pasó por el backend.
    fs::write(repo.join("src/main.rs"), "fn main() { }\n").unwrap();
    let seen = wait_for_path(&seen, "src/main.rs");
    drop(watching);

    assert!(seen.contains(&"dist/app.js".to_string()), "{seen:?}");
    assert!(
        !seen.iter().any(|path| path.starts_with("target")),
        "{seen:?}"
    );
}

/// El mismo repositorio con el watcher completo: lo de `target/` no llega a
/// avisar de un cambio de estado; lo de `src/` sí.
#[cfg(target_os = "linux")]
#[test]
fn changes_inside_gitignored_directories_do_not_emit_status_changes() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().to_path_buf();
    let git = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .args(["-c", "user.name=T", "-c", "user.email=t@example.com"])
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .current_dir(&repo)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .expect("git");
        assert!(status.success(), "git {args:?}");
    };
    git(&["init", "-q", "-b", "main"]);
    fs::write(repo.join(".gitignore"), "target/\n").unwrap();
    fs::write(repo.join("a.txt"), "a\n").unwrap();
    git(&["add", ".gitignore", "a.txt"]);
    git(&["commit", "-q", "-m", "first"]);
    fs::create_dir_all(repo.join("target/debug")).unwrap();

    let watcher = RepositoryStatusWatcher::with_options(StatusWatcherOptions {
        debounce: Duration::from_millis(20),
        ..StatusWatcherOptions::default()
    });
    let (changes, received) = mpsc::channel();
    assert_eq!(
        watcher.start(&repo, move |change| {
            let _ = changes.send(change.reason);
        }),
        WatchMode::FsWatch
    );

    for index in 0..20 {
        fs::write(repo.join(format!("target/debug/{index}.o")), "x").unwrap();
    }
    assert!(
        received.recv_timeout(Duration::from_millis(400)).is_err(),
        "a build inside target/ must not refresh the status"
    );

    fs::write(repo.join("a.txt"), "changed\n").unwrap();
    assert_eq!(
        received.recv_timeout(WAIT).unwrap(),
        WatchReason::FsWatchWorkingTree
    );
}
