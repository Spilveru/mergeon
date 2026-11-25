//! Diálogo «Remote» para añadir un remoto
//! (`src/components/repository/AddRemoteDialog.vue`). Como el original, solo
//! recoge los datos: probar la conexión y añadir el remoto lo hace quien lo
//! abre, que le devuelve el progreso con `set_*`.

use gpui_kit::prelude::*;
use gpui_kit::{
    App, Context, Entity, EventEmitter, SharedString, Subscription, Window, div, px, relative, rems,
};
use mergeon_backend::RemoteConnectionTest;
use mergeon_ui::dialog::{self, AppDialog, DialogId, DialogWidth};
use mergeon_ui::{ActivePaletteExt as _, InputEvent, InputState, Lucide, MergeonStyled as _};

use super::parts::icon_badge;

/// `emit('test-connection', ...)`, `emit('submit', ...)` y `emit('close')`.
/// Nombre y URL llegan sin espacios alrededor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddRemoteDialogEvent {
    TestConnection { name: String, url: String },
    Submit { name: String, url: String },
    Close,
}

/// Estado del diálogo. Se abre con [`open_add_remote_dialog`].
pub struct AddRemoteDialog {
    remote_name: Entity<InputState>,
    remote_url: Entity<InputState>,
    submitting: bool,
    testing_connection: bool,
    connection_result: Option<RemoteConnectionTest>,
    /// Número de la última prueba de conexión: la respuesta de una anterior
    /// (otra URL) se descarta.
    connection_test: u64,
    dialog_id: Option<DialogId>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<AddRemoteDialogEvent> for AddRemoteDialog {}

impl AddRemoteDialog {
    fn new(
        default_name: &str,
        default_url: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let remote_name = cx.new(|cx| {
            // Sin nombre propuesto (ya hay un `origin`) el campo empieza vacío.
            let mut state = InputState::new(window, cx).placeholder("origin");
            if !default_name.is_empty() {
                state.set_value(default_name.to_string(), window, cx);
            }
            state
        });
        let remote_url = cx.new(|cx| {
            let mut state = InputState::new(window, cx)
                .placeholder("https://github.com/username/repository.git");
            if !default_url.is_empty() {
                state.set_value(default_url.to_string(), window, cx);
            }
            state
        });
        // Lo escrito decide qué botones están habilitados; Enter añade.
        let mut subscriptions: Vec<Subscription> = [&remote_name, &remote_url]
            .into_iter()
            .map(|input| {
                cx.subscribe(
                    input,
                    |this: &mut Self, _, event: &InputEvent, cx| match event {
                        InputEvent::Change => cx.notify(),
                        InputEvent::PressEnter { .. } => this.submit(cx),
                        _ => {}
                    },
                )
            })
            .collect();
        // Otra URL: el resultado de probar la anterior ya no dice nada.
        subscriptions.push(
            cx.subscribe(&remote_url, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.url_changed(cx);
                }
            }),
        );

