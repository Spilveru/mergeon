//! Alta de una cuenta de GitHub (`src/components/github/GitHubAccountDialog.vue`):
//! device flow con su código, o un token personal.

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Div, Entity, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, Role, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window, div, px, relative, rems,
};
use mergeon_app::AppModel;
use mergeon_app::github::GitHubAuthFlow;
use mergeon_ui::dialog::{self, AppDialog, DialogId, DialogWidth};
use mergeon_ui::theme::with_alpha;
use mergeon_ui::{
    ActivePaletteExt as _, IconTone, InputEvent, InputState, Lucide, MergeonStyled as _, Radius,
};

use crate::parts::{HomeDialogs, button_app_icon, github_app_icon, shadow_sm};

const TITLE: &str = "Connect GitHub Account";

/// Contenido del diálogo de cuenta. El estado del alta (código, mensajes,
/// error) vive en `AppModel::github_auth_flow`.
pub struct GitHubAccountDialog {
    model: Entity<AppModel>,
    pub(crate) token: Entity<InputState>,
    dialog_id: Option<DialogId>,
    /// El estado del alta, leído al avisar el modelo: pintar no lo lee (la
    /// ventana repintaría con cada aviso suyo).
    flow: GitHubAuthFlow,
    _subscriptions: Vec<Subscription>,
}

impl GitHubAccountDialog {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let model = AppModel::global(cx);
        let token = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder("ghp_… or github_pat_…")
        });
        // Del modelo solo pinta el estado del alta: el resto de sus avisos
        // (toasts, repositorios, git...) no lo repintan.
        let flow = model.read(cx).github_auth_flow().clone();
        let subscriptions = vec![
            cx.observe(&model, |this: &mut Self, model, cx| {
                let flow = model.read(cx).github_auth_flow();
                if *flow != this.flow {
                    this.flow = flow.clone();
                    cx.notify();
                }
            }),
            cx.subscribe_in(&token, window, |this, _, event: &InputEvent, window, cx| {
                match event {
                    // `@keydown.enter="handleTokenLogin"`
                    InputEvent::PressEnter { .. } => this.handle_token_login(window, cx),
                    InputEvent::Change => cx.notify(),
                    _ => {}
                }
            }),
        ];

        Self {
            model,
            token,
            dialog_id: None,
            flow,
            _subscriptions: subscriptions,
        }
    }

    /// El diálogo de `mergeon_ui::dialog` que muestra esta vista.
    pub fn dialog_id(&self) -> Option<DialogId> {
        self.dialog_id
    }

    fn toggle_token_form(&mut self, cx: &mut Context<Self>) {
        let dialogs = HomeDialogs::get(cx);
        dialogs.show_token_form = !dialogs.show_token_form;
        cx.notify();
    }

    /// `emitAdded`: la cuenta quedó guardada.
    fn account_added(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.model
            .update(cx, |model, cx| model.handle_account_added(cx));
        self.handle_close(window, cx);
    }

    /// `handleClose`.
    fn handle_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.dialog_id.take() {
            dialog::close_id(window, cx, id);
        }
        closed(cx);
    }

    /// `handleOAuthLogin`.
    fn handle_oauth_login(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let login = self.model.update(cx, |model, cx| model.start_login(cx));
        cx.spawn_in(window, async move |this, cx| {
            if login.await.is_some() {
                this.update_in(cx, |this, window, cx| this.account_added(window, cx))
                    .ok();
            }
        })
        .detach();
    }

    /// `handleTokenLogin`.
    fn handle_token_login(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let token = self.token.read(cx).value();
        if token.trim().is_empty() {
            return;
        }

        let login = self
            .model
            .update(cx, |model, cx| model.login_with_token(&token, cx));
        cx.spawn_in(window, async move |this, cx| {
            if login.await.is_some() {
                this.update_in(cx, |this, window, cx| this.account_added(window, cx))
                    .ok();
            }
        })
        .detach();
    }

    /// El código del device flow, con la separación de `tracking-[0.2em]`.
    fn user_code(code: &str, cx: &App) -> Div {
        let palette = *cx.palette();
        div()
            .mt_2()
            .flex()
            .justify_center()
            .font_family(cx.theme().mono_font_family.clone())
            .text_size(rems(1.875))
            .line_height(relative(1.2))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(palette.foreground)
            .children(code.chars().map(|character| {
                div()
                    .mr(rems(1.875 * 0.2))
                    .child(SharedString::from(character.to_string()))
            }))
    }

    /// Las cajas de estado y de error bajo los botones.
    fn message_box(text: SharedString, error: bool, cx: &App) -> Div {
        let palette = *cx.palette();
        let color = if error {
            palette.destructive
        } else {
            palette.primary
        };
        div()
            .mt_5()
            .w_full()
            .max_w(rems(32.))
            .radius(Radius::Xl, cx)
            .border_1()
            .border_color(with_alpha(color, if error { 0.3 } else { 0.2 }))
            .bg(with_alpha(color, 0.1))
            .px(rems(0.875))
            .py(rems(0.625))
            .text_left()
            .tw_text_sm()
            .text_color(color)
            .child(text)
    }
}

