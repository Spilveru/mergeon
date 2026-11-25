//! Gestor de cuentas de GitHub (`src/components/github/GitHubAccountsManager.vue`):
//! la sección Accounts de Settings y, opcionalmente, un diálogo.

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, FontWeight,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString, Styled as _,
    Subscription, Window, div, px, rems,
};
use mergeon_app::AppModel;
use mergeon_app::model_types::dialogs::ConfirmOptions;
use mergeon_store::GitHubAccount;
use mergeon_ui::dialog::{self, AppDialog, DialogId, DialogWidth};
use mergeon_ui::theme::with_alpha;
use mergeon_ui::{ActivePaletteExt as _, BadgeVariant, Lucide, MergeonStyled as _, Radius};

use crate::avatar::{AvatarCache, avatar};
use crate::parts::{HomeDialogs, github_icon};

const TITLE: &str = "GitHub Accounts";

/// Lo que el gestor pide a quien lo aloja.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitHubAccountsEvent {
    /// `emit('add-account')`: abrir el alta de una cuenta.
    AddAccount,
    /// `emit('close')`: solo como diálogo.
    Close,
}

/// La acción en curso sobre una cuenta (`activeAction`): mientras dura, todos
/// los botones quedan bloqueados.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountAction {
    SetActive(String),
    Remove(String),
}

/// Lista de cuentas con sus acciones.
pub struct GitHubAccountsManager {
    model: Entity<AppModel>,
    embedded: bool,
    active_action: Option<AccountAction>,
    dialog_id: Option<DialogId>,
    /// Las cuentas y la activa, leídas al avisar el modelo: pintar no lo lee
    /// (la ventana repintaría con cada aviso suyo).
    shown: (Vec<GitHubAccount>, Option<String>),
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<GitHubAccountsEvent> for GitHubAccountsManager {}

impl GitHubAccountsManager {
    /// `embedded`: sin botón "Close", para incrustarlo en Settings.
    pub fn new(embedded: bool, cx: &mut Context<Self>) -> Self {
        let model = AppModel::global(cx);
        let avatars = AvatarCache::global(cx);
        // Del modelo solo pinta las cuentas y cuál es la activa: el resto de
        // sus avisos (toasts, repositorios, git...) no lo repintan.
        let shown = shown_accounts(model.read(cx));
        let subscriptions = vec![
            cx.observe(&model, |this: &mut Self, model, cx| {
                let accounts = shown_accounts(model.read(cx));
                if accounts != this.shown {
                    this.shown = accounts;
                    cx.notify();
                }
            }),
            cx.observe(&avatars, |_, _, cx| cx.notify()),
        ];
        Self {
            model,
            embedded,
            active_action: None,
            dialog_id: None,
            shown,
            _subscriptions: subscriptions,
        }
    }

    /// La acción en curso, si la hay.
    pub fn active_action(&self) -> Option<&AccountAction> {
        self.active_action.as_ref()
    }

