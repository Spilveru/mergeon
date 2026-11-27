//! Diálogos de la app: `AppDialog.vue` (el diálogo "glass") y los
//! confirm / prompt globales (`GlobalDialogs.vue` + `src/lib/ui/dialogs.js`).
//!
//! ```ignore
//! use mergeon_ui::dialog::{self, AppDialog, ConfirmOptions, DialogWidth};
//!
//! AppDialog::new()
//!     .title("Fetch")
//!     .description("Fetch latest changes from remote repository")
//!     .width(DialogWidth::Lg)
//!     .body(move |_, _| form.clone())
//!     .footer(|_, _| mergeon_ui::button("fetch").sm().label("Fetch"))
//!     .open(window, cx);
//!
//! dialog::confirm(
//!     window,
//!     cx,
//!     ConfirmOptions { title: "Delete tag".into(), ..Default::default() },
//!     |ok, window, cx| { /* ... */ },
//! );
//! ```
//!
//! El foco atrapado, Escape / Enter, el clic en el backdrop y el apilado son los
//! de `gpui_kit::base::Dialog`. La pila vive en la capa de Mergeon de la raíz de
//! la ventana ([`crate::init`]) y no en la de `gpui_kit::component`, cuyo
//! `Dialog` ancla la superficie arriba, la desliza al abrir y fija su sombra:
//! con él no se puede centrar el diálogo ni darle la sombra de `.dialog-glass`.
//!
//! Un diálogo abierto después queda siempre delante: el confirm que lanza otro
//! diálogo no puede quedar tapado por él (lo que en Vue resolvía `stackLevel`).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::base::{Button as BaseButton, Dialog as BaseDialog, active_focus_trap};
use gpui_kit::component::input::{
    Enter, IndentInline, InputEvent, InputState, OutdentInline, TextareaState,
};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{Icon, Sizable as _, Size};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, App, AppContext as _, Context, Div, Entity,
    FocusHandle, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    ScrollHandle, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription,
    TestSupportExt as _, WeakFocusHandle, Window, div, px, rems,
};

use crate::Lucide;
use crate::button::{BUTTON_KEY_CONTEXT, button};
use crate::checkbox::checkbox;
use crate::root::MergeonLayer;
use crate::style::{MergeonStyled as _, Radius, dialog_shadow};
use crate::theme::ActivePaletteExt as _;

/// Ancho máximo del diálogo (las utilidades `max-w-*` que recibe `width`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum DialogWidth {
    /// `max-w-sm` (24rem)
    Sm,
    /// `max-w-md` (28rem): confirm y prompt.
    Md,
    /// `max-w-lg` (32rem)
    Lg,
    /// `max-w-xl` (36rem): el ancho por defecto de `AppDialog`.
    #[default]
    Xl,
    /// `max-w-2xl` (42rem)
    Xl2,
    /// `max-w-3xl` (48rem)
    Xl3,
    /// `max-w-4xl` (56rem)
    Xl4,
    /// `max-w-5xl` (64rem)
    Xl5,
    /// `max-w-6xl` (72rem)
    Xl6,
    /// Ancho arbitrario en rems.
    Rems(f32),
}

impl DialogWidth {
    /// Ancho máximo en rems.
    pub fn rems(self) -> f32 {
        match self {
            Self::Sm => 24.,
            Self::Md => 28.,
            Self::Lg => 32.,
            Self::Xl => 36.,
            Self::Xl2 => 42.,
            Self::Xl3 => 48.,
            Self::Xl4 => 56.,
            Self::Xl5 => 64.,
            Self::Xl6 => 72.,
            Self::Rems(value) => value,
        }
    }
}

type Builder = Rc<dyn Fn(&mut Window, &mut App) -> AnyElement>;
type Callback = Rc<dyn Fn(&mut Window, &mut App)>;
type ConfirmHandler = Rc<dyn Fn(&mut Window, &mut App) -> bool>;

/// Diálogo modal con el estilo "glass" de la app.
#[derive(Clone)]
pub struct AppDialog {
    title: Option<SharedString>,
    description: Option<SharedString>,
    width: DialogWidth,
    close_on_backdrop: bool,
    close_button: bool,
    header: Option<Builder>,
    body: Option<Builder>,
    footer: Option<Builder>,
    on_close: Option<Callback>,
    on_confirm: Option<ConfirmHandler>,
    top_anchored: bool,
    flush_body: bool,
}

impl Default for AppDialog {
    fn default() -> Self {
        Self::new()
    }
}

