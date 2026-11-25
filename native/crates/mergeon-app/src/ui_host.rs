//! Lo que en la app Vue era interfaz global (`notify`, `confirm`, `prompt`,
//! el selector de directorio) y que los flujos piden sin saber cómo se pinta.
//!
//! La cáscara instala un [`UiHost`] con [`AppModel::set_ui_host`]; los tests
//! usan [`crate::testing::RecordingUiHost`]. Los flujos quedan lineales: piden
//! y esperan la respuesta.

use std::path::PathBuf;
use std::rc::Rc;

use futures::channel::oneshot;
use gpui_kit::{App, Context, Task};
use mergeon_model::dialogs::{
    ConfirmAnswer, ConfirmOptions, ConfirmValue, PromptAnswer, PromptOptions,
};
use mergeon_model::format::now_epoch_ms;
use mergeon_model::notify::{
    NotifyError, NotifyOptions, NotifyOutcome, NotifyType, ProgressToast, Toast, ToastId,
    notification_for_operation_result, notification_for_status_flow_result,
};
use mergeon_model::operation_types::OperationResult;

use crate::AppModel;
use crate::credentials::{CredentialsAnswer, CredentialsRequest};

/// Respuesta pendiente de la interfaz. Si quien debía responder suelta el
/// emisor sin contestar, cuenta como diálogo cerrado sin elegir.
pub type Answer<T> = oneshot::Receiver<T>;

/// Interfaz global que los flujos de [`AppModel`] necesitan.
///
/// Las llamadas llegan desde dentro de una actualización de [`AppModel`]: la
/// implementación **no debe leer ni actualizar el modelo** en la propia
/// llamada (GPUI lo tiene tomado); si lo necesita, que lo haga después con
/// `cx.defer` o al responder.
pub trait UiHost: 'static {
    /// Muestra (o reescribe, si el id ya existe) un aviso ya decidido por
    /// `mergeon_model::notify::Notifier`: recortado, deduplicado y con su
    /// duración y botones.
    fn notify(&self, toast: Toast, cx: &mut App);

    /// Muestra (o reescribe) el aviso de una operación larga en curso. No
    /// caduca solo: lo reemplaza el aviso del resultado con el mismo id, o
    /// [`Self::dismiss_toast`].
    fn show_progress(&self, _toast: ProgressToast, _cx: &mut App) {}

    /// Retira un aviso antes de que caduque.
    fn dismiss_toast(&self, _id: &ToastId, _cx: &mut App) {}

    /// Avisos que siguen en pantalla; mientras se vea, un aviso no se repite.
    fn visible_toast_ids(&self, _cx: &App) -> Vec<ToastId> {
        Vec::new()
    }

    /// `confirm(options)`: responde lo elegido, o el `dismiss_value` de las
    /// opciones si el diálogo se cierra sin elegir.
    fn confirm(&self, options: ConfirmOptions, cx: &mut App) -> Answer<ConfirmValue>;

    /// `confirmWithOption(options)`: confirmación con una decisión secundaria.
    fn confirm_with_option(&self, options: ConfirmOptions, cx: &mut App) -> Answer<ConfirmAnswer>;

    /// `prompt(options)`.
    fn prompt(&self, options: PromptOptions, cx: &mut App) -> Answer<PromptAnswer>;

    /// `dialog.openDirectory()`: `None` si se cancela.
    fn pick_directory(&self, cx: &mut App) -> Answer<Option<PathBuf>>;

    /// Pide usuario y contraseña (o token) para un remoto HTTPS. `None` si se
    /// cancela. Sin implementación, nunca contesta nadie (cuenta como
    /// cancelado).
    fn ask_credentials(
        &self,
        _request: CredentialsRequest,
        _cx: &mut App,
    ) -> Answer<Option<CredentialsAnswer>> {
        let (_sender, receiver) = oneshot::channel();
        receiver
    }
}

impl AppModel {
    /// Instala la interfaz global. Los avisos que se pidieron antes de tenerla
    /// (al restaurar la sesión, por ejemplo) se entregan ahora.
    pub fn set_ui_host(&mut self, host: Rc<dyn UiHost>, cx: &mut Context<Self>) {
        for toast in std::mem::take(&mut self.pending_toasts) {
            host.notify(toast, cx);
        }
        self.ui_host = Some(host);
        cx.notify();
    }

