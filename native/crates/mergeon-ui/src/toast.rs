//! Toasts de la app (`src/components/ui/toast/ToasterHost.vue` + `src/lib/ui/notify.js`).
//!
//! ```ignore
//! mergeon_ui::toast::success(window, cx, "Pushed.");
//! mergeon_ui::toast::show(
//!     window,
//!     cx,
//!     ToastKind::Error,
//!     "Could not push",
//!     ToastOptions::new().error(&error),
//! );
//! ```
//!
//! El ciclo de vida (entrada, tiempo en pantalla, pausa, salida) es el
//! `ToastManager` de `gpui_kit::base`; la presentación y las reglas de
//! `notify.js` (resumen recortado, botón "Details", duraciones, avisos
//! repetidos) viven aquí. El `Notification` de `gpui_kit::component` no sirve
//! porque fija en 5s el autocierre, solo muestra el botón de cerrar al pasar
//! el cursor y apila los toasts superpuestos: en la app se listan uno bajo
//! otro, el más reciente arriba (`ToasterHost.vue` los deja `position: relative`).
//!
//! Dos variantes de Señal: con botones (`.action(..)` / `.cancel(..)`, p. ej.
//! "Resolve" / "Abort merge" de un pull con conflictos) y de progreso
//! (`.progress(..)`, para fetch/pull/push/clone): sin temporizador ni X, con
//! una barra de 2px determinada o indeterminada, hasta que el resultado de la
//! operación lo reemplaza (mismo id) o se retira con [`dismiss`].

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use gpui_kit::base::{
    Button as BaseButton, Toast as BaseToast, ToastManager, ToastMotion,
    ToastOptions as BaseToastOptions, ToastTransitionStatus,
};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Size};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, App, Context, ElementId, Hsla,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, px, relative, rems,
};

use crate::Lucide;
use crate::dialog::{AppDialog, DialogWidth};
use crate::root::MergeonLayer;
use crate::style::{MergeonStyled as _, Radius, floating_shadow};
use crate::theme::{ActivePaletteExt as _, with_alpha};

/// Un toast es un resumen de dos o tres líneas; el texto completo (respuesta
/// HTTP, stderr de Git) se abre desde "Details".
const MAX_TOAST_LENGTH: usize = 160;
/// Dos llamadas idénticas casi seguidas son un solo aviso para el usuario.
const DUPLICATE_WINDOW: Duration = Duration::from_millis(2000);
const MAX_DURATION: Duration = Duration::from_millis(8000);
/// Intervalo con el que avanza el ciclo de vida de los toasts montados.
const ADVANCE_INTERVAL: Duration = Duration::from_millis(50);
/// `top: 48px` y `right: 16px` de `[data-sonner-toaster]`.
const TOASTER_TOP: f32 = 48.;
const TOASTER_RIGHT: f32 = 16.;
const TOAST_WIDTH: f32 = 360.;
/// Aire entre una tarjeta y la siguiente: pegadas, los bordes y las sombras se
/// funden y la pila se lee como un solo bloque.
const TOAST_GAP: f32 = 8.;
/// Tres toasts a la vista. Uno nuevo cierra al más antiguo en vez de dejarlo
/// esperando oculto: si no, al cerrarse uno reciente reaparecería un aviso
/// viejo que ya no viene al caso.
const MAX_VISIBLE: usize = 3;

const ICON_SUCCESS: &[u8] = include_bytes!("../assets/icons/toast-success.svg");
const ICON_INFO: &[u8] = include_bytes!("../assets/icons/toast-info.svg");
const ICON_WARNING: &[u8] = include_bytes!("../assets/icons/toast-warning.svg");
const ICON_ERROR: &[u8] = include_bytes!("../assets/icons/toast-error.svg");
/// La X de sonner: 12px de trazo fino.
const ICON_CLOSE: &[u8] = include_bytes!("../assets/icons/toast-close.svg");

/// Tipo de aviso (`notify.success`, `notify.info`, `notify.warning`, `notify.error`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToastKind {
    Success,
    Info,
    Warning,
    Error,
}

impl ToastKind {
    /// Tiempo en pantalla por defecto.
    pub fn default_duration(self) -> Duration {
        Duration::from_millis(match self {
            Self::Success => 4000,
            Self::Info => 5000,
            Self::Warning => 7000,
            Self::Error => 8000,
        })
    }

    fn icon(self) -> &'static [u8] {
        match self {
            Self::Success => ICON_SUCCESS,
            Self::Info => ICON_INFO,
            Self::Warning => ICON_WARNING,
            Self::Error => ICON_ERROR,
        }
    }

    /// Color del ícono de estado (Señal: el título va en tinta).
    fn color(self, cx: &App) -> Hsla {
        let palette = cx.palette();
        match self {
            Self::Success => palette.success.into(),
            Self::Info => palette.primary.into(),
            Self::Warning => palette.warning_foreground.into(),
            Self::Error => palette.destructive_foreground.into(),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }
}

