//! Fachada del backend: equivalente síncrono de `window.api`
//! (`electron/preload.js` + `electron/ipc/*`).
//!
//! [`Backend`] une los crates de backend y expone un método por canal IPC,
//! con el mismo nombre en snake_case, los mismos argumentos y la misma forma
//! de resultado (ver [`channels`] para la tabla canal → método):
//!
//! - [`Backend::git`] → [`GitApi`]: los canales `git:*`. Las lecturas delegan
//!   en `mergeon-git-query` y devuelven `Result` con el mensaje de
//!   `createReadError`. Las mutaciones pasan por la cola de `mergeon-runtime`
//!   igual que `registerRepoMutationHandler` y devuelven siempre un
//!   [`Mutation`], nunca un error.
//! - [`Backend::storage`] → `mergeon_store::Store` (canales `storage:*`).
//! - [`Backend::github`] → `mergeon_github::GitHubClient` (canales `github:*`).
//! - [`Backend::shell`] → [`Shell`] (canales `shell:*`).
//! - [`Backend::updater`] → [`UpdaterApi`] (canales `updater:*`), sobre el
//!   actualizador que se enchufe con [`UpdaterBridge`].
//!
//! Lo que el proceso principal empujaba al renderer (`git:status-changed`,
//! `git:operation-event`, `updater:event`) llega como [`BackendEvent`] al
//! callback fijado en [`BackendOptions::on_event`].
//!
//! Todo es síncrono y bloqueante: la interfaz llama desde hilos de fondo. Los
//! canales `dialog:*` y `app:*` no están aquí: son de la ventana.
//!
//! ```no_run
//! use std::sync::Arc;
//! use mergeon_backend::{Backend, BackendEvent, BackendOptions};
//!
//! let backend = Arc::new(
//!     Backend::new(BackendOptions::new().on_event(|event| match event {
//!         BackendEvent::StatusChanged(change) => println!("cambió {}", change.repo_path),
//!         BackendEvent::Operation(event) => println!("{} {}", event.operation, event.phase.as_str()),
//!         BackendEvent::OperationProgress(update) => println!("{}: {}", update.operation, update.progress.phase),
//!         BackendEvent::Updater(state) => println!("updater: {state}"),
//!     }))
//!     .expect("settings store"),
//! );
//!
//! let git = backend.git();
//! git.watch_status("/path/to/repo");
//! let status = git.get_status("/path/to/repo").expect("status");
//! let staged = git.stage("/path/to/repo", "README.md");
//! if !staged.success() {
//!     eprintln!("{} ({})", staged.message(), staged.code());
//! }
//! # let _ = status;
//! ```

mod backend;
pub mod channels;
mod cipher;
pub mod editor;
mod events;
mod git;
mod mutation;
mod shell;
mod updater;

#[cfg(test)]
mod tests;

pub use backend::{Backend, BackendError, BackendOptions};
pub use cipher::{
    KEYRING_ACCOUNT, KEYRING_SERVICE, KeyStoreError, KeychainCipher, KeyringKeyStore,
    MasterKeyStore, MemoryKeyStore,
};
pub use events::{BackendEvent, EventCallback};
pub use git::{
    AddToIgnoreCode, AddToIgnoreResult, CloneCode, CloneResult, GitApi, HandlerOperation,
    InitRepositoryCode, InitRepositoryResult, SetupTrackingCode, SetupTrackingResult,
    SuccessResult, WatchStatusResult,
};
pub use mutation::{
    FailureCode, HandlerFailure, Mutation, MutationBody, NETWORK_MUTATIONS, StructuredResult,
    build_mutation_request_key, create_generic_operation_failure, create_structured_handler_error,
    fingerprint_key_value, get_mutation_timeout, normalize_mutation_key_value,
};
pub use shell::{
    LaunchError, LaunchErrorKind, ProcessLauncher, RunOutcome, Shell, ShellError, ShellPlatform,
    SystemLauncher,
};
pub use updater::{UpdaterApi, UpdaterBridge, UpdaterEventSink};

// Tipos de los crates que une la fachada y que aparecen en su API, para que
// la capa de arriba pueda depender solo de este crate.
pub use editor::{DetectedEditor, EditorLaunch, EditorOpened};
pub use mergeon_git_ops::AddWorktreeOptions;
/// Deshacer y rehacer: captura alrededor de una mutación y pasos.
pub use mergeon_git_ops::undo;
pub use mergeon_git_ops::{
    ApplyPatchOptions, CleanupBranchRequest, CleanupBranchesOptions, Code, CommitOptions,
    ConflictResolution, ConflictSide, FetchOptions, GitIdentity, IdentityValues,
    InteractiveRebaseOptions, MergeOptions, MutationResult, Operation, OperationResult, Outcome,
    PullOptions, PushOptions, RebaseActionParams, RebaseOptions, RebaseTodoEntry,
    RemoteFetchResult, RemoteTagOptions, ResolveConflictResult, SequenceActionResult,
    SetIdentityOptions, SetIdentityResult, StashCapabilities, StashOptions, SubmoduleUpdateOptions,
};
pub use mergeon_git_ops::{FetchPullRequestOptions, PullRequestHead, pull_request_branch_name};
pub use mergeon_git_query::{
    BlameLine, BlameResult, Branch, BranchComparison, BranchesOverview, COMPARE_FILE_LIMIT,
    CleanupCandidate, Commit, CommitLogOptions, CommitSearchOptions, CompareLimits, CompareMode,
    ComparedCommit, ComparedFile, ComparisonFiles, ComparisonRevisions, ConflictKind,
    ConflictSides, ConflictStage, ConflictVersions, DiffOptions, DiffPayload,
    FILE_HISTORY_PAGE_SIZE, FileAtCommit, FileChange, FileHistoryOptions, FileHistoryPage,
    FileRevision, FixupCandidate, MergeConflictCheck, MergeState, MergedBranchesReport,
    NO_COMMON_HISTORY, QueryError, QueryResult, ReflogEntry, ReflogOptions, Remote,
    RemoteConnectionTest, RepositoryInspection, RepositoryKind, RepositorySummary,
    RevisionComparison, StashEntry, Submodule, TagDetails, TreeEntries, TreeEntry,
    UncommittedChanges,
};
pub use mergeon_git_query::{ImageBlob, ImagePair, ImagePairRequest, MAX_IMAGE_BYTES};
pub use mergeon_git_query::{RangeStats, Worktree};
pub use mergeon_gitcmd::{
    ConflictState, ControlCode, FileStatus, GitProgress, OperationState, OperationType,
    StructuredStatus,
};
pub use mergeon_github::GitHubClient;
pub use mergeon_runtime::{
    CancelPhase, CancelReason, CancelResult, OperationEvent, OperationEventError, OperationPhase,
    OperationProgress, OperationQueue, OperationSummary, StatusChange, StatusWatcherOptions,
    WatchMode, WatchReason,
};
pub use mergeon_store::{
    ExternalEditorSettings, PlainTextCipher, SecretCipher, Store, StoreError, StoreOptions,
    default_user_data_dir,
};