impl AppDialog {
    pub fn new() -> Self {
        Self {
            title: None,
            description: None,
            width: DialogWidth::default(),
            close_on_backdrop: true,
            close_button: true,
            header: None,
            body: None,
            footer: None,
            on_close: None,
            on_confirm: None,
            top_anchored: false,
            flush_body: false,
        }
    }

    /// Arriba y centrado en horizontal en vez de en el centro de la ventana
    /// (la paleta de comandos: no salta cuando su lista crece o encoge).
    pub fn top_anchored(mut self) -> Self {
        self.top_anchored = true;
        self
    }

    /// El cuerpo sin relleno ni scroll propio: lo pone quien lo dibuja (una
    /// lista virtual que hace su propio scroll).
    pub fn flush_body(mut self) -> Self {
        self.flush_body = true;
        self
    }

    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn width(mut self, width: DialogWidth) -> Self {
        self.width = width;
        self
    }

    /// Si un clic fuera del diálogo lo cierra (`closeOnBackdrop`, por defecto sí).
    pub fn close_on_backdrop(mut self, close: bool) -> Self {
        self.close_on_backdrop = close;
        self
    }

    /// Muestra la X de la esquina (por defecto sí).
    pub fn close_button(mut self, close_button: bool) -> Self {
        self.close_button = close_button;
        self
    }

    /// Slot `header`: sustituye al título y la descripción.
    pub fn header<E: IntoElement>(
        mut self,
        header: impl Fn(&mut Window, &mut App) -> E + 'static,
    ) -> Self {
        self.header = Some(Rc::new(move |window, cx| {
            header(window, cx).into_any_element()
        }));
        self
    }

    /// Slot por defecto: el cuerpo, que hace scroll si no cabe. Se reconstruye
    /// en cada frame, así que suele devolver una entidad o leer estado propio.
    pub fn body<E: IntoElement>(
        mut self,
        body: impl Fn(&mut Window, &mut App) -> E + 'static,
    ) -> Self {
        self.body = Some(Rc::new(move |window, cx| {
            body(window, cx).into_any_element()
        }));
        self
    }

    /// Slot `footer`: fila de acciones alineada a la derecha.
    pub fn footer<E: IntoElement>(
        mut self,
        footer: impl Fn(&mut Window, &mut App) -> E + 'static,
    ) -> Self {
        self.footer = Some(Rc::new(move |window, cx| {
            footer(window, cx).into_any_element()
        }));
        self
    }

    /// Se llama cuando el usuario descarta el diálogo (X, Escape o clic fuera):
    /// el `@close` del original. No se llama al cerrarlo con [`close`].
    pub fn on_close(mut self, on_close: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_close = Some(Rc::new(on_close));
        self
    }

    /// Acción por defecto al pulsar Enter fuera de un campo de texto. Devuelve
    /// `true` para cerrar el diálogo.
    pub fn on_confirm(
        mut self,
        on_confirm: impl Fn(&mut Window, &mut App) -> bool + 'static,
    ) -> Self {
        self.on_confirm = Some(Rc::new(on_confirm));
        self
    }

    /// Abre el diálogo sobre la ventana y le da el foco.
    pub fn open(self, window: &mut Window, cx: &mut App) -> DialogId {
        let Some(layer) = MergeonLayer::get(window, cx) else {
            log::warn!("diálogo sin ventana donde abrirse (falta mergeon_ui::init)");
            return DialogId(0);
        };
        layer.update(cx, |layer, cx| {
            let id = layer.dialogs.open(self, window, cx);
            cx.notify();
            id
        })
    }

    /// `embedded`: el cuerpo y el pie del diálogo, sin superficie ni backdrop,
    /// para incrustarlos en una página.
    pub fn into_embedded(self, window: &mut Window, cx: &mut App) -> Div {
        div()
            .flex()
            .flex_col()
            .gap_4()
            .children(self.body.map(|body| body(window, cx)))
            .children(self.footer.map(|footer| {
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap_2()
                    .child(footer(window, cx))
            }))
    }
}

/// Identidad de un diálogo abierto.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DialogId(usize);

struct ActiveDialog {
    id: DialogId,
    spec: Rc<AppDialog>,
    focus_handle: FocusHandle,
    /// Lo que tenía el foco al abrir, para devolvérselo al cerrar.
    previous_focus: Option<WeakFocusHandle>,
}

