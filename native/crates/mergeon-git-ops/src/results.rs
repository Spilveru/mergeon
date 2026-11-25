//! Constructores de los resultados estructurados.
//!
//! Las mutaciones nunca devuelven `Err`: todo fallo se traduce aquí a un
//! resultado con `success: false` y el `outcome` / `code` que corresponda.

use mergeon_gitcmd::{
    ControlCode, OperationState, OperationType, StructuredStatus, conflict_files,
    detect_operation_state, get_structured_status,
};
use std::path::Path;

use crate::exec::{OperationError, git, raw};
use crate::messages::{
    capitalize, clean_message, has_auth_error, has_conflict_output, is_checkout_blocked,
    is_tag_already_exists, js_trim,
};
use crate::types::{
    Code, MutationResult, Operation, OperationResult, Outcome, RemoteFetchResult,
    ResolveConflictResult, SequenceActionResult,
};

const TIMEOUT_MESSAGE: &str = "Git operation timed out and was stopped.";
const CANCELLED_MESSAGE: &str = "Git operation was cancelled.";

fn control_message(code: ControlCode) -> &'static str {
    match code {
        ControlCode::Timeout => TIMEOUT_MESSAGE,
        ControlCode::Cancelled => CANCELLED_MESSAGE,
    }
}

/// `safeGetStatus`: el estado del repositorio, o el estado vacío si no se pudo leer.
pub fn safe_get_status(repo_path: &Path) -> StructuredStatus {
    get_structured_status(repo_path).unwrap_or_else(|_| StructuredStatus::empty())
}

/// Campos opcionales de `createMutationResult`: lo que cada operación añade al
/// resultado (y lo que en el original viaja en `extra`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MutationExtra {
    pub action: Option<String>,
    pub conflict_files: Vec<String>,
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
    pub refs_changed: Option<bool>,
}

/// Argumento de `createMutationResult` sin el estado del repositorio.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MutationPayload {
    pub success: bool,
    pub operation: Operation,
    pub outcome: Outcome,
    pub code: Code,
    pub message: String,
    pub extra: MutationExtra,
}

impl MutationPayload {
    pub fn new(
        success: bool,
        operation: Operation,
        outcome: Outcome,
        code: Code,
        message: impl Into<String>,
    ) -> Self {
        Self {
            success,
            operation,
            outcome,
            code,
            message: message.into(),
            extra: MutationExtra::default(),
        }
    }

    /// Resultado con éxito (`outcome: completed`, `code: OK`).
    pub fn completed(operation: Operation, message: impl Into<String>) -> Self {
        Self::new(true, operation, Outcome::Completed, Code::Ok, message)
    }

    /// Resultado sin éxito con el `outcome` y `code` dados.
    pub fn failure(
        operation: Operation,
        outcome: Outcome,
        code: Code,
        message: impl Into<String>,
    ) -> Self {
        Self::new(false, operation, outcome, code, message)
    }

    pub fn with(mut self, extra: MutationExtra) -> Self {
        self.extra = extra;
        self
    }
}

/// `createMutationResult`.
pub fn create_mutation_result(
    payload: MutationPayload,
    status: Option<StructuredStatus>,
) -> MutationResult {
    let MutationPayload {
        success,
        operation,
        outcome,
        code,
        message,
        extra,
    } = payload;

    let conflict_files = if !extra.conflict_files.is_empty() {
        extra.conflict_files
    } else {
        status
            .as_ref()
            .map(|status| status.conflicted.clone())
            .unwrap_or_default()
    };

    MutationResult {
        success,
        operation,
        action: extra.action,
        outcome,
        code,
        message,
        conflict_files,
        operation_state: status.as_ref().map(|status| status.operation_state.clone()),
        conflict_state: status
            .as_ref()
            .map(|status| status.conflict_state.clone())
            .unwrap_or_default(),
        status,
        branch: extra.branch,
        remote: extra.remote,
        all_remotes: extra.all_remotes,
        remote_results: extra.remote_results,
        summary: extra.summary,
        rebase: extra.rebase,
        autostash: extra.autostash,
        upstream_set: extra.upstream_set,
        tag: extra.tag,
        stash_ref: extra.stash_ref,
        commit: extra.commit,
        mode: extra.mode,
        file_path: extra.file_path,
        files_affected: extra.files_affected,
        refs_changed: extra.refs_changed,
    }
}

/// `finalizeMutationResult`: completa el resultado con el estado actual del repositorio.
pub fn finalize_mutation_result(repo_path: &Path, payload: MutationPayload) -> MutationResult {
    let status = safe_get_status(repo_path);
    create_mutation_result(payload, Some(status))
}

/*
 * Huella de todos los refs del repositorio. Un fetch que no trae nada es
 * indistinguible de uno que sí por su código de salida, así que se compara la
 * huella antes y después: sin esto la UI recarga el historial completo en cada
 * fetch (incluido el auto-fetch periódico) aunque no haya cambiado nada.
 */
