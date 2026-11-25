//! Worktrees: crear (con una rama existente o nueva), quitar, bloquear,
//! desbloquear y podar los que perdieron su carpeta.
//!
//! `git worktree` escribe su progreso en stderr ("Preparing worktree…"), así
//! que estos comandos se juzgan por código de salida, no por stderr.

use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::exec::{OperationError, OperationOutcome, exec_checked, git};
use crate::messages::{clean_message, js_trim};
use crate::results::{
    MutationExtra, MutationPayload, build_mutation_error_result, finalize_mutation_result,
};
use crate::types::{Code, MutationResult, Operation, Outcome};

/// Qué desproteger en el worktree nuevo: una rama que ya existe o una rama
/// nueva desde `start_point` (HEAD si no se da).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AddWorktreeOptions {
    /// Carpeta del worktree (no debe existir, o estar vacía).
    pub path: String,
    /// Rama existente.
    pub branch: Option<String>,
    /// Rama nueva (`git worktree add -b`).
    pub new_branch: Option<String>,
    /// Desde dónde sale la rama nueva.
    pub start_point: Option<String>,
}

static IN_OTHER_WORKTREE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"'([^']+)' is already (?:checked out|used by worktree) at '([^']+)'").unwrap()
});

/// "La rama ya está en otro worktree", con la rama y la ruta, a partir del
/// error de git (`is already checked out at` o, desde git 2.42, `is already
/// used by worktree at`).
pub fn branch_in_other_worktree(message: &str) -> Option<(String, String)> {
    IN_OTHER_WORKTREE
        .captures(message)
        .map(|found| (found[1].to_string(), found[2].to_string()))
}

/// El texto que ve el usuario cuando la rama está en otro worktree.
pub fn branch_in_other_worktree_message(branch: &str, path: &str) -> String {
    format!(
        "Branch '{branch}' is already checked out in the worktree at {path}. A branch can only be \
         checked out in one worktree: open that worktree, or check out another branch here."
    )
}

fn short_name(path: &str) -> &str {
    path.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(path)
}

/// Traduce los errores de `git worktree` que el usuario puede arreglar.
fn worktree_error(
    repo_path: &Path,
    operation: Operation,
    error: OperationError,
    path: &str,
) -> MutationResult {
    let message = error.message.as_str();
    let lower = message.to_lowercase();
    let failure = |code, text: String| {
        finalize_mutation_result(
            repo_path,
            MutationPayload::failure(operation, Outcome::Blocked, code, text).with(MutationExtra {
                file_path: Some(path.to_string()),
                ..Default::default()
            }),
        )
    };
    if error.control_code().is_none() {
        if let Some((branch, other)) = branch_in_other_worktree(message) {
            return failure(
                Code::BranchInOtherWorktree,
                branch_in_other_worktree_message(&branch, &other),
            );
        }
        if lower.contains("contains modified or untracked files") {
            return failure(
                Code::WorktreeHasChanges,
                format!(
                    "The worktree at {path} has uncommitted changes. Commit or stash them, or \
                     remove it anyway to throw them away."
                ),
            );
        }
        if lower.contains("cannot remove a locked working tree") || lower.contains("is locked") {
            return failure(
                Code::WorktreeLocked,
                format!("The worktree at {path} is locked. Unlock it first."),
            );
        }
    }
    build_mutation_error_result(
        repo_path,
        operation,
        &error,
        MutationExtra {
            file_path: Some(path.to_string()),
            ..Default::default()
        },
    )
}

fn completed(
    repo_path: &Path,
    operation: Operation,
    path: &str,
    message: String,
) -> MutationResult {
    finalize_mutation_result(
        repo_path,
        MutationPayload::completed(operation, message).with(MutationExtra {
            file_path: Some(path.to_string()),
            ..Default::default()
        }),
    )
}

fn blocked(repo_path: &Path, operation: Operation, message: &str) -> MutationResult {
    finalize_mutation_result(
        repo_path,
        MutationPayload::failure(operation, Outcome::Blocked, Code::GitError, message),
    )
}