/// Pila de diálogos de una ventana.
#[derive(Default)]
pub(crate) struct DialogStack {
    dialogs: Vec<ActiveDialog>,
    next_id: usize,
}

impl DialogStack {
    fn open(&mut self, spec: AppDialog, window: &mut Window, cx: &mut App) -> DialogId {
        self.next_id += 1;
        let id = DialogId(self.next_id);
        let focus_handle = cx.focus_handle();
        // Si el diálogo se abrió desde un menú emergente, ese menú está a punto
        // de cerrarse y desmontarse: no tiene sentido devolverle el foco al salir.
        let from_popup_menu = window
            .context_stack()
            .iter()
            .any(|context| context.contains("PopupMenu"));
        let previous_focus = (!from_popup_menu)
            .then(|| window.focused(cx).map(|handle| handle.downgrade()))
            .flatten();
        focus_handle.focus(window, cx);
        self.dialogs.push(ActiveDialog {
            id,
            spec: Rc::new(spec),
            focus_handle,
            previous_focus,
        });
        id
    }

    /// Cierra el diálogo `id`, o el de más arriba si no se indica.
    fn close(&mut self, id: Option<DialogId>, window: &mut Window, cx: &mut App) -> bool {
        let position = match id {
            Some(id) => self.dialogs.iter().position(|dialog| dialog.id == id),
            None => self.dialogs.len().checked_sub(1),
        };
        let Some(position) = position else {
            return false;
        };
        let closed = self.dialogs.remove(position);
        match self.dialogs.get_mut(position) {
            // Se cerró uno de debajo: el foco sigue en el de arriba, que hereda
            // a quién devolvérselo cuando se cierre.
            Some(above) => above.previous_focus = closed.previous_focus,
            None => {
                if let Some(handle) = closed.previous_focus.and_then(|handle| handle.upgrade()) {
                    handle.focus(window, cx);
                }
            }
        }
        true
    }