pub fn read_refs_fingerprint(repo_path: &Path) -> Option<String> {
    // Sin huella no se puede afirmar que nada cambió: se asume que sí.
    raw(
        &git(repo_path),
        &["for-each-ref", "--format=%(objectname) %(refname)"],
    )
    .ok()
}

/// `computeRefsChanged`: `None` si falta alguna de las dos huellas.
pub fn compute_refs_changed(before: Option<&str>, after: Option<&str>) -> Option<bool> {
    match (before, after) {
        (Some(before), Some(after)) => Some(before != after),
        _ => None,
    }
}

/// Resultado de control (cancelación / timeout) común a los tres constructores de error.
fn control_payload(operation: Operation, code: ControlCode) -> MutationPayload {
    MutationPayload::failure(operation, code.into(), code.into(), control_message(code))
}

/// `buildRemoteMutationErrorResult`: errores de push, fetch y pull.
pub fn build_remote_mutation_error_result(
    repo_path: &Path,
    operation: Operation,
    error: &OperationError,
    extra: MutationExtra,
) -> MutationResult {
    let message = error.message.as_str();

    let payload = if let Some(code) = error.control_code() {
        control_payload(operation, code)
    } else if has_auth_error(message) {
        MutationPayload::failure(
            operation,
            Outcome::Blocked,
            Code::AuthRequired,
            clean_message(
                message,
                "Authentication is required to complete this operation.",
            ),
        )
    } else if has_conflict_output(message) {
        MutationPayload::failure(
            operation,
            Outcome::Conflicted,
            Code::Conflict,
            clean_message(
                message,
                &format!(
                    "{} stopped because Git detected conflicts.",
                    capitalize(operation.as_str())
                ),
            ),
        )
    } else {
        MutationPayload::failure(
            operation,
            Outcome::Failed,
            Code::GitError,
            clean_message(message, &format!("Git {operation} failed.")),
        )
    };

    finalize_mutation_result(repo_path, payload.with(extra))
}

/// `buildMutationErrorResult`: errores de las mutaciones locales.
pub fn build_mutation_error_result(
    repo_path: &Path,
    operation: Operation,
    error: &OperationError,
    extra: MutationExtra,
) -> MutationResult {
    let message = error.message.as_str();
    let is_stash_apply = matches!(operation, Operation::ApplyStash | Operation::PopStash);

    let other_worktree = matches!(
        operation,
        Operation::Checkout | Operation::CheckoutRemoteBranch
    )
    .then(|| crate::worktrees::branch_in_other_worktree(message))
    .flatten();
    let payload = if let Some(code) = error.control_code() {
        control_payload(operation, code)
    } else if let Some((branch, path)) = other_worktree {
        MutationPayload::failure(
            operation,
            Outcome::Blocked,
            Code::BranchInOtherWorktree,
            crate::worktrees::branch_in_other_worktree_message(&branch, &path),
        )
    } else if operation == Operation::Checkout && is_checkout_blocked(message) {
        MutationPayload::failure(
            operation,
            Outcome::Blocked,
            Code::CheckoutBlocked,
            clean_message(message, "Checkout was blocked by local changes."),
        )
    } else if is_stash_apply && has_conflict_output(message) {
        MutationPayload::failure(
            operation,
            Outcome::Conflicted,
            Code::StashConflict,
            clean_message(message, "Applying the stash created conflicts."),
        )
    } else if operation == Operation::CreateTag && is_tag_already_exists(message) {
        MutationPayload::failure(
            operation,
            Outcome::Blocked,
            Code::TagAlreadyExists,
            clean_message(
                message,
                &format!(
                    "Tag '{}' already exists.",
                    extra.tag.as_deref().unwrap_or_default()
                ),
            ),
        )
    } else if has_conflict_output(message) {
        MutationPayload::failure(
            operation,
            Outcome::Conflicted,
            Code::GitError,
            clean_message(
                message,
                &format!("Git {operation} stopped because conflicts were detected."),
            ),
        )
    } else {
        MutationPayload::failure(
            operation,
            Outcome::Failed,
            Code::GitError,
            clean_message(message, &format!("Git {operation} failed.")),
        )
    };

    finalize_mutation_result(repo_path, payload.with(extra))
}

/// Guard compartido de argumentos obligatorios: recorta `raw_values` y, si falta
/// alguno, devuelve el resultado bloqueado con los valores parciales que cada
/// operación hace eco vía `extra(values)`.
pub(crate) fn require_args<const N: usize>(
    repo_path: &Path,
    operation: Operation,
    raw_values: [&str; N],
    message: &str,
    extra: impl FnOnce(&[String; N]) -> MutationExtra,
) -> Result<[String; N], Box<MutationResult>> {
    let values = raw_values.map(|value| js_trim(value).to_string());

    if values.iter().all(|value| !value.is_empty()) {
        return Ok(values);
    }

    let payload = MutationPayload::failure(operation, Outcome::Blocked, Code::GitError, message)
        .with(extra(&values));
    Err(Box::new(finalize_mutation_result(repo_path, payload)))
}

/// `Some(valor)` si no está vacío (el `valor || null` del original).
pub(crate) fn non_empty(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_string())
}

