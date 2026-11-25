//! De los tipos de runtime, git-ops, git-query y store a los tipos espejo de
//! `mergeon-model`, que no depende de aquellos crates.

use mergeon_backend::undo::RefCapture;
use mergeon_backend::{
    BranchesOverview, CloneResult, Commit, Mutation, OperationEvent, OperationPhase,
    OperationQueue, OperationSummary, Remote, SetIdentityResult, StructuredResult,
};
use mergeon_model::commit_view_utils::RawCommit;
use mergeon_model::operation_types as model;
use mergeon_model::repositories::{MutationQueueSnapshot, RecentRepository, WorkspaceSession};
use mergeon_model::repository_read_model::{
    BranchData, BranchInfo, RemoteData, RemoteRefs as ModelRemoteRefs,
};
use serde_json::Value;

pub(crate) fn operation_phase(phase: OperationPhase) -> model::OperationPhase {
    match phase {
        OperationPhase::Queued => model::OperationPhase::Queued,
        OperationPhase::Running => model::OperationPhase::Running,
        OperationPhase::Cancelling => model::OperationPhase::Cancelling,
        OperationPhase::Cancelled => model::OperationPhase::Cancelled,
        OperationPhase::Timeout => model::OperationPhase::Timeout,
        OperationPhase::Failed => model::OperationPhase::Failed,
        OperationPhase::Completed => model::OperationPhase::Completed,
    }
}

pub(crate) fn queue_operation(summary: &OperationSummary) -> model::QueueOperation {
    model::QueueOperation {
        operation_id: summary.operation_id.clone(),
        operation: summary.operation.clone(),
        phase: operation_phase(summary.phase),
        position: summary.position,
        cancellable: summary.cancellable,
        started_at: summary.started_at.clone(),
        queued_at: Some(summary.queued_at.clone()),
    }
}

pub(crate) fn operation_event(event: &OperationEvent) -> model::OperationEvent {
    model::OperationEvent {
        operation_id: event.operation_id.clone(),
        repo_path: event.repo_path.clone(),
        operation: event.operation.clone(),
        phase: Some(operation_phase(event.phase)),
        operations: event.operations.iter().map(queue_operation).collect(),
        error: event
            .error
            .as_ref()
            .map(|error| model::OperationEventError {
                code: Some(error.code.clone()),
                message: Some(error.message.clone()),
            }),
    }
}

/// `git:getOperationQueue` tal como lo entiende `normalizeMutationQueue`.
pub(crate) fn queue_snapshot(queue: &OperationQueue) -> MutationQueueSnapshot {
    MutationQueueSnapshot {
        operations: None,
        active: queue.active.as_ref().map(queue_operation),
        queued: Some(queue.queued.iter().map(queue_operation).collect()),
    }
}

pub(crate) fn branch_data(overview: &BranchesOverview) -> BranchData {
    BranchData {
        current: overview.current.clone(),
        all: overview.all.clone(),
        branches: overview
            .branches
            .iter()
            .map(|branch| {
                (
                    branch.name.clone(),
                    BranchInfo {
                        tracking: branch.tracking.clone(),
                        ahead: branch.ahead,
                        behind: branch.behind,
                    },
                )
            })
            .collect(),
    }
}

pub(crate) fn remote_data(remote: &Remote) -> RemoteData {
    RemoteData {
        name: remote.name.clone(),
        refs: ModelRemoteRefs {
            fetch: remote.refs.fetch.clone(),
            push: remote.refs.push.clone(),
        },
    }
}

pub(crate) fn raw_commit(commit: &Commit) -> RawCommit {
    RawCommit {
        hash: commit.hash.clone(),
        full_hash: commit.full_hash.clone(),
        message: commit.message.clone(),
        author: commit.author.clone(),
        email: commit.email.clone(),
        date: commit.date.clone(),
        body: commit.body.clone(),
        refs: commit.refs.clone(),
        parents: commit.parents.clone(),
    }
}