    fn close_all(&mut self, window: &mut Window, cx: &mut App) {
        let previous = self
            .dialogs
            .first()
            .and_then(|dialog| dialog.previous_focus.clone());
        self.dialogs.clear();
        if let Some(handle) = previous.and_then(|handle| handle.upgrade()) {
            handle.focus(window, cx);
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.dialogs.len()
    }

    /// Títulos de los diálogos abiertos, de abajo hacia arriba.
    fn titles(&self) -> Vec<SharedString> {
        self.dialogs
            .iter()
            .map(|dialog| dialog.spec.title.clone().unwrap_or_default())
            .collect()
    }

    pub(crate) fn render(&self, window: &mut Window, cx: &mut App) -> Vec<AnyElement> {
        let count = self.dialogs.len();
        self.dialogs
            .iter()
            .enumerate()
            .map(|(layer, dialog)| render_dialog(dialog, layer, layer + 1 == count, window, cx))
            .collect()
    }
}

fn render_dialog(
    dialog: &ActiveDialog,
    layer: usize,
    topmost: bool,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let palette = *cx.palette();
    let spec = dialog.spec.clone();
    let id = dialog.id;
    let viewport_height = window.viewport_size().height;
    let has_header = spec.header.is_some() || spec.title.is_some() || spec.description.is_some();
    let on_close = spec.on_close.clone();
    let on_confirm = spec.on_confirm.clone();

    let dismiss: Callback = {
        let on_close = on_close.clone();
        Rc::new(move |window, cx| {
            close_dialog(Some(id), window, cx);
            if let Some(on_close) = &on_close {
                on_close(window, cx);
            }
        })
    };

    let scroll_handle = window
        .use_keyed_state(("mergeon-dialog-scroll", id.0), cx, |_, _| {
            ScrollHandle::new()
        })
        .read(cx)
        .clone();

    let panel = div()
        .id(("mergeon-dialog", id.0))
        .occlude()
        .relative()
        .flex()
        .flex_col()
        .w_full()
        .max_w(rems(spec.width.rems()))
        .max_h_full()
        .min_h_0()
        // Señal: radio 6, `bg-raised`, borde base y `shadow-3`.
        .radius(Radius::Xxl, cx)
        .bg(palette.dialog_glass)
        .border_1()
        .border_color(palette.dialog_border)
        .shadow(dialog_shadow(cx))
        .text_color(palette.foreground)
        .when(has_header, |this| {
            this.child(
                div()
                    .flex()
                    .flex_col()
                    .flex_shrink_0()
                    .gap_1()
                    .pt(px(14.))
                    .px_4()
                    .pr_10()
                    .map(|header| match &spec.header {
                        Some(builder) => header.child(builder(window, cx)),
                        None => header
                            .when_some(spec.title.clone(), |this, title| {
                                this.child(
                                    div()
                                        .text_size(px(15.))
                                        .line_height(px(20.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(palette.foreground)
                                        .child(title),
                                )
                            })
                            .when_some(spec.description.clone(), |this, description| {
                                this.child(div().text_color(palette.text_2).child(description))
                            }),
                    }),
            )
        })
        .when(spec.flush_body, |this| {
            this.children(
                spec.body
                    .clone()
                    .map(|body| div().flex().flex_col().min_h_0().child(body(window, cx))),
            )
        })
        .when_some(
            spec.body.clone().filter(|_| !spec.flush_body),
            |this, body| {
                // El cuerpo mide lo que su contenido y solo encoge (y hace scroll)
                // cuando el diálogo llega al alto de la ventana.
                this.child(
                    div()
                        .relative()
                        .flex()
                        .flex_col()
                        .min_h_0()
                        .child(
                            div()
                                .id("body")
                                .min_h_0()
                                .overflow_y_scroll()
                                .track_scroll(&scroll_handle)
                                .px_4()
                                .pt(px(if has_header { 12. } else { 16. }))
                                .pb_4()
                                .text_color(palette.text_2)
                                .child(body(window, cx)),
                        )
                        .vertical_scrollbar(&scroll_handle),
                )
            },
        )
        .when(spec.body.is_none() && has_header, |this| {
            this.child(div().h_4())
        })
        .when_some(spec.footer.clone(), |this, footer| {
            // Pie en `bg-inset` con la pista `Esc` a la izquierda.
            this.child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .justify_end()
                    .gap_2()
                    .px_3()
                    .py(px(10.))
                    .border_t_1()
                    .border_color(palette.border)
                    .bg(palette.bg_inset)
                    .rounded_b(Radius::Xxl.px(cx) - px(1.))
                    .child(
                        div()
                            .flex()
                            .flex_shrink_0()
                            .items_center()
                            .gap(px(6.))
                            .text_size(px(12.))
                            .text_color(palette.text_3)
                            .child(crate::kbd::kbd("Esc"))
                            .child("close"),
                    )
                    // Las acciones a la derecha; el contenedor cede el ancho que
                    // ocupe la pista aunque el pie del diálogo pida `w_full`.
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .justify_end()
                            .child(footer(window, cx)),
                    ),
            )
        })
        .when(spec.close_button, |this| {
            let dismiss = dismiss.clone();
            this.child(
                BaseButton::new("close")
                    .key_context(BUTTON_KEY_CONTEXT)
                    .accessibility_label("Close")
                    .absolute()
                    .right(px(10.))
                    .top(px(10.))
                    .p_1()
                    .rounded(Radius::Md.px(cx))
                    .cursor_pointer()
                    .text_color(palette.text_3)
                    .hover(|style| style.bg(palette.accent).text_color(palette.foreground))
                    .on_click(move |_, window, cx| dismiss(window, cx))
                    .child(Icon::new(Lucide::X).with_size(Size::Size(px(16.)))),
            )
        });

    BaseDialog::new(cx)
        .layer(layer, topmost)
        .focus_handle(dialog.focus_handle.clone())
        .close_on_escape(true)
        .close_on_backdrop_press(spec.close_on_backdrop)
        .backdrop(div().size_full().bg(palette.dialog_backdrop))
        .popup(panel)
        // Enter ejecuta la acción por defecto; el cierre lo decide ella.
        .on_ok(move |_, window, cx| {
            if on_confirm
                .as_ref()
                .is_some_and(|confirm| confirm(window, cx))
            {
                close_dialog(Some(id), window, cx);
            }
            false
        })
        .request_close(move |_, window, cx| {
            close_dialog(Some(id), window, cx);
        })
        .on_close(move |_, window, cx| {
            if let Some(on_close) = &on_close {
                on_close(window, cx);
            }
        })
        // `fixed inset-0 flex items-center justify-center p-6`
        .p_6()
        // Arriba, a un octavo del alto de la ventana.
        .when(spec.top_anchored, |this| {
            this.items_start().pt(viewport_height / 8.)
        })
        // `.app-modal-enter-active { transition: opacity 0.15s ease }`
        .with_animation(
            ("mergeon-dialog-fade", id.0),
            Animation::new(Duration::from_millis(150)).with_easing(gpui_kit::ease_in_out),
            |this, delta| this.opacity(delta),
        )
        .into_any_element()
}

fn close_dialog(id: Option<DialogId>, window: &mut Window, cx: &mut App) -> bool {
    let Some(layer) = MergeonLayer::get(window, cx) else {
        return false;
    };
    layer.update(cx, |layer, cx| {
        let closed = layer.dialogs.close(id, window, cx);
        if closed {
            cx.notify();
        }
        closed
    })
}

/// Cierra el diálogo de más arriba. Devuelve `false` si no había ninguno.
pub fn close(window: &mut Window, cx: &mut App) -> bool {
    close_dialog(None, window, cx)
}

/// Cierra un diálogo concreto, esté o no arriba de la pila.
pub fn close_id(window: &mut Window, cx: &mut App, id: DialogId) -> bool {
    close_dialog(Some(id), window, cx)
}

/// Cierra todos los diálogos de la ventana.
pub fn close_all(window: &mut Window, cx: &mut App) {
    if let Some(layer) = MergeonLayer::get(window, cx) {
        layer.update(cx, |layer, cx| {
            layer.dialogs.close_all(window, cx);
            cx.notify();
        });
    }
}

/// Número de diálogos abiertos en la ventana.
pub fn open_count(window: &Window, cx: &App) -> usize {
    MergeonLayer::get(window, cx).map_or(0, |layer| layer.read(cx).dialogs.len())
}

/// Títulos de los diálogos abiertos, de abajo hacia arriba.
pub fn open_titles(window: &Window, cx: &App) -> Vec<SharedString> {
    MergeonLayer::get(window, cx)
        .map(|layer| layer.read(cx).dialogs.titles())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Confirm / prompt (GlobalDialogs.vue + dialogs.js)
// ---------------------------------------------------------------------------

/// Tono del botón de confirmación.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConfirmVariant {
    #[default]
    Default,
    /// Botón rojo para lo que no tiene vuelta atrás.
    Destructive,
}

/// Opciones de [`confirm`] y [`confirm_with_option`]. Un texto vacío toma el
/// valor por defecto del original (`"Are you sure?"`, `"Confirm"`, `"Cancel"`).
#[derive(Debug, Clone, Default)]
pub struct ConfirmOptions {
    pub title: SharedString,
    pub description: SharedString,
    pub confirm_text: SharedString,
    pub cancel_text: SharedString,
    /// Respuesta cuando el diálogo se descarta con la X o Escape.
    pub dismiss_value: bool,
    pub variant: ConfirmVariant,
}

/// Decisión secundaria que acompaña a una confirmación (p. ej. borrar también
/// en el remoto). Empieza apagada salvo que se diga lo contrario: lo
/// irreversible no debería venir marcado de fábrica.
#[derive(Debug, Clone, Default)]
pub struct ConfirmOption {
    pub label: SharedString,
    pub checked: bool,
}

/// Respuesta de [`confirm_with_option`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfirmOutcome {
    pub confirmed: bool,
    /// Estado de la opción al cerrar; solo importa si `confirmed`.
    pub checked: bool,
}

