//! Formas de datos de las operaciones: resultados estructurados y opciones.
//!
//! Los nombres de campo serializados son los del objeto JS original
//! (`createMutationResult`, `createOperationResult`...), en camelCase.

use mergeon_gitcmd::{ConflictState, ControlCode, OperationState, OperationType, StructuredStatus};
use serde::{Deserialize, Serialize};

/// Enum de strings: cada variante se serializa con su texto y lo expone en `as_str()`.
macro_rules! string_enum {
    (
        $(#[$meta:meta])*
        pub enum $name:ident {
            $($variant:ident => $text:literal,)+
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub enum $name {
            $(#[serde(rename = $text)] $variant,)+
        }

        impl $name {
            pub fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $text,)+
                }
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

string_enum! {
    /// Cómo terminó la operación (`outcome`).
    pub enum Outcome {
        Completed => "completed",
        Noop => "noop",
        Blocked => "blocked",
        Conflicted => "conflicted",
        Failed => "failed",
        Partial => "partial",
        Timeout => "timeout",
        Cancelled => "cancelled",
    }
}

string_enum! {
    /// Motivo concreto del resultado (`code`).
    pub enum Code {
        Ok => "OK",
        GitError => "GIT_ERROR",
        AuthRequired => "AUTH_REQUIRED",
        Conflict => "CONFLICT",
        Cancelled => "CANCELLED",
        Timeout => "TIMEOUT",
        CheckoutBlocked => "CHECKOUT_BLOCKED",
        StashConflict => "STASH_CONFLICT",
        StashNotFound => "STASH_NOT_FOUND",
        TagAlreadyExists => "TAG_ALREADY_EXISTS",
        NoCurrentBranch => "NO_CURRENT_BRANCH",
        RemoteNotFound => "REMOTE_NOT_FOUND",
        RemotePushUrlMissing => "REMOTE_PUSH_URL_MISSING",
        UpstreamCreated => "UPSTREAM_CREATED",
        AlreadyUpToDate => "ALREADY_UP_TO_DATE",
        NoRemotes => "NO_REMOTES",
        PartialFailure => "PARTIAL_FAILURE",
        NothingToStage => "NOTHING_TO_STAGE",
        NothingToUnstage => "NOTHING_TO_UNSTAGE",
        NothingToDiscard => "NOTHING_TO_DISCARD",
        EmptyPatch => "EMPTY_PATCH",
        PatchDoesNotApply => "PATCH_DOES_NOT_APPLY",
        CommitMessageRequired => "COMMIT_MESSAGE_REQUIRED",
        CommitBlockedByOperation => "COMMIT_BLOCKED_BY_OPERATION",
        NoChanges => "NO_CHANGES",
        DetachedHead => "DETACHED_HEAD",
        NoBranchChange => "NO_BRANCH_CHANGE",
        ActiveBranch => "ACTIVE_BRANCH",
        BranchNotCheckedOut => "BRANCH_NOT_CHECKED_OUT",
        WriteFailed => "WRITE_FAILED",
        NoOperationInProgress => "NO_OPERATION_IN_PROGRESS",
        ActionNotSupported => "ACTION_NOT_SUPPORTED",
        ConflictsRemain => "CONFLICTS_REMAIN",
        SigningFailed => "SIGNING_FAILED",
        BranchInOtherWorktree => "BRANCH_IN_OTHER_WORKTREE",
        WorktreeHasChanges => "WORKTREE_HAS_CHANGES",
        WorktreeLocked => "WORKTREE_LOCKED",
        UndoStale => "UNDO_STALE",
        UndoBlocked => "UNDO_BLOCKED",
    }
}

impl From<ControlCode> for Code {
    fn from(code: ControlCode) -> Self {
        match code {
            ControlCode::Cancelled => Code::Cancelled,
            ControlCode::Timeout => Code::Timeout,
        }
    }
}

impl From<ControlCode> for Outcome {
    fn from(code: ControlCode) -> Self {
        match code {
            ControlCode::Cancelled => Outcome::Cancelled,
            ControlCode::Timeout => Outcome::Timeout,
        }
    }
}

string_enum! {
    /// Nombre de la operación (`operation`).
    pub enum Operation {
        Push => "push",
        Fetch => "fetch",
        Pull => "pull",
        Stage => "stage",
        StageHunk => "stage-hunk",
        UnstageHunk => "unstage-hunk",
        DiscardHunk => "discard-hunk",
        StageAll => "stage-all",
        Unstage => "unstage",
        UnstageAll => "unstage-all",
        Discard => "discard",
        DiscardAll => "discard-all",
        Commit => "commit",
        Checkout => "checkout",
        CheckoutRemoteBranch => "checkout-remote-branch",
        CreateBranch => "create-branch",
        DeleteRemoteBranch => "delete-remote-branch",
        RenameBranch => "rename-branch",
        DeleteBranch => "delete-branch",
        FastForwardBranch => "fast-forward-branch",
        Reset => "reset",
        RebaseAction => "rebase-action",
        InteractiveRebase => "interactive-rebase",
        Stash => "stash",
        ApplyStash => "apply-stash",
        PopStash => "pop-stash",
        DropStash => "drop-stash",
        StashBranch => "stash-branch",
        CreateTag => "create-tag",
        PushTag => "push-tag",
        DeleteRemoteTag => "delete-remote-tag",
        DeleteTag => "delete-tag",
        CheckoutTag => "checkout-tag",
        AddRemote => "add-remote",
        RemoveRemote => "remove-remote",
        RenameRemote => "rename-remote",
        SetRemoteUrl => "set-remote-url",
        SubmoduleUpdate => "submodule-update",
        Merge => "merge",
        Rebase => "rebase",
        Revert => "revert",
        CherryPick => "cherry-pick",
        SetIdentity => "set-identity",
        CleanupBranches => "cleanup-branches",
        WorktreeAdd => "worktree-add",
        WorktreeRemove => "worktree-remove",
        WorktreeLock => "worktree-lock",
        WorktreeUnlock => "worktree-unlock",
        WorktreePrune => "worktree-prune",
        Undo => "undo",
        Redo => "redo",
    }
}

/// Resultado de un remoto dentro de un fetch (`remoteResults`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteFetchResult {
    pub remote: String,
    pub success: bool,
    /// Siempre `None`: el original guarda aquí el resultado de `git.fetch()`
    /// solo si es un string, y simple-git devuelve un objeto.
    pub summary: Option<String>,
    /// Solo en los remotos que fallaron.
    pub code: Option<Code>,
    /// Solo en los remotos que fallaron.
    pub message: Option<String>,
}

/// Resultado de una mutación (`createMutationResult`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MutationResult {
    pub success: bool,
    pub operation: Operation,
    pub action: Option<String>,
    pub outcome: Outcome,
    pub code: Code,
    pub message: String,
    pub status: Option<StructuredStatus>,
    pub conflict_files: Vec<String>,
    pub operation_state: Option<OperationState>,
    pub conflict_state: ConflictState,
    pub branch: Option<String>,
    pub remote: Option<String>,
    pub all_remotes: bool,
    pub remote_results: Vec<RemoteFetchResult>,
    pub summary: Option<String>,
    pub rebase: bool,
    pub autostash: bool,
    pub upstream_set: bool,
    pub tag: Option<String>,
    pub stash_ref: Option<String>,
    pub commit: Option<String>,
    pub mode: Option<String>,
    pub file_path: Option<String>,
    pub files_affected: Vec<String>,
    /// `None` = no se midió. `Some(false)` solo cuando de verdad se comprobó que
    /// ningún ref se movió: la UI lo usa para no recargar el historial.
    pub refs_changed: Option<bool>,
}

/// Resultado de merge, rebase, revert y cherry-pick (`createOperationResult`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationResult {
    pub success: bool,
    pub operation: Operation,
    pub outcome: Outcome,
    pub code: Code,
    pub message: String,
    pub conflict_files: Vec<String>,
    /// Solo se rellena cuando la operación quedó en conflicto.
    pub status: Option<StructuredStatus>,
    pub operation_state: Option<OperationState>,
    pub conflict_state: ConflictState,
    pub branch: Option<String>,
    pub remote: Option<String>,
    pub upstream_set: bool,
}

/// Una de las etapas de un archivo en conflicto en el índice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConflictSide {
    /// `:1:`, el ancestro común.
    Base,
    /// `:2:`, lo actual (HEAD).
    Ours,
    /// `:3:`, lo entrante.
    Theirs,
}