/// Identidad de un toast. Con un id explícito el llamador es dueño del aviso y
/// lo reescribe en sitio.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ToastId(SharedString);

impl ToastId {
    pub fn as_str(&self) -> &str {
        self.0.as_ref()
    }
}

/// El id que se pasó con [`ToastOptions::id`], para retirarlo con [`dismiss`].
impl<T: Into<SharedString>> From<T> for ToastId {
    fn from(id: T) -> Self {
        Self(id.into())
    }
}

type ActionHandler = Rc<dyn Fn(&mut Window, &mut App)>;

/// Botón de un toast.
#[derive(Clone)]
pub struct ToastAction {
    label: SharedString,
    handler: ActionHandler,
}

impl ToastAction {
    pub fn new(
        label: impl Into<SharedString>,
        handler: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            label: label.into(),
            handler: Rc::new(handler),
        }
    }

    pub fn label(&self) -> &SharedString {
        &self.label
    }
}

/// Avance de una operación larga en un toast de progreso.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ToastProgress {
    /// Sin cifra conocida: un tramo naranja recorre la barra.
    Indeterminate,
    /// Avance entre 0 y 1; el título lleva el porcentaje.
    Fraction(f32),
}

/// Opciones de [`show`]. Todas tienen un valor por defecto.
#[derive(Clone, Default)]
pub struct ToastOptions {
    id: Option<SharedString>,
    description: Option<SharedString>,
    details: Option<SharedString>,
    source: Option<SharedString>,
    duration: Option<Duration>,
    action: Option<ToastAction>,
    cancel: Option<ToastAction>,
    progress: Option<ToastProgress>,
    icon: Option<Lucide>,
}

impl ToastOptions {
    pub fn new() -> Self {
        Self::default()
    }