fn or_default(text: &SharedString, default: &'static str) -> SharedString {
    if text.is_empty() {
        default.into()
    } else {
        text.clone()
    }
}

/// Respuesta pendiente que se resuelve una sola vez, llegue por donde llegue
/// (botón, Enter, Escape o la X).
struct Pending<T>(RefCell<Option<Answer<T>>>);

type Answer<T> = Box<dyn FnOnce(T, &mut Window, &mut App)>;
type ConfirmAnswer = Rc<dyn Fn(bool, &mut Window, &mut App)>;
type PromptAccept = Rc<dyn Fn(&Entity<PromptView>, &mut Window, &mut App)>;

/// Validador de [`PromptOptions::validate`]: `true` si el texto es aceptable.
pub type PromptValidator = Rc<dyn Fn(&str) -> bool>;

impl<T> Pending<T> {
    fn new(callback: impl FnOnce(T, &mut Window, &mut App) + 'static) -> Rc<Self> {
        Rc::new(Self(RefCell::new(Some(Box::new(callback)))))
    }

    fn resolve(&self, value: T, window: &mut Window, cx: &mut App) {
        let callback = self.0.borrow_mut().take();
        if let Some(callback) = callback {
            callback(value, window, cx);
        }
    }
}

/// Pide una confirmación. `on_result` recibe `true` si se confirmó.
///
/// ```ignore
/// dialog::confirm(
///     window,
///     cx,
///     ConfirmOptions {
///         title: "Delete branch".into(),
///         description: "This cannot be undone.".into(),
///         confirm_text: "Delete".into(),
///         variant: ConfirmVariant::Destructive,
///         ..Default::default()
///     },
///     move |ok, window, cx| if ok { /* ... */ },
/// );
/// ```
pub fn confirm(
    window: &mut Window,
    cx: &mut App,
    options: ConfirmOptions,
    on_result: impl FnOnce(bool, &mut Window, &mut App) + 'static,
) -> DialogId {
    open_confirm(window, cx, options, None, move |outcome, window, cx| {
        on_result(outcome.confirmed, window, cx)
    })
}

