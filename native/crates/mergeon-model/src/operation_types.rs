//! Formas de datos que comparten la cola de mutaciones, el log de operaciones y
//! los avisos: el evento `git:operation-event` del gate y el resultado
//! estructurado de una mutación.
//!
//! Son espejo, campo a campo, de lo que emite el backend (`mergeon-runtime` y
//! `mergeon-git-ops`), reducido a lo que la interfaz lee. Este crate no depende
//! de aquellos, así que la interfaz convierte de unos a otros.

use mergeon_gitcmd::StructuredStatus;
use serde::{Deserialize, Serialize};

/// Fase de una operación, tal como viaja en `git:operation-event`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OperationPhase {
    #[default]
    Queued,
    Running,
    Cancelling,
    Cancelled,
    Timeout,
    Failed,
    Completed,
}

impl OperationPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            OperationPhase::Queued => "queued",
            OperationPhase::Running => "running",
            OperationPhase::Cancelling => "cancelling",
            OperationPhase::Cancelled => "cancelled",
            OperationPhase::Timeout => "timeout",
            OperationPhase::Failed => "failed",
            OperationPhase::Completed => "completed",
        }
    }

    /// Fases finales: la operación ya salió de la cola.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            OperationPhase::Cancelled
                | OperationPhase::Timeout
                | OperationPhase::Failed
                | OperationPhase::Completed
        )
    }
}

/// Una operación dentro del estado de la cola.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct QueueOperation {
    pub operation_id: Option<String>,
    pub operation: String,
    pub phase: OperationPhase,
    /// Lugar en la cola, desde 1. `normalize_mutation_queue` lo recalcula.
    pub position: usize,
    pub cancellable: bool,
    /// Timestamp ISO, como lo manda el gate.
    pub started_at: Option<String>,
    pub queued_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OperationEventError {
    pub code: Option<String>,
    pub message: Option<String>,
}

/// Evento del mutation gate (`git:operation-event`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OperationEvent {
    pub operation_id: String,
    pub repo_path: String,
    pub operation: String,
    pub phase: Option<OperationPhase>,
    /// Estado completo de la cola tras el evento.
    pub operations: Vec<QueueOperation>,
    pub error: Option<OperationEventError>,
}

/// Lo que la interfaz lee del resultado estructurado de una mutación.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OperationResult {
    pub success: bool,
    pub operation: Option<String>,
    pub operation_id: Option<String>,
    /// `completed`, `noop`, `blocked`, `conflicted`, `failed`...
    pub outcome: Option<String>,
    pub code: Option<String>,
    pub message: Option<String>,
    /// Estado del repositorio tras la operación, cuando el backend lo adjunta.
    pub status: Option<StructuredStatus>,
}

/// Cadena no vacía: lo que en JavaScript es "truthy" para un texto opcional.
pub(crate) fn truthy(text: &Option<String>) -> Option<&str> {
    text.as_deref().filter(|text| !text.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_gate_events_as_the_backend_sends_them() {
        let event: OperationEvent = serde_json::from_value(serde_json::json!({
            "operationId": "push-1",
            "repoPath": "/repo",
            "operation": "push",
            "phase": "running",
            "position": 0,
            "queueLength": 1,
            "activeOperationId": "push-1",
            "operations": [{
                "operationId": "push-1",
                "operation": "push",
                "phase": "running",
                "position": 0,
                "cancellable": true,
                "startedAt": "2026-03-09T10:00:00.000Z",
                "queuedAt": "2026-03-09T09:59:59.000Z"
            }],
            "error": null
        }))
        .unwrap();

        assert_eq!(event.phase, Some(OperationPhase::Running));
        assert_eq!(event.operations[0].operation_id.as_deref(), Some("push-1"));
        assert!(event.operations[0].cancellable);
        assert!(!OperationPhase::Running.is_terminal());
        assert!(OperationPhase::Timeout.is_terminal());
        assert_eq!(OperationPhase::Cancelling.as_str(), "cancelling");
    }

    #[test]
    fn reads_a_structured_result_with_only_what_it_needs() {
        let result: OperationResult = serde_json::from_value(serde_json::json!({
            "success": false,
            "operation": "pull",
            "outcome": "blocked",
            "code": "AUTH_REQUIRED",
            "message": "Authentication required.",
            "conflictFiles": [],
            "status": null
        }))
        .unwrap();

        assert_eq!(result.code.as_deref(), Some("AUTH_REQUIRED"));
        assert_eq!(result.operation_id, None);
        assert_eq!(result.status, None);
    }
}
