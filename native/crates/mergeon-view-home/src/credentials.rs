//! Diálogo de usuario y contraseña (o token) para un remoto HTTPS que no es
//! de GitHub. Lo abre la cáscara cuando un flujo de `AppModel` lo pide
//! (`UiHost::ask_credentials`, ver `mergeon_app::credentials`).

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Div, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, Role, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window, div, rems,
};
use mergeon_app::{CredentialsAnswer, CredentialsRequest};
use mergeon_ui::dialog::{self, AppDialog, DialogId, DialogWidth};
use mergeon_ui::{ActivePaletteExt as _, InputEvent, InputState, MergeonStyled as _};

type OnAnswer = Box<dyn FnOnce(Option<CredentialsAnswer>)>;

/// Responde una sola vez (aceptar, cancelar o cerrar el diálogo).
#[derive(Clone)]
struct Responder(Rc<RefCell<Option<OnAnswer>>>);

impl Responder {
    fn respond(&self, answer: Option<CredentialsAnswer>) {
        let callback = self.0.borrow_mut().take();
        if let Some(callback) = callback {
            callback(answer);
        }
    }
}

/// Contenido del diálogo: usuario, contraseña enmascarada y "Remember".
pub struct CredentialsDialog {
    request: CredentialsRequest,
    pub(crate) username: Entity<InputState>,
    pub(crate) password: Entity<InputState>,
    remember: bool,
    error: Option<SharedString>,
    dialog_id: Option<DialogId>,
    responder: Responder,
    _subscriptions: Vec<Subscription>,
}

/// Verbo de la operación para la descripción ("Push", "Fetch"…).
fn operation_label(operation: &str) -> String {
    let mut characters = operation.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().chain(characters).collect(),
        None => "This operation".to_string(),
    }
}

impl CredentialsDialog {
    fn new(
        request: CredentialsRequest,
        responder: Responder,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let username = cx.new(|cx| {
            let mut state = InputState::new(window, cx).placeholder("Username");
            if let Some(name) = request.username.clone() {
                state.set_value(name, window, cx);
            }
            state
        });
        let password = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder("Password or access token")
        });
        let subscriptions = vec![
            // Enter en el usuario pasa a la contraseña; en la contraseña, entra.
            cx.subscribe_in(
                &username,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        this.password
                            .update(cx, |state, cx| state.focus(window, cx));
                    }
                },
            ),
            cx.subscribe_in(
                &password,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        let view = cx.entity();
                        window.defer(cx, move |window, cx| {
                            view.update(cx, |this, cx| this.submit(window, cx));
                        });
                    } else if matches!(event, InputEvent::Change) && this.error.is_some() {
                        this.error = None;
                        cx.notify();
                    }
                },
            ),
        ];
        Self {
            request,
            username,
            password,
            remember: false,
            error: None,
            dialog_id: None,
            responder,
            _subscriptions: subscriptions,
        }
    }

    /// Valida, responde con lo tecleado, vacía los campos y cierra.
    pub(crate) fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let username = self.username.read(cx).value().trim().to_string();
        let password = self.password.read(cx).value().to_string();
        let problem = if username.is_empty() {
            Some("Enter a username.")
        } else if password.is_empty() {
            Some("Enter a password or access token.")
        } else {
            None
        };
        if let Some(problem) = problem {
            self.error = Some(problem.into());
            cx.notify();
            return;
        }
        // Que la contraseña no se quede en el campo más de lo necesario.
        self.password
            .update(cx, |state, cx| state.set_value("", window, cx));
        self.responder.respond(Some(CredentialsAnswer {
            username,
            password,
            remember: self.remember && self.request.can_remember,
        }));
        self.close(window, cx);
    }

    pub(crate) fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.responder.respond(None);
        self.close(window, cx);
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.dialog_id.take() {
            dialog::close_id(window, cx, id);
        }
    }

    fn label(text: &'static str, cx: &App) -> Div {
        div()
            .text_size(rems(0.8125))
            .line_height(gpui_kit::relative(1.5))
            .tw_font_medium()
            .text_color(cx.palette().foreground)
            .child(text)
    }
}

