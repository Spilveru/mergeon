//! Vista de cambios locales: lista, staging, commit, diff y resolución de
//! conflictos.
//!
//! Port de `ChangesView.vue`, `RepositoryChangesSidebar.vue`,
//! `RepositoryCommitPanel.vue`, `ConflictResolver.vue`, `StashDiffDialog.vue`
//! y del diálogo de blame. API y eventos: `native/docs/view-changes.md`.
//!
//! ```ignore
//! mergeon_ui::init(cx);
//! mergeon_view_changes::init(cx);
//!
//! // La pestaña "Local Changes" de un repositorio:
//! let changes = cx.new(|cx| ChangesView::new(repo_id, ChangesMode::List, window, cx));
//! // La pestaña de un archivo del working tree o del índice:
//! let file = cx.new(|cx| ChangesView::new(repo_id, ChangesMode::FileTab { tab_id }, window, cx));
//!
//! // Y los dos diálogos:
//! mergeon_view_changes::open_stash_diff_dialog(&repo_id, &stash.hash, window, cx);
//! mergeon_view_changes::open_blame_dialog(&repo_id, "src/app.js", window, cx);
//! ```

mod changes_view;
mod commit_panel;
mod conflict_merge;
mod conflict_resolver;
mod dialogs;
mod logic;
mod parts;
mod sidebar;

pub use changes_view::{ChangesMode, ChangesView, ChangesViewEvent};
pub use commit_panel::{CommitPanel, SubmitCommit};
pub use conflict_merge::{
    Chunk, ChunkKind, ConflictChoice, DocumentKind, LineEnding, LineTone, MergeDocument, Pane,
    PaneData, PaneRow, Pick, Resync, SplitText, analyze, count_marker_lines, document_from_markers,
    finish_result, resync, split_lines,
};
pub use conflict_resolver::{
    ConflictResolver, ConflictResolverEvent, WholeChoice, WholeOption, whole_options,
};
pub use dialogs::{BlameDialog, StashDiffDialog, open_blame_dialog, open_stash_diff_dialog};
pub use logic::{
    BusyAction, ChangedFile, DEFAULT_LOCAL_CHANGES_WIDTH, FileMenuAction, FileMenuEntry,
    STACKED_LAYOUT_MAX_WIDTH, SUBJECT_LIMIT, SUBJECT_SOFT_LIMIT, SidebarSource, SubjectTone,
    amend_is_pushed, amend_message, amend_warning, can_commit, clamp_list_width,
    clamp_local_changes_width, commit_blocked_reason, commit_button_label, commit_label,
    commit_shortcut_keys, commit_shortcut_label, commit_target, commit_template_message,
    compose_commit_message, conflicted_files, discard_description, effective_status,
    file_descriptor, file_menu_entries, files_count_label, hunk_action, is_partially_staged,
    selection_summary, split_commit_message, staged_files, stash_selection, subject_counter,
    subject_tone, unstaged_files, untracked_files,
};
pub use sidebar::{
    ChangesSection, ChangesSidebar, FileAction, FileActionKind, MultiSelection, StatsByPath,
};

use gpui_kit::App;

/// Registra el atajo de commit de la caja de mensaje (`Ctrl+Enter` y
/// `Cmd+Enter`). Debe llamarse después de `mergeon_ui::init`.
pub fn init(cx: &mut App) {
    commit_panel::bind_keys(cx);
}
