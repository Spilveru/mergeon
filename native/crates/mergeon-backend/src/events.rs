//! Eventos que el backend empuja hacia arriba: lo que en la app Electron viaja
//! por `webContents.send` (`git:status-changed`, `git:operation-event`,
//! `updater:event`).

use std::sync::Arc;

use mergeon_runtime::{OperationEvent, OperationProgress, StatusChange};

/// Un evento del backend. Se entrega por el callback fijado al construir el
/// [`crate::Backend`], desde el hilo que lo provoca (el del watcher, el de la
/// cola de mutaciones o el del actualizador): quien lo recibe debe reenviarlo
/// a su propio hilo y volver enseguida.
#[derive(Debug, Clone, PartialEq)]
pub enum BackendEvent {
    /// `git:status-changed`: cambió el working tree o HEAD del repositorio
    /// vigilado.
    StatusChanged(StatusChange),
    /// `git:operation-event`: una operación de la cola cambió de fase.
    Operation(OperationEvent),
    /// `git:operation-progress` (sin original): el progreso que git informa
    /// en fetch, pull, push y clone, unas 10 veces por segundo como mucho.
    OperationProgress(OperationProgress),
    /// `updater:event`: el estado del actualizador, tal como lo publica quien
    /// implemente [`crate::UpdaterBridge`].
    Updater(serde_json::Value),
}

impl BackendEvent {
    /// Canal IPC por el que viajaba este evento en la app Electron.
    pub fn channel(&self) -> &'static str {
        match self {
            BackendEvent::StatusChanged(_) => "git:status-changed",
            BackendEvent::Operation(_) => "git:operation-event",
            BackendEvent::OperationProgress(_) => "git:operation-progress",
            BackendEvent::Updater(_) => "updater:event",
        }
    }
}

/// Receptor de los eventos del backend.
pub type EventCallback = Arc<dyn Fn(BackendEvent) + Send + Sync>;

/// Receptor que descarta los eventos (un backend sin nadie escuchando).
pub(crate) fn discard_events() -> EventCallback {
    Arc::new(|_event| {})
}