    /// `handleSetActive`.
    fn handle_set_active(&mut self, account_id: &str, cx: &mut Context<Self>) {
        self.active_action = Some(AccountAction::SetActive(account_id.to_string()));
        cx.notify();

        let model = self.model.clone();
        let activation = model.update(cx, |model, cx| {
            model.set_active_account(Some(account_id), cx)
        });
        cx.spawn(async move |this, cx| {
            let result = activation.await;
            model.update(cx, |model, cx| match result {
                Ok(()) => {
                    model.notify_success("Account set as active", cx);
                }
                Err(error) => {
                    model.notify_error(&format!("Failed to set active account: {error}"), cx);
                }
            });
            this.update(cx, |this, cx| {
                this.active_action = None;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// `handleRemove`.
    fn handle_remove(&mut self, account_id: &str, cx: &mut Context<Self>) {
        let Some(account) = self
            .model
            .read(cx)
            .github_account_by_id(account_id)
            .cloned()
        else {
            return;
        };

        let model = self.model.clone();
        let account_id = account_id.to_string();
        let confirmation = model.update(cx, |model, cx| {
            model.confirm(
                ConfirmOptions {
                    title: Some("Remove account".to_string()),
                    description: Some(format!(
                        "Remove @{} and its stored product session?",
                        account.username
                    )),
                    confirm_text: Some("Remove".to_string()),
                    cancel_text: Some("Cancel".to_string()),
                    variant: Some("destructive".to_string()),
                    ..Default::default()
                },
                cx,
            )
        });
        cx.spawn(async move |this, cx| {
            if !confirmation.await {
                return;
            }

            let removal = this.update(cx, |this, cx| {
                this.active_action = Some(AccountAction::Remove(account_id.clone()));
                cx.notify();
                this.model
                    .update(cx, |model, cx| model.remove_account(&account_id, cx))
            });
            let Ok(removal) = removal else {
                return;
            };
            let result = removal.await;
            model.update(cx, |model, cx| match result {
                Ok(()) => {
                    model.notify_success("Account removed successfully", cx);
                }
                Err(error) => {
                    model.notify_error(&format!("Failed to remove account: {error}"), cx);
                }
            });
            this.update(cx, |this, cx| {
                this.active_action = None;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn render_account(
        &self,
        account: &GitHubAccount,
        is_active: bool,
        row_layout: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = *cx.palette();
        let busy = self.active_action.is_some();
        let setting_active =
            self.active_action == Some(AccountAction::SetActive(account.id.clone()));
        let removing = self.active_action == Some(AccountAction::Remove(account.id.clone()));
        let image = avatar(
            account.avatar_url.as_deref(),
            rems(3.),
            Radius::Xl.px(cx),
            cx,
        );
        let set_active_id = account.id.clone();
        let remove_id = account.id.clone();

        div()
            .id(SharedString::from(format!("github-account:{}", account.id)))
            .test_support()
            .flex()
            .gap_3()
            .radius(Radius::Xxl, cx)
            .border_1()
            .p_4()
            .map(|this| {
                // `sm:flex-row sm:items-center`
                if row_layout {
                    this.items_center()
                } else {
                    this.flex_col()
                }
            })
            .map(|this| {
                if is_active {
                    this.border_color(with_alpha(palette.primary, 0.4))
                        .bg(with_alpha(palette.primary, 0.05))
                } else {
                    this.border_color(palette.border)
                        .bg(with_alpha(palette.card, 0.7))
                }
            })
            .child(image.unwrap_or_else(|| {
                div()
                    .flex()
                    .size_12()
                    .flex_shrink_0()
                    .items_center()
                    .justify_center()
                    .radius(Radius::Xl, cx)
                    .border_1()
                    .border_color(palette.border)
                    .bg(palette.muted)
                    .text_color(palette.muted_foreground)
                    .child(github_icon(px(24.)))
                    .into_any_element()
            }))
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .tw_text_base()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(palette.foreground)
                                    .child(SharedString::from(
                                        account.name.clone().unwrap_or_default(),
                                    )),
                            )
                            .map(|this| {
                                if is_active {
                                    this.child(mergeon_ui::badge("Active"))
                                } else if account.requires_reconnect {
                                    this.child(
                                        mergeon_ui::badge("Reconnect required")
                                            .variant(BadgeVariant::Destructive),
                                    )
                                } else if account.requires_installation == Some(true) {
                                    this.child(
                                        mergeon_ui::badge("Install required")
                                            .variant(BadgeVariant::Secondary),
                                    )
                                } else {
                                    this
                                }
                            }),
                    )
                    .child(
                        div()
                            .mt(rems(0.125))
                            .tw_text_sm()
                            .text_color(palette.muted_foreground)
                            .child(SharedString::from(format!("@{}", account.username))),
                    )
                    .when_some(
                        account.email.clone().filter(|email| !email.is_empty()),
                        |this, email| {
                            this.child(
                                div()
                                    .mt(rems(0.125))
                                    .tw_text_sm()
                                    .text_color(palette.muted_foreground)
                                    .child(SharedString::from(email)),
                            )
                        },
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .when(row_layout, |this| this.justify_end())
                    .when(!is_active, |this| {
                        this.child(
                            mergeon_ui::button(SharedString::from(format!(
                                "github-account-set-active:{}",
                                account.id
                            )))
                            .outline()
                            .sm()
                            .loading(setting_active)
                            .disabled(busy)
                            .label("Set active")
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.handle_set_active(&set_active_id, cx);
                                },
                            )),
                        )
                    })
                    .child(
                        mergeon_ui::button(SharedString::from(format!(
                            "github-account-remove:{}",
                            account.id
                        )))
                        .destructive()
                        .sm()
                        .loading(removing)
                        .disabled(busy)
                        .label("Remove")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.handle_remove(&remove_id, cx);
                        })),
                    ),
            )
            .into_any_element()
    }
}

/// Lo que el gestor enseña del modelo: las cuentas y la activa.
fn shown_accounts(model: &AppModel) -> (Vec<GitHubAccount>, Option<String>) {
    (
        model.github_accounts().to_vec(),
        model.active_github_account_id().map(str::to_string),
    )
}

impl Render for GitHubAccountsManager {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (accounts, active_id) = self.shown.clone();
        // Las variantes `sm:` de Tailwind.
        let row_layout = window.viewport_size().width >= px(640.);
        let embedded = self.embedded;

        let body: AnyElement = if accounts.is_empty() {
            mergeon_ui::empty_state("No GitHub accounts configured")
                .description(
                    "Add an account to clone repositories and use GitHub App authentication.",
                )
                .icon(Lucide::Puzzle)
                .child(
                    mergeon_ui::button("github-accounts-add-first")
                        .label("Add Account")
                        .on_click(cx.listener(|_, _, _, cx| {
                            cx.emit(GitHubAccountsEvent::AddAccount);
                        })),
                )
                .into_any_element()
        } else {
            // `space-y-3`
            div()
                .flex()
                .flex_col()
                .gap_3()
                .children(accounts.iter().map(|account| {
                    let is_active = active_id.as_deref() == Some(account.id.as_str());
                    self.render_account(account, is_active, row_layout, cx)
                }))
                .into_any_element()
        };

        div().flex().flex_col().child(body).child(
            // Slot `footer`: `flex w-full justify-between`, con `mt-4` incrustado.
            div()
                .flex()
                .w_full()
                .justify_between()
                .mt_4()
                .when(!embedded, |this| {
                    this.child(
                        mergeon_ui::button("github-accounts-close")
                            .ghost()
                            .label("Close")
                            .on_click(cx.listener(|this, _, window, cx| {
                                // `handleClose`
                                if let Some(id) = this.dialog_id.take() {
                                    dialog::close_id(window, cx, id);
                                    HomeDialogs::forget(TITLE, cx);
                                }
                                cx.emit(GitHubAccountsEvent::Close);
                            })),
                    )
                })
                .when(!accounts.is_empty(), |this| {
                    this.child(
                        mergeon_ui::button("github-accounts-add")
                            .label("Add Account")
                            .on_click(cx.listener(|_, _, _, cx| {
                                cx.emit(GitHubAccountsEvent::AddAccount);
                            })),
                    )
                }),
        )
    }
}

/// Abre el gestor de cuentas como diálogo. "Add Account" pide el alta con
/// `AppModel::add_account_from_settings`.
pub fn open_github_accounts_dialog(window: &mut Window, cx: &mut App) -> DialogId {
    if let Some((id, _)) = HomeDialogs::open_dialog(TITLE, window, cx) {
        return id;
    }

    let view = cx.new(|cx| GitHubAccountsManager::new(false, cx));
    cx.subscribe(&view, |_, event: &GitHubAccountsEvent, cx| {
        if *event == GitHubAccountsEvent::AddAccount {
            AppModel::global(cx).update(cx, |model, cx| model.add_account_from_settings(cx));
        }
    })
    .detach();

    let id = AppDialog::new()
        .title(TITLE)
        .description("Manage the GitHub App sessions available in this workspace.")
        .width(DialogWidth::Xl)
        .close_on_backdrop(false)
        .body({
            let view = view.clone();
            move |_, _| view.clone()
        })
        .on_close(|_, cx| HomeDialogs::forget(TITLE, cx))
        .open(window, cx);

    view.update(cx, |view, _| view.dialog_id = Some(id));
    HomeDialogs::register(TITLE, id, Some(view.into_any()), cx);
    id
}