    /// `notify.<tipo>(message, options)`. Devuelve el id del aviso que
    /// representa al mensaje (el nuevo, o el que ya estaba en pantalla).
    pub fn notify(
        &mut self,
        kind: NotifyType,
        message: &str,
        options: NotifyOptions,
        cx: &mut Context<Self>,
    ) -> ToastId {
        let visible_ids = match &self.ui_host {
            Some(host) => host.visible_toast_ids(cx),
            None => self
                .pending_toasts
                .iter()
                .map(|toast| toast.id.clone())
                .collect(),
        };

        match self
            .notifier
            .notify(kind, message, options, now_epoch_ms(), &visible_ids)
        {
            NotifyOutcome::Show(toast) => {
                let id = toast.id.clone();
                match self.ui_host.clone() {
                    Some(host) => host.notify(toast, cx),
                    None => {
                        // Un aviso con id propio reescribe al anterior.
                        self.pending_toasts.retain(|pending| pending.id != id);
                        self.pending_toasts.push(toast);
                    }
                }
                id
            }
            NotifyOutcome::Suppressed(id) => id,
        }
    }

    /// Aviso de progreso de una operación larga (ver [`ProgressToast`]). Sin
    /// interfaz instalada no se muestra: no tendría quién lo retire.
    pub fn show_progress(&mut self, toast: ProgressToast, cx: &mut Context<Self>) {
        if let Some(host) = self.ui_host.clone() {
            host.show_progress(toast, cx);
        }
    }

    /// Retira un aviso (el de progreso de una operación que terminó sin
    /// resultado que anunciar, por ejemplo).
    pub fn dismiss_toast(&mut self, id: &ToastId, cx: &mut Context<Self>) {
        if let Some(host) = self.ui_host.clone() {
            host.dismiss_toast(id, cx);
        }
    }

    /// `notify.success(message)`.
    pub fn notify_success(&mut self, message: &str, cx: &mut Context<Self>) -> ToastId {
        self.notify(NotifyType::Success, message, NotifyOptions::default(), cx)
    }

    /// `notify.info(message)`.
    pub fn notify_info(&mut self, message: &str, cx: &mut Context<Self>) -> ToastId {
        self.notify(NotifyType::Info, message, NotifyOptions::default(), cx)
    }

    /// `notify.warning(message)`.
    pub fn notify_warning(&mut self, message: &str, cx: &mut Context<Self>) -> ToastId {
        self.notify(NotifyType::Warning, message, NotifyOptions::default(), cx)
    }

    /// `notify.error(message)`.
    pub fn notify_error(&mut self, message: &str, cx: &mut Context<Self>) -> ToastId {
        self.notify(NotifyType::Error, message, NotifyOptions::default(), cx)
    }

    /// `notify.error(message, { error })`: el texto completo del error va a
    /// "Details".
    pub fn notify_error_with(
        &mut self,
        message: &str,
        error: &dyn std::fmt::Display,
        cx: &mut Context<Self>,
    ) -> ToastId {
        self.notify(
            NotifyType::Error,
            message,
            NotifyOptions {
                error: Some(NotifyError::new(error.to_string())),
                ..Default::default()
            },
            cx,
        )
    }

    /// `reportPersistenceError(context, error)`.
    pub fn report_persistence_error(
        &mut self,
        context: &str,
        error: &dyn std::fmt::Display,
        cx: &mut Context<Self>,
    ) -> ToastId {
        let message = mergeon_model::notify::persistence_error_message(context);
        log::error!("{context}: {error}");
        self.notify(
            NotifyType::Warning,
            &message,
            NotifyOptions {
                error: Some(NotifyError::new(error.to_string())),
                ..Default::default()
            },
            cx,
        )
    }

