//! Progreso real de fetch, pull, push y clone en su aviso.
//!
//! El aviso de progreso (`ProgressToast`) se abre al lanzar la operación con
//! una barra indeterminada. La cola de mutaciones publica después lo que git
//! informa con `--progress` (`BackendEvent::OperationProgress`, ya limitado a
//! ~10 por segundo) y aquí se traduce a la barra y al texto del mismo aviso:
//! "Fetch origin… 45 %" / "Receiving objects 450/1,000 · 2.00 MiB/s". Ninguna
//! actualización avisa a las vistas del modelo: solo se reescribe el aviso
//! en el `UiHost`, y solo si cambia lo que se ve.

use gpui_kit::Context;
use mergeon_backend::{GitProgress, OperationEvent, OperationPhase, OperationProgress};
use mergeon_model::notify::{ProgressToast, ToastAction, ToastId};

use crate::AppModel;
use crate::shell::same_repository_path;

/// Prefijo de la acción "Cancel" del aviso; le sigue el id de la operación.
pub(crate) const CANCEL_OPERATION_ACTION: &str = "operation:cancel:";

/// Un aviso de progreso a la vista y lo último que se pintó en él.
#[derive(Debug, Clone)]
struct ActiveProgress {
    toast: ProgressToast,
    repo_path: String,
    operation_id: Option<String>,
}

#[derive(Debug, Default)]
pub(crate) struct ProgressToasts {
    active: Vec<ActiveProgress>,
}

/// Lo que el aviso muestra de una actualización: el avance de la fase en
/// curso (`None`: barra indeterminada) y su texto.
pub fn describe_progress(progress: &GitProgress) -> (Option<f32>, String) {
    let mut text = if progress.remote {
        format!("Remote: {}", progress.phase)
    } else {
        progress.phase.clone()
    };
    match progress.total {
        Some(total) => text.push_str(&format!(
            " {}/{}",
            group_digits(progress.processed),
            group_digits(total)
        )),
        None if progress.processed > 0 => {
            text.push_str(&format!(" {}", group_digits(progress.processed)))
        }
        None => {}
    }
    if let Some(throughput) = &progress.throughput {
        text.push_str(" · ");
        text.push_str(throughput);
    } else if let Some(transferred) = progress.transferred.as_ref().filter(|_| !progress.done) {
        text.push_str(" · ");
        text.push_str(transferred);
    }
    (progress.fraction(), text)
}