/// `git worktree add`: una carpeta nueva con `branch` desprotegida, o con una
/// rama nueva `new_branch` creada desde `start_point`.
pub fn add_worktree_operation(repo_path: &Path, options: &AddWorktreeOptions) -> MutationResult {
    let operation = Operation::WorktreeAdd;
    let path = js_trim(&options.path).to_string();
    let branch = options
        .branch
        .as_deref()
        .map(js_trim)
        .filter(|branch| !branch.is_empty());
    let new_branch = options
        .new_branch
        .as_deref()
        .map(js_trim)
        .filter(|branch| !branch.is_empty());
    if path.is_empty() {
        return blocked(repo_path, operation, "Choose a folder for the worktree.");
    }
    let mut arguments = vec!["worktree", "add"];
    let what = match (new_branch, branch) {
        (Some(new_branch), None) => {
            arguments.extend(["-b", new_branch, "--", path.as_str()]);
            if let Some(start) = options
                .start_point
                .as_deref()
                .map(js_trim)
                .filter(|start| !start.is_empty())
            {
                arguments.push(start);
            }
            format!("new branch {new_branch}")
        }
        (None, Some(branch)) => {
            arguments.extend(["--", path.as_str(), branch]);
            branch.to_string()
        }
        _ => {
            return blocked(
                repo_path,
                operation,
                "Choose an existing branch or a name for a new one.",
            );
        }
    };

    let run = || -> OperationOutcome<()> {
        exec_checked(&git(repo_path), &arguments)?;
        Ok(())
    };
    match run() {
        Ok(()) => completed(
            repo_path,
            operation,
            &path,
            format!("Created worktree {} with {what}.", short_name(&path)),
        ),
        Err(error) => worktree_error(repo_path, operation, error, &path),
    }
}

/// `git worktree remove`: borra la carpeta del worktree (la rama se queda).
/// Sin `force`, git se niega si tiene cambios sin commitear; con `force` los
/// pierde.
pub fn remove_worktree_operation(repo_path: &Path, path: &str, force: bool) -> MutationResult {
    let operation = Operation::WorktreeRemove;
    let mut arguments = vec!["worktree", "remove"];
    if force {
        arguments.push("--force");
    }
    arguments.extend(["--", path]);
    match exec_checked(&git(repo_path), &arguments) {
        Ok(_) => completed(
            repo_path,
            operation,
            path,
            format!("Removed worktree {}.", short_name(path)),
        ),
        Err(error) => worktree_error(repo_path, operation, error.into(), path),
    }
}

/// `git worktree lock`: que `prune` no lo quite aunque su carpeta no esté
/// (un disco externo, por ejemplo).
pub fn lock_worktree_operation(
    repo_path: &Path,
    path: &str,
    reason: Option<&str>,
) -> MutationResult {
    let operation = Operation::WorktreeLock;
    let mut arguments = vec!["worktree", "lock"];
    if let Some(reason) = reason.map(js_trim).filter(|reason| !reason.is_empty()) {
        arguments.extend(["--reason", reason]);
    }
    arguments.extend(["--", path]);
    match exec_checked(&git(repo_path), &arguments) {
        Ok(_) => completed(
            repo_path,
            operation,
            path,
            format!("Locked worktree {}.", short_name(path)),
        ),
        Err(error) => worktree_error(repo_path, operation, error.into(), path),
    }
}

/// `git worktree unlock`.
pub fn unlock_worktree_operation(repo_path: &Path, path: &str) -> MutationResult {
    let operation = Operation::WorktreeUnlock;
    match exec_checked(&git(repo_path), &["worktree", "unlock", "--", path]) {
        Ok(_) => completed(
            repo_path,
            operation,
            path,
            format!("Unlocked worktree {}.", short_name(path)),
        ),
        Err(error) => worktree_error(repo_path, operation, error.into(), path),
    }
}

/// `git worktree prune`: olvida los worktrees cuya carpeta ya no existe (los
/// bloqueados no).
pub fn prune_worktrees_operation(repo_path: &Path) -> MutationResult {
    let operation = Operation::WorktreePrune;
    let git = git(repo_path);
    let prunable = |git: &mergeon_gitcmd::Git| -> OperationOutcome<usize> {
        let list = exec_checked(git, &["worktree", "list", "--porcelain"])?;
        Ok(list
            .lines()
            .filter(|line| *line == "prunable" || line.starts_with("prunable "))
            .count())
    };
    let run = || -> OperationOutcome<usize> {
        let before = prunable(&git)?;
        exec_checked(&git, &["worktree", "prune"])?;
        let after = prunable(&git)?;
        Ok(before.saturating_sub(after))
    };
    match run() {
        Ok(0) => finalize_mutation_result(
            repo_path,
            MutationPayload::new(
                true,
                operation,
                Outcome::Noop,
                Code::NoChanges,
                "No stale worktrees to prune.",
            ),
        ),
        Ok(count) => finalize_mutation_result(
            repo_path,
            MutationPayload::completed(
                operation,
                if count == 1 {
                    "Pruned 1 stale worktree.".to_string()
                } else {
                    format!("Pruned {count} stale worktrees.")
                },
            ),
        ),
        Err(error) => build_mutation_error_result(
            repo_path,
            operation,
            &OperationError::new(clean_message(&error.message, "Failed to prune worktrees.")),
            MutationExtra::default(),
        ),
    }
}
