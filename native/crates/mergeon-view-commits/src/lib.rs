//! Vista de commits: historial con grafo, inspector, pestañas de archivo
//! histórico y rebase interactivo (`src/components/repository/CommitView.vue` y
//! sus piezas).
//!
//! API, eventos y lo que conecta la cáscara: `native/docs/view-commits.md`.
//!
//! ```ignore
//! let commits = cx.new(|cx| CommitsView::new(repo_id, window, cx));
//! let file_tab = cx.new(|cx| HistoricalFileTabView::new(repo_id, tab_id, window, cx));
//! ```

mod changes_tab;
mod checkout;
mod colors;
mod commits_view;
mod compare_view;
mod file_history_list;
mod github_link;
pub mod graph;
mod historical_file_tab;
mod history_list;
mod images;
mod inspector;
mod interactive_rebase;
mod monogram;
mod resizer;
mod selection;
mod summary_header;
mod tree_tab;

pub use changes_tab::{
    CommitChangesTab, CommitChangesTabEvent, DEFAULT_FILES_PANE_WIDTH, FileRow,
    clamp_files_pane_width, file_row_id, file_rows,
};
pub use checkout::{
    BRANCH_TARGET_PREFIX, CheckoutDialogRequest, CheckoutTargetKind, CheckoutTargetOption,
    CreateBranchDialogRequest, DETACHED_TARGET, REMOTE_TARGET_PREFIX, build_checkout_targets,
    ref_checkout_target,
};
pub use commits_view::{
    BatchCommand, BatchMenuEntry, CommitCommand, CommitCreated, CommitLoadOptions, CommitMenuEntry,
    CommitsView, CommitsViewEvent, DEFAULT_HISTORY_WIDTH, MULTI_ROW_HEIGHT, MultiStats, batch_menu,
    clamp_history_width, commit_menu, is_checkout_disabled,
};
pub use compare_view::{
    CompareCommitRow, CompareView, CompareViewEvent, compare_commit_rows, compare_mode_description,
    open_compare_view,
};
pub use file_history_list::{
    FILE_HISTORY_ROW_HEIGHT, FileHistoryList, FileHistoryListEvent, FileHistorySource, HistoryRow,
    file_history_row_id, revision_badge,
};
pub use github_link::{
    github_blob_url, github_commit_url, github_repository_url, repository_github_url,
};
pub use historical_file_tab::{HistoricalFileTabView, file_menu_labels};
pub use history_list::{
    CommitHistoryList, CommitHistoryListEvent, GRAPH_COLUMN_MIN_WIDTH, GRAPH_LABEL_INSET,
    ROW_HEIGHT, RowFlags, WORKING_CHANGES_ROW_ID, WorkingChanges, ref_badge_title, refs_tooltip,
    row_key,
};
pub use inspector::{BODY_MAX_FRACTION, CommitInspector, CommitTotals, MIN_FILES_HEIGHT};
pub use interactive_rebase::{
    InteractiveRebaseDialog, InteractiveRebaseDialogEvent, RebaseAction, RebaseCommit, RebaseRow,
    available_actions, open_interactive_rebase_dialog, rebase_description,
};
pub use selection::MultiSelection;
pub use summary_header::{
    BODY_CLAMP_LINES, BODY_LINE_HEIGHT, CommitSummaryHeader, MessageBody, parent_hashes,
    short_hash, split_trailers, visual_line_count,
};
pub use tree_tab::{CommitTreeTab, CommitTreeTabEvent, TreeRow};

/// Punto de entrada común de los crates de vistas.
pub fn init(_cx: &mut gpui_kit::App) {}
