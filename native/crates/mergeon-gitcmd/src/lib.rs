//! Base de la capa Git de Mergeon: ejecutor del binario `git`, contexto de
//! operación (cancelación y timeout) y lectura de estado. No depende de la UI.
//!
//! Todas las funciones son síncronas y bloqueantes; quien las llama desde la
//! interfaz debe hacerlo en un hilo de fondo.

mod auth;
mod context;
pub mod credentials;
pub mod paths;
mod progress;
mod runner;
mod status;
#[doc(hidden)]
pub mod test_support;

pub use auth::{GITHUB_ORIGIN, github_credentials};
pub use context::{
    ControlCode, OperationContext, current_operation_context, operation_control_code,
    run_with_operation_context,
};
pub use progress::{
    GitProgress, PROGRESS_INTERVAL, ProgressFilter, ProgressSink, ProgressThrottle,
    is_progress_output, parse_progress_line,
};
pub use runner::{
    CappedOutput, Git, GitError, GitErrorKind, GitOutput, GitResult, LOCAL_TIMEOUT,
    NETWORK_TIMEOUT, READ_TIMEOUT,
};
pub use status::{
    ConflictState, FileStatus, OperationState, OperationType, RenamedFile, StatusSummary,
    StructuredStatus, build_raw_conflict_state, conflict_files, detect_operation_state,
    forget_git_dir_path, get_structured_status, git_dir_path, head_short_hash,
    is_missing_head_error, parse_status_summary,
};