    /// Id propio: un segundo aviso con el mismo id reemplaza al primero (así un
    /// flujo como el de actualizaciones pisa su propio toast) y no se deduplica.
    pub fn id(mut self, id: impl Into<SharedString>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Segunda línea, en tono apagado.
    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Texto completo que abre el botón "Details".
    pub fn details(mut self, details: impl Into<SharedString>) -> Self {
        self.details = Some(details.into());
        self
    }

    /// El error que originó el aviso: su texto pasa a "Details" y además
    /// identifica al fallo, de modo que varias capas que avisan del mismo error
    /// con títulos distintos producen un solo toast.
    pub fn error(mut self, error: &dyn std::fmt::Display) -> Self {
        let text: SharedString = error.to_string().into();
        self.source = Some(text.clone());
        if self.details.is_none() {
            self.details = Some(text);
        }
        self
    }

    /// Tiempo en pantalla; nunca supera los 8 segundos.
    pub fn duration(mut self, duration: Duration) -> Self {
        self.duration = Some(duration);
        self
    }

    /// Acción propia. "Details", si corresponde, pasa a su lado.
    pub fn action(
        mut self,
        label: impl Into<SharedString>,
        handler: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        self.action = Some(ToastAction::new(label, handler));
        self
    }

    /// Toast de una operación larga: no caduca, no trae "Details" ni X y
    /// dibuja la barra de progreso. Conviene darle un `.id(..)` para que el
    /// resultado lo reemplace.
    pub fn progress(mut self, progress: ToastProgress) -> Self {
        self.progress = Some(progress);
        self
    }

    /// Ícono propio en lugar del del tipo (p. ej. la flecha de fetch). Toma el
    /// color del tipo.
    pub fn icon(mut self, icon: Lucide) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Segundo botón, a la derecha.
    pub fn cancel(
        mut self,
        label: impl Into<SharedString>,
        handler: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        self.cancel = Some(ToastAction::new(label, handler));
        self
    }
}

/// Recorta un mensaje al resumen que cabe en un toast: colapsa los espacios y
/// lo corta en 160 caracteres con `…`.
pub fn truncate_notification(message: &str) -> String {
    let text = message.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() <= MAX_TOAST_LENGTH {
        return text;
    }
    let head: String = text.chars().take(MAX_TOAST_LENGTH - 1).collect();
    format!("{}…", head.trim_end())
}

/// Lo que se va a mostrar, ya resuelto con las reglas de `notify.js`.
struct ToastPlan {
    title: SharedString,
    full_message: SharedString,
    /// `None`: no caduca (toasts de progreso).
    duration: Option<Duration>,
    shows_details: bool,
    keys: Vec<String>,
}

fn plan(kind: ToastKind, message: &str, options: &ToastOptions) -> ToastPlan {
    let title = truncate_notification(message);
    let full_message: SharedString = options
        .details
        .clone()
        .unwrap_or_else(|| SharedString::from(message.to_string()));
    // "Details" aparece cuando hay más texto del que cabe en el resumen o cuando
    // el resumen puede quedar recortado por el límite de tres líneas.
    let in_progress = options.progress.is_some();
    let shows_details = !in_progress
        && (full_message.as_ref() != title
            || title.chars().count() > 90
            || kind == ToastKind::Error);
    let duration = (!in_progress).then(|| {
        options
            .duration
            .filter(|duration| !duration.is_zero())
            .map_or(kind.default_duration(), |duration| {
                duration.min(MAX_DURATION)
            })
    });

    // El mismo fallo llega por dos caminos y hay que reconocer los dos: varias
    // capas avisan del mismo error con títulos distintos, y un reintento repite
    // el mismo texto. Un toast se registra bajo las dos claves y alcanza con que
    // una siga viva para no repetirlo.
    let action_label = options
        .action
        .as_ref()
        .map(|action| action.label.to_string())
        .unwrap_or_default();
    let mut keys = vec![format!(
        "content:{}\u{1f}{title}\u{1f}{full_message}\u{1f}{action_label}",
        kind.as_str()
    )];
    if let Some(source) = &options.source {
        keys.push(format!("error:{source}"));
    }

    ToastPlan {
        title: title.into(),
        full_message,
        duration,
        shows_details,
        keys,
    }
}

/// Registro único de "esto ya se avisó". Todo toast mostrado se apunta bajo cada
/// una de sus claves y todas caducan con la misma regla.
#[derive(Default)]
struct Announced {
    entries: HashMap<String, (Instant, ToastId)>,
}

impl Announced {
    /// Devuelve el aviso vivo que ya representa a estas claves, si lo hay.
    ///
    /// Un aviso deja de representar a su fallo cuando ya no se ve y además pasó
    /// su ventana: mientras siga en pantalla los reintentos no apilan nada.
    fn live(
        &mut self,
        keys: &[String],
        now: Instant,
        visible: &HashSet<ToastId>,
    ) -> Option<ToastId> {
        self.entries.retain(|_, (time, id)| {
            now.saturating_duration_since(*time) < DUPLICATE_WINDOW || visible.contains(id)
        });
        let live = keys.iter().find_map(|key| self.entries.get(key).cloned())?;
        // El aviso vivo pasa a representar también al resto de las claves: si
        // esta llamada se calló por su texto, la capa siguiente que traiga el
        // mismo error tampoco debe abrir un segundo toast.
        for key in keys {
            self.entries.insert(key.clone(), live.clone());
        }
        Some(live.1)
    }

    fn record(&mut self, keys: Vec<String>, now: Instant, id: &ToastId) {
        for key in keys {
            self.entries.insert(key, (now, id.clone()));
        }
    }
}

struct ToastData {
    kind: ToastKind,
    title: SharedString,
    description: Option<SharedString>,
    action: Option<ToastAction>,
    cancel: Option<ToastAction>,
    progress: Option<ToastProgress>,
    icon: Option<Lucide>,
}

/// Anfitrión de toasts de una ventana (el `<Toaster>` de `ToasterHost.vue`).
pub(crate) struct ToastHost {
    toasts: ToastManager<ToastId, Rc<ToastData>>,
    /// Contenido nuevo de los toasts de progreso reescritos en su sitio (el
    /// gestor no deja cambiar un valor sin volver a apilarlo y animarlo).
    live: HashMap<ToastId, Rc<ToastData>>,
    /// El cursor está sobre la lista: los temporizadores se pausan.
    hovered: bool,
    announced: Announced,
    is_advancing: bool,
}

static NEXT_TOAST: AtomicUsize = AtomicUsize::new(1);

impl ToastHost {
    pub(crate) fn new(_: &mut Context<Self>) -> Self {
        Self {
            toasts: ToastManager::new(ToastMotion::sonner()),
            live: HashMap::new(),
            hovered: false,
            announced: Announced::default(),
            is_advancing: false,
        }
    }

    fn visible_ids(&self) -> HashSet<ToastId> {
        self.toasts
            .iter()
            .filter(|(_, _, status)| *status != ToastTransitionStatus::Ending)
            .map(|(id, _, _)| id.clone())
            .collect()
    }

