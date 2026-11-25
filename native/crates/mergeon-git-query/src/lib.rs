//! Lado de lectura de la capa Git de Mergeon.
//!
//! Port de los handlers de lectura de `electron/ipc/git-handlers.js` y de los
//! servicios `commit-log`, `commit-tree`, `branch-status`, `reflog`,
//! `submodules` y `repository-summary`. Cada canal IPC de lectura es una función
//! pública síncrona con el mismo nombre en snake_case; quien las llama desde la
//! interfaz debe hacerlo en un hilo de fondo.
//!
//! Las lecturas devuelven `Result<T, QueryError>`, y el mensaje del error es el
//! que construía `createReadError(prefix, error)` en la app Electron.

mod blame;
mod branches;
mod collate;
mod commit_log;
mod commit_template;
mod commit_tree;
mod compare;
mod conflict;
mod diff;
mod error;
mod exec;
mod file_history;
mod files;
mod fixup_targets;
mod fs_util;
mod images;
mod inspect;
mod merged_branches;
mod paths;
mod reflog;
mod remotes;
mod simple_log;
mod stashes;
mod status;
mod submodules;
mod summary;
mod tags;
mod text;
mod worktree_stats;
mod worktrees;

#[cfg(test)]
mod testutil;
#[cfg(test)]
mod user_config_tests;

pub use blame::{BlameLine, BlameResult, MAX_BLAME_FILE_BYTES, blame, blame_at_revision};
pub use branches::{
    Branch, BranchTracking, BranchesOverview, UpstreamTrack, get_branches,
    parse_branch_tracking_refs, parse_upstream_track,
};
pub use commit_log::{Commit, CommitLogOptions, CommitSearchOptions, get_commits, search_commits};
pub use commit_template::{
    CommitTemplateRead, CommitTemplateStamp, MAX_COMMIT_TEMPLATE_BYTES, get_commit_template,
    read_commit_template_if_changed,
};
pub use commit_tree::{
    FileAtCommit, MAX_VIEWABLE_FILE_BYTES, TREE_ENTRY_LIMIT, TreeEntries, TreeEntry, TreeEntryType,
    get_file_at_commit, get_tree_entries, looks_binary, looks_binary_text, parse_tree_entries,
};
pub use compare::{
    BranchComparison, COMPARE_COMMIT_LIMIT, COMPARE_FILE_LIMIT, CompareLimits, CompareMode,
    ComparedCommit, ComparedFile, ComparisonFiles, ComparisonRevisions, MergeConflictCheck,
    NO_COMMON_HISTORY, RangeStats, RevisionComparison, check_merge_conflicts, compare_branches,
    compare_revisions, get_comparison_diff, get_comparison_files, get_range_stats,
    resolve_comparison,
};
pub use conflict::{
    ConflictKind, ConflictSides, ConflictStage, ConflictVersions, read_conflict_versions,
};
pub use diff::{
    DiffOptions, DiffPayload, FileChange, MAX_DIFF_BYTES, WHOLE_FILE_CONTEXT_ARG, get_commit_diff,
    get_commit_files, get_diff,
};
pub use error::{QueryError, QueryResult};
pub use file_history::{
    FILE_HISTORY_PAGE_SIZE, FileHistoryOptions, FileHistoryPage, FileRevision, get_file_history,
    get_file_revision_diff,
};
pub use files::{MAX_READ_FILE_BYTES, read_file};
pub use fixup_targets::{FixupCandidate, MAX_FIXUP_CANDIDATES, get_fixup_candidates};
pub use images::{ImageBlob, ImagePair, ImagePairRequest, MAX_IMAGE_BYTES, read_image_pair};
pub use inspect::{RepositoryInspection, RepositoryKind, inspect_repository, is_valid_repository};
pub use merged_branches::{
    CleanupCandidate, MergeState, MergedBranchesReport, SQUASH_CHECK_LIMIT, get_merged_branches,
    is_protected_branch,
};
pub use paths::{
    PathError, assert_path_inside_repository, normalize_repo_relative_path,
    normalize_repository_file_path,
};
pub use reflog::{ReflogEntry, ReflogOptions, get_reflog};
pub use remotes::{Remote, RemoteConnectionTest, RemoteRefs, get_remotes, test_remote_connection};
pub use simple_log::LogEntry;
pub use stashes::{StashEntry, get_stash_diff, get_stash_files, get_stashes};
pub use status::{UncommittedChanges, get_operation_state, get_status, has_uncommitted_changes};
pub use submodules::{Submodule, SubmoduleState, get_submodules};
pub use summary::{
    InvalidReason, RepositorySummary, get_repository_summary, refresh_repository_summary,
};
pub use tags::{TagDetails, get_tag_details, get_tags};
pub use worktree_stats::{
    LineStats, MAX_UNTRACKED_STATS_BYTES, WorkingTreeStats, get_working_tree_stats,
};
pub use worktrees::{
    Worktree, comparable_path, get_worktrees, parse_worktree_list, worktree_has_changes,
};

// Tipos de `mergeon_gitcmd` que aparecen en la API de este crate.
pub use mergeon_gitcmd::{FileStatus, OperationState, StructuredStatus};