impl Render for CredentialsDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let can_remember = self.request.can_remember;
        let remember = self.remember;
        let insecure_http =
            mergeon_gitcmd::credentials::is_insecure_http_origin(&self.request.origin);
        div()
            .id("credentials-dialog")
            .test_support()
            .flex()
            .flex_col()
            .gap(rems(0.875))
            .when(insecure_http, |this| {
                this.child(
                    div()
                        .id("credentials-insecure-http")
                        .role(Role::Status)
                        .test_support()
                        .tw_text_xs()
                        .text_color(palette.warning)
                        .child(
                            "Unencrypted HTTP connection — your credentials will be sent in plain text.",
                        ),
                )
            })
            .when(self.request.refused, |this| {
                this.child(
                    div()
                        .id("credentials-refused")
                        .role(Role::Status)
                        .test_support()
                        .tw_text_sm()
                        .text_color(palette.destructive)
                        .child("Those credentials were refused. Check them and try again."),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(rems(0.375))
                    .child(Self::label("Username", cx))
                    .child(mergeon_ui::input(&self.username)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(rems(0.375))
                    .child(Self::label("Password or token", cx))
                    .child(mergeon_ui::input(&self.password).mask_toggle()),
            )
            .when_some(self.error.clone(), |this, error| {
                this.child(
                    div()
                        .id("credentials-error")
                        .role(Role::Status)
                        .aria_label(error.clone())
                        .test_support()
                        .tw_text_xs()
                        .text_color(palette.destructive)
                        .child(error),
                )
            })
            .when(can_remember, |this| {
                this.child(
                    div().flex().child(
                        mergeon_ui::checkbox("credentials-remember")
                            .label("Remember with Git's credential helper")
                            .checked(remember)
                            .on_change(cx.listener(|this, checked: &bool, _, cx| {
                                this.remember = *checked;
                                cx.notify();
                            })),
                    ),
                )
            })
            .child(
                div()
                    .text_size(rems(0.75))
                    .line_height(gpui_kit::relative(1.5))
                    .text_color(palette.muted_foreground)
                    .child(if can_remember {
                        "Kept in memory until you close this tab, unless you ask Git to remember them."
                    } else {
                        "Kept in memory until you close this tab."
                    }),
            )
    }
}

/// Abre el diálogo de credenciales. `on_answer` recibe lo tecleado, o `None`
/// si se cancela o se cierra; se llama una sola vez.
pub fn open_credentials_dialog(
    request: CredentialsRequest,
    window: &mut Window,
    cx: &mut App,
    on_answer: impl FnOnce(Option<CredentialsAnswer>) + 'static,
) -> Entity<CredentialsDialog> {
    let responder = Responder(Rc::new(RefCell::new(Some(Box::new(on_answer)))));
    let title = format!("Sign in to {}", request.host);
    let description = format!(
        "{} needs a username and a password or access token for {}.",
        operation_label(&request.operation),
        request.host
    );
    let view = cx.new(|cx| CredentialsDialog::new(request, responder.clone(), window, cx));

    let id = AppDialog::new()
        .title(title)
        .description(description)
        .width(DialogWidth::Md)
        .close_on_backdrop(false)
        .body({
            let view = view.clone();
            move |_, _| view.clone()
        })
        .footer({
            let view = view.downgrade();
            move |_, _| {
                let cancel = view.clone();
                let accept = view.clone();
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        mergeon_ui::button("credentials-cancel")
                            .ghost()
                            .sm()
                            .label("Cancel")
                            .on_click(move |_, window, cx| {
                                cancel.update(cx, |view, cx| view.cancel(window, cx)).ok();
                            }),
                    )
                    .child(
                        mergeon_ui::button("credentials-sign-in")
                            .sm()
                            .label("Sign in")
                            .on_click(move |_, window, cx| {
                                accept.update(cx, |view, cx| view.submit(window, cx)).ok();
                            }),
                    )
            }
        })
        .on_confirm({
            let view = view.downgrade();
            move |window, cx| {
                view.update(cx, |view, cx| view.submit(window, cx)).ok();
                false
            }
        })
        // Cerrar con Esc o la X cuenta como cancelar.
        .on_close(move |_, _| responder.respond(None))
        .open(window, cx);

    view.update(cx, |view, cx| {
        view.dialog_id = Some(id);
        // Con el usuario ya propuesto, se empieza por la contraseña.
        let field = if view.request.username.is_some() {
            view.password.clone()
        } else {
            view.username.clone()
        };
        field.update(cx, |state, cx| state.focus(window, cx));
    });
    view
}
