//! Contexto de una operación Git en curso: cancelación y timeout.
//!
//! Port de `electron/services/git-operation-context.js`. Allí el contexto viaja
//! implícito con `AsyncLocalStorage`; aquí las operaciones son síncronas, así
//! que viaja en un thread-local: todo `Git` creado dentro de
//! [`run_with_operation_context`] hereda el contexto del hilo.

use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::progress::{GitProgress, ProgressSink};
use crate::runner::GitError;

/// Motivo por el que una operación terminó por control externo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ControlCode {
    #[serde(rename = "CANCELLED")]
    Cancelled,
    #[serde(rename = "TIMEOUT")]
    Timeout,
}

impl ControlCode {
    pub fn as_str(self) -> &'static str {
        match self {
            ControlCode::Cancelled => "CANCELLED",
            ControlCode::Timeout => "TIMEOUT",
        }
    }

    /// Mismo texto que `GitOperationControlError` en la app Electron.
    pub fn message(self) -> &'static str {
        match self {
            ControlCode::Cancelled => "Git operation was cancelled.",
            ControlCode::Timeout => "Git operation timed out.",
        }
    }
}

struct Inner {
    operation_id: Option<String>,
    timeout: Duration,
    cancelled: AtomicBool,
    control_code: Mutex<Option<ControlCode>>,
    /// Adónde va el progreso de los git de la operación que lo piden
    /// (`Git::with_progress`). Lo fija la cola de mutaciones.
    progress: Mutex<Option<ProgressSink>>,
}

/// Contexto compartido entre quien lanza la operación (que puede cancelarla) y
/// el ejecutor de git (que la observa mientras el proceso corre).
#[derive(Clone)]
pub struct OperationContext {
    inner: Arc<Inner>,
}

impl OperationContext {
    pub fn new(operation_id: Option<String>, timeout: Duration) -> Self {
        Self {
            inner: Arc::new(Inner {
                operation_id,
                timeout,
                cancelled: AtomicBool::new(false),
                control_code: Mutex::new(None),
                progress: Mutex::new(None),
            }),
        }
    }

    pub fn operation_id(&self) -> Option<&str> {
        self.inner.operation_id.as_deref()
    }

    /// Tiempo máximo sin salida del proceso antes de matarlo.
    pub fn timeout(&self) -> Duration {
        self.inner.timeout
    }

    /// Pide cancelar: el proceso git en curso se mata y los siguientes no arrancan.
    pub fn cancel(&self) {
        self.set_control_code(ControlCode::Cancelled);
        self.inner.cancelled.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::SeqCst)
    }

    pub fn control_code(&self) -> Option<ControlCode> {
        *self
            .inner
            .control_code
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Fija el receptor del progreso de la operación.
    pub fn set_progress_sink(&self, sink: ProgressSink) {
        *self
            .inner
            .progress
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(sink);
    }

    /// Receptor del progreso, si alguien lo escucha.
    pub fn progress_sink(&self) -> Option<ProgressSink> {
        self.inner
            .progress
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Publica una actualización de progreso (nada si nadie escucha).
    pub fn report_progress(&self, progress: GitProgress) {
        if let Some(sink) = self.progress_sink() {
            sink(progress);
        }
    }

    /// Registra el motivo de control; el primero gana.
    pub fn set_control_code(&self, code: ControlCode) {
        let mut current = self
            .inner
            .control_code
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if current.is_none() {
            *current = Some(code);
        }
    }
}

impl std::fmt::Debug for OperationContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OperationContext")
            .field("operation_id", &self.inner.operation_id)
            .field("timeout", &self.inner.timeout)
            .field("cancelled", &self.is_cancelled())
            .field("control_code", &self.control_code())
            .finish()
    }
}

thread_local! {
    static CURRENT: RefCell<Option<OperationContext>> = const { RefCell::new(None) };
}

struct Restore(Option<OperationContext>);

impl Drop for Restore {
    fn drop(&mut self) {
        CURRENT.with(|current| *current.borrow_mut() = self.0.take());
    }
}

/// Ejecuta `task` con `context` como contexto del hilo actual y restaura el
/// anterior al salir (también si `task` entra en pánico).
pub fn run_with_operation_context<T>(context: OperationContext, task: impl FnOnce() -> T) -> T {
    let previous = CURRENT.with(|current| current.borrow_mut().replace(context));
    let _restore = Restore(previous);
    task()
}

pub fn current_operation_context() -> Option<OperationContext> {
    CURRENT.with(|current| current.borrow().clone())
}

/// Equivalente a `getGitOperationControlCode`: primero el contexto del hilo,
/// después lo que diga el propio error.
pub fn operation_control_code(error: Option<&GitError>) -> Option<ControlCode> {
    if let Some(code) = current_operation_context().and_then(|context| context.control_code()) {
        return Some(code);
    }
    error.and_then(|error| error.control)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_is_scoped_to_the_task() {
        assert!(current_operation_context().is_none());
        let context = OperationContext::new(Some("op-1".into()), Duration::from_secs(1));
        run_with_operation_context(context, || {
            let current = current_operation_context().expect("context");
            assert_eq!(current.operation_id(), Some("op-1"));
        });
        assert!(current_operation_context().is_none());
    }

    #[test]
    fn cancel_sets_the_control_code_once() {
        let context = OperationContext::new(None, Duration::from_secs(1));
        context.set_control_code(ControlCode::Timeout);
        context.cancel();
        assert!(context.is_cancelled());
        assert_eq!(context.control_code(), Some(ControlCode::Timeout));
    }

    #[test]
    fn control_code_prefers_the_context() {
        let context = OperationContext::new(None, Duration::from_secs(1));
        context.cancel();
        run_with_operation_context(context, || {
            assert_eq!(operation_control_code(None), Some(ControlCode::Cancelled));
        });
        assert_eq!(operation_control_code(None), None);
    }
}