impl ConflictSide {
    /// Número de etapa en el índice.
    pub fn stage(self) -> u8 {
        match self {
            Self::Base => 1,
            Self::Ours => 2,
            Self::Theirs => 3,
        }
    }
}

/// Con qué se queda un archivo en conflicto al marcarlo como resuelto.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "camelCase")]
pub enum ConflictResolution {
    /// Este contenido, tal cual (con sus finales de línea).
    Content(String),
    /// Una de las versiones entera, byte a byte y con su modo (binarios, o
    /// conservar el lado que no borró).
    Keep(ConflictSide),
    /// Borrar el archivo (un lado lo borró y se le da la razón).
    Delete,
}

/// Resultado de `resolve_conflict_operation` (`createResolveConflictResult`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolveConflictResult {
    pub success: bool,
    pub code: Code,
    pub message: String,
    pub file_path: String,
    pub status: Option<StructuredStatus>,
    pub operation_state: Option<OperationState>,
    pub conflict_state: ConflictState,
}

/// Resultado de `sequence_action_operation` (`createSequenceActionResult`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SequenceActionResult {
    pub success: bool,
    /// Operación en curso sobre la que se actuó; `None` si no había ninguna.
    pub operation: Option<OperationType>,
    pub action: String,
    pub outcome: Outcome,
    pub code: Code,
    pub message: String,
    pub status: Option<StructuredStatus>,
    pub operation_state: Option<OperationState>,
    pub conflict_state: ConflictState,
}