/// Confirma y, de paso, decide algo secundario. Va aparte de [`confirm`] porque
/// devuelve dos valores: mezclar las dos formas se presta a leer la respuesta
/// como un sí.
pub fn confirm_with_option(
    window: &mut Window,
    cx: &mut App,
    options: ConfirmOptions,
    option: ConfirmOption,
    on_result: impl FnOnce(ConfirmOutcome, &mut Window, &mut App) + 'static,
) -> DialogId {
    // Con opción, descartar siempre es un no.
    let options = ConfirmOptions {
        dismiss_value: false,
        ..options
    };
    open_confirm(window, cx, options, Some(option), on_result)
}

fn open_confirm(
    window: &mut Window,
    cx: &mut App,
    options: ConfirmOptions,
    option: Option<ConfirmOption>,
    on_result: impl FnOnce(ConfirmOutcome, &mut Window, &mut App) + 'static,
) -> DialogId {
    let pending = Pending::new(on_result);
    let checked = Rc::new(Cell::new(
        option.as_ref().is_some_and(|option| option.checked),
    ));
    let dialog_id = Rc::new(Cell::new(DialogId(0)));
    let confirm_text = or_default(&options.confirm_text, "Confirm");
    let cancel_text = or_default(&options.cancel_text, "Cancel");
    let destructive = options.variant == ConfirmVariant::Destructive;
    let dismiss_value = options.dismiss_value;

    // Responde y cierra este diálogo, no el que esté arriba en ese momento.
    let answer: ConfirmAnswer = {
        let pending = pending.clone();
        let checked = checked.clone();
        let dialog_id = dialog_id.clone();
        Rc::new(move |confirmed, window, cx| {
            close_dialog(Some(dialog_id.get()), window, cx);
            pending.resolve(
                ConfirmOutcome {
                    confirmed,
                    checked: checked.get(),
                },
                window,
                cx,
            );
        })
    };

    let mut dialog = AppDialog::new()
        .title(or_default(&options.title, "Are you sure?"))
        .width(DialogWidth::Md)
        .close_on_backdrop(false)
        .footer({
            let answer = answer.clone();
            move |_, _| {
                let cancel = answer.clone();
                let accept = answer.clone();
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        button("confirm-cancel")
                            .ghost()
                            .sm()
                            .label(cancel_text.clone())
                            .on_click(move |_, window, cx| cancel(false, window, cx)),
                    )
                    .child(
                        button("confirm-accept")
                            .sm()
                            .when(destructive, |this| this.destructive())
                            .label(confirm_text.clone())
                            .on_click(move |_, window, cx| accept(true, window, cx)),
                    )
            }
        })
        .on_confirm({
            let answer = answer.clone();
            move |window, cx| {
                answer(true, window, cx);
                false
            }
        })
        .on_close({
            let pending = pending.clone();
            let checked = checked.clone();
            move |window, cx| {
                pending.resolve(
                    ConfirmOutcome {
                        confirmed: dismiss_value,
                        checked: checked.get(),
                    },
                    window,
                    cx,
                );
            }
        });
    if !options.description.is_empty() {
        dialog = dialog.description(options.description.clone());
    }
    if let Some(option) = option.filter(|option| !option.label.is_empty()) {
        let checked = checked.clone();
        dialog = dialog.body(move |_, _| {
            let state = checked.clone();
            div()
                // En un confirm destructivo, Enter sobre la casilla la alterna
                // en vez de disparar el borrado accidentalmente.
                .when(destructive, |this| this.key_context(BUTTON_KEY_CONTEXT))
                .child(
                    checkbox("confirm-option")
                        .label(option.label.clone())
                        .checked(checked.get())
                        .on_click(move |value, window, _| {
                            state.set(*value);
                            window.refresh();
                        }),
                )
        });
    }

    let id = dialog.open(window, cx);
    dialog_id.set(id);
    id
}

