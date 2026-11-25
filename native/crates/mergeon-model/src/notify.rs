//! Avisos (toasts) sin la parte que pinta (`src/lib/ui/notify.js`).
//!
//! Aquí queda lo que decide: el recorte del mensaje, la deduplicación, la
//! duración y qué botones lleva el aviso. [`Notifier::notify`] devuelve el
//! [`Toast`] que la interfaz debe mostrar, o avisa de que ese fallo ya está en
//! pantalla. También decide qué aviso corresponde al resultado de una operación
//! (`notifyOperationResult` y `notifyStatusFlowResult`, que vivían en
//! `useGit.js`).
//!
//! Diferencias con el original:
//!
//! - Las acciones son datos ([`ToastAction`]), no funciones: la interfaz
//!   reacciona a `Details` abriendo el detalle con [`Notifier::open_details`] y
//!   a `Custom(clave)` como le corresponda a quien la pidió.
//! - La identidad de un objeto de error (el `WeakMap` del original) es un token
//!   explícito: [`Notifier::error_token`] da uno y las capas que avisen del
//!   mismo fallo lo comparten en [`NotifyError::token`].
//! - Los ids automáticos los reparte el propio [`Notifier`] (`vue-sonner` los
//!   devolvía al pintar).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;

use crate::jsstr::{collapse_js_whitespace, js_trim, js_trim_end, utf16_len, utf16_prefix};
use crate::operation_types::{OperationResult, truthy};

// Un toast es un resumen de dos o tres líneas; el texto completo (respuesta
// HTTP, stderr de Git) se abre desde "Details", no se pinta en la esquina.
pub const MAX_TOAST_LENGTH: usize = 160;
// Dos llamadas idénticas casi seguidas (p. ej. una capa que notifica y otra
// que la envuelve y vuelve a notificar) son un solo aviso para el usuario.
pub const DUPLICATE_WINDOW_MS: i64 = 2000;
pub const MAX_DURATION_MS: f64 = 8000.0;

static IPC_PREFIX: LazyLock<Regex> = LazyLock::new(|| {
    // `\s` de JavaScript no es el de Rust, así que el conjunto va escrito.
    const JS_SPACE: &str = r"[\t\n\x0B\x0C\r \u{a0}\u{1680}\u{2000}-\u{200a}\u{2028}\u{2029}\u{202f}\u{205f}\u{3000}\u{feff}]";
    Regex::new(&format!(
        r"Error invoking remote method '[^']+':{JS_SPACE}*(?:Error:{JS_SPACE}*)?"
    ))
    .expect("ipc prefix regex")
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NotifyType {
    Success,
    Info,
    Warning,
    Error,
}

impl NotifyType {
    pub fn as_str(self) -> &'static str {
        match self {
            NotifyType::Success => "success",
            NotifyType::Info => "info",
            NotifyType::Warning => "warning",
            NotifyType::Error => "error",
        }
    }

    /// Cuánto se queda en pantalla un aviso de este tipo, en ms.
    pub fn default_duration_ms(self) -> f64 {
        match self {
            NotifyType::Success => 4000.0,
            NotifyType::Info => 5000.0,
            NotifyType::Warning => 7000.0,
            NotifyType::Error => 8000.0,
        }
    }
}

/// Resumen de un mensaje para el toast: sin el prefijo de IPC, en una línea y
/// con tope de [`MAX_TOAST_LENGTH`] (medido, como el original, en unidades
/// UTF-16).
pub fn truncate_notification(message: &str) -> String {
    // El prefijo de IPC de Electron no aporta nada al usuario.
    let without_prefix = IPC_PREFIX.replace_all(message, "");
    let collapsed = collapse_js_whitespace(&without_prefix);
    let text = js_trim(&collapsed);

    if utf16_len(text) > MAX_TOAST_LENGTH {
        format!("{}…", js_trim_end(utf16_prefix(text, MAX_TOAST_LENGTH - 1)))
    } else {
        text.to_string()
    }
}

/// Identificador de un toast: automático (número) o elegido por quien avisa.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToastId {
    Auto(u64),
    Named(String),
}

