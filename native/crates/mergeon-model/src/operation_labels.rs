//! Nombre visible de cada operación git (`src/lib/operationLabels.js`). Vivía
//! dentro de useGit y duplicado (más pobre) en MutationQueueBanner; ahora es la
//! única fuente para toasts, cola de mutaciones y el panel de operaciones.

pub const OPERATION_LABELS: &[(&str, &str)] = &[
    ("fetch", "Fetch"),
    ("pull", "Pull"),
    ("push", "Push"),
    ("clone", "Clone"),
    ("merge", "Merge"),
    ("rebase", "Rebase"),
    ("revert", "Revert"),
    ("cherry-pick", "Cherry-pick"),
    ("stage", "Stage"),
    ("stage-all", "Stage"),
    ("stage-hunk", "Stage hunk"),
    ("unstage", "Unstage"),
    ("unstage-all", "Unstage"),
    ("unstage-hunk", "Unstage hunk"),
    ("discard-hunk", "Discard hunk"),
    ("discard", "Discard"),
    ("discard-all", "Discard"),
    ("commit", "Commit"),
    ("create-branch", "Create branch"),
    ("add-remote", "Add remote"),
    ("remove-remote", "Remove remote"),
    ("rename-remote", "Rename remote"),
    ("set-remote-url", "Update remote URL"),
    ("rename-branch", "Rename branch"),
    ("delete-branch", "Delete branch"),
    ("delete-remote-branch", "Delete remote branch"),
    ("cleanup-branches", "Clean up branches"),
    ("fast-forward-branch", "Fast-forward branch"),
    ("setup-tracking", "Set up tracking"),
    ("checkout", "Checkout"),
    ("checkout-remote-branch", "Checkout"),
    ("reset", "Reset"),
    ("rebase-action", "Rebase action"),
    ("interactive-rebase", "Interactive rebase"),
    ("resolve-conflict", "Resolve conflict"),
    ("sequence-action", "Continue/abort operation"),
    ("stash", "Stash"),
    ("apply-stash", "Apply stash"),
    ("pop-stash", "Pop stash"),
    ("drop-stash", "Drop stash"),
    ("stash-branch", "Branch from stash"),
    ("create-tag", "Create tag"),
    ("delete-tag", "Delete tag"),
    ("push-tag", "Push tag"),
    ("delete-remote-tag", "Delete remote tag"),
    ("checkout-tag", "Checkout tag"),
    ("submodule-update", "Update submodules"),
    ("set-identity", "Update git identity"),
    ("init-repository", "Initialize repository"),
    ("apply-patch", "Apply patch"),
    ("worktree-add", "Add worktree"),
    ("worktree-remove", "Remove worktree"),
    ("worktree-lock", "Lock worktree"),
    ("worktree-unlock", "Unlock worktree"),
    ("worktree-prune", "Prune worktrees"),
    ("undo", "Undo"),
    ("redo", "Redo"),
];

/// `OPERATION_LABELS[operation]`: la etiqueta de una operación conocida.
pub fn known_operation_label(operation: &str) -> Option<&'static str> {
    OPERATION_LABELS
        .iter()
        .find(|(name, _)| *name == operation)
        .map(|(_, label)| *label)
}

/// Etiqueta de una operación; una desconocida se muestra con su propio nombre y
/// una vacía como "Git operation".
pub fn operation_label(operation: &str) -> String {
    match known_operation_label(operation) {
        Some(label) => label.to_string(),
        None if operation.is_empty() => "Git operation".to_string(),
        None => operation.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn labels_known_operations() {
        assert_eq!(operation_label("fetch"), "Fetch");
        assert_eq!(operation_label("stage-hunk"), "Stage hunk");
        assert_eq!(operation_label("discard-hunk"), "Discard hunk");
        assert_eq!(
            operation_label("sequence-action"),
            "Continue/abort operation"
        );
        assert_eq!(operation_label("checkout-remote-branch"), "Checkout");
    }

    #[test]
    fn falls_back_to_the_operation_name_or_a_generic_label() {
        assert_eq!(operation_label("undo"), "Undo");
        assert_eq!(operation_label("bisect"), "bisect");
        assert_eq!(operation_label(""), "Git operation");
        assert_eq!(known_operation_label("bisect"), None);
    }

    #[test]
    fn lists_each_operation_once() {
        let names: HashSet<&str> = OPERATION_LABELS.iter().map(|(name, _)| *name).collect();
        assert_eq!(names.len(), OPERATION_LABELS.len());
        assert_eq!(OPERATION_LABELS.len(), 57);
    }
}