/// Resultado de `set_git_identity_operation`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetIdentityResult {
    pub success: bool,
    pub operation: Operation,
    pub outcome: Outcome,
    pub code: Code,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentityValues {
    pub name: Option<String>,
    pub email: Option<String>,
}

/// Identidad de commit por ámbito (`getGitIdentity`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitIdentity {
    pub local: IdentityValues,
    pub global: IdentityValues,
}

fn default_remote() -> String {
    "origin".to_string()
}

/// Opciones de `push_operation`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PushOptions {
    pub remote: String,
    /// Rama a empujar; sin ella (o con `HEAD`) se usa la rama actual.
    pub branch: Option<String>,
    /// Token de GitHub (`authContext` en el original).
    pub auth_token: Option<String>,
    pub target_branch: Option<String>,
    pub force: bool,
    pub push_tags: bool,
}

impl Default for PushOptions {
    fn default() -> Self {
        Self {
            remote: default_remote(),
            branch: None,
            auth_token: None,
            target_branch: None,
            force: false,
            push_tags: false,
        }
    }
}

/// Una rama de [`CleanupBranchesOptions`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CleanupBranchRequest {
    pub name: String,
    /// Borrar con `-D` aunque no esté integrada en la base (squash-merge o
    /// upstream borrado, elegida a mano).
    pub force: bool,
}

/// Opciones de `cleanup_branches_operation`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CleanupBranchesOptions {
    /// Rama en la que deben estar integradas las que no llevan `force`.
    pub base: String,
    pub branches: Vec<CleanupBranchRequest>,
    /// Borrar también la rama remota de cada una (si su upstream existe).
    pub delete_remote: bool,
    /// Token de GitHub para el borrado remoto.
    pub auth_token: Option<String>,
}

/// Opciones de `fetch_operation`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FetchOptions {
    pub remote: String,
    pub all_remotes: bool,
    /// Token de GitHub (`authContext` en el original).
    pub auth_token: Option<String>,
}

impl Default for FetchOptions {
    fn default() -> Self {
        Self {
            remote: default_remote(),
            all_remotes: false,
            auth_token: None,
        }
    }
}

/// Opciones de `pull_operation`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PullOptions {
    /// `None` o vacío: git elige el remoto configurado para la rama.
    pub remote: Option<String>,
    pub branch: Option<String>,
    /// Token de GitHub (`authContext` en el original).
    pub auth_token: Option<String>,
    pub rebase: bool,
    pub autostash: bool,
}

