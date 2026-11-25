//! Notificación del sistema cuando termina una operación de red larga (fetch,
//! pull, push, clone de más de [`LONG_OPERATION`]) con la ventana sin foco:
//! "Push main to origin finished", "Pull origin/main failed".
//!
//! Se publica con `App::show_system_notification` de GPUI: centro de
//! notificaciones en macOS, notificaciones de Windows y, en Linux, el
//! servicio de notificaciones de D-Bus (vía `notify-rust`, en un hilo
//! propio: sin servidor de notificaciones solo deja un aviso en el log, no
//! bloquea). Se apaga en Settings → Git → Notifications.

use std::time::Duration;

use gpui_kit::{Context, SharedString, SystemNotification};
use mergeon_model::operation_types::OperationResult;
use serde_json::json;

use crate::AppModel;

/// Duración a partir de la cual una operación merece notificación.
pub const LONG_OPERATION: Duration = Duration::from_secs(5);

/// Operaciones que pueden notificar.
pub const NOTIFIED_OPERATIONS: [&str; 4] = ["fetch", "pull", "push", "clone"];

/// Clave de los ajustes donde se guarda la preferencia (sin ella: encendida).
pub const DESKTOP_NOTIFICATIONS_KEY: &str = "desktopNotifications";

/// Lo que decide si una operación terminada se notifica.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FinishedOperation<'a> {
    pub operation: &'a str,
    pub elapsed: Duration,
    pub window_focused: bool,
    /// Sin avisos (auto-fetch en segundo plano).
    pub silent: bool,
}

/// La regla: activada, operación de red, larga, no silenciosa y con la
/// ventana sin foco.
pub fn should_notify_desktop(
    finished: FinishedOperation<'_>,
    enabled: bool,
    threshold: Duration,
) -> bool {
    enabled
        && !finished.silent
        && !finished.window_focused
        && finished.elapsed >= threshold
        && NOTIFIED_OPERATIONS.contains(&finished.operation)
}

fn capitalize(text: &str) -> String {
    let mut characters = text.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().chain(characters).collect(),
        None => String::new(),
    }
}

/// Título y cuerpo de la notificación. `subject` es el título del aviso de
/// progreso sin los puntos suspensivos ("Push main to origin"); `place` dice
/// de qué repositorio es.
pub fn desktop_notification_text(
    operation: &str,
    subject: Option<&str>,
    place: &str,
    result: &OperationResult,
) -> (String, String) {
    let subject = subject
        .map(str::trim)
        .filter(|subject| !subject.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| capitalize(operation));
    let succeeded = result.success || result.outcome.as_deref() == Some("noop");
    if succeeded {
        (format!("{subject} finished"), place.to_string())
    } else {
        let reason = result
            .message
            .as_deref()
            .and_then(|message| message.lines().map(str::trim).find(|line| !line.is_empty()))
            .unwrap_or("Git reported an error.");
        let body = if place.is_empty() {
            reason.to_string()
        } else {
            format!("{place}: {reason}")
        };
        (format!("{subject} failed"), body)
    }
}

impl AppModel {
    /// Notificaciones del sistema al terminar operaciones largas.
    pub fn desktop_notifications_enabled(&self) -> bool {
        self.settings.desktop_notifications
    }

    /// Las enciende o apaga y lo guarda.
    pub fn set_desktop_notifications_enabled(
        &mut self,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Task<()> {
        if self.settings.desktop_notifications == enabled {
            return gpui_kit::Task::ready(());
        }
        self.settings.desktop_notifications = enabled;
        cx.notify();
        self.persist_settings(
            json!({ DESKTOP_NOTIFICATIONS_KEY: enabled }),
            "Failed to save the notification setting",
            cx,
        )
    }

    /// Cuánto debe durar una operación para notificarla (tests).
    #[doc(hidden)]
    pub fn set_desktop_notification_threshold(&mut self, threshold: Duration) {
        self.shell.desktop_notification_threshold = threshold;
    }

    /// Publica la notificación si la regla lo pide.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn notify_desktop_if_unfocused(
        &mut self,
        operation: &str,
        subject: Option<&str>,
        repo_path: &str,
        result: &OperationResult,
        elapsed: Duration,
        silent: bool,
        cx: &mut Context<Self>,
    ) {
        let finished = FinishedOperation {
            operation,
            elapsed,
            window_focused: self.shell.window_focused,
            silent,
        };
        if !should_notify_desktop(
            finished,
            self.settings.desktop_notifications,
            self.shell.desktop_notification_threshold,
        ) {
            return;
        }
        let place = self
            .repositories
            .repositories
            .iter()
            .find(|repository| repository.path == repo_path)
            .map(|repository| repository.name.clone())
            .unwrap_or_else(|| {
                std::path::Path::new(repo_path)
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default()
            });
        let (title, body) = desktop_notification_text(operation, subject, &place, result);
        cx.show_system_notification(SystemNotification {
            tag: SharedString::from(format!("mergeon-{operation}-{repo_path}")),
            title: title.into(),
            body: body.into(),
            actions: Vec::new(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finished(
        operation: &str,
        seconds: u64,
        focused: bool,
        silent: bool,
    ) -> FinishedOperation<'_> {
        FinishedOperation {
            operation,
            elapsed: Duration::from_secs(seconds),
            window_focused: focused,
            silent,
        }
    }

    #[test]
    fn notifies_only_long_network_operations_that_end_without_focus() {
        let rule = |finished, enabled| should_notify_desktop(finished, enabled, LONG_OPERATION);
        for operation in NOTIFIED_OPERATIONS {
            assert!(
                rule(finished(operation, 6, false, false), true),
                "{operation}"
            );
        }
        // Justo en el umbral cuenta.
        assert!(rule(finished("push", 5, false, false), true));
        // Corta, con foco, silenciosa (auto-fetch), apagada u otra operación: no.
        assert!(!rule(finished("push", 4, false, false), true));
        assert!(!rule(finished("push", 60, true, false), true));
        assert!(!rule(finished("fetch", 60, false, true), true));
        assert!(!rule(finished("push", 60, false, false), false));
        assert!(!rule(finished("commit", 60, false, false), true));
        assert!(!rule(finished("checkout", 60, false, false), true));
    }

    #[test]
    fn says_what_finished_or_why_it_failed() {
        let done = OperationResult {
            success: true,
            outcome: Some("completed".into()),
            ..Default::default()
        };
        assert_eq!(
            desktop_notification_text("push", Some("Push main to origin"), "web", &done),
            (
                "Push main to origin finished".to_string(),
                "web".to_string()
            )
        );
        assert_eq!(
            desktop_notification_text("fetch", None, "web", &done).0,
            "Fetch finished"
        );
        let noop = OperationResult {
            success: true,
            outcome: Some("noop".into()),
            ..Default::default()
        };
        assert_eq!(
            desktop_notification_text("fetch", Some("Fetch origin"), "web", &noop).0,
            "Fetch origin finished"
        );

        let failed = OperationResult {
            success: false,
            message: Some(
                "\n  Updates were rejected because the tip is behind.\nhint: pull".into(),
            ),
            ..Default::default()
        };
        assert_eq!(
            desktop_notification_text("pull", Some("Pull origin/main"), "web", &failed),
            (
                "Pull origin/main failed".to_string(),
                "web: Updates were rejected because the tip is behind.".to_string()
            )
        );
    }
}