/// Opciones de [`prompt`]. Un texto vacío toma el valor por defecto del
/// original (`"Enter a value"`, `"Confirm"`, `"Cancel"`, `"Invalid value"`).
#[derive(Clone, Default)]
pub struct PromptOptions {
    pub title: SharedString,
    pub description: SharedString,
    pub confirm_text: SharedString,
    pub cancel_text: SharedString,
    pub placeholder: SharedString,
    pub default_value: SharedString,
    /// Valida el texto antes de aceptar (el `pattern` del original).
    pub validate: Option<PromptValidator>,
    /// Mensaje bajo el campo cuando la validación falla.
    pub input_error_message: SharedString,
    /// Usa un área de texto de `rows` filas en vez de un campo de una línea.
    pub textarea: bool,
    /// Filas del área de texto (3 por defecto).
    pub rows: usize,
}

enum PromptField {
    Input(Entity<InputState>),
    Textarea(Entity<TextareaState>),
}

/// Contenido del prompt: el campo y su mensaje de error.
struct PromptView {
    field: PromptField,
    rows: usize,
    error: Option<SharedString>,
    accept: PromptAccept,
    _subscription: Subscription,
}

impl PromptView {
    fn value(&self, cx: &App) -> SharedString {
        match &self.field {
            PromptField::Input(state) => state.read(cx).value(),
            PromptField::Textarea(state) => state.read(cx).value(),
        }
    }
}

/// Avanza o retrocede el foco respetando la trampa de foco activa (el diálogo),
/// igual que `Root::on_action_tab` / `on_action_tab_prev`.
fn focus_in_trap(window: &mut Window, cx: &mut App, forward: bool) {
    let step = |window: &mut Window, cx: &mut App| {
        if forward {
            window.focus_next(cx);
        } else {
            window.focus_prev(cx);
        }
    };
    if let Some(container) = active_focus_trap(window, cx) {
        let before = window.focused(cx);
        step(window, cx);
        if !container.contains_focused(window, cx) {
            for _ in 0..100 {
                step(window, cx);
                if container.contains_focused(window, cx) || window.focused(cx) == before {
                    break;
                }
            }
        }
        return;
    }
    step(window, cx);
}

impl Render for PromptView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let destructive = cx.palette().destructive;
        let accept = self.accept.clone();
        let view = cx.entity();
        div()
            .flex()
            .flex_col()
            // `space-y-2.5`
            .gap(rems(0.625))
            .child(match &self.field {
                PromptField::Input(state) => crate::input::input(state).into_any_element(),
                PromptField::Textarea(state) => div()
                    .id("prompt-textarea")
                    .capture_action(move |action: &Enter, window, cx| {
                        if action.secondary {
                            cx.stop_propagation();
                            let view = view.clone();
                            let accept = accept.clone();
                            window.defer(cx, move |window, cx| accept(&view, window, cx));
                        }
                    })
                    .capture_action(|_: &IndentInline, window, cx| {
                        cx.stop_propagation();
                        focus_in_trap(window, cx, true);
                    })
                    .capture_action(|_: &OutdentInline, window, cx| {
                        cx.stop_propagation();
                        focus_in_trap(window, cx, false);
                    })
                    .child(crate::input::textarea(state).rows(self.rows))
                    .test_support()
                    .into_any_element(),
            })
            .when_some(self.error.clone(), |this, error| {
                this.child(
                    div()
                        .id("prompt-error")
                        .tw_text_xs()
                        .text_color(destructive)
                        .child(error)
                        .test_support(),
                )
            })
    }
}