        Self {
            remote_name,
            remote_url,
            submitting: false,
            testing_connection: false,
            connection_result: None,
            connection_test: 0,
            dialog_id: None,
            _subscriptions: subscriptions,
        }
    }

    pub fn remote_name_input(&self) -> &Entity<InputState> {
        &self.remote_name
    }

    pub fn remote_url_input(&self) -> &Entity<InputState> {
        &self.remote_url
    }

    fn name(&self, cx: &App) -> String {
        self.remote_name.read(cx).value().trim().to_string()
    }

    fn url(&self, cx: &App) -> String {
        self.remote_url.read(cx).value().trim().to_string()
    }

    /// `canSubmit`: hay nombre y URL.
    pub fn is_submittable(&self, cx: &App) -> bool {
        !self.name(cx).is_empty() && !self.url(cx).is_empty()
    }

    /// `canTest`: hay URL.
    pub fn is_testable(&self, cx: &App) -> bool {
        !self.url(cx).is_empty()
    }

    pub fn is_submitting(&self) -> bool {
        self.submitting
    }

    pub fn is_testing_connection(&self) -> bool {
        self.testing_connection
    }

    pub fn connection_result(&self) -> Option<&RemoteConnectionTest> {
        self.connection_result.as_ref()
    }

    pub fn set_submitting(&mut self, submitting: bool, cx: &mut Context<Self>) {
        self.submitting = submitting;
        cx.notify();
    }

    pub fn set_testing_connection(&mut self, testing: bool, cx: &mut Context<Self>) {
        self.testing_connection = testing;
        cx.notify();
    }

    pub fn set_connection_result(
        &mut self,
        result: Option<RemoteConnectionTest>,
        cx: &mut Context<Self>,
    ) {
        self.connection_result = result;
        cx.notify();
    }

    /// La URL cambió: el resultado (o la prueba en curso) era de la anterior.
    pub fn url_changed(&mut self, cx: &mut Context<Self>) {
        if self.connection_result.is_some() || self.testing_connection {
            self.connection_test += 1;
            self.connection_result = None;
            self.testing_connection = false;
            cx.notify();
        }
    }

    /// Empieza una prueba de conexión; devuelve su número para
    /// [`Self::finish_connection_test`].
    pub fn begin_connection_test(&mut self, cx: &mut Context<Self>) -> u64 {
        self.connection_test += 1;
        self.testing_connection = true;
        self.connection_result = None;
        cx.notify();
        self.connection_test
    }

    /// El resultado de la prueba `token`. Si después empezó otra (o la URL
    /// cambió), se descarta.
    pub fn finish_connection_test(
        &mut self,
        token: u64,
        result: RemoteConnectionTest,
        cx: &mut Context<Self>,
    ) -> bool {
        if token != self.connection_test {
            return false;
        }
        self.connection_result = Some(result);
        self.testing_connection = false;
        cx.notify();
        true
    }

    /// `handleTestConnection`.
    pub fn test_connection(&mut self, cx: &mut Context<Self>) {
        if !self.is_testable(cx) || self.submitting {
            return;
        }
        cx.emit(AddRemoteDialogEvent::TestConnection {
            name: self.name(cx),
            url: self.url(cx),
        });
    }

    /// `handleSubmit`. El diálogo sigue abierto: lo cierra quien lo abrió
    /// cuando el remoto queda añadido.
    pub fn submit(&mut self, cx: &mut Context<Self>) {
        if !self.is_submittable(cx) || self.submitting {
            return;
        }
        cx.emit(AddRemoteDialogEvent::Submit {
            name: self.name(cx),
            url: self.url(cx),
        });
    }

    /// Cierra el diálogo sin avisar (lo llama quien lo abrió).
    pub fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.dialog_id.take() {
            dialog::close_id(window, cx, id);
        }
    }

    /// El usuario pidió cerrar con Cancel. No hace nada mientras se añade el
    /// remoto.
    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.submitting {
            return;
        }
        self.dismiss(window, cx);
        cx.emit(AddRemoteDialogEvent::Close);
    }

    /// El diálogo se descartó con la X, Escape o un clic fuera. Mientras se
    /// añade el remoto no se puede cerrar (`if (addingRemote) return` del
    /// original): como la capa de diálogos ya lo quitó, se vuelve a mostrar.
    fn dismissed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dialog_id = None;
        if self.submitting {
            let view = cx.entity();
            window.defer(cx, move |window, cx| Self::present(&view, window, cx));
            return;
        }
        cx.emit(AddRemoteDialogEvent::Close);
    }

    pub fn is_open(&self) -> bool {
        self.dialog_id.is_some()
    }

    fn field(
        label: &'static str,
        input: &Entity<InputState>,
        disabled: bool,
        cx: &App,
    ) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .gap(rems(0.45))
            .child(
                div()
                    .w(px(120.))
                    .flex_shrink_0()
                    .tw_text_sm()
                    .tw_font_medium()
                    .text_color(cx.palette().foreground)
                    .child(label),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(mergeon_ui::input(input).disabled(disabled)),
            )
    }
}

