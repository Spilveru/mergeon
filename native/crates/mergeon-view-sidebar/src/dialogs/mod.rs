//! Los diálogos que abre el sidebar. Cada uno es una entidad con su estado y
//! una función `open_*` que la crea y la pone en pantalla sobre
//! `mergeon_ui::dialog`; quien la abre se suscribe a sus eventos y conserva la
//! entidad mientras le interesen: el diálogo cerrado no la retiene, y los
//! eventos de una entidad que ya nadie guarda no se entregan.

mod add_remote;
mod branch_action;
mod checkout_branch;
mod cleanup_branches;
mod create_branch;
mod new_worktree;
mod parts;
mod tag_details;

pub use add_remote::{AddRemoteDialog, AddRemoteDialogEvent, open_add_remote_dialog};
pub use branch_action::{
    BranchActionCompleted, BranchActionDialog, BranchActionDialogEvent, open_branch_action_dialog,
    open_branch_comparison_dialog,
};
pub use checkout_branch::{
    CheckoutBranchDialog, CheckoutBranchDialogEvent, CheckoutBranchOptions, CheckoutTargetOption,
    open_checkout_branch_dialog,
};
pub use cleanup_branches::{CleanupBranchesDialog, open_cleanup_branches_dialog};
pub use create_branch::{
    CreateBranchDialog, CreateBranchDialogEvent, CreateBranchOptions, open_create_branch_dialog,
};
pub use new_worktree::{
    NewWorktreeDialog, NewWorktreeDialogEvent, NewWorktreeMode, NewWorktreeOptions,
    branch_name_problem, open_new_worktree_dialog, suggest_worktree_path, validate_new_worktree,
};
pub use tag_details::{TagDetailsDialog, open_tag_details_dialog};