    /// `notifyOperationResult(result)` de `useGit.js`. Con `id` el aviso
    /// reescribe al anterior con ese id en vez de sumarse a la pila.
    pub fn notify_operation_result(
        &mut self,
        result: &OperationResult,
        id: Option<ToastId>,
        cx: &mut Context<Self>,
    ) {
        if let Some(notification) = notification_for_operation_result(result) {
            let options = NotifyOptions {
                id,
                ..Default::default()
            };
            self.notify(notification.kind, &notification.message, options, cx);
        }
    }

    /// `notifyStatusFlowResult(result)` de `useGit.js`. `id` como en
    /// [`Self::notify_operation_result`].
    pub fn notify_status_flow_result(
        &mut self,
        result: &OperationResult,
        id: Option<ToastId>,
        cx: &mut Context<Self>,
    ) {
        if let Some(notification) = notification_for_status_flow_result(result) {
            let options = NotifyOptions {
                id,
                ..Default::default()
            };
            self.notify(notification.kind, &notification.message, options, cx);
        }
    }

    /// Lo que hace el botón "Details" de un aviso.
    pub fn open_notification_details(&mut self, toast: &Toast, cx: &mut Context<Self>) {
        self.notifier.open_details(toast);
        cx.notify();
    }

    /// Cierra el diálogo de detalle de un aviso.
    pub fn close_notification_details(&mut self, cx: &mut Context<Self>) {
        self.notifier.close_details();
        cx.notify();
    }

    /// Texto del diálogo de detalle; `None` lo mantiene cerrado.
    pub fn notification_details(&self) -> Option<&str> {
        self.notifier.notification_details.as_deref()
    }

    /// `confirm(options)` de `dialogs.js`. Sin interfaz instalada, o si el
    /// diálogo se cierra sin elegir, responde `false`.
    pub fn confirm(&mut self, options: ConfirmOptions, cx: &mut Context<Self>) -> Task<bool> {
        let Some(host) = self.ui_host.clone() else {
            return Task::ready(false);
        };
        let dismiss_value = options.dismiss_value.unwrap_or(Some(false));
        let answer = host.confirm(options, cx);
        cx.foreground_executor()
            .spawn(async move { answer.await.unwrap_or(dismiss_value) == Some(true) })
    }

    /// `confirmWithOption(options)` de `dialogs.js`.
    pub fn confirm_with_option(
        &mut self,
        options: ConfirmOptions,
        cx: &mut Context<Self>,
    ) -> Task<ConfirmAnswer> {
        let dismissed = ConfirmAnswer::WithOption {
            confirmed: false,
            checked: options.option_checked,
        };
        let Some(host) = self.ui_host.clone() else {
            return Task::ready(dismissed);
        };
        let answer = host.confirm_with_option(options, cx);
        cx.foreground_executor()
            .spawn(async move { answer.await.unwrap_or(dismissed) })
    }

    /// `prompt(options)` de `dialogs.js`.
    pub fn prompt(&mut self, options: PromptOptions, cx: &mut Context<Self>) -> Task<PromptAnswer> {
        let dismissed = PromptAnswer {
            confirmed: false,
            value: None,
        };
        let Some(host) = self.ui_host.clone() else {
            return Task::ready(dismissed);
        };
        let answer = host.prompt(options, cx);
        cx.foreground_executor()
            .spawn(async move { answer.await.unwrap_or(dismissed) })
    }

    /// El diálogo de credenciales ([`UiHost::ask_credentials`]).
    pub fn ask_credentials(
        &mut self,
        request: CredentialsRequest,
        cx: &mut Context<Self>,
    ) -> Task<Option<CredentialsAnswer>> {
        let Some(host) = self.ui_host.clone() else {
            return Task::ready(None);
        };
        let answer = host.ask_credentials(request, cx);
        cx.foreground_executor()
            .spawn(async move { answer.await.ok().flatten() })
    }

    /// `window.api.dialog.openDirectory()`.
    pub fn pick_directory(&mut self, cx: &mut Context<Self>) -> Task<Option<PathBuf>> {
        let Some(host) = self.ui_host.clone() else {
            return Task::ready(None);
        };
        let answer = host.pick_directory(cx);
        cx.foreground_executor()
            .spawn(async move { answer.await.ok().flatten() })
    }
}