/// Argumento de `createOperationResult` sin el estado del repositorio.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationPayload {
    pub success: bool,
    pub operation: Operation,
    pub outcome: Outcome,
    pub code: Code,
    pub message: String,
    pub branch: Option<String>,
    pub remote: Option<String>,
    pub upstream_set: bool,
}

impl OperationPayload {
    pub fn new(
        success: bool,
        operation: Operation,
        outcome: Outcome,
        code: Code,
        message: impl Into<String>,
        branch: Option<String>,
        remote: Option<String>,
    ) -> Self {
        Self {
            success,
            operation,
            outcome,
            code,
            message: message.into(),
            branch,
            remote,
            upstream_set: false,
        }
    }
}

/// `createOperationResult`.
pub fn create_operation_result(
    payload: OperationPayload,
    conflict_files: Vec<String>,
    status: Option<StructuredStatus>,
    operation_state: Option<OperationState>,
) -> OperationResult {
    OperationResult {
        success: payload.success,
        operation: payload.operation,
        outcome: payload.outcome,
        code: payload.code,
        message: payload.message,
        conflict_files,
        conflict_state: status
            .as_ref()
            .map(|status| status.conflict_state.clone())
            .unwrap_or_default(),
        status,
        operation_state,
        branch: payload.branch,
        remote: payload.remote,
        upstream_set: payload.upstream_set,
    }
}

/// `finalizeOperationResult`: añade la operación en curso y, si quedó en
/// conflicto, los archivos en conflicto y el estado completo.
pub fn finalize_operation_result(repo_path: &Path, payload: OperationPayload) -> OperationResult {
    let conflicted = payload.outcome == Outcome::Conflicted;
    // El original deja escapar este error al handler; aquí una mutación nunca
    // falla, así que la operación en curso queda sin informar.
    let operation_state = detect_operation_state(repo_path).ok();
    let conflict_files = if conflicted {
        conflict_files(repo_path)
    } else {
        Vec::new()
    };
    let status = conflicted.then(|| safe_get_status(repo_path));

    create_operation_result(payload, conflict_files, status, operation_state)
}

/// `buildConflictResult`.
pub fn build_conflict_result(
    repo_path: &Path,
    operation: Operation,
    branch: Option<String>,
    remote: Option<String>,
    message: Option<&str>,
) -> OperationResult {
    let message = match message {
        Some(message) => message.to_string(),
        None => format!("{operation} stopped because Git detected conflicts."),
    };

    finalize_operation_result(
        repo_path,
        OperationPayload::new(
            false,
            operation,
            Outcome::Conflicted,
            Code::Conflict,
            message,
            branch,
            remote,
        ),
    )
}

/// `buildGenericErrorResult`: errores de merge, rebase, revert y cherry-pick.
pub fn build_generic_error_result(
    repo_path: &Path,
    operation: Operation,
    error: &OperationError,
    branch: Option<String>,
    remote: Option<String>,
) -> OperationResult {
    let raw_message = error.message.as_str();

    if let Some(code) = error.control_code() {
        return finalize_operation_result(
            repo_path,
            OperationPayload::new(
                false,
                operation,
                code.into(),
                code.into(),
                control_message(code),
                branch,
                remote,
            ),
        );
    }

    if has_auth_error(raw_message) {
        return finalize_operation_result(
            repo_path,
            OperationPayload::new(
                false,
                operation,
                Outcome::Blocked,
                Code::AuthRequired,
                clean_message(
                    raw_message,
                    "Authentication is required to complete this operation.",
                ),
                branch,
                remote,
            ),
        );
    }

    if has_conflict_output(raw_message) {
        return build_conflict_result(repo_path, operation, branch, remote, None);
    }

    finalize_operation_result(
        repo_path,
        OperationPayload::new(
            false,
            operation,
            Outcome::Failed,
            Code::GitError,
            clean_message(raw_message, &format!("Git {operation} failed.")),
            branch,
            remote,
        ),
    )
}

/// `createResolveConflictResult`.
pub fn create_resolve_conflict_result(
    success: bool,
    code: Code,
    message: impl Into<String>,
    file_path: &str,
    status: Option<StructuredStatus>,
) -> ResolveConflictResult {
    ResolveConflictResult {
        success,
        code,
        message: message.into(),
        file_path: file_path.to_string(),
        operation_state: status.as_ref().map(|status| status.operation_state.clone()),
        conflict_state: status
            .as_ref()
            .map(|status| status.conflict_state.clone())
            .unwrap_or_default(),
        status,
    }
}

/// `createSequenceActionResult`.
pub fn create_sequence_action_result(
    success: bool,
    operation: Option<OperationType>,
    action: &str,
    outcome: Outcome,
    code: Code,
    message: impl Into<String>,
    status: Option<StructuredStatus>,
) -> SequenceActionResult {
    SequenceActionResult {
        success,
        operation,
        action: action.to_string(),
        outcome,
        code,
        message: message.into(),
        operation_state: status.as_ref().map(|status| status.operation_state.clone()),
        conflict_state: status
            .as_ref()
            .map(|status| status.conflict_state.clone())
            .unwrap_or_default(),
        status,
    }
}
