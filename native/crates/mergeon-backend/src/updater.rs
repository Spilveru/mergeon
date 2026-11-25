//! Punto de enchufe del actualizador (canales `updater:*`, port de
//! `electron/ipc/updater-handlers.js`).
//!
//! `mergeon-updater` se está escribiendo en paralelo y este crate todavía no
//! depende de él. Para no bloquear a la capa de arriba, el actualizador entra
//! por un trait: quien construye el [`crate::Backend`] le pasa (o no) algo que
//! implemente [`UpdaterBridge`]. Sin actualizador los canales responden como
//! el original cuando `getAppUpdater()` no devolvía nada.
//!
//! Cuando `mergeon-updater` esté listo bastará con implementar
//! [`UpdaterBridge`] para su `AppUpdater` (serializando `UpdaterState` e
//! `InstallResult`, y llamando al receptor desde su `subscribe`), o con
//! sustituir aquí los `serde_json::Value` por esos tipos.

use std::sync::Arc;

use serde_json::{Value, json};

/// Receptor de `updater:event`: recibe el estado del actualizador cada vez
/// que cambia.
pub type UpdaterEventSink = Box<dyn Fn(Value) + Send + Sync>;

/// Lo que el backend necesita de un actualizador.
pub trait UpdaterBridge: Send + Sync {
    /// `updater.getState()`: el estado actual, con la forma de `updater:event`.
    fn get_state(&self) -> Value;

    /// `updater.checkForUpdates({ manual })`: busca actualizaciones (bloquea
    /// hasta terminar la comprobación).
    fn check_for_updates(&self, manual: bool);

    /// `updater.installUpdate()`: `{ ok: true }` o `{ ok: false, reason }`.
    fn install_update(&self) -> Value;

    /// Fija el receptor de los cambios de estado. El backend lo llama una vez,
    /// al construirse; cada cambio debe llegar al receptor como el estado
    /// completo.
    fn subscribe(&self, sink: UpdaterEventSink);

    /// La app se cierra: deja de comprobar y de emitir.
    fn shutdown(&self) {}
}

/// `window.api.updater`.
#[derive(Clone, Default)]
pub struct UpdaterApi {
    bridge: Option<Arc<dyn UpdaterBridge>>,
}

impl std::fmt::Debug for UpdaterApi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UpdaterApi")
            .field("available", &self.is_available())
            .finish()
    }
}

impl UpdaterApi {
    pub(crate) fn new(bridge: Option<Arc<dyn UpdaterBridge>>) -> Self {
        Self { bridge }
    }

    /// Hay un actualizador enchufado.
    pub fn is_available(&self) -> bool {
        self.bridge.is_some()
    }

    /// Canal `updater:getState`. `None` si no hay actualizador.
    pub fn get_state(&self) -> Option<Value> {
        self.bridge.as_ref().map(|bridge| bridge.get_state())
    }

    /// Canal `updater:check`: comprueba y devuelve el estado resultante.
    /// `None` si no hay actualizador.
    pub fn check(&self, manual: bool) -> Option<Value> {
        let bridge = self.bridge.as_ref()?;
        bridge.check_for_updates(manual);
        Some(bridge.get_state())
    }

    /// Canal `updater:install`.
    pub fn install(&self) -> Value {
        match &self.bridge {
            Some(bridge) => bridge.install_update(),
            None => json!({ "ok": false, "reason": "unavailable" }),
        }
    }

    pub(crate) fn shutdown(&self) {
        if let Some(bridge) = &self.bridge {
            bridge.shutdown();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeUpdater {
        checks: Mutex<Vec<bool>>,
        sink: Mutex<Option<UpdaterEventSink>>,
    }

    impl UpdaterBridge for FakeUpdater {
        fn get_state(&self) -> Value {
            json!({ "status": "idle", "checks": self.checks.lock().unwrap().len() })
        }

        fn check_for_updates(&self, manual: bool) {
            self.checks.lock().unwrap().push(manual);
            if let Some(sink) = self.sink.lock().unwrap().as_ref() {
                sink(self.get_state());
            }
        }

        fn install_update(&self) -> Value {
            json!({ "ok": true })
        }

        fn subscribe(&self, sink: UpdaterEventSink) {
            *self.sink.lock().unwrap() = Some(sink);
        }
    }

    #[test]
    fn answers_like_the_original_when_there_is_no_updater() {
        let api = UpdaterApi::new(None);

        assert!(!api.is_available());
        assert_eq!(api.get_state(), None);
        assert_eq!(api.check(true), None);
        assert_eq!(
            api.install(),
            json!({ "ok": false, "reason": "unavailable" })
        );
    }

    #[test]
    fn delegates_to_the_bridge() {
        let updater = Arc::new(FakeUpdater::default());
        let api = UpdaterApi::new(Some(updater.clone()));

        assert!(api.is_available());
        assert_eq!(
            api.get_state(),
            Some(json!({ "status": "idle", "checks": 0 }))
        );
        assert_eq!(
            api.check(true),
            Some(json!({ "status": "idle", "checks": 1 }))
        );
        assert_eq!(*updater.checks.lock().unwrap(), [true]);
        assert_eq!(api.install(), json!({ "ok": true }));
    }
}