/// Pide un texto. `on_result` recibe `Some(valor)` al confirmar y `None` al
/// cancelar o descartar.
///
/// ```ignore
/// dialog::prompt(
///     window,
///     cx,
///     PromptOptions {
///         title: "Rename branch".into(),
///         default_value: branch.name.clone().into(),
///         ..Default::default()
///     },
///     move |name, window, cx| if let Some(name) = name { /* ... */ },
/// );
/// ```
pub fn prompt(
    window: &mut Window,
    cx: &mut App,
    options: PromptOptions,
    on_result: impl FnOnce(Option<SharedString>, &mut Window, &mut App) + 'static,
) -> DialogId {
    let pending = Pending::new(on_result);
    let dialog_id = Rc::new(Cell::new(DialogId(0)));
    let confirm_text = or_default(&options.confirm_text, "Confirm");
    let cancel_text = or_default(&options.cancel_text, "Cancel");
    let error_message = or_default(&options.input_error_message, "Invalid value");
    let validate = options.validate.clone();

    // Valida y, si pasa, responde y cierra.
    let accept: PromptAccept = {
        let pending = pending.clone();
        let dialog_id = dialog_id.clone();
        Rc::new(move |view, window, cx| {
            let value = view.read(cx).value(cx);
            if validate.as_ref().is_some_and(|validate| !validate(&value)) {
                let message = error_message.clone();
                view.update(cx, |view, cx| {
                    view.error = Some(message);
                    cx.notify();
                });
                return;
            }
            close_dialog(Some(dialog_id.get()), window, cx);
            pending.resolve(Some(value), window, cx);
        })
    };

    let view = cx.new(|cx| {
        let placeholder = options.placeholder.clone();
        let default_value = options.default_value.clone();
        let rows = if options.rows == 0 { 3 } else { options.rows };
        if options.textarea {
            let state = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .rows(rows)
                    .placeholder(placeholder)
                    .default_value(default_value)
            });
            let subscription = cx.subscribe_in(
                &state,
                window,
                move |this: &mut PromptView, _, event: &InputEvent, _, cx| {
                    if matches!(event, InputEvent::Change) && this.error.take().is_some() {
                        cx.notify();
                    }
                },
            );
            PromptView {
                field: PromptField::Textarea(state),
                rows,
                error: None,
                accept: accept.clone(),
                _subscription: subscription,
            }
        } else {
            // `set_value` y no `default_value`: deja el cursor al final del
            // texto, listo para seguir escribiendo.
            let state = cx.new(|cx| {
                let mut state = InputState::new(window, cx).placeholder(placeholder);
                state.set_value(default_value, window, cx);
                state
            });
            // Enter dentro del campo confirma, como el botón; editar borra el error.
            let accept_on_enter = accept.clone();
            let subscription = cx.subscribe_in(
                &state,
                window,
                move |this: &mut PromptView, _, event: &InputEvent, window, cx| match event {
                    InputEvent::Change => {
                        if this.error.take().is_some() {
                            cx.notify();
                        }
                    }
                    InputEvent::PressEnter { .. } => {
                        let view = cx.entity();
                        let accept = accept_on_enter.clone();
                        window.defer(cx, move |window, cx| accept(&view, window, cx));
                    }
                    _ => {}
                },
            );
            PromptView {
                field: PromptField::Input(state),
                rows,
                error: None,
                accept: accept.clone(),
                _subscription: subscription,
            }
        }
    });

    let cancel: Callback = {
        let pending = pending.clone();
        let dialog_id = dialog_id.clone();
        Rc::new(move |window, cx| {
            close_dialog(Some(dialog_id.get()), window, cx);
            pending.resolve(None, window, cx);
        })
    };

    let mut dialog = AppDialog::new()
        .title(or_default(&options.title, "Enter a value"))
        .width(DialogWidth::Md)
        .close_on_backdrop(false)
        .body({
            let view = view.clone();
            move |_, _| view.clone()
        })
        .footer({
            let view = view.clone();
            let accept = accept.clone();
            move |_, _| {
                let cancel = cancel.clone();
                let accept = accept.clone();
                let view = view.clone();
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        button("prompt-cancel")
                            .ghost()
                            .sm()
                            .label(cancel_text.clone())
                            .on_click(move |_, window, cx| cancel(window, cx)),
                    )
                    .child(
                        button("prompt-accept")
                            .sm()
                            .label(confirm_text.clone())
                            .on_click(move |_, window, cx| accept(&view, window, cx)),
                    )
            }
        })
        .on_confirm({
            let view = view.clone();
            let accept = accept.clone();
            move |window, cx| {
                accept(&view, window, cx);
                false
            }
        })
        .on_close({
            let pending = pending.clone();
            move |window, cx| pending.resolve(None, window, cx)
        });
    if !options.description.is_empty() {
        dialog = dialog.description(options.description.clone());
    }

    let id = dialog.open(window, cx);
    dialog_id.set(id);
    // El campo recibe el foco: se abre para escribir.
    match &view.read(cx).field {
        PromptField::Input(state) => {
            let state = state.clone();
            state.update(cx, |state, cx| state.focus(window, cx));
        }
        PromptField::Textarea(state) => {
            let state = state.clone();
            state.update(cx, |state, cx| state.focus(window, cx));
        }
    }
    id
}
