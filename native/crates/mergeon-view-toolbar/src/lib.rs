//! Barra de herramientas del repositorio, banners de operación y diálogos de
//! fetch / pull / push / rama / log / reflog.
//!
//! Port de `Toolbar.vue`, `OperationIndicator.vue`, `OperationStatusSlot.vue`,
//! `OperationBanner.vue`, `MutationQueueBanner.vue` y de los diálogos
//! `FetchDialog`, `PullDialog`, `PushDialog`, `CreateBranchDialog`,
//! `OperationsLogDialog` y `ReflogDialog`. API y eventos:
//! `native/docs/view-toolbar.md`.
//!
//! ```ignore
//! // En la cáscara, sobre el repositorio activo:
//! let toolbar = cx.new(|cx| ToolbarView::new(window, cx));
//! let status = cx.new(|cx| OperationStatusSlot::new(window, cx));
//! let status_bar = cx.new(|cx| StatusBarView::new(window, cx)); // al pie
//!
//! // Y al recibir un diálogo pedido por un flujo de `AppModel`:
//! cx.subscribe_in(&model, window, |_, _, event: &AppEvent, window, cx| {
//!     if let AppEvent::ShowDialog(request) = event {
//!         mergeon_view_toolbar::handle_dialog_request(request, window, cx);
//!     }
//! })
//! ```

mod dialogs;
mod mutation_queue_banner;
mod operation_banner;
mod operation_indicator;
mod operation_status_slot;
mod parts;
mod pull_request_indicator;
mod status_bar;
mod toolbar;

pub use dialogs::{
    CreateBranchDialogOptions, StashChoice, StashEntryPreview, StashSummary,
    open_create_branch_dialog, open_fetch_dialog, open_operations_log, open_pull_dialog,
    open_push_dialog, open_reflog, open_stash_dialog, stash_summary,
};
pub use mutation_queue_banner::{MutationQueueBanner, queue_operation_count, queue_phase_label};
pub use operation_banner::{
    OperationBanner, SequenceAction, operation_banner_detail, operation_banner_title,
};
pub use operation_indicator::OperationIndicator;
pub use operation_status_slot::OperationStatusSlot;
pub use pull_request_indicator::{
    PullRequestIndicator, pull_request_indicator_label, pull_request_indicator_tooltip,
};
pub use status_bar::{
    HistoryPosition, StatusBarView, fetched_ago_label, git_version_label, history_position_label,
    last_remote_sync,
};
pub use toolbar::{
    SEARCH_PLACEHOLDER, TOOLBAR_HEIGHT, ToolbarEvent, ToolbarView, search_shortcut_keys,
    sync_counts_label, toolbar_branch_label, toolbar_head_hash, toolbar_head_title, undo_tooltip,
};

use gpui_kit::{App, Window};
use mergeon_app::DialogRequest;

/// No registra nada: el crate no tiene acciones ni atajos propios (los de
/// fetch, pull, push, nueva rama, log y reflog los resuelve la cáscara con
/// `AppModel::handle_shortcut`). Existe para que todos los crates de vistas
/// se inicialicen igual.
pub fn init(_cx: &mut App) {}

/// Abre el diálogo de este crate que corresponda a lo que pide un flujo de
/// `AppModel` (`AppEvent::ShowDialog`). Devuelve `true` si la petición era de
/// las suyas, se haya abierto ahora o estuviera ya abierto.
pub fn handle_dialog_request(request: &DialogRequest, window: &mut Window, cx: &mut App) -> bool {
    match request {
        DialogRequest::Fetch => {
            open_fetch_dialog(window, cx);
        }
        DialogRequest::Pull => {
            open_pull_dialog(window, cx);
        }
        DialogRequest::Push => {
            open_push_dialog(window, cx);
        }
        DialogRequest::CreateBranch {
            base_label,
            has_local_changes,
            // La referencia de partida la conserva el modelo desde
            // `handle_new_branch`; `handle_create_branch_submit` la usa.
            base_reference: _,
        } => {
            open_create_branch_dialog(
                CreateBranchDialogOptions::new(base_label.clone())
                    .has_local_changes(*has_local_changes),
                window,
                cx,
            );
        }
        DialogRequest::OperationsLog => {
            open_operations_log(window, cx);
        }
        DialogRequest::Reflog => {
            open_reflog(window, cx);
        }
        DialogRequest::Stash { paths } => {
            open_stash_dialog(paths.clone(), window, cx);
        }
        DialogRequest::Launcher
        | DialogRequest::GitHubAccount
        | DialogRequest::GitHubRepositoryPicker
        | DialogRequest::CloneFromUrl => return false,
    }
    true
}