impl Render for GitHubAccountDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let flow = self.flow.clone();
        let show_token_form = HomeDialogs::get(cx).show_token_form;
        let token_is_empty = self.token.read(cx).value().trim().is_empty();
        let has_code = !flow.user_code.is_empty();

        // `space-y-4`
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                // `rounded-2xl border border-border bg-card/70 p-5 text-center shadow-sm`
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .radius(Radius::Xxl, cx)
                    .border_1()
                    .border_color(palette.border)
                    .bg(with_alpha(palette.card, 0.7))
                    .p_5()
                    .text_center()
                    .shadow(shadow_sm())
                    .child(
                        div()
                            .mb_3()
                            .flex()
                            .size_16()
                            .items_center()
                            .justify_center()
                            .radius(Radius::Xxl, cx)
                            .border_1()
                            .border_color(palette.border)
                            .bg(with_alpha(palette.primary, 0.1))
                            .child(github_app_icon().size(px(34.)).tone(IconTone::Active)),
                    )
                    .child(
                        div()
                            .max_w(rems(32.))
                            .tw_text_sm()
                            .text_color(palette.muted_foreground)
                            .child(
                                "GitHub will show a short code. Enter it in your browser to \
                                 authorize this app — no password is shared with it.",
                            ),
                    )
                    // Código del device flow
                    .when(has_code, |this| {
                        this.child(
                            div()
                                .id("github-user-code")
                                .aria_label(flow.user_code.clone())
                                .test_support()
                                .mt_5()
                                .w_full()
                                .max_w(rems(24.))
                                .radius(Radius::Xxl, cx)
                                .border_1()
                                .border_color(with_alpha(palette.primary, 0.3))
                                .bg(with_alpha(palette.primary, 0.05))
                                .p_4()
                                .child(
                                    div()
                                        .tw_text_xs()
                                        .text_color(palette.muted_foreground)
                                        .child("Your code"),
                                )
                                .child(Self::user_code(&flow.user_code, cx))
                                .child(
                                    div()
                                        .mt_3()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .gap_2()
                                        .child(
                                            mergeon_ui::button("github-copy-code")
                                                .outline()
                                                .sm()
                                                .child(button_app_icon(mergeon_ui::icon(
                                                    Lucide::Copy,
                                                )))
                                                .child("Copy code")
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.model.update(cx, |model, cx| {
                                                        model.copy_user_code(cx);
                                                    });
                                                })),
                                        )
                                        .child(
                                            mergeon_ui::button("github-open-verification")
                                                .outline()
                                                .sm()
                                                .child(button_app_icon(mergeon_ui::icon(
                                                    Lucide::ExternalLink,
                                                )))
                                                .child("Open GitHub")
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.model
                                                        .update(cx, |model, cx| {
                                                            model.open_verification_page(cx)
                                                        })
                                                        .detach();
                                                })),
                                        ),
                                ),
                        )
                    })
                    .child(
                        // `mt-5 flex flex-col items-center gap-2.5`
                        div()
                            .mt_5()
                            .w_full()
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap(rems(0.625))
                            .when(!has_code, |this| {
                                this.child(
                                    mergeon_ui::button("github-device-login")
                                        .sm()
                                        .w_full()
                                        .max_w(rems(24.))
                                        .loading(flow.is_authenticating)
                                        .label(if flow.is_authenticating {
                                            "Waiting for GitHub..."
                                        } else {
                                            "Get a device code"
                                        })
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.handle_oauth_login(window, cx);
                                        })),
                                )
                            })
                            .when(!flow.install_url.is_empty(), |this| {
                                this.child(
                                    mergeon_ui::button("github-open-install")
                                        .outline()
                                        .sm()
                                        .w_full()
                                        .max_w(rems(24.))
                                        .label("Open Install Page")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.model
                                                .update(cx, |model, cx| model.open_install_page(cx))
                                                .detach();
                                        })),
                                )
                            }),
                    )
                    .map(|this| {
                        if !flow.validation_error.is_empty() {
                            this.child(
                                Self::message_box(flow.validation_error.clone().into(), true, cx)
                                    .id("github-auth-error")
                                    .role(Role::Status)
                                    .aria_label(flow.validation_error.clone())
                                    .test_support(),
                            )
                        } else if !flow.status_message.is_empty() {
                            this.child(
                                Self::message_box(flow.status_message.clone().into(), false, cx)
                                    .id("github-auth-status")
                                    .role(Role::Status)
                                    .aria_label(flow.status_message.clone())
                                    .test_support(),
                            )
                        } else {
                            this
                        }
                    }),
            )
            // Alternativa: token personal
            .child(
                div()
                    .radius(Radius::Xxl, cx)
                    .border_1()
                    .border_color(palette.border)
                    .bg(with_alpha(palette.card, 0.7))
                    .p_4()
                    .child(
                        mergeon_ui::button("github-token-toggle")
                            .ghost()
                            .full_width(true)
                            .justify_between()
                            .px_0()
                            .child(
                                div()
                                    .tw_text_sm()
                                    .tw_font_medium()
                                    .text_color(palette.foreground)
                                    .child("Use a personal access token"),
                            )
                            .child(button_app_icon(
                                mergeon_ui::icon(if show_token_form {
                                    Lucide::ChevronUp
                                } else {
                                    Lucide::ChevronDown
                                })
                                .tone(IconTone::Muted),
                            ))
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_token_form(cx))),
                    )
                    .when(show_token_form, |this| {
                        this.child(
                            // `mt-3 space-y-2.5`
                            div()
                                .mt_3()
                                .flex()
                                .flex_col()
                                .items_start()
                                .gap(rems(0.625))
                                .child(
                                    div()
                                        .tw_text_sm()
                                        .text_color(palette.muted_foreground)
                                        .child(
                                            "Useful for GitHub Enterprise or if you prefer not \
                                             to use the device flow. The token needs repository \
                                             access.",
                                        ),
                                )
                                .child(mergeon_ui::input(&self.token))
                                .child(
                                    mergeon_ui::button("github-token-login")
                                        .sm()
                                        .disabled(token_is_empty)
                                        .loading(flow.is_authenticating)
                                        .label("Connect with token")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.handle_token_login(window, cx);
                                        })),
                                ),
                        )
                    }),
            )
    }
}

