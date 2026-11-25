//! Operaciones Git que mutan el repositorio (port de
//! `electron/services/git-operations.js`).
//!
//! Todas las funciones son síncronas y bloqueantes: quien las llama desde la
//! interfaz debe hacerlo en un hilo de fondo, dentro de
//! `mergeon_gitcmd::run_with_operation_context` si quiere poder cancelarlas o
//! darles un timeout.
//!
//! Las mutaciones (`*_operation`) devuelven siempre un resultado estructurado
//! (`success`, `operation`, `outcome`, `code`, `message`...), nunca `Err`. Las
//! dos lecturas que vivían en el mismo archivo JS conservan su contrato:
//! [`get_git_identity`] no falla y [`resolve_ref_to_commit`] devuelve `Result`.
//!
//! Los comandos son los mismos que lanzaba la app Electron, incluidos los que
//! armaba simple-git por debajo, y pasan por el mismo guard de argumentos
//! peligrosos (módulo `guard`).

mod branches;
mod cleanup;
mod exec;
mod guard;
mod identity;
pub mod messages;
mod pull_request_ops;
mod rebase_edit;
mod remote_ops;
mod remotes;
pub mod results;
mod sequence;
mod stash;
mod tags;
mod types;
pub mod undo;
mod version;
mod working_tree;
mod worktrees;

#[cfg(test)]
mod tests;

pub use branches::{
    checkout_operation, checkout_remote_branch_operation, create_branch_operation,
    delete_branch_operation, delete_remote_branch_operation,
    delete_remote_branch_with_auth_operation, fast_forward_branch_operation,
    rename_branch_operation, reset_operation,
};
pub use cleanup::cleanup_branches_operation;
pub use exec::OperationError;
pub use identity::{get_git_identity, resolve_ref_to_commit, set_git_identity_operation};
pub use pull_request_ops::{
    FetchPullRequestOptions, PullRequestHead, fetch_pull_request_operation,
    pull_request_branch_name,
};
pub use rebase_edit::{interactive_rebase_operation, rebase_action_operation};
pub use remote_ops::{fetch_operation, pull_operation, push_operation};
pub use remotes::{
    add_remote_operation, remove_remote_operation, rename_remote_operation,
    set_remote_url_operation, submodule_update_operation,
};
pub use sequence::{
    cherry_pick_commits_operation, cherry_pick_operation, merge_operation, rebase_operation,
    resolve_conflict_operation, resolve_conflict_with_operation, revert_commits_operation,
    revert_operation, sequence_action_operation,
};
pub use stash::{
    apply_stash_operation, drop_stash_operation, pop_stash_operation, stash_branch_operation,
    stash_operation, stash_push_operation,
};
pub use tags::{
    checkout_tag_operation, create_tag_operation, delete_remote_tag_operation,
    delete_tag_operation, push_tag_operation,
};
pub use types::{
    ApplyPatchOptions, CleanupBranchRequest, CleanupBranchesOptions, Code, CommitOptions,
    ConflictResolution, ConflictSide, FetchOptions, GitIdentity, IdentityValues,
    InteractiveRebaseOptions, MergeOptions, MutationResult, Operation, OperationResult, Outcome,
    PullOptions, PushOptions, RebaseActionParams, RebaseOptions, RebaseTodoEntry,
    RemoteFetchResult, RemoteTagOptions, ResolveConflictResult, SequenceActionResult,
    SetIdentityOptions, SetIdentityResult, StashOptions, SubmoduleUpdateOptions,
};
pub use version::{
    GitVersion, StashCapabilities, installed_git_version, parse_git_version, stash_capabilities,
};
pub use working_tree::{
    apply_patch_operation, commit_operation, discard_all_operation, discard_changes_operation,
    discard_patch_operation, stage_all_operation, stage_operation, unstage_all_operation,
    unstage_operation,
};
pub use worktrees::{
    AddWorktreeOptions, add_worktree_operation, branch_in_other_worktree,
    branch_in_other_worktree_message, lock_worktree_operation, prune_worktrees_operation,
    remove_worktree_operation, unlock_worktree_operation,
};