impl From<&str> for ToastId {
    fn from(id: &str) -> Self {
        ToastId::Named(id.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToastActionKind {
    /// Abre el texto completo del aviso.
    Details,
    /// Acción de quien avisa; la clave dice cuál.
    Custom(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToastAction {
    pub label: String,
    pub kind: ToastActionKind,
}

impl ToastAction {
    pub fn details() -> Self {
        Self {
            label: "Details".to_string(),
            kind: ToastActionKind::Details,
        }
    }

    pub fn custom(label: impl Into<String>, key: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            kind: ToastActionKind::Custom(key.into()),
        }
    }
}

/// El error que motivó el aviso.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NotifyError {
    /// Texto completo, que va a "Details".
    pub message: String,
    /// Identidad del fallo, de [`Notifier::error_token`]: dos avisos con el
    /// mismo token son el mismo fallo aunque cambie el título.
    pub token: Option<u64>,
}

impl NotifyError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            token: None,
        }
    }

    pub fn with_token(message: impl Into<String>, token: u64) -> Self {
        Self {
            message: message.into(),
            token: Some(token),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct NotifyOptions {
    /// Con `id` el llamador es dueño del aviso y lo reescribe en sitio.
    pub id: Option<ToastId>,
    pub error: Option<NotifyError>,
    /// Texto completo para "Details"; manda sobre el del error.
    pub details: Option<String>,
    /// En ms. Solo cuenta si es finita y positiva, y nunca pasa del tope.
    pub duration: Option<f64>,
    pub action: Option<ToastAction>,
    pub cancel: Option<ToastAction>,
    /// Segunda línea, en tono apagado (p. ej. los archivos en conflicto).
    pub description: Option<String>,
}

/// Aviso listo para pintar.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Toast {
    pub id: ToastId,
    #[serde(rename = "type")]
    pub kind: NotifyType,
    /// Resumen recortado.
    pub title: String,
    /// Texto completo, para el diálogo de detalle.
    pub full_message: String,
    /// En ms.
    pub duration: f64,
    pub action: Option<ToastAction>,
    pub cancel: Option<ToastAction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl Toast {
    pub fn duration(&self) -> Duration {
        Duration::from_secs_f64(self.duration.max(0.0) / 1000.0)
    }
}

/// Aviso de una operación larga en curso (fetch, pull, push, clone): sin
/// temporizador y con barra de progreso. El resultado de la operación lo
/// reemplaza (mismo `id`) o lo retira.
#[derive(Debug, Clone, PartialEq)]
pub struct ProgressToast {
    pub id: ToastId,
    /// `fetch`, `pull`, `push` o `clone`: elige el ícono.
    pub operation: String,
    pub title: String,
    pub description: Option<String>,
    /// Avance de la fase en curso entre 0 y 1; `None` es una barra
    /// indeterminada (antes de la primera actualización de git, o en fases
    /// sin porcentaje).
    pub fraction: Option<f32>,
    /// Botón para cancelar la operación, en cuanto la cola la arranca.
    pub cancel: Option<ToastAction>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum NotifyOutcome {
    /// Hay que mostrar (o reescribir, si el id ya existía) este aviso.
    Show(Toast),
    /// Un aviso vivo ya representa a este fallo; no se pinta nada.
    Suppressed(ToastId),
}

impl NotifyOutcome {
    /// Lo que devolvía `notify.<tipo>()`: el id del aviso que representa al mensaje.
    pub fn id(&self) -> &ToastId {
        match self {
            NotifyOutcome::Show(toast) => &toast.id,
            NotifyOutcome::Suppressed(id) => id,
        }
    }

    pub fn toast(&self) -> Option<&Toast> {
        match self {
            NotifyOutcome::Show(toast) => Some(toast),
            NotifyOutcome::Suppressed(_) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct Announced {
    time: i64,
    id: ToastId,
}

/// Registro único de "esto ya se avisó". Todo toast mostrado se apunta aquí bajo
/// cada una de sus claves y todas caducan con la misma regla, la de `is_dead`.
#[derive(Debug, Clone, Default)]
pub struct Notifier {
    announced: HashMap<String, Announced>,
    error_count: u64,
    toast_count: u64,
    /// Texto que muestra el diálogo de detalle; `None` lo mantiene cerrado.
    pub notification_details: Option<String>,
}

// El mismo fallo llega por dos caminos distintos y hay que reconocer los dos:
// varias capas capturan el mismo objeto de error y lo avisan con títulos
// distintos, y un reintento en segundo plano repite el mismo texto con un error
// nuevo cada vez. Por eso un toast se registra bajo las dos claves y alcanza con
// que una siga viva para no repetirlo.
fn keys_for(
    kind: NotifyType,
    title: &str,
    full_message: &str,
    options: &NotifyOptions,
) -> Vec<String> {
    let action_label = options.action.as_ref().map(|action| action.label.as_str());
    let content = serde_json::to_string(&(kind.as_str(), title, full_message, action_label))
        .unwrap_or_default();
    let mut keys = vec![format!("content:{content}")];
    if let Some(token) = options.error.as_ref().and_then(|error| error.token) {
        keys.push(format!("error:{token}"));
    }
    keys
}

// Un aviso deja de representar a su fallo cuando ya no se ve y además pasó su
// ventana: mientras siga en pantalla (el usuario puede tenerlo bajo el cursor)
// los reintentos no apilan nada encima.
fn is_dead(entry: &Announced, now_ms: i64, visible_ids: &[ToastId]) -> bool {
    let age = now_ms - entry.time;
    // Una edad negativa significa que el reloj retrocedió (ajuste de NTP, cambio
    // de hora, suspensión): esa entrada ya no mide nada, y si se la diera por
    // fresca se quedaría callando avisos para siempre sin dar señal de nada.
    let within_window = (0..DUPLICATE_WINDOW_MS).contains(&age);
    !within_window && !visible_ids.contains(&entry.id)
}

impl Notifier {
    pub fn new() -> Self {
        Self::default()
    }

    /// Identidad estable para un fallo que varias capas pueden avisar.
    pub fn error_token(&mut self) -> u64 {
        self.error_count += 1;
        self.error_count
    }

    /// Decide qué hacer con un aviso. `now_ms` es el reloj (epoch ms) y
    /// `visible_ids` los toasts que siguen en pantalla.
    pub fn notify(
        &mut self,
        kind: NotifyType,
        message: &str,
        options: NotifyOptions,
        now_ms: i64,
        visible_ids: &[ToastId],
    ) -> NotifyOutcome {
        let full_message = match (&options.details, &options.error) {
            (Some(details), _) => details.clone(),
            (None, Some(error)) => error.message.clone(),
            (None, None) => message.to_string(),
        };
        let title = truncate_notification(message);
        let keys = keys_for(kind, &title, &full_message, &options);

        self.announced
            .retain(|_, entry| !is_dead(entry, now_ms, visible_ids));

        // Con `id` el llamador es dueño del aviso y lo reescribe en sitio (así el
        // flujo de actualizaciones pisa su propio toast), así que no se deduplica.
        if options.id.is_none()
            && let Some(live) = keys.iter().find_map(|key| self.announced.get(key)).cloned()
        {
            // El aviso vivo ya representa a este fallo, así que el resto de sus claves
            // pasan a apuntarlo: si esta llamada se calló por su texto, la siguiente
            // capa que traiga el mismo objeto de error tampoco debe abrir un segundo
            // toast. Comparten entrada, y por tanto caducan juntas.
            let id = live.id.clone();
            for key in keys {
                self.announced.insert(key, live.clone());
            }
            return NotifyOutcome::Suppressed(id);
        }

        // "Details" aparece cuando hay más texto del que cabe en el resumen o cuando
        // el resumen puede quedar recortado por el clamp de CSS.
        let show_details =
            full_message != title || utf16_len(&title) > 90 || kind == NotifyType::Error;
        let details_action = show_details.then(ToastAction::details);
        let duration = match options.duration {
            Some(duration) if duration.is_finite() && duration > 0.0 => {
                duration.min(MAX_DURATION_MS)
            }
            _ => kind.default_duration_ms(),
        };
        let cancel = match (options.cancel, &options.action) {
            (Some(cancel), _) => Some(cancel),
            (None, Some(_)) => details_action.clone(),
            (None, None) => None,
        };
        let id = options.id.unwrap_or_else(|| {
            self.toast_count += 1;
            ToastId::Auto(self.toast_count)
        });

        for key in keys {
            self.announced.insert(
                key,
                Announced {
                    time: now_ms,
                    id: id.clone(),
                },
            );
        }

        NotifyOutcome::Show(Toast {
            id,
            kind,
            title,
            full_message,
            duration,
            action: options.action.or(details_action),
            cancel,
            description: options.description,
        })
    }

    /// Un fallo al guardar o leer una preferencia no puede quedarse solo en el
    /// log: el usuario cambia algo, no se persiste y nunca se entera. El detalle
    /// técnico va a "Details"; el toast solo dice qué no se guardó.
    pub fn report_persistence_error(
        &mut self,
        context: &str,
        error: NotifyError,
        now_ms: i64,
        visible_ids: &[ToastId],
    ) -> NotifyOutcome {
        log::error!("{context}: {}", error.message);
        self.notify(
            NotifyType::Warning,
            &persistence_error_message(context),
            NotifyOptions {
                error: Some(error),
                ..Default::default()
            },
            now_ms,
            visible_ids,
        )
    }

    /// Lo que hace el botón "Details": abre el diálogo con el texto completo.
    pub fn open_details(&mut self, toast: &Toast) {
        self.notification_details = Some(toast.full_message.clone());
    }

    pub fn close_details(&mut self) {
        self.notification_details = None;
    }
}

/// Texto del aviso de `reportPersistenceError`.
pub fn persistence_error_message(context: &str) -> String {
    format!("{context}. The change may not persist after restarting.")
}

/// Aviso que corresponde a un resultado: de qué tipo y con qué texto.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResultNotification {
    #[serde(rename = "type")]
    pub kind: NotifyType,
    pub message: String,
}

/// Qué aviso merece el resultado de una mutación (`notifyOperationResult`).
/// Sin mensaje no hay aviso.
pub fn notification_for_operation_result(result: &OperationResult) -> Option<ResultNotification> {
    let message = truthy(&result.message)?;
    let code = result.code.as_deref().unwrap_or_default();

    let kind = if result.success && result.outcome.as_deref() == Some("completed") {
        if code == "DETACHED_HEAD" {
            NotifyType::Warning
        } else {
            NotifyType::Success
        }
    } else {
        match code {
            "UPSTREAM_CREATED" => NotifyType::Success,
            "ALREADY_UP_TO_DATE" | "NO_REMOTES" | "ALREADY_IGNORED" | "NO_CHANGES"
            | "NOTHING_TO_STAGE" | "NOTHING_TO_UNSTAGE" | "NOTHING_TO_DISCARD" => NotifyType::Info,
            "CONFLICT"
            | "PARTIAL_FAILURE"
            | "AUTH_REQUIRED"
            | "REMOTE_NOT_FOUND"
            | "REMOTE_PUSH_URL_MISSING"
            | "NO_CURRENT_BRANCH"
            | "COMMIT_MESSAGE_REQUIRED"
            | "COMMIT_BLOCKED_BY_OPERATION"
            | "DETACHED_HEAD"
            | "CHECKOUT_BLOCKED"
            | "STASH_CONFLICT"
            | "TAG_ALREADY_EXISTS" => NotifyType::Warning,
            _ => NotifyType::Error,
        }
    };

    Some(ResultNotification {
        kind,
        message: message.to_string(),
    })
}

/// Qué aviso merece el resultado de continuar/abortar/saltar una operación
/// (`notifyStatusFlowResult`).
pub fn notification_for_status_flow_result(result: &OperationResult) -> Option<ResultNotification> {
    let message = truthy(&result.message)?;

    let kind = match result.code.as_deref().unwrap_or_default() {
        "OK" => NotifyType::Success,
        "NO_OPERATION_IN_PROGRESS" | "ACTION_NOT_SUPPORTED" | "CONFLICTS_REMAIN" => {
            NotifyType::Warning
        }
        _ => NotifyType::Error,
    };

    Some(ResultNotification {
        kind,
        message: message.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_789_000_000_000;

    fn show(outcome: NotifyOutcome) -> Toast {
        match outcome {
            NotifyOutcome::Show(toast) => toast,
            NotifyOutcome::Suppressed(id) => panic!("expected a toast, got suppressed {id:?}"),
        }
    }

    fn simple(notifier: &mut Notifier, kind: NotifyType, message: &str, now: i64) -> NotifyOutcome {
        notifier.notify(kind, message, NotifyOptions::default(), now, &[])
    }

    fn with_error(
        notifier: &mut Notifier,
        message: &str,
        error: NotifyError,
        now: i64,
    ) -> NotifyOutcome {
        let options = NotifyOptions {
            error: Some(error),
            ..Default::default()
        };
        notifier.notify(NotifyType::Error, message, options, now, &[])
    }

    #[test]
    fn leaves_normal_messages_untouched() {
        assert_eq!(
            truncate_notification("No se pudo hacer push."),
            "No se pudo hacer push."
        );
    }

    #[test]
    fn caps_a_message_that_arrives_with_a_whole_http_response_inside() {
        let message = format!(
            "No se pudo comprobar si hay actualizaciones: {}",
            "detalle ".repeat(200)
        );

        let result = truncate_notification(&message);

        assert_eq!(result.chars().count(), 160);
        assert!(result.ends_with('…'));
    }

    #[test]
    fn strips_the_electron_ipc_prefix_and_collapses_whitespace() {
        assert_eq!(
            truncate_notification(
                "Error invoking remote method 'git:push': Error: fatal:\n  could not read\n\nfrom remote"
            ),
            "fatal: could not read from remote"
        );
    }

    #[test]
    fn strips_every_ipc_prefix_and_never_splits_a_character() {
        assert_eq!(
            truncate_notification(
                "Error invoking remote method 'a': Error invoking remote method 'b':   boom"
            ),
            "boom"
        );
        assert_eq!(truncate_notification(" \u{a0} nbsp \u{feff} "), "nbsp");
        assert_eq!(truncate_notification(&"x".repeat(160)), "x".repeat(160));
        // El tope se mide en unidades UTF-16; un emoji que no cabe entero se
        // queda fuera (el original dejaba medio par subrogado).
        let emoji = truncate_notification(&"😀".repeat(100));
        assert_eq!(emoji, format!("{}…", "😀".repeat(79)));
        // El recorte no deja un espacio colgando antes de los puntos suspensivos.
        let spaced = truncate_notification(&format!("{} {}", "x".repeat(158), "y".repeat(20)));
        assert_eq!(spaced, format!("{}…", "x".repeat(158)));
    }

    #[test]
    fn truncates_through_the_public_helpers_and_keeps_the_full_text_behind_details() {
        let mut notifier = Notifier::new();
        let message = "x".repeat(500);

        let toast = show(simple(&mut notifier, NotifyType::Warning, &message, NOW));

        assert_eq!(toast.title, format!("{}…", "x".repeat(159)));
        assert_eq!(toast.duration, 7000.0);
        assert_eq!(toast.action, Some(ToastAction::details()));
        notifier.open_details(&toast);
        assert_eq!(
            notifier.notification_details.as_deref(),
            Some(message.as_str())
        );
        notifier.close_details();
        assert_eq!(notifier.notification_details, None);
    }

    #[test]
    fn keeps_short_informative_toasts_free_of_a_details_button() {
        let mut notifier = Notifier::new();

        let toast = show(simple(&mut notifier, NotifyType::Success, "Pushed.", NOW));

        assert_eq!(toast.title, "Pushed.");
        assert_eq!(toast.duration, 4000.0);
        assert_eq!(toast.action, None);
        assert_eq!(toast.cancel, None);
    }

    #[test]
    fn moves_details_next_to_the_caller_action_so_both_stay_reachable() {
        let mut notifier = Notifier::new();
        let action = ToastAction::custom("Restart", "restart");

        let toast = show(notifier.notify(
            NotifyType::Error,
            "Update failed",
            NotifyOptions {
                action: Some(action.clone()),
                details: Some("stderr output".to_string()),
                ..Default::default()
            },
            NOW,
            &[],
        ));

        assert_eq!(toast.title, "Update failed");
        assert_eq!(toast.action, Some(action));
        assert_eq!(toast.cancel, Some(ToastAction::details()));
        notifier.open_details(&toast);
        assert_eq!(
            notifier.notification_details.as_deref(),
            Some("stderr output")
        );
    }

    #[test]
    fn collapses_identical_toasts_fired_within_the_duplicate_window() {
        let mut notifier = Notifier::new();

        let first = simple(
            &mut notifier,
            NotifyType::Error,
            "Could not read the repository status",
            NOW,
        );
        let second = simple(
            &mut notifier,
            NotifyType::Error,
            "Could not read the repository status",
            NOW,
        );

        assert!(first.toast().is_some());
        assert_eq!(second, NotifyOutcome::Suppressed(first.id().clone()));
    }

    #[test]
    fn shows_the_same_toast_again_once_the_duplicate_window_has_passed() {
        let mut notifier = Notifier::new();

        let first = simple(&mut notifier, NotifyType::Info, "Fetched", NOW);
        let second = simple(&mut notifier, NotifyType::Info, "Fetched", NOW + 3000);

        assert!(first.toast().is_some());
        assert!(second.toast().is_some());
        assert_ne!(first.id(), second.id());
    }

    #[test]
    fn reports_one_error_object_only_once_even_when_several_layers_catch_it() {
        let mut notifier = Notifier::new();
        let token = notifier.error_token();
        let error = NotifyError::with_token("spawn git ENOENT", token);

        let first = with_error(&mut notifier, "Error loading status", error.clone(), NOW);
        let second = with_error(
            &mut notifier,
            "Could not read the repository status",
            error,
            NOW,
        );

        let toast = show(first);
        assert_eq!(toast.title, "Error loading status");
        assert_eq!(second, NotifyOutcome::Suppressed(toast.id.clone()));
        notifier.open_details(&toast);
        assert_eq!(
            notifier.notification_details.as_deref(),
            Some("spawn git ENOENT")
        );
    }

    #[test]
    fn lets_a_caller_update_a_toast_in_place_through_an_explicit_id() {
        let mut notifier = Notifier::new();
        let options = || NotifyOptions {
            id: Some("app-update".into()),
            ..Default::default()
        };

        let first = notifier.notify(
            NotifyType::Info,
            "Checking for updates…",
            options(),
            NOW,
            &[],
        );
        let second = notifier.notify(
            NotifyType::Info,
            "Checking for updates…",
            options(),
            NOW,
            &[],
        );

        assert_eq!(show(first).id, ToastId::Named("app-update".to_string()));
        assert_eq!(show(second).id, ToastId::Named("app-update".to_string()));
    }

    #[test]
    fn never_lets_a_toast_stay_on_screen_longer_than_the_cap() {
        let mut notifier = Notifier::new();
        let lasting = |duration| NotifyOptions {
            duration: Some(duration),
            ..Default::default()
        };

        let slow = show(notifier.notify(
            NotifyType::Warning,
            "Slow",
            lasting(f64::INFINITY),
            NOW,
            &[],
        ));
        let also_slow =
            show(notifier.notify(NotifyType::Info, "Also slow", lasting(60000.0), NOW, &[]));
        let quick = show(notifier.notify(NotifyType::Info, "Quick", lasting(1500.0), NOW, &[]));

        assert_eq!(slow.duration, 7000.0);
        assert_eq!(also_slow.duration, 8000.0);
        assert_eq!(quick.duration(), Duration::from_millis(1500));
    }

    #[test]
    fn clears_stale_actions_when_a_flow_notification_is_replaced() {
        let mut notifier = Notifier::new();
        let with_id = |action| NotifyOptions {
            id: Some("update".into()),
            action,
            ..Default::default()
        };

        let download = ToastAction::custom("Download", "download");
        notifier.notify(
            NotifyType::Info,
            "Update available",
            with_id(Some(download)),
            NOW,
            &[],
        );
        let replaced =
            show(notifier.notify(NotifyType::Success, "Up to date", with_id(None), NOW, &[]));

        assert_eq!(replaced.id, ToastId::Named("update".to_string()));
        assert_eq!(replaced.action, None);
        assert_eq!(replaced.cancel, None);
    }

    #[test]
    fn coalesces_retries_for_as_long_as_the_previous_notice_remains_visible() {
        let mut notifier = Notifier::new();
        let message = "Repeated background failure";

        let first = simple(&mut notifier, NotifyType::Error, message, NOW);
        let visible = [first.id().clone()];
        let retry = notifier.notify(
            NotifyType::Error,
            message,
            NotifyOptions::default(),
            NOW + 12000,
            &visible,
        );
        assert_eq!(retry, NotifyOutcome::Suppressed(first.id().clone()));

        let once_gone = simple(&mut notifier, NotifyType::Error, message, NOW + 12000);
        assert!(once_gone.toast().is_some());
    }

    #[test]
    fn reports_the_same_error_object_again_once_its_notice_is_gone() {
        let mut notifier = Notifier::new();
        let token = notifier.error_token();
        let error = NotifyError::with_token("spawn git ENOENT", token);

        let first = with_error(&mut notifier, "Error loading status", error.clone(), NOW);
        let later = with_error(
            &mut notifier,
            "Error loading status",
            error,
            NOW + 5 * 60 * 1000,
        );

        assert!(first.toast().is_some());
        assert!(later.toast().is_some());
    }

    #[test]
    fn keeps_a_second_layer_quiet_when_the_first_notice_was_silenced_by_its_own_text() {
        // El poll automático ya avisó, así que el reintento se calla por su texto;
        // aun así la capa siguiente trae el mismo error y no debe abrir otro toast.
        let mut notifier = Notifier::new();
        let first_token = notifier.error_token();
        let retry_token = notifier.error_token();
        let retry = NotifyError::with_token("spawn git ENOENT", retry_token);

        let first = with_error(
            &mut notifier,
            "Error loading status",
            NotifyError::with_token("spawn git ENOENT", first_token),
            NOW,
        );
        let silenced = with_error(&mut notifier, "Error loading status", retry.clone(), NOW);
        let second_layer = with_error(
            &mut notifier,
            "Could not read the repository status",
            retry,
            NOW,
        );

        assert!(first.toast().is_some());
        assert_eq!(silenced, NotifyOutcome::Suppressed(first.id().clone()));
        assert_eq!(second_layer, NotifyOutcome::Suppressed(first.id().clone()));
    }

    #[test]
    fn does_not_go_silent_forever_when_the_clock_jumps_backwards() {
        let mut notifier = Notifier::new();

        let first = simple(&mut notifier, NotifyType::Info, "Fetched", NOW);
        let earlier = simple(&mut notifier, NotifyType::Info, "Fetched", NOW - 3_600_000);

        assert!(first.toast().is_some());
        assert!(earlier.toast().is_some());
    }

    #[test]
    fn an_error_without_identity_is_only_recognised_by_its_text() {
        let mut notifier = Notifier::new();

        let first = with_error(
            &mut notifier,
            "Error loading status",
            NotifyError::new("ENOENT"),
            NOW,
        );
        let other_title = with_error(
            &mut notifier,
            "Could not read",
            NotifyError::new("ENOENT"),
            NOW,
        );
        let same_text = with_error(
            &mut notifier,
            "Error loading status",
            NotifyError::new("ENOENT"),
            NOW,
        );

        assert!(first.toast().is_some());
        assert!(other_title.toast().is_some());
        assert_eq!(same_text, NotifyOutcome::Suppressed(first.id().clone()));
    }

    #[test]
    fn an_error_toast_always_offers_details() {
        let mut notifier = Notifier::new();

        let toast = show(simple(&mut notifier, NotifyType::Error, "Push failed", NOW));

        assert_eq!(toast.action, Some(ToastAction::details()));
        assert_eq!(toast.full_message, "Push failed");
        assert_eq!(toast.duration, 8000.0);
    }

    #[test]
    fn reports_persistence_failures_as_a_warning_with_the_cause_in_details() {
        let mut notifier = Notifier::new();

        let toast = show(notifier.report_persistence_error(
            "Failed to save diff preferences",
            NotifyError::new("EACCES: permission denied"),
            NOW,
            &[],
        ));

        assert_eq!(toast.kind, NotifyType::Warning);
        assert_eq!(
            toast.title,
            "Failed to save diff preferences. The change may not persist after restarting."
        );
        assert_eq!(toast.full_message, "EACCES: permission denied");
        assert_eq!(toast.action, Some(ToastAction::details()));
    }

    fn operation_result(
        success: bool,
        outcome: &str,
        code: &str,
        message: &str,
    ) -> OperationResult {
        OperationResult {
            success,
            outcome: Some(outcome.to_string()),
            code: Some(code.to_string()),
            message: Some(message.to_string()),
            ..Default::default()
        }
    }

    fn kind_for(success: bool, outcome: &str, code: &str) -> NotifyType {
        notification_for_operation_result(&operation_result(success, outcome, code, "message"))
            .expect("notification")
            .kind
    }

    #[test]
    fn picks_the_notification_that_matches_an_operation_result() {
        assert_eq!(kind_for(true, "completed", "OK"), NotifyType::Success);
        // Quedar en HEAD desanclado es un éxito que conviene avisar más fuerte.
        assert_eq!(
            kind_for(true, "completed", "DETACHED_HEAD"),
            NotifyType::Warning
        );
        assert_eq!(
            kind_for(false, "blocked", "DETACHED_HEAD"),
            NotifyType::Warning
        );
        assert_eq!(
            kind_for(true, "noop", "UPSTREAM_CREATED"),
            NotifyType::Success
        );
        assert_eq!(
            kind_for(true, "noop", "ALREADY_UP_TO_DATE"),
            NotifyType::Info
        );
        assert_eq!(
            kind_for(false, "noop", "NOTHING_TO_STAGE"),
            NotifyType::Info
        );
        assert_eq!(
            kind_for(false, "conflicted", "CONFLICT"),
            NotifyType::Warning
        );
        assert_eq!(
            kind_for(false, "blocked", "AUTH_REQUIRED"),
            NotifyType::Warning
        );
        assert_eq!(
            kind_for(false, "blocked", "TAG_ALREADY_EXISTS"),
            NotifyType::Warning
        );
        assert_eq!(kind_for(false, "failed", "GIT_ERROR"), NotifyType::Error);
        assert_eq!(kind_for(false, "failed", "IPC_ERROR"), NotifyType::Error);
        // Un "éxito" que no llegó a completarse no es verde por defecto.
        assert_eq!(kind_for(true, "noop", "OK"), NotifyType::Error);

        let notification = notification_for_operation_result(&operation_result(
            true,
            "completed",
            "OK",
            "Pushed.",
        ));
        assert_eq!(
            notification,
            Some(ResultNotification {
                kind: NotifyType::Success,
                message: "Pushed.".to_string()
            })
        );
    }

    #[test]
    fn has_no_notification_for_a_result_without_message() {
        assert_eq!(
            notification_for_operation_result(&OperationResult::default()),
            None
        );
        assert_eq!(
            notification_for_operation_result(&operation_result(true, "completed", "OK", "")),
            None
        );
        assert_eq!(
            notification_for_status_flow_result(&OperationResult::default()),
            None
        );
    }

    #[test]
    fn picks_the_notification_for_continue_abort_and_skip() {
        let kind = |code: &str| {
            notification_for_status_flow_result(&operation_result(false, "failed", code, "message"))
                .expect("notification")
                .kind
        };

        assert_eq!(kind("OK"), NotifyType::Success);
        assert_eq!(kind("NO_OPERATION_IN_PROGRESS"), NotifyType::Warning);
        assert_eq!(kind("ACTION_NOT_SUPPORTED"), NotifyType::Warning);
        assert_eq!(kind("CONFLICTS_REMAIN"), NotifyType::Warning);
        assert_eq!(kind("GIT_ERROR"), NotifyType::Error);
    }

    #[test]
    fn serializes_a_toast_as_plain_data() {
        let mut notifier = Notifier::new();
        let toast = show(simple(&mut notifier, NotifyType::Error, "Push failed", NOW));

        assert_eq!(
            serde_json::to_value(&toast).unwrap(),
            serde_json::json!({
                "id": 1,
                "type": "error",
                "title": "Push failed",
                "fullMessage": "Push failed",
                "duration": 8000.0,
                "action": { "label": "Details", "kind": "details" },
                "cancel": null
            })
        );
    }
}