/// Lo que `handleClose` deja hecho, se cierre por donde se cierre: abandona el
/// alta en curso y avisa a la cáscara (`closeAccountDialog`).
fn closed(cx: &mut App) {
    HomeDialogs::forget(TITLE, cx);
    AppModel::global(cx).update(cx, |model, cx| {
        model.cancel_login(cx);
        model.close_account_dialog();
    });
}

/// Abre el diálogo "Connect GitHub Account". Si ya está abierto devuelve el
/// que hay.
///
/// Al añadirse la cuenta llama a `AppModel::handle_account_added` (que, salvo
/// que se viniera de Settings, pide el selector de repositorios) y se cierra.
pub fn open_github_account_dialog(
    window: &mut Window,
    cx: &mut App,
) -> Entity<GitHubAccountDialog> {
    if let Some(view) = HomeDialogs::open_view::<GitHubAccountDialog>(TITLE, window, cx) {
        return view;
    }

    // `watch(visible)`: empieza limpio.
    AppModel::global(cx).update(cx, |model, cx| model.reset_login(cx));
    let view = cx.new(|cx| GitHubAccountDialog::new(window, cx));

    let id = AppDialog::new()
        .title(TITLE)
        .description("Authorize with a device code, or use a personal access token.")
        .width(DialogWidth::Lg)
        .close_on_backdrop(false)
        .body({
            let view = view.clone();
            move |_, _| view.clone()
        })
        .footer({
            let view = view.downgrade();
            move |_, _| {
                let view = view.clone();
                div().flex().justify_end().child(
                    mergeon_ui::button("github-account-cancel")
                        .ghost()
                        .label("Cancel")
                        .on_click(move |_, window, cx| {
                            view.update(cx, |view, cx| view.handle_close(window, cx))
                                .ok();
                        }),
                )
            }
        })
        .on_close(|_, cx| closed(cx))
        .open(window, cx);

    view.update(cx, |view, _| view.dialog_id = Some(id));
    HomeDialogs::register(TITLE, id, Some(view.clone().into_any()), cx);
    view
}
