//! Historial de operaciones git de la sesión (`src/stores/operationsLog.js`).
//! Los toasts mueren en segundos y truncados; aquí queda el registro completo —
//! fases con timestamps (desde los eventos del mutation gate) y el resultado
//! estructurado (mensaje entero, outcome, código) — para poder revisar qué pasó
//! y por qué.
//!
//! Solo memoria de la sesión, capado a [`MAX_ENTRIES`]. La llave del join entre
//! evento y resultado es el operationId que comparten.

use chrono::Local;
use serde::{Deserialize, Serialize};

use crate::format::{now_epoch_ms, parse_js_date};
use crate::operation_types::{OperationEvent, OperationPhase, OperationResult, truthy};

pub const MAX_ENTRIES: usize = 200;

fn is_false(value: &bool) -> bool {
    !*value
}

// El gate manda timestamps ISO; aquí todo se guarda como epoch ms.
fn to_epoch_ms(value: &str) -> Option<i64> {
    if value.is_empty() {
        return None;
    }
    parse_js_date(value, &Local)
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationLogEntry {
    /// El del gate, o `local-<ms>-<n>` para operaciones que no pasan por él.
    pub operation_id: String,
    pub repo_path: Option<String>,
    pub repo_name: Option<String>,
    pub operation: Option<String>,
    pub phase: Option<OperationPhase>,
    /// `completed`, `noop`, `blocked`, `conflicted`, `failed`... o, si solo hubo
    /// eventos del gate, la fase final (`cancelled`, `timeout`...).
    pub outcome: Option<String>,
    pub code: Option<String>,
    pub message: Option<String>,
    /// Epoch ms.
    pub queued_at: Option<i64>,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub created_at: i64,
    /// Ya sumó a `unseen_errors`: un fallo cuenta una vez aunque llegue por
    /// evento y por resultado.
    #[serde(default, skip_serializing_if = "is_false")]
    pub error_counted: bool,
}

/// De qué repositorio es el resultado que se registra.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ResultContext<'a> {
    pub repo_name: Option<&'a str>,
    pub repo_path: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationsLog {
    /// Las más nuevas primero.
    pub entries: Vec<OperationLogEntry>,
    pub unseen_errors: usize,
}

impl OperationsLog {
    pub fn new() -> Self {
        Self::default()
    }

    /// Entradas que no terminaron bien.
    pub fn error_entries(&self) -> impl Iterator<Item = &OperationLogEntry> {
        self.entries
            .iter()
            .filter(|entry| truthy(&entry.outcome).is_some_and(|outcome| outcome != "completed"))
    }

    fn trim(&mut self) {
        self.entries.truncate(MAX_ENTRIES);
    }

    /// Posición de la entrada con ese id; si no existe (o no hay id) crea una al
    /// principio con `seed`.
    fn find_or_create(
        &mut self,
        operation_id: Option<&str>,
        seed: OperationLogEntry,
        now_ms: i64,
    ) -> usize {
        if let Some(operation_id) = operation_id
            && let Some(index) = self
                .entries
                .iter()
                .position(|candidate| candidate.operation_id == operation_id)
        {
            return index;
        }

        let entry = OperationLogEntry {
            operation_id: match operation_id {
                Some(operation_id) => operation_id.to_string(),
                None => format!("local-{now_ms}-{}", self.entries.len()),
            },
            created_at: now_ms,
            ..seed
        };
        self.entries.insert(0, entry);
        self.trim();
        0
    }

    /// Fases del mutation gate (queued/running/terminales) con sus timestamps.
    pub fn record_event(&mut self, payload: &OperationEvent) {
        self.record_event_at(payload, now_epoch_ms());
    }

    /// [`Self::record_event`] con el reloj explícito (epoch ms).
    pub fn record_event_at(&mut self, payload: &OperationEvent, now_ms: i64) {
        if payload.operation_id.is_empty() {
            return;
        }

        let non_empty = |text: &String| Some(text.clone()).filter(|text| !text.is_empty());
        let index = self.find_or_create(
            Some(&payload.operation_id),
            OperationLogEntry {
                repo_path: non_empty(&payload.repo_path),
                operation: non_empty(&payload.operation),
                ..Default::default()
            },
            now_ms,
        );
        let entry = &mut self.entries[index];

        entry.repo_path = non_empty(&payload.repo_path).or(entry.repo_path.take());
        entry.operation = non_empty(&payload.operation).or(entry.operation.take());
        entry.phase = payload.phase.or(entry.phase);

        let summary = payload.operations.iter().find(|candidate| {
            candidate.operation_id.as_deref() == Some(payload.operation_id.as_str())
        });
        if let Some(queued_at) = summary.and_then(|summary| truthy(&summary.queued_at)) {
            entry.queued_at = to_epoch_ms(queued_at);
        }
        if let Some(started_at) = summary.and_then(|summary| truthy(&summary.started_at)) {
            entry.started_at = to_epoch_ms(started_at);
        }

        let Some(phase) = payload.phase.filter(|phase| phase.is_terminal()) else {
            return;
        };

        entry.finished_at = entry.finished_at.or(Some(now_ms));
        if truthy(&entry.outcome).is_none() {
            entry.outcome = Some(phase.as_str().to_string());
        }
        if let Some(error) = &payload.error {
            if truthy(&entry.code).is_none() {
                entry.code = truthy(&error.code).map(str::to_string);
            }
            if truthy(&entry.message).is_none() {
                entry.message = truthy(&error.message).map(str::to_string);
            }
        }
        if phase != OperationPhase::Completed && !entry.error_counted {
            entry.error_counted = true;
            self.unseen_errors += 1;
        }
    }

    /// Resultado estructurado completo (mensaje sin truncar, outcome, código).
    /// Para operaciones sin gate (sin operationId) crea su propia entrada.
    pub fn record_result(&mut self, result: &OperationResult, context: ResultContext<'_>) {
        self.record_result_at(result, context, now_epoch_ms());
    }

    /// [`Self::record_result`] con el reloj explícito (epoch ms).
    pub fn record_result_at(
        &mut self,
        result: &OperationResult,
        context: ResultContext<'_>,
        now_ms: i64,
    ) {
        let Some(operation) = truthy(&result.operation) else {
            return;
        };
        let repo_name = context.repo_name.filter(|name| !name.is_empty());
        let repo_path = context.repo_path.filter(|path| !path.is_empty());

        let index = self.find_or_create(
            truthy(&result.operation_id),
            OperationLogEntry {
                operation: Some(operation.to_string()),
                repo_path: repo_path.map(str::to_string),
                repo_name: repo_name.map(str::to_string),
                ..Default::default()
            },
            now_ms,
        );
        let entry = &mut self.entries[index];

        entry.operation = Some(operation.to_string());
        if let Some(repo_name) = repo_name {
            entry.repo_name = Some(repo_name.to_string());
        }
        if let Some(repo_path) = repo_path {
            entry.repo_path = Some(repo_path.to_string());
        }
        let outcome = truthy(&result.outcome)
            .unwrap_or(if result.success {
                "completed"
            } else {
                "failed"
            })
            .to_string();
        if let Some(code) = truthy(&result.code) {
            entry.code = Some(code.to_string());
        }
        if let Some(message) = truthy(&result.message) {
            entry.message = Some(message.to_string());
        }
        entry.finished_at = entry.finished_at.or(Some(now_ms));

        if outcome != "completed" && outcome != "noop" && !entry.error_counted {
            entry.error_counted = true;
            self.unseen_errors += 1;
        }
        entry.outcome = Some(outcome);
    }

    pub fn mark_seen(&mut self) {
        self.unseen_errors = 0;
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.unseen_errors = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::to_iso_string;
    use crate::operation_types::{OperationEventError, QueueOperation};

    fn event(operation_id: &str, phase: OperationPhase) -> OperationEvent {
        OperationEvent {
            operation_id: operation_id.to_string(),
            repo_path: "/repo".to_string(),
            operation: operation_id
                .split('-')
                .next()
                .unwrap_or_default()
                .to_string(),
            phase: Some(phase),
            ..Default::default()
        }
    }

    fn result(operation: &str, success: bool, outcome: &str, message: &str) -> OperationResult {
        OperationResult {
            success,
            operation: Some(operation.to_string()),
            outcome: Some(outcome.to_string()),
            message: Some(message.to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn joins_gate_events_and_structured_results_by_operation_id() {
        let mut log = OperationsLog::new();

        // El gate manda timestamps ISO; el log los normaliza a epoch ms.
        log.record_event(&OperationEvent {
            operations: vec![QueueOperation {
                operation_id: Some("push-1".to_string()),
                queued_at: Some(to_iso_string(100)),
                ..Default::default()
            }],
            ..event("push-1", OperationPhase::Queued)
        });
        log.record_event(&OperationEvent {
            operations: vec![QueueOperation {
                operation_id: Some("push-1".to_string()),
                queued_at: Some(to_iso_string(100)),
                started_at: Some(to_iso_string(150)),
                ..Default::default()
            }],
            ..event("push-1", OperationPhase::Running)
        });
        log.record_result(
            &OperationResult {
                operation_id: Some("push-1".to_string()),
                code: Some("OK".to_string()),
                ..result("push", true, "completed", "Pushed 3 commits.")
            },
            ResultContext {
                repo_name: Some("demo"),
                repo_path: Some("/repo"),
            },
        );

        assert_eq!(log.entries.len(), 1);
        let entry = &log.entries[0];
        assert_eq!(entry.queued_at, Some(100));
        assert_eq!(entry.started_at, Some(150));
        assert_eq!(entry.phase, Some(OperationPhase::Running));
        assert_eq!(entry.outcome.as_deref(), Some("completed"));
        assert_eq!(entry.message.as_deref(), Some("Pushed 3 commits."));
        assert_eq!(entry.repo_name.as_deref(), Some("demo"));
        assert_eq!(entry.repo_path.as_deref(), Some("/repo"));
        assert!(entry.finished_at.is_some());
        assert_eq!(log.unseen_errors, 0);
    }

    #[test]
    fn counts_unseen_errors_once_per_entry_and_clears_them_on_mark_seen() {
        let mut log = OperationsLog::new();

        log.record_result(
            &OperationResult {
                operation_id: Some("pull-1".to_string()),
                ..result("pull", false, "failed", "x")
            },
            ResultContext::default(),
        );
        log.record_event(&OperationEvent {
            error: Some(OperationEventError {
                code: Some("GIT_ERROR".to_string()),
                message: Some("x".to_string()),
            }),
            ..event("pull-1", OperationPhase::Failed)
        });

        assert_eq!(log.unseen_errors, 1);
        assert_eq!(log.entries.len(), 1);
        // El resultado no traía código: lo aporta el evento.
        assert_eq!(log.entries[0].code.as_deref(), Some("GIT_ERROR"));
        assert_eq!(log.error_entries().count(), 1);
        log.mark_seen();
        assert_eq!(log.unseen_errors, 0);
    }

    #[test]
    fn creates_standalone_entries_for_results_without_an_operation_id_and_caps_the_list() {
        let mut log = OperationsLog::new();

        for index in 0..230 {
            log.record_result(
                &result("commit", true, "completed", &format!("m{index}")),
                ResultContext::default(),
            );
        }

        assert_eq!(log.entries.len(), MAX_ENTRIES);
        // Las más nuevas quedan primero
        assert_eq!(log.entries[0].message.as_deref(), Some("m229"));
        assert!(log.entries[0].operation_id.starts_with("local-"));
    }

    #[test]
    fn noop_results_do_not_count_as_errors() {
        let mut log = OperationsLog::new();
        log.record_result(
            &result("fetch", false, "noop", "Already up to date."),
            ResultContext::default(),
        );

        assert_eq!(log.unseen_errors, 0);
        // No es un error para el contador, pero sí queda en la lista de lo que
        // no terminó como `completed`, igual que en el original.
        assert_eq!(log.error_entries().count(), 1);
    }

    #[test]
    fn ignores_events_and_results_that_cannot_be_attributed() {
        let mut log = OperationsLog::new();

        log.record_event(&OperationEvent::default());
        log.record_result(&OperationResult::default(), ResultContext::default());

        assert_eq!(log.entries, []);
    }

    #[test]
    fn a_gate_only_failure_takes_its_outcome_from_the_final_phase() {
        let mut log = OperationsLog::new();

        log.record_event_at(&event("fetch-1", OperationPhase::Running), 1_000);
        log.record_event_at(
            &OperationEvent {
                error: Some(OperationEventError {
                    code: Some("TIMEOUT".to_string()),
                    message: Some("fetch timed out".to_string()),
                }),
                ..event("fetch-1", OperationPhase::Timeout)
            },
            2_500,
        );

        let entry = &log.entries[0];
        assert_eq!(entry.created_at, 1_000);
        assert_eq!(entry.finished_at, Some(2_500));
        assert_eq!(entry.outcome.as_deref(), Some("timeout"));
        assert_eq!(entry.message.as_deref(), Some("fetch timed out"));
        assert_eq!(entry.operation.as_deref(), Some("fetch"));
        assert_eq!(log.unseen_errors, 1);

        // El resultado que llega después manda sobre el outcome, pero el fallo
        // ya estaba contado.
        log.record_result_at(
            &OperationResult {
                operation_id: Some("fetch-1".to_string()),
                ..result("fetch", false, "failed", "fatal: unable to access")
            },
            ResultContext::default(),
            3_000,
        );
        assert_eq!(log.entries[0].outcome.as_deref(), Some("failed"));
        assert_eq!(
            log.entries[0].message.as_deref(),
            Some("fatal: unable to access")
        );
        assert_eq!(log.entries[0].finished_at, Some(2_500));
        assert_eq!(log.unseen_errors, 1);
    }

    #[test]
    fn a_result_without_outcome_falls_back_to_its_success_flag() {
        let mut log = OperationsLog::new();
        let bare = |success| OperationResult {
            success,
            operation: Some("stage".to_string()),
            ..Default::default()
        };

        log.record_result_at(&bare(true), ResultContext::default(), 10);
        log.record_result_at(&bare(false), ResultContext::default(), 11);

        assert_eq!(log.entries[0].outcome.as_deref(), Some("failed"));
        assert_eq!(log.entries[0].operation_id, "local-11-1");
        assert_eq!(log.entries[1].outcome.as_deref(), Some("completed"));
        assert_eq!(log.entries[1].operation_id, "local-10-0");
        assert_eq!(log.unseen_errors, 1);

        log.clear();
        assert_eq!(log, OperationsLog::default());
    }

    #[test]
    fn serializes_entries_with_the_same_field_names_as_the_store() {
        let mut log = OperationsLog::new();
        log.record_result_at(
            &result("commit", true, "completed", "done"),
            ResultContext::default(),
            42,
        );

        assert_eq!(
            serde_json::to_value(&log).unwrap(),
            serde_json::json!({
                "entries": [{
                    "operationId": "local-42-0",
                    "repoPath": null,
                    "repoName": null,
                    "operation": "commit",
                    "phase": null,
                    "outcome": "completed",
                    "code": null,
                    "message": "done",
                    "queuedAt": null,
                    "startedAt": null,
                    "finishedAt": 42,
                    "createdAt": 42
                }],
                "unseenErrors": 0
            })
        );
    }
}