impl Render for AddRemoteDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let submitting = self.submitting;
        let testing = self.testing_connection;
        let testable = self.is_testable(cx);
        let feedback = self
            .connection_result
            .as_ref()
            .filter(|result| !result.message.is_empty())
            .map(|result| (SharedString::from(result.message.clone()), result.success));

        div()
            .flex()
            .gap(rems(0.85))
            .child(
                div()
                    .w(px(58.))
                    .flex_shrink_0()
                    .child(icon_badge(Lucide::Link2, 3.15, 28., cx)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(rems(0.7))
                    .child(Self::field("Remote:", &self.remote_name, submitting, cx))
                    .child(Self::field(
                        "Repository URL:",
                        &self.remote_url,
                        submitting,
                        cx,
                    ))
                    .child(
                        // `self-start`: el botón no se estira al ancho de la columna.
                        div().flex().child(
                            mergeon_ui::button("add-remote-test")
                                .ghost()
                                .sm()
                                .disabled(!testable || testing || submitting)
                                .loading(testing)
                                .label(if testing {
                                    "Testing..."
                                } else {
                                    "Test Connection"
                                })
                                .on_click(cx.listener(|this, _, _, cx| this.test_connection(cx))),
                        ),
                    )
                    .when_some(feedback, |this, (message, success)| {
                        this.child(
                            div()
                                .text_size(rems(0.8125))
                                .line_height(relative(1.4))
                                .text_color(if success {
                                    palette.primary
                                } else {
                                    palette.destructive
                                })
                                .child(message),
                        )
                    }),
            )
    }
}

impl AddRemoteDialog {
    /// Pone el diálogo en pantalla sobre la entidad ya creada.
    fn present(view: &Entity<Self>, window: &mut Window, cx: &mut App) {
        let id = AppDialog::new()
            .title("Remote")
            .description("Add new remote repository reference")
            .width(DialogWidth::Xl)
            .body({
                let view = view.clone();
                move |_, _| view.clone()
            })
            .footer({
                let view = view.clone();
                move |_, cx| {
                    let (submitting, submittable) = {
                        let state = view.read(cx);
                        (state.submitting, state.is_submittable(cx))
                    };
                    div()
                        .flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            mergeon_ui::button("add-remote-cancel")
                                .ghost()
                                .sm()
                                .disabled(submitting)
                                .label("Cancel")
                                .on_click({
                                    let view = view.clone();
                                    move |_, window, cx| {
                                        view.update(cx, |view, cx| view.close(window, cx));
                                    }
                                }),
                        )
                        .child(
                            mergeon_ui::button("add-remote-submit")
                                .sm()
                                .disabled(!submittable)
                                .loading(submitting)
                                .label("Add New Remote")
                                .on_click({
                                    let view = view.clone();
                                    move |_, _, cx| view.update(cx, |view, cx| view.submit(cx))
                                }),
                        )
                }
            })
            .on_close({
                let view = view.clone();
                move |window, cx| view.update(cx, |view, cx| view.dismissed(window, cx))
            })
            .open(window, cx);

        view.update(cx, |view, _| view.dialog_id = Some(id));
    }
}

/// Abre el diálogo «Remote» con el foco en el nombre. Quien llama se suscribe
/// al [`AddRemoteDialogEvent`] de la entidad devuelta.
pub fn open_add_remote_dialog(
    default_name: &str,
    default_url: &str,
    window: &mut Window,
    cx: &mut App,
) -> Entity<AddRemoteDialog> {
    let view = cx.new(|cx| AddRemoteDialog::new(default_name, default_url, window, cx));
    AddRemoteDialog::present(&view, window, cx);
    // `autofocus` del primer campo.
    let name = view.read(cx).remote_name.clone();
    name.update(cx, |state, cx| state.focus(window, cx));
    view
}
