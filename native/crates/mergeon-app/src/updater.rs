//! Actualizador: el store `src/stores/updater.js`, el composable
//! `src/composables/useUpdater.js` y el adaptador que enchufa
//! `mergeon_updater::AppUpdater` en el backend.
//!
//! La interfaz solo refleja el estado que publica el actualizador; nunca
//! conduce la descarga.

use std::sync::Arc;

use gpui_kit::{Context, Task};
use mergeon_backend::{UpdaterBridge, UpdaterEventSink};
use mergeon_model::notify::{NotifyOptions, NotifyType, ToastAction, ToastId};
use mergeon_updater::{
    AppUpdater, CheckOptions, NoticeKind, UPDATE_NOTICE_ID, UpdateNotifier, UpdaterAction,
    UpdaterState, resolve_activate_action,
};
use serde_json::Value;

use crate::AppModel;
use crate::tasks::flow;

/// Clave de la acción "Restart" del aviso de actualización lista.
pub const UPDATE_ACTION_INSTALL: &str = "updater:install";
/// Clave de las acciones que llevan a la página de descargas.
pub const UPDATE_ACTION_OPEN_RELEASES: &str = "updater:open-releases";
/// Clave de la acción de comprobar actualizaciones.
pub const UPDATE_ACTION_CHECK: &str = "updater:check";

/// Enchufa un [`AppUpdater`] en el backend
/// (`BackendOptions::updater(Arc::new(AppUpdaterBridge::new(updater)))`): los
/// canales `updater:*` y el evento `updater:event` salen de él.
pub struct AppUpdaterBridge {
    updater: Arc<AppUpdater>,
}

impl AppUpdaterBridge {
    pub fn new(updater: Arc<AppUpdater>) -> Self {
        Self { updater }
    }

    /// El actualizador, para arrancar su comprobación periódica
    /// (`AppUpdater::start`) o instalar al salir.
    pub fn updater(&self) -> &Arc<AppUpdater> {
        &self.updater
    }
}

impl std::fmt::Debug for AppUpdaterBridge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppUpdaterBridge").finish_non_exhaustive()
    }
}

fn state_value(state: &UpdaterState) -> Value {
    serde_json::to_value(state).unwrap_or(Value::Null)
}

impl UpdaterBridge for AppUpdaterBridge {
    fn get_state(&self) -> Value {
        state_value(&self.updater.state())
    }

    fn check_for_updates(&self, manual: bool) {
        self.updater.check(CheckOptions { manual });
    }

    fn install_update(&self) -> Value {
        serde_json::to_value(self.updater.install()).unwrap_or(Value::Null)
    }

    fn subscribe(&self, sink: UpdaterEventSink) {
        self.updater
            .subscribe(move |state| sink(state_value(state)));
    }

    fn shutdown(&self) {
        self.updater.shutdown();
    }
}

/// Estado del store `updater` y lo que `useUpdater` recordaba entre eventos.
#[derive(Debug, Default)]
pub(crate) struct UpdaterModel {
    pub state: UpdaterState,
    /// Recuerda la versión ya anunciada para no repetir el aviso de descarga.
    pub notifier: UpdateNotifier,
}

fn action_key(action: UpdaterAction) -> &'static str {
    match action {
        UpdaterAction::Install => UPDATE_ACTION_INSTALL,
        UpdaterAction::OpenReleases => UPDATE_ACTION_OPEN_RELEASES,
        UpdaterAction::Check => UPDATE_ACTION_CHECK,
    }
}

impl AppModel {
    /// Estado del actualizador (el store `updater`): `status`, versiones,
    /// progreso, error y los getters `is_visible`, `is_ready`, `is_busy`...
    pub fn updater_state(&self) -> &UpdaterState {
        &self.updater.state
    }

    /// `store.apply(payload)`.
    pub fn apply_updater_state(&mut self, payload: &UpdaterState, cx: &mut Context<Self>) {
        self.updater.state.apply(payload);
        cx.notify();
    }

    /// `updater.getState()` al arrancar (`onMounted` de `useUpdater`). Sin
    /// actualizador enchufado no hay nada que aplicar.
    pub fn load_updater_state(&mut self, cx: &mut Context<Self>) -> Task<()> {
        let read = self.blocking(cx, |backend| backend.updater().get_state());
        flow(cx, async move |this, cx| {
            let Some(initial) = read.await else {
                return;
            };
            match serde_json::from_value::<UpdaterState>(initial) {
                Ok(state) => this.update(cx, |this, cx| this.apply_updater_state(&state, cx)),
                Err(error) => log::warn!("unexpected updater state: {error}"),
            }
        })
    }