impl Default for PullOptions {
    fn default() -> Self {
        Self {
            remote: Some(default_remote()),
            branch: None,
            auth_token: None,
            rebase: false,
            autostash: false,
        }
    }
}

/// Opciones de `apply_patch_operation`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ApplyPatchOptions {
    pub reverse: bool,
    pub file_path: Option<String>,
    /// Aplica al working tree en vez de al índice (sin `--cached`). Con
    /// `reverse` es descartar un bloque o unas líneas.
    pub worktree: bool,
}

/// Opciones de `commit_operation`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CommitOptions {
    pub amend: bool,
    /// Firma el commit (`git commit -S`) con la clave que diga la
    /// configuración de git (`user.signingkey`, `gpg.format`).
    pub sign: bool,
    /// Crea un commit `fixup! <asunto>` para ese commit (`git commit
    /// --fixup=<sha>`) con lo preparado. El mensaje lo pone git: el que se
    /// pase se ignora. No se combina con `amend`.
    pub fixup: Option<String>,
}

/// Opciones de `stash_push_operation` (`git stash push`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StashOptions {
    /// `-m`; vacío o `None` deja el mensaje de git (`WIP on <rama>: …`).
    pub message: Option<String>,
    /// Solo estas rutas (literales, sin comodines). Vacío guarda todo. Un
    /// renombre se guarda entero aunque solo venga su ruta nueva.
    pub paths: Vec<String>,
    /// `--include-untracked`: también lo que no se sigue.
    pub include_untracked: bool,
    /// `--keep-index`: lo preparado se guarda y además se queda en el índice.
    pub keep_index: bool,
    /// `--staged` (git 2.35+): solo lo preparado. No se combina con
    /// `include_untracked` ni con `keep_index`.
    pub staged_only: bool,
}

/// Parámetros de `rebase_action_operation`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RebaseActionParams {
    /// Mensaje nuevo; obligatorio para `reword`.
    pub new_message: Option<String>,
}

/// Una línea del todo-list de `interactive_rebase_operation`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RebaseTodoEntry {
    /// `pick`, `reword`, `squash`, `fixup` o `drop`.
    pub action: String,
    pub hash: String,
    /// Mensaje nuevo, solo para `reword`.
    pub message: Option<String>,
}

/// Opciones de `interactive_rebase_operation`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct InteractiveRebaseOptions {
    /// Commit sobre el que se rebasa; `None` rebasa desde la raíz (`--root`).
    pub base_commit: Option<String>,
    pub todo: Vec<RebaseTodoEntry>,
}

/// Opciones de `push_tag_operation` y `delete_remote_tag_operation`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RemoteTagOptions {
    pub remote: String,
    /// Token de GitHub (`authContext` en el original).
    pub auth_token: Option<String>,
}

impl Default for RemoteTagOptions {
    fn default() -> Self {
        Self {
            remote: default_remote(),
            auth_token: None,
        }
    }
}

/// Opciones de `submodule_update_operation`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SubmoduleUpdateOptions {
    /// Submódulos a actualizar; vacío actualiza todos.
    pub paths: Vec<String>,
}

/// Opciones de `merge_operation`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MergeOptions {
    /// Rama a la que cambiar antes de fusionar; sin ella se fusiona en la actual.
    pub target_branch: Option<String>,
    pub no_fast_forward: bool,
    pub squash: bool,
    pub message: Option<String>,
}

/// Opciones de `rebase_operation`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RebaseOptions {
    /// Rama a la que cambiar antes de rebasar; sin ella se rebasa la actual.
    pub target_branch: Option<String>,
    pub interactive: bool,
}

/// Opciones de `set_git_identity_operation`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SetIdentityOptions {
    /// `local` o `global`.
    pub scope: String,
    pub name: Option<String>,
    pub email: Option<String>,
}

impl Default for SetIdentityOptions {
    fn default() -> Self {
        Self {
            scope: "local".to_string(),
            name: None,
            email: None,
        }
    }
}