    /// Títulos de los toasts a la vista, del más antiguo al más reciente.
    pub(crate) fn titles(&self) -> Vec<SharedString> {
        self.toasts
            .iter()
            .filter(|(_, _, status)| *status != ToastTransitionStatus::Ending)
            .map(|(id, toast, _)| self.live.get(id).unwrap_or(toast).title.clone())
            .collect()
    }

    /// Barra y descripción de un toast de progreso a la vista.
    pub(crate) fn progress_of(
        &self,
        id: &ToastId,
    ) -> Option<(ToastProgress, Option<SharedString>)> {
        let (_, toast, _) = self.toasts.iter().find(|(existing, _, status)| {
            *existing == id && *status != ToastTransitionStatus::Ending
        })?;
        let toast = self.live.get(id).unwrap_or(toast);
        Some((toast.progress?, toast.description.clone()))
    }

    fn show(
        &mut self,
        kind: ToastKind,
        message: &str,
        options: ToastOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ToastId {
        let plan = plan(kind, message, &options);
        let now = cx.background_executor().now();

        // Con `id` no se deduplica: el llamador reescribe su propio aviso.
        if options.id.is_none() {
            let visible = self.visible_ids();
            if let Some(live) = self.announced.live(&plan.keys, now, &visible) {
                return live;
            }
        }

        let id = ToastId(options.id.clone().unwrap_or_else(|| {
            format!("toast-{}", NEXT_TOAST.fetch_add(1, Ordering::Relaxed)).into()
        }));
        let details = plan.shows_details.then(|| {
            let text = plan.full_message.clone();
            ToastAction::new("Details", move |window, cx| {
                open_details(text.clone(), window, cx)
            })
        });
        // "Details" ocupa el sitio de la acción; con una acción propia se
        // mueve a su lado para que las dos sigan a mano.
        let (action, cancel) = match options.action {
            Some(action) => (Some(action), options.cancel.or(details)),
            None => (details, options.cancel),
        };
        let data = Rc::new(ToastData {
            kind,
            title: plan.title,
            description: options.description,
            action,
            cancel,
            progress: options.progress,
            icon: options.icon,
        });

        // El avance de un toast de progreso que ya está a la vista se pinta en
        // su sitio: sin volver a entrar, sin moverse a la cabeza de la pila.
        if data.progress.is_some()
            && self.toasts.iter().any(|(existing, value, status)| {
                *existing == id
                    && status != ToastTransitionStatus::Ending
                    && value.progress.is_some()
            })
        {
            self.live.insert(id.clone(), data);
            cx.notify();
            return id;
        }
        self.live.remove(&id);

        self.toasts.push(
            id.clone(),
            data,
            BaseToastOptions {
                timeout: plan.duration,
            },
            now,
        );
        self.announced.record(plan.keys, now, &id);
        let active: Vec<ToastId> = self
            .toasts
            .iter()
            .filter(|(_, _, status)| *status != ToastTransitionStatus::Ending)
            .map(|(id, _, _)| id.clone())
            .collect();
        for oldest in active.iter().take(active.len().saturating_sub(MAX_VISIBLE)) {
            self.toasts.dismiss(oldest, now);
        }
        self.start_advancing(window, cx);
        cx.notify();
        id
    }

    fn dismiss(&mut self, id: &ToastId, window: &mut Window, cx: &mut Context<Self>) {
        if self.toasts.dismiss(id, cx.background_executor().now()) {
            self.start_advancing(window, cx);
            cx.notify();
        }
    }

    /// Avanza el ciclo de vida mientras quede algún toast montado. Sin toasts
    /// no queda ningún temporizador armado.
    fn start_advancing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_advancing {
            return;
        }
        self.is_advancing = true;
        cx.spawn_in(window, async move |host, cx| {
            loop {
                cx.background_executor().timer(ADVANCE_INTERVAL).await;
                let running = host.update(cx, |host, cx| {
                    let now = cx.background_executor().now();
                    // El cursor encima de la lista pausa los temporizadores,
                    // como en sonner.
                    let changes = host.toasts.advance(now, host.hovered);
                    for (id, _) in &changes.removed {
                        host.live.remove(id);
                    }
                    if changes.changed {
                        cx.notify();
                    }
                    host.is_advancing = !host.toasts.is_empty();
                    host.is_advancing
                });
                if !matches!(running, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    fn render_toast(
        &self,
        id: &ToastId,
        toast: &ToastData,
        status: ToastTransitionStatus,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = *cx.palette();
        let closing = status == ToastTransitionStatus::Ending;
        let color = toast.kind.color(cx);
        let shadow = floating_shadow(cx);

        // Señal: la acción es el primario pequeño (naranja con tinta) y la de
        // cancelar, el secundario.
        let button = |slot: &'static str, action: &ToastAction| {
            let handler = action.handler.clone();
            let id = id.clone();
            let primary = slot == "action";
            BaseButton::new(slot)
                .flex_shrink_0()
                .flex()
                .items_center()
                .h(px(24.))
                .px(px(8.))
                .radius(Radius::Md, cx)
                .border_1()
                .map(|this| {
                    if primary {
                        this.bg(palette.primary)
                            .border_color(palette.primary_active)
                            .text_color(palette.primary_foreground)
                            .hover(|style| style.bg(palette.primary_hover))
                    } else {
                        this.bg(palette.bg_base)
                            .border_color(palette.border_base)
                            .text_color(palette.text_1)
                            .hover(|style| style.bg(palette.bg_hover))
                    }
                })
                .text_size(px(12.))
                .font_weight(gpui_kit::FontWeight::MEDIUM)
                .line_height(relative(1.2))
                .cursor_pointer()
                .on_click(cx.listener(move |host, _, window, cx| {
                    handler(window, cx);
                    host.dismiss(&id, window, cx);
                }))
                .child(action.label.clone())
        };
        let has_buttons = toast.action.is_some() || toast.cancel.is_some();
        let in_progress = toast.progress.is_some();
        let mono = cx.theme().mono_font_family.clone();
        let percent = match toast.progress {
            Some(ToastProgress::Fraction(fraction)) => Some(format!(
                "{} %",
                (fraction.clamp(0., 1.) * 100.).round() as u32
            )),
            _ => None,
        };

        BaseToast::new("toast")
            .transition_status(status)
            .relative()
            .occlude()
            .flex()
            .items_start()
            .w_full()
            // Señal: `padding: 10px 12px; gap: 10px`, sitio para la X.
            .pt(px(10.))
            .map(|this| {
                if in_progress {
                    this.pr_3()
                } else {
                    this.pr_10()
                }
            })
            .pb(px(10.))
            .pl_3()
            .gap(px(10.))
            .floating_surface(cx)
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .justify_center()
                    .size(px(18.))
                    .child(match toast.icon {
                        Some(icon) => Icon::new(icon)
                            .with_size(Size::Size(px(16.)))
                            .text_color(color),
                        None => Icon::default()
                            .data(toast.kind.icon())
                            .with_size(Size::Size(px(16.)))
                            .text_color(color),
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(
                        // `64 %` en mono apagado, en la misma línea que el
                        // título (un hijo de bloque bajaba a la línea siguiente).
                        div()
                            .flex()
                            .items_start()
                            .min_w_0()
                            .line_height(px(18.))
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .text_color(palette.text_1)
                            .child(
                                div()
                                    .min_w_0()
                                    .line_clamp(3)
                                    .text_ellipsis()
                                    .child(toast.title.clone()),
                            )
                            .when_some(percent, |this, percent| {
                                this.child(
                                    gpui_kit::div()
                                        .flex_shrink_0()
                                        .ml(px(6.))
                                        .font_family(mono.clone())
                                        .font_weight(gpui_kit::FontWeight::NORMAL)
                                        .text_color(palette.text_3)
                                        .child(percent),
                                )
                            }),
                    )
                    .when_some(toast.description.clone(), |this, description| {
                        this.child(
                            div()
                                .mt(px(1.))
                                .text_size(px(12.))
                                .line_height(px(16.))
                                .text_color(palette.text_2)
                                .line_clamp(2)
                                .text_ellipsis()
                                .child(description),
                        )
                    })
                    .when_some(toast.progress, |this, progress| {
                        this.child(progress_bar(progress, &palette))
                    })
                    .when(has_buttons, |this| {
                        this.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .mt(px(8.))
                                .children(
                                    toast.action.as_ref().map(|action| button("action", action)),
                                )
                                .children(
                                    toast.cancel.as_ref().map(|action| button("cancel", action)),
                                ),
                        )
                    }),
            )
            .when(!in_progress, |this| {
                this.child({
                    let id = id.clone();
                    // La X de 12px queda donde la pone sonner (`top: 0.75rem;
                    // right: 0.75rem`), pero el blanco del clic es una caja de 24px
                    // que se marca al pasar el cursor.
                    BaseButton::new("close")
                        .absolute()
                        .top(rems(0.375))
                        .right(rems(0.375))
                        .size_6()
                        .flex()
                        .items_center()
                        .justify_center()
                        .radius(Radius::Md, cx)
                        .accessibility_label("Close toast")
                        .cursor_pointer()
                        .text_color(with_alpha(palette.muted_foreground, 0.82))
                        .hover(|style| style.bg(palette.accent).text_color(palette.foreground))
                        .on_click(cx.listener(move |host, _, window, cx| {
                            host.dismiss(&id, window, cx);
                        }))
                        .child(
                            Icon::default()
                                .data(ICON_CLOSE)
                                .with_size(Size::Size(px(12.))),
                        )
                })
            })
            .with_animation(
                ElementId::NamedInteger("toast-motion".into(), closing as u64),
                Animation::new(if closing {
                    ToastMotion::sonner().exit_duration
                } else {
                    ToastMotion::sonner().duration
                })
                .with_easing(gpui_kit::ease_out_quint()),
                move |this, delta| {
                    let opacity = if closing { 1. - delta } else { delta };
                    // GPUI pinta la sombra debajo de la tarjeta: mientras la
                    // tarjeta aún es translúcida su propia sombra se vería a
                    // través. Se sube con el cubo de la opacidad.
                    let ink = opacity.powi(3);
                    let shadow = shadow
                        .iter()
                        .map(|layer| gpui_kit::BoxShadow {
                            color: with_alpha(layer.color, ink),
                            ..*layer
                        })
                        .collect();
                    this.top(px(-16. * (1. - opacity)))
                        .opacity(opacity)
                        .shadow(shadow)
                },
            )
            .into_any_element()
    }
}

impl Render for ToastHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.toasts.is_empty() {
            return div().into_any_element();
        }
        let viewport = window.viewport_size();
        // `width: min(360px, calc(100vw - 32px))`
        let width = px(TOAST_WIDTH).min(viewport.width - px(TOASTER_RIGHT * 2.));

        // El más reciente arriba; los demás se listan debajo, pegados.
        let mut items: Vec<_> = self
            .toasts
            .visible(MAX_VISIBLE)
            .map(|(id, toast, status)| {
                let toast = self.live.get(id).unwrap_or(toast).clone();
                (id.clone(), toast, status)
            })
            .collect();
        items.reverse();
        let toasts: Vec<_> = items
            .into_iter()
            .map(|(id, toast, status)| {
                div()
                    .id(ElementId::Name(id.0.clone()))
                    .child(self.render_toast(&id, &toast, status, cx))
            })
            .collect();

        div()
            .size_full()
            .child(
                div()
                    .id("mergeon-toasts")
                    .absolute()
                    .top(px(TOASTER_TOP))
                    .right(px(TOASTER_RIGHT))
                    .w(width)
                    .flex()
                    .flex_col()
                    .gap(px(TOAST_GAP))
                    .on_hover(cx.listener(|host, hovered: &bool, _, _| {
                        host.hovered = *hovered;
                    }))
                    .children(toasts),
            )
            .into_any_element()
    }
}

/// Vuelta del tramo de la barra indeterminada.
const INDETERMINATE_PERIOD: Duration = Duration::from_millis(1400);
/// Ancho del tramo naranja de la barra indeterminada.
const INDETERMINATE_SPAN: f32 = 0.32;

/// `.prog` de Señal: filete de 2px en `border-base` y el avance en naranja.
fn progress_bar(progress: ToastProgress, palette: &crate::theme::Palette) -> AnyElement {
    let track = div()
        .id("toast-progress")
        .relative()
        .mt(px(8.))
        .h(px(2.))
        .w_full()
        .overflow_hidden()
        .bg(palette.border_base);
    let fill = div().absolute().top_0().bottom_0().bg(palette.primary);
    match progress {
        ToastProgress::Fraction(fraction) => track
            .child(fill.left_0().w(relative(fraction.clamp(0., 1.))))
            .into_any_element(),
        ToastProgress::Indeterminate => track
            .child(
                fill.w(relative(INDETERMINATE_SPAN)).with_animation(
                    "toast-progress-sweep",
                    Animation::new(INDETERMINATE_PERIOD)
                        .repeat()
                        .with_easing(gpui_kit::ease_in_out),
                    |this, delta| {
                        this.left(relative(
                            -INDETERMINATE_SPAN + (1. + INDETERMINATE_SPAN) * delta,
                        ))
                    },
                ),
            )
            .into_any_element(),
    }
}

/// Diálogo "Notification details" de `ToasterHost.vue`.
fn open_details(text: SharedString, window: &mut Window, cx: &mut App) {
    AppDialog::new()
        .title("Notification details")
        .width(DialogWidth::Xl2)
        .body(move |_, cx| {
            let mono = cx.theme().mono_font_family.clone();
            div()
                .font_family(mono)
                // `.notification-details { font-size: 0.8rem }`
                .text_size(rems(0.8))
                .child(text.clone())
        })
        .open(window, cx);
}

/// Muestra un toast en la ventana y devuelve su id.
///
/// Si un aviso idéntico (o del mismo error) sigue vivo no se abre otro: se
/// devuelve el id del que ya está en pantalla.
pub fn show(
    window: &mut Window,
    cx: &mut App,
    kind: ToastKind,
    message: impl AsRef<str>,
    options: ToastOptions,
) -> ToastId {
    let message = message.as_ref();
    match MergeonLayer::toasts(window, cx) {
        Some(host) => host.update(cx, |host, cx| host.show(kind, message, options, window, cx)),
        None => {
            // Una ventana sin `Root` de gpui-kit no tiene dónde pintarlo.
            log::warn!("toast sin ventana donde mostrarse: {message}");
            ToastId(options.id.unwrap_or_default())
        }
    }
}

/// `notify.success(message)`.
pub fn success(window: &mut Window, cx: &mut App, message: impl AsRef<str>) -> ToastId {
    show(window, cx, ToastKind::Success, message, ToastOptions::new())
}

/// `notify.info(message)`.
pub fn info(window: &mut Window, cx: &mut App, message: impl AsRef<str>) -> ToastId {
    show(window, cx, ToastKind::Info, message, ToastOptions::new())
}

/// `notify.warning(message)`.
pub fn warning(window: &mut Window, cx: &mut App, message: impl AsRef<str>) -> ToastId {
    show(window, cx, ToastKind::Warning, message, ToastOptions::new())
}

/// `notify.error(message)`. Siempre trae "Details".
pub fn error(window: &mut Window, cx: &mut App, message: impl AsRef<str>) -> ToastId {
    show(window, cx, ToastKind::Error, message, ToastOptions::new())
}

/// Cierra un toast antes de que caduque.
pub fn dismiss(window: &mut Window, cx: &mut App, id: &ToastId) {
    if let Some(host) = MergeonLayer::toasts(window, cx) {
        host.update(cx, |host, cx| host.dismiss(id, window, cx));
    }
}

/// Cierra todos los toasts a la vista (el `clearToasts` de las pruebas).
pub fn dismiss_all(window: &mut Window, cx: &mut App) {
    if let Some(host) = MergeonLayer::toasts(window, cx) {
        host.update(cx, |host, cx| {
            for id in host.visible_ids() {
                host.dismiss(&id, window, cx);
            }
        });
    }
}

/// Títulos de los toasts a la vista en la ventana, del más antiguo al más
/// reciente.
pub fn visible_titles(window: &Window, cx: &App) -> Vec<SharedString> {
    MergeonLayer::toasts(window, cx)
        .map(|host| host.read(cx).titles())
        .unwrap_or_default()
}

/// Barra y descripción del toast de progreso `id`, si está a la vista.
pub fn visible_progress(
    window: &Window,
    cx: &App,
    id: &ToastId,
) -> Option<(ToastProgress, Option<SharedString>)> {
    MergeonLayer::toasts(window, cx).and_then(|host| host.read(cx).progress_of(id))
}

/// `reportPersistenceError`: un fallo al guardar una preferencia no puede
/// quedarse solo en el log. El toast dice qué no se guardó; el detalle técnico
/// va al log y a "Details".
pub fn report_persistence_error(
    window: &mut Window,
    cx: &mut App,
    context: &str,
    error: &dyn std::fmt::Display,
) -> ToastId {
    log::error!("{context}: {error}");
    show(
        window,
        cx,
        ToastKind::Warning,
        format!("{context}. The change may not persist after restarting."),
        ToastOptions::new().error(error),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // Port de la lógica pura de `src/lib/ui/notify.test.js`.

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
    fn collapses_whitespace() {
        assert_eq!(
            truncate_notification("fatal:\n  could not read\n\nfrom remote"),
            "fatal: could not read from remote"
        );
    }

    #[test]
    fn keeps_the_full_text_behind_details_when_the_summary_is_cut() {
        let message = "x".repeat(500);

        let plan = plan(ToastKind::Warning, &message, &ToastOptions::new());

        assert_eq!(plan.title.chars().count(), 160);
        assert!(plan.title.ends_with('…'));
        assert_eq!(plan.duration, Some(Duration::from_millis(7000)));
        assert!(plan.shows_details);
        assert_eq!(plan.full_message.as_ref(), message);
    }

    #[test]
    fn keeps_short_informative_toasts_free_of_a_details_button() {
        let plan = plan(ToastKind::Success, "Pushed.", &ToastOptions::new());

        assert_eq!(plan.duration, Some(Duration::from_millis(4000)));
        assert!(!plan.shows_details);
    }

    #[test]
    fn keeps_a_progress_toast_on_screen_without_details() {
        let plan = plan(
            ToastKind::Info,
            "Fetch origin…",
            &ToastOptions::new().progress(ToastProgress::Indeterminate),
        );

        assert_eq!(plan.duration, None);
        assert!(!plan.shows_details);
    }

    #[test]
    fn errors_always_offer_details() {
        assert!(plan(ToastKind::Error, "Update failed", &ToastOptions::new()).shows_details);
    }

    #[test]
    fn never_lets_a_toast_stay_on_screen_longer_than_the_cap() {
        let long = ToastOptions::new().duration(Duration::from_secs(60));
        assert_eq!(
            plan(ToastKind::Info, "Also slow", &long).duration,
            Some(Duration::from_millis(8000))
        );
        let zero = ToastOptions::new().duration(Duration::ZERO);
        assert_eq!(
            plan(ToastKind::Warning, "Slow", &zero).duration,
            Some(Duration::from_millis(7000))
        );
    }

    fn keys(kind: ToastKind, message: &str, options: &ToastOptions) -> Vec<String> {
        plan(kind, message, options).keys
    }

    #[test]
    fn collapses_identical_toasts_fired_within_the_duplicate_window() {
        let mut announced = Announced::default();
        let now = Instant::now();
        let first = ToastId("a".into());
        let message = "Could not read the repository status";
        let key = keys(ToastKind::Error, message, &ToastOptions::new());

        assert_eq!(announced.live(&key, now, &HashSet::new()), None);
        announced.record(key.clone(), now, &first);

        let later = now + Duration::from_millis(500);
        assert_eq!(
            announced.live(&key, later, &HashSet::new()),
            Some(first.clone())
        );
    }

    #[test]
    fn shows_the_same_toast_again_once_the_duplicate_window_has_passed() {
        let mut announced = Announced::default();
        let now = Instant::now();
        let key = keys(ToastKind::Info, "Fetched", &ToastOptions::new());
        announced.record(key.clone(), now, &ToastId("a".into()));

        let later = now + Duration::from_secs(3);
        assert_eq!(announced.live(&key, later, &HashSet::new()), None);
    }

    #[test]
    fn coalesces_retries_for_as_long_as_the_previous_notice_remains_visible() {
        let mut announced = Announced::default();
        let now = Instant::now();
        let id = ToastId("a".into());
        let key = keys(
            ToastKind::Error,
            "Repeated background failure",
            &ToastOptions::new(),
        );
        announced.record(key.clone(), now, &id);

        let later = now + Duration::from_secs(12);
        let visible = HashSet::from([id.clone()]);
        assert_eq!(announced.live(&key, later, &visible), Some(id));
        assert_eq!(announced.live(&key, later, &HashSet::new()), None);
    }

    #[test]
    fn reports_one_error_only_once_even_when_several_layers_catch_it() {
        let mut announced = Announced::default();
        let now = Instant::now();
        let id = ToastId("a".into());
        let error = "spawn git ENOENT";

        let first = keys(
            ToastKind::Error,
            "Error loading status",
            &ToastOptions::new().error(&error),
        );
        announced.record(first, now, &id);

        let second = keys(
            ToastKind::Error,
            "Could not read the repository status",
            &ToastOptions::new().error(&error),
        );
        assert_eq!(announced.live(&second, now, &HashSet::new()), Some(id));
    }

    /// El poll automático ya avisó y el reintento se calla por su texto; la
    /// capa siguiente trae el mismo error y tampoco debe abrir otro toast.
    #[test]
    fn keeps_a_second_layer_quiet_when_the_first_notice_was_silenced_by_its_own_text() {
        let mut announced = Announced::default();
        let now = Instant::now();
        let id = ToastId("a".into());

        announced.record(
            keys(
                ToastKind::Error,
                "Error loading status",
                &ToastOptions::new().details("spawn git ENOENT"),
            ),
            now,
            &id,
        );
        let retry = keys(
            ToastKind::Error,
            "Error loading status",
            &ToastOptions::new()
                .details("spawn git ENOENT")
                .error(&"retry #2"),
        );
        assert_eq!(
            announced.live(&retry, now, &HashSet::new()),
            Some(id.clone())
        );

        let next_layer = keys(
            ToastKind::Error,
            "Could not read the repository status",
            &ToastOptions::new().error(&"retry #2"),
        );
        assert_eq!(announced.live(&next_layer, now, &HashSet::new()), Some(id));
    }

    #[test]
    fn reports_the_same_error_again_once_its_notice_is_gone() {
        let mut announced = Announced::default();
        let now = Instant::now();
        let options = ToastOptions::new().error(&"spawn git ENOENT");
        let key = keys(ToastKind::Error, "Error loading status", &options);
        announced.record(key.clone(), now, &ToastId("a".into()));

        let later = now + Duration::from_secs(300);
        assert_eq!(announced.live(&key, later, &HashSet::new()), None);
    }
}