    /// Un `updater:event` tal como lo entrega el backend.
    pub(crate) fn handle_updater_payload(&mut self, payload: &Value, cx: &mut Context<Self>) {
        match serde_json::from_value::<UpdaterState>(payload.clone()) {
            Ok(state) => self.handle_updater_event(&state, cx),
            Err(error) => log::warn!("unexpected updater event: {error}"),
        }
    }

    /// `handleEvent` de `useUpdater`: refleja el estado y avisa de lo que el
    /// usuario pidió ver (comprobación manual) o de una descarga lista. Los
    /// avisos comparten id, así que uno nuevo reemplaza al anterior.
    pub fn handle_updater_event(&mut self, payload: &UpdaterState, cx: &mut Context<Self>) {
        self.apply_updater_state(payload, cx);

        for notice in self.updater.notifier.notices_for(payload) {
            let kind = match notice.kind {
                NoticeKind::Info => NotifyType::Info,
                NoticeKind::Success => NotifyType::Success,
                NoticeKind::Warning => NotifyType::Warning,
            };
            self.notify(
                kind,
                &notice.message,
                NotifyOptions {
                    id: Some(ToastId::from(UPDATE_NOTICE_ID)),
                    action: notice
                        .action
                        .map(|action| ToastAction::custom(action.label, action_key(action.action))),
                    ..Default::default()
                },
                cx,
            );
        }
    }

    /// Ejecuta la acción de un aviso (`ToastActionKind::Custom(clave)`) cuando
    /// el usuario pulsa su botón. Devuelve `false` si la clave no es de este
    /// modelo.
    pub fn run_toast_action(&mut self, key: &str, cx: &mut Context<Self>) -> bool {
        match key {
            UPDATE_ACTION_INSTALL => self.install_update(cx).detach(),
            UPDATE_ACTION_OPEN_RELEASES => self.open_releases(cx).detach(),
            UPDATE_ACTION_CHECK => self.check_for_updates(cx).detach(),
            crate::shell::CHOOSE_EDITOR_ACTION => self.open_settings("git", cx),
            _ => return self.run_git_toast_action(key, cx),
        }
        true
    }

    /// `openReleases`: abre la página de descargas.
    pub fn open_releases(&mut self, cx: &mut Context<Self>) -> Task<()> {
        let url = self.updater.state.releases_url.clone();
        let open = self.open_external(&url, cx);
        cx.background_executor().spawn(async move {
            if let Err(error) = open.await {
                log::warn!("could not open the releases page: {error}");
            }
        })
    }

    /// `install`: reinicia e instala la actualización descargada. Las
    /// plataformas que no se instalan solas (mac ad-hoc, .deb de Linux) caen a
    /// la página de descargas para bajar el instalador a mano.
    pub fn install_update(&mut self, cx: &mut Context<Self>) -> Task<()> {
        let install = self.blocking(cx, |backend| backend.updater().install());
        flow(cx, async move |this, cx| {
            let result = install.await;
            // `!result || result.ok === false`
            let failed = result.is_null() || result.get("ok") == Some(&Value::Bool(false));
            if failed {
                this.update(cx, |this, cx| this.open_releases(cx)).await;
            }
        })
    }

    /// `check`: comprobación pedida por el usuario; el resultado llega como
    /// eventos del actualizador (y sus avisos).
    pub fn check_for_updates(&mut self, cx: &mut Context<Self>) -> Task<()> {
        let check = self.blocking(cx, |backend| backend.updater().check(true));
        flow(cx, async move |_, _| {
            check.await;
        })
    }

    /// `activate`: lo que hace el botón de actualizaciones de Settings según
    /// el estado: instalar, ir a las descargas o volver a comprobar.
    pub fn activate_update(&mut self, cx: &mut Context<Self>) -> Task<()> {
        match resolve_activate_action(&self.updater.state) {
            UpdaterAction::Install => self.install_update(cx),
            UpdaterAction::OpenReleases => self.open_releases(cx),
            UpdaterAction::Check => self.check_for_updates(cx),
        }
    }
}