pub(crate) fn recent_from_store(recent: &mergeon_store::RecentRepository) -> RecentRepository {
    RecentRepository {
        id: recent.id.clone(),
        name: recent.name.clone(),
        path: recent.path.clone(),
        current_branch: recent.current_branch.clone(),
        is_github: recent.is_github,
        remote_url: recent.remote_url.clone(),
        last_opened: Some(recent.last_opened.clone()).filter(|opened| !opened.is_empty()),
    }
}

pub(crate) fn recent_to_store(recent: &RecentRepository) -> mergeon_store::RecentRepository {
    mergeon_store::RecentRepository {
        id: recent.id.clone(),
        name: recent.name.clone(),
        path: recent.path.clone(),
        last_opened: recent.last_opened.clone().unwrap_or_default(),
        current_branch: recent.current_branch.clone(),
        is_github: recent.is_github,
        remote_url: recent.remote_url.clone(),
        extra: Default::default(),
    }
}

pub(crate) fn session_to_store(session: &WorkspaceSession) -> mergeon_store::WorkspaceSession {
    mergeon_store::WorkspaceSession {
        repository_workspaces: session
            .repository_workspaces
            .iter()
            .map(|(path, workspace)| {
                (
                    path.clone(),
                    serde_json::to_value(workspace).unwrap_or(Value::Null),
                )
            })
            .collect(),
        open_repositories: session.open_repositories.clone(),
        active_repository_path: session.active_repository_path.clone(),
        workspaces: session
            .workspaces
            .iter()
            .map(|workspace| mergeon_store::RepoWorkspace {
                id: workspace.id.clone(),
                name: workspace.name.clone(),
                repositories: workspace.repositories.clone(),
                active_repository_path: workspace.active_repository_path.clone(),
            })
            .collect(),
        active_workspace_id: session.active_workspace_id.clone(),
    }
}

/// Lo que la interfaz lee de cualquier resultado estructurado: los campos que
/// usan `applyRepositoryOperationResult`, `syncRepositoryReadModel`, el log de
/// operaciones y los avisos.
pub trait StructuredOutcome {
    fn to_operation_result(&self) -> model::OperationResult;

    /// Lo leído antes y después de la operación para deshacerla, si su canal
    /// lo pide.
    fn ref_capture(&self) -> Option<&RefCapture> {
        None
    }

    /// `result.success || result.outcome === 'noop'`.
    fn succeeded_or_noop(&self) -> bool {
        let result = self.to_operation_result();
        result.success || result.outcome.as_deref() == Some("noop")
    }
}

impl<T: StructuredResult> StructuredOutcome for Mutation<T> {
    fn to_operation_result(&self) -> model::OperationResult {
        model::OperationResult {
            success: self.success(),
            operation: self.operation().map(str::to_string),
            operation_id: self.operation_id().map(str::to_string),
            outcome: self.outcome().map(|outcome| outcome.as_str().to_string()),
            code: Some(self.code().to_string()),
            message: Some(self.message().to_string()),
            status: self.status().cloned(),
        }
    }

    fn succeeded_or_noop(&self) -> bool {
        self.success() || self.outcome() == Some(mergeon_backend::Outcome::Noop)
    }

    fn ref_capture(&self) -> Option<&RefCapture> {
        self.capture()
    }
}

impl StructuredOutcome for CloneResult {
    fn to_operation_result(&self) -> model::OperationResult {
        model::OperationResult {
            success: self.success,
            operation: Some(self.operation.as_str().to_string()),
            operation_id: self.operation_id.clone(),
            outcome: Some(self.outcome.as_str().to_string()),
            code: Some(self.code.as_str().to_string()),
            message: Some(self.message.clone()),
            status: None,
        }
    }
}

impl StructuredOutcome for SetIdentityResult {
    fn to_operation_result(&self) -> model::OperationResult {
        model::OperationResult {
            success: self.success,
            operation: Some(self.operation.as_str().to_string()),
            operation_id: None,
            outcome: Some(self.outcome.as_str().to_string()),
            code: Some(self.code.as_str().to_string()),
            message: Some(self.message.clone()),
            status: None,
        }
    }
}

impl StructuredOutcome for model::OperationResult {
    fn to_operation_result(&self) -> model::OperationResult {
        self.clone()
    }
}