/// `1812` → `1,812`.
fn group_digits(value: u64) -> String {
    let digits = value.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

/// Lo visible de un aviso: el porcentaje entero (lo que se pinta) y el texto.
fn visible(toast: &ProgressToast) -> (Option<u32>, Option<&str>, bool) {
    (
        toast
            .fraction
            .map(|fraction| (fraction.clamp(0., 1.) * 100.).round() as u32),
        toast.description.as_deref(),
        toast.cancel.is_some(),
    )
}

fn cancel_action(operation_id: &str) -> ToastAction {
    ToastAction::custom("Cancel", format!("{CANCEL_OPERATION_ACTION}{operation_id}"))
}

impl ProgressToasts {
    fn find(
        &mut self,
        operation_id: &str,
        operation: &str,
        repo_path: &str,
    ) -> Option<&mut ActiveProgress> {
        // Primero por id (ya visto); si no, la operación de ese repositorio
        // que aún no lo tiene.
        let index = self
            .active
            .iter()
            .position(|active| active.operation_id.as_deref() == Some(operation_id))
            .or_else(|| {
                self.active.iter().position(|active| {
                    active.operation_id.is_none()
                        && active.toast.operation == operation
                        && same_repository_path(&active.repo_path, repo_path)
                })
            })?;
        let active = &mut self.active[index];
        active.operation_id = Some(operation_id.to_string());
        Some(active)
    }
}

impl AppModel {
    /// Abre el aviso de progreso de una operación y lo apunta para las
    /// actualizaciones que lleguen.
    pub(crate) fn begin_progress(
        &mut self,
        toast: ProgressToast,
        repo_path: &str,
        cx: &mut Context<Self>,
    ) {
        self.progress_toasts
            .active
            .retain(|active| active.toast.id != toast.id);
        self.progress_toasts.active.push(ActiveProgress {
            toast: toast.clone(),
            repo_path: repo_path.to_string(),
            operation_id: None,
        });
        self.show_progress(toast, cx);
    }

    /// La operación terminó: sus actualizaciones ya no se pintan.
    pub(crate) fn end_progress(&mut self, id: &ToastId) {
        self.progress_toasts
            .active
            .retain(|active| active.toast.id != *id);
    }

    /// Avisos de progreso abiertos (para tests y diagnósticos).
    pub fn active_progress_toasts(&self) -> Vec<ProgressToast> {
        self.progress_toasts
            .active
            .iter()
            .map(|active| active.toast.clone())
            .collect()
    }

    /// `BackendEvent::OperationProgress`: reescribe el aviso de la operación
    /// si lo que muestra cambia. No avisa a las vistas.
    pub fn handle_operation_progress(
        &mut self,
        update: &OperationProgress,
        cx: &mut Context<Self>,
    ) {
        let Some(active) =
            self.progress_toasts
                .find(&update.operation_id, &update.operation, &update.repo_path)
        else {
            return;
        };
        let (fraction, description) = describe_progress(&update.progress);
        let mut next = active.toast.clone();
        next.fraction = fraction;
        next.description = Some(description);
        next.cancel = Some(cancel_action(&update.operation_id));
        if visible(&next) == visible(&active.toast) {
            return;
        }
        active.toast = next.clone();
        self.show_progress(next, cx);
    }

    /// Al arrancar una operación con aviso de progreso, el aviso gana el
    /// botón "Cancel" (la cola sabe cancelar la operación en curso).
    pub(crate) fn attach_progress_operation(
        &mut self,
        event: &OperationEvent,
        cx: &mut Context<Self>,
    ) {
        if event.phase != OperationPhase::Running {
            return;
        }
        let Some(active) =
            self.progress_toasts
                .find(&event.operation_id, &event.operation, &event.repo_path)
        else {
            return;
        };
        if active.toast.cancel.is_some() {
            return;
        }
        active.toast.cancel = Some(cancel_action(&event.operation_id));
        let toast = active.toast.clone();
        self.show_progress(toast, cx);
    }

    /// Acción "Cancel" del aviso de progreso. `false` si la clave no es suya.
    pub(crate) fn run_progress_toast_action(&mut self, key: &str, cx: &mut Context<Self>) -> bool {
        let Some(operation_id) = key.strip_prefix(CANCEL_OPERATION_ACTION) else {
            return false;
        };
        self.handle_cancel_operation(operation_id, cx).detach();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describes_each_kind_of_update() {
        let receiving = GitProgress {
            phase: "Receiving objects".into(),
            percent: Some(45),
            processed: 1812,
            total: Some(4026),
            transferred: Some("1.20 MiB".into()),
            throughput: Some("2.00 MiB/s".into()),
            ..Default::default()
        };
        let (fraction, text) = describe_progress(&receiving);
        assert!((fraction.unwrap() - 0.45).abs() < 0.01);
        assert_eq!(text, "Receiving objects 1,812/4,026 · 2.00 MiB/s");

        let remote = GitProgress {
            phase: "Counting objects".into(),
            remote: true,
            percent: Some(12),
            processed: 55,
            total: Some(458),
            ..Default::default()
        };
        assert_eq!(
            describe_progress(&remote).1,
            "Remote: Counting objects 55/458"
        );

        let enumerating = GitProgress {
            phase: "Enumerating objects".into(),
            remote: true,
            processed: 1_234_567,
            ..Default::default()
        };
        let (fraction, text) = describe_progress(&enumerating);
        assert_eq!(fraction, None);
        assert_eq!(text, "Remote: Enumerating objects 1,234,567");
    }

    #[test]
    fn groups_digits() {
        assert_eq!(group_digits(0), "0");
        assert_eq!(group_digits(999), "999");
        assert_eq!(group_digits(1000), "1,000");
        assert_eq!(group_digits(12_345_678), "12,345,678");
    }
}
