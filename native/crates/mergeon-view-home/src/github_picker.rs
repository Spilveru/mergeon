//! Selector de repositorios de GitHub para clonar
//! (`src/components/github/GitHubRepositoryPicker.vue`).

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::prelude::FluentBuilder as _;
use std::ops::Range;

use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, FontWeight, InteractiveElement as _,
    IntoElement, ListSizingBehavior, ParentElement as _, Render, ScrollStrategy, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, UniformListScrollHandle, Window,
    div, px, rems, uniform_list,
};
use mergeon_app::AppModel;
use mergeon_app::github::{InstallationRepository, ListAllRepositoriesOptions};
use mergeon_app::model_types::format::format_updated_ago;
use mergeon_store::{GitHubAccount, GitHubInstallation};
use mergeon_ui::dialog::{self, AppDialog, DialogId, DialogWidth};
use mergeon_ui::theme::with_alpha;
use mergeon_ui::{
    ActivePaletteExt as _, BadgeVariant, InputEvent, InputState, Lucide, MergeonStyled as _,
    Radius, SelectOption,
};

use crate::avatar::{AvatarCache, avatar};
use crate::github_account::open_github_account_dialog;
use crate::parts::{HomeDialogs, card_section, field_label, github_icon, nowrap, picker_label};
use crate::util::join_local_path;

const TITLE: &str = "Clone From GitHub";

/// La fuente de emojis en color de cada plataforma.
const EMOJI_FONT: &str = if cfg!(target_os = "macos") {
    "Apple Color Emoji"
} else if cfg!(target_os = "windows") {
    "Segoe UI Emoji"
} else {
    "Noto Color Emoji"
};

/// Alto máximo de la lista de resultados (`max-height: 320px`).
const RESULTS_MAX_HEIGHT: f32 = 320.;

/// Contenido del selector: cuenta, búsqueda, lista y destino del clon.
pub struct GitHubRepositoryPicker {
    model: Entity<AppModel>,
    dialog_id: Option<DialogId>,
    selected_account_id: Option<String>,
    all_repositories: Vec<InstallationRepository>,
    selected_repo: Option<InstallationRepository>,
    pub(crate) search: Entity<InputState>,
    pub(crate) destination: Entity<InputState>,
    is_cloning: bool,
    is_refreshing_account: bool,
    clone_parent_path: String,
    /// La lista de repositorios es virtual: con muchas instalaciones trae
    /// cientos y solo se construyen las tarjetas a la vista. El buscador
    /// filtra sobre el conjunto completo.
    results_scroll: UniformListScrollHandle,
    /// Los índices en `all_repositories` de lo que lista ahora (filtrado por
    /// el buscador), para las filas de la lista virtual.
    shown_results: Vec<usize>,
    /// Lo que se pinta del modelo, leído al avisar éste: pintar no lo lee
    /// (la ventana repintaría con cada aviso suyo).
    shown_model: PickerModelState,
    renders: usize,
    _subscriptions: Vec<Subscription>,
}

/// Lo que el selector enseña del modelo: las cuentas (y la elegida), si se
/// están cargando repositorios. Los demás avisos del modelo (toasts, otros
/// repositorios, git...) no lo repintan.
#[derive(Debug, Clone, PartialEq)]
struct PickerModelState {
    accounts: Vec<GitHubAccount>,
    loading: bool,
}

impl PickerModelState {
    fn read(model: &AppModel) -> Self {
        Self {
            accounts: model.github_accounts().to_vec(),
            loading: model.github_repositories_loading(),
        }
    }
}

/// `repositories`: el filtro del buscador, sobre nombre completo y descripción.
pub fn filter_repositories<'a>(
    repositories: &'a [InstallationRepository],
    query: &str,
) -> Vec<&'a InstallationRepository> {
    filter_repository_indices(repositories, query)
        .into_iter()
        .map(|index| &repositories[index])
        .collect()
}

/// Como [`filter_repositories`], con las posiciones en `repositories`.
fn filter_repository_indices(repositories: &[InstallationRepository], query: &str) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return (0..repositories.len()).collect();
    }
    repositories
        .iter()
        .enumerate()
        .filter(|(_, item)| {
            let name = item
                .repository
                .full_name
                .as_deref()
                .unwrap_or_default()
                .to_lowercase();
            let description = item
                .repository
                .description
                .as_deref()
                .unwrap_or_default()
                .to_lowercase();
            name.contains(&query) || description.contains(&query)
        })
        .map(|(index, _)| index)
        .collect()
}

impl GitHubRepositoryPicker {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let model = AppModel::global(cx);
        let avatars = AvatarCache::global(cx);
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search repositories..."));
        let destination = cx.new(|cx| InputState::new(window, cx).placeholder("Choose a folder…"));
        let subscriptions = vec![
            cx.observe(&model, |this: &mut Self, model, cx| {
                let state = PickerModelState::read(model.read(cx));
                if state != this.shown_model {
                    this.shown_model = state;
                    cx.notify();
                }
            }),
            cx.observe(&avatars, |_, _, cx| cx.notify()),
            cx.subscribe(&search, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    // Cada búsqueda empieza de nuevo arriba.
                    this.results_scroll.scroll_to_item(0, ScrollStrategy::Top);
                    cx.notify();
                }
            }),
        ];
        let shown_model = PickerModelState::read(model.read(cx));

        let mut this = Self {
            model,
            dialog_id: None,
            selected_account_id: None,
            all_repositories: Vec::new(),
            selected_repo: None,
            search,
            destination,
            is_cloning: false,
            is_refreshing_account: false,
            clone_parent_path: String::new(),
            results_scroll: UniformListScrollHandle::new(),
            shown_results: Vec::new(),
            shown_model,
            renders: 0,
            _subscriptions: subscriptions,
        };

        // `watch(visible)`: al abrir se elige la cuenta activa y se cargan sus
        // repositorios.
        if let Some(active_id) = this
            .model
            .read(cx)
            .active_github_account_id()
            .map(str::to_string)
        {
            this.selected_account_id = Some(active_id);
            this.load_repositories(cx);
        }
        this
    }

    /// `selectedAccount`.
    fn selected_account(&self, cx: &App) -> Option<GitHubAccount> {
        let id = self.selected_account_id.as_deref()?;
        self.model.read(cx).github_account_by_id(id).cloned()
    }

    /// Veces que se pintó (tests).
    #[doc(hidden)]
    pub fn render_count(&self) -> usize {
        self.renders
    }

    /// [`Self::selected_account`] según lo último leído del modelo (para
    /// pintar).
    fn shown_account(&self) -> Option<GitHubAccount> {
        let id = self.selected_account_id.as_deref()?;
        self.shown_model
            .accounts
            .iter()
            .find(|account| account.id == id)
            .cloned()
    }

    /// Cuenta elegida en el selector.
    pub fn selected_account_id(&self) -> Option<&str> {
        self.selected_account_id.as_deref()
    }

    /// Repositorios cargados de la cuenta elegida, del más reciente al más antiguo.
    pub fn all_repositories(&self) -> &[InstallationRepository] {
        &self.all_repositories
    }

    /// Repositorio elegido para clonar.
    pub fn selected_repository(&self) -> Option<&InstallationRepository> {
        self.selected_repo.as_ref()
    }

    /// Si hay un clon en curso.
    pub fn is_cloning(&self) -> bool {
        self.is_cloning
    }

    /// Si se están refrescando las instalaciones de la cuenta.
    pub fn is_refreshing_account(&self) -> bool {
        self.is_refreshing_account
    }

    /// `clonePath`: la carpeta elegida más el nombre del repositorio.
    pub fn clone_path(&self) -> String {
        if self.clone_parent_path.is_empty() {
            return String::new();
        }
        match self
            .selected_repo
            .as_ref()
            .and_then(|item| item.repository.name.as_deref())
        {
            Some(name) if self.selected_repo.is_some() => {
                join_local_path(&self.clone_parent_path, name, cfg!(windows))
            }
            _ => self.clone_parent_path.clone(),
        }
    }

    fn sync_destination(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let path = self.clone_path();
        self.destination
            .update(cx, |state, cx| state.set_value(path, window, cx));
    }

    fn select_repository(
        &mut self,
        item: InstallationRepository,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selected_repo = Some(item);
        self.sync_destination(window, cx);
        cx.notify();
    }

    /// `handleAccountChange`.
    pub(crate) fn handle_account_change(
        &mut self,
        account_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selected_account_id = Some(account_id.to_string());
        self.all_repositories.clear();
        self.selected_repo = None;
        self.results_scroll.scroll_to_item(0, ScrollStrategy::Top);
        self.search
            .update(cx, |state, cx| state.set_value("", window, cx));
        self.sync_destination(window, cx);
        self.load_repositories(cx);
        cx.notify();
    }

    /// `loadRepositories`.
    fn load_repositories(&mut self, cx: &mut Context<Self>) {
        let Some(account) = self.selected_account(cx) else {
            return;
        };

        let model = self.model.clone();
        if !model.read(cx).auth_service_available() {
            model.update(cx, |model, cx| {
                let message = model.auth_service_message().to_string();
                model.notify_error(
                    if message.is_empty() {
                        "GitHub is unreachable"
                    } else {
                        &message
                    },
                    cx,
                );
            });
            self.all_repositories.clear();
            cx.notify();
            return;
        }

        if account.requires_installation == Some(true) || account.token.is_empty() {
            self.all_repositories.clear();
            cx.notify();
            return;
        }

        // Cuentas con token personal no tienen instalaciones: se listan por
        // afiliación, lo que además incluye los repos donde solo eres colaborador.
        let (installations, options) = if account.installations.is_empty() {
            (
                vec![GitHubInstallation::default()],
                ListAllRepositoriesOptions::default(),
            )
        } else {
            // Aggregate every installation (own account, orgs, and accounts where the
            // user collaborates) so client-side search covers all reachable repos.
            (
                account.installations.clone(),
                ListAllRepositoriesOptions {
                    active_installation_id: account.active_installation_id,
                    ..Default::default()
                },
            )
        };

        let request = model.update(cx, |model, cx| {
            model.list_all_repositories(&account.token, installations, options, cx)
        });
        cx.spawn(async move |this, cx| {
            let result = request.await;
            let warning = this
                .update(cx, |this, cx| {
                    let warning = match result {
                        // Otra petición (u otro cierre) la dejó obsoleta.
                        Ok(None) => return None,
                        Ok(Some(all)) => {
                            this.all_repositories = all.repositories;
                            (!all.failures.is_empty()).then(|| {
                                let failed: Vec<String> = all
                                    .failures
                                    .iter()
                                    .map(|failure| {
                                        if failure.account_login.is_empty() {
                                            format!("installation {}", failure.installation_id)
                                        } else {
                                            failure.account_login.clone()
                                        }
                                    })
                                    .collect();
                                (
                                    false,
                                    format!(
                                        "Some repositories could not be loaded ({})",
                                        failed.join(", ")
                                    ),
                                )
                            })
                        }
                        Err(error) => {
                            this.all_repositories.clear();
                            Some((
                                true,
                                format!("Failed to load repositories: {}", error.message),
                            ))
                        }
                    };
                    cx.notify();
                    warning
                })
                .ok()
                .flatten();

            if let Some((is_error, message)) = warning {
                model.update(cx, |model, cx| {
                    if is_error {
                        model.notify_error(&message, cx);
                    } else {
                        model.notify_warning(&message, cx);
                    }
                });
            }
        })
        .detach();
    }

    /// `selectCloneLocation`.
    fn select_clone_location(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let pick = self.model.update(cx, |model, cx| model.pick_directory(cx));
        cx.spawn_in(window, async move |this, cx| {
            let Some(path) = pick.await else {
                return;
            };
            this.update_in(cx, |this, window, cx| {
                this.clone_parent_path = path.to_string_lossy().into_owned();
                this.sync_destination(window, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// `handleClone`.
    fn handle_clone(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let clone_path = self.clone_path();
        let (Some(item), Some(account)) = (self.selected_repo.clone(), self.selected_account(cx))
        else {
            return;
        };
        if clone_path.is_empty() || account.requires_installation == Some(true) {
            return;
        }

        self.is_cloning = true;
        cx.notify();

        let clone_url = item.repository.clone_url.clone().unwrap_or_default();
        let model = self.model.clone();
        let cloning = model.update(cx, |model, cx| {
            model.clone_repository(&clone_url, &clone_path, Some(account.token.clone()), cx)
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = cloning.await;
            this.update_in(cx, |this, window, cx| {
                this.is_cloning = false;
                cx.notify();
                if let Some(path) = result
                    .path
                    .filter(|path| result.success && !path.is_empty())
                {
                    // `emit('repository-cloned', path)`
                    this.model
                        .update(cx, |model, cx| model.handle_repository_cloned(&path, cx))
                        .detach();
                    this.handle_close(window, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// `handleClose`.
    fn handle_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.dialog_id.take() {
            dialog::close_id(window, cx, id);
        }
        closed(cx);
    }

    /// `openInstallationPage`.
    fn open_installation_page(&mut self, cx: &mut Context<Self>) {
        let Some(install_url) = self
            .selected_account(cx)
            .and_then(|account| account.install_url)
            .filter(|url| !url.is_empty())
        else {
            return;
        };
        self.model
            .update(cx, |model, cx| model.open_external(&install_url, cx))
            .detach();
    }

    /// `refreshSelectedAccount`.
    fn refresh_selected_account(&mut self, cx: &mut Context<Self>) {
        let Some(account_id) = self.selected_account_id.clone() else {
            return;
        };

        self.is_refreshing_account = true;
        cx.notify();

        let model = self.model.clone();
        let refresh = model.update(cx, |model, cx| model.refresh_account(&account_id, cx));
        cx.spawn(async move |this, cx| {
            let result = refresh.await;
            match result {
                Ok(refreshed) => {
                    let still_requires_installation = refreshed
                        .is_some_and(|account| account.requires_installation == Some(true));
                    if !still_requires_installation {
                        this.update(cx, |this, cx| this.load_repositories(cx)).ok();
                    }
                }
                Err(error) => model.update(cx, |model, cx| {
                    model.notify_error(
                        &format!("Failed to refresh installations: {}", error.message),
                        cx,
                    );
                }),
            }
            this.update(cx, |this, cx| {
                this.is_refreshing_account = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // ------------------------------------------------------------------
    // Render
    // ------------------------------------------------------------------

    fn render_account_section(
        &mut self,
        account: Option<&GitHubAccount>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = *cx.palette();
        let accounts = self.shown_model.accounts.clone();
        let options: Vec<SelectOption> = accounts
            .iter()
            .map(|account| SelectOption::new(account.id.clone(), format!("@{}", account.username)))
            .collect();
        // `@media (max-height: 700px)`: la cuenta ya se nombra en el selector.
        let show_summary = window.viewport_size().height > px(700.);

        card_section(cx)
            .child(
                // `.picker-toolbar`
                div()
                    .flex()
                    .items_end()
                    .gap_4()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .gap_2()
                            .child(picker_label("GitHub account", cx))
                            .child(
                                mergeon_ui::select("github-picker-account")
                                    .items(options)
                                    .value(self.selected_account_id.clone().unwrap_or_default())
                                    .placeholder("Select an account")
                                    .on_change(cx.listener(
                                        |this, value: &SharedString, window, cx| {
                                            this.handle_account_change(value, window, cx);
                                        },
                                    )),
                            ),
                    )
                    .child(
                        mergeon_ui::button("github-picker-add-account")
                            .outline()
                            .label(if accounts.is_empty() {
                                "Add Account"
                            } else {
                                "Add Another Account"
                            })
                            // `emit('add-account')`
                            .on_click(|_, window, cx| {
                                open_github_account_dialog(window, cx);
                            }),
                    ),
            )
            .when_some(account.filter(|_| show_summary), |this, account| {
                let installations = account.installations.len();
                let image = avatar(
                    account.avatar_url.as_deref(),
                    rems(2.25),
                    Radius::Md.px(cx),
                    cx,
                );
                this.child(
                    // `.picker-account-summary`
                    div()
                        .flex()
                        .items_center()
                        .gap(rems(0.85))
                        .mt_4()
                        .pt_4()
                        .border_t_1()
                        .border_color(palette.border)
                        .child(image.unwrap_or_else(|| {
                            github_icon(px(36.))
                                .text_color(palette.muted_foreground)
                                .into_any_element()
                        }))
                        .child(
                            div()
                                .min_w_0()
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .child(
                                            div()
                                                .text_size(rems(0.875))
                                                .line_height(gpui_kit::relative(20. / 14.))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(palette.foreground)
                                                .child(SharedString::from(
                                                    account.name.clone().unwrap_or_default(),
                                                )),
                                        )
                                        .when(
                                            account.requires_installation == Some(true),
                                            |this| {
                                                this.child(
                                                    mergeon_ui::badge("Install required")
                                                        .variant(BadgeVariant::Secondary),
                                                )
                                            },
                                        ),
                                )
                                .child(
                                    div()
                                        .mt(rems(0.15))
                                        .text_size(rems(0.8125))
                                        .line_height(gpui_kit::relative(20. / 14.))
                                        .text_color(palette.muted_foreground)
                                        .child(SharedString::from(if installations > 0 {
                                            format!(
                                                "@{} · {installations} installation{} available",
                                                account.username,
                                                if installations > 1 { "s" } else { "" }
                                            )
                                        } else {
                                            format!("@{}", account.username)
                                        })),
                                ),
                        ),
                )
            })
            .into_any_element()
    }

    fn render_repository_card(
        &self,
        item: &InstallationRepository,
        account: Option<&GitHubAccount>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = *cx.palette();
        let repo = &item.repository;
        let selected = self
            .selected_repo
            .as_ref()
            .is_some_and(|selected| selected.repository.id == repo.id);
        let owner_login = repo.owner.login.clone().filter(|login| !login.is_empty());
        let is_foreign = owner_login
            .as_ref()
            .is_some_and(|login| account.is_none_or(|account| &account.username != login));
        let image = avatar(
            repo.owner.avatar_url.as_deref(),
            rems(1.25),
            Radius::Xs.px(cx),
            cx,
        );
        let chip = || {
            div()
                .radius(Radius::Sm, cx)
                .bg(palette.secondary)
                .px_2()
                .py(rems(0.125))
        };
        let clicked = item.clone();
        // El espacio entre el emoji y la cifra, en `text-xs`.
        let space = rems(0.75 * 0.25);

        // Todas las tarjetas miden lo mismo (lo pide la lista virtual): el
        // nombre y la descripción van en una línea, recortados, y la línea de
        // la descripción se reserva aunque no haya.
        let card = div()
            .id(SharedString::from(format!(
                "github-repo:{}",
                repo.id.unwrap_or_default()
            )))
            .test_support()
            .flex_shrink_0()
            .cursor_pointer()
            .radius(Radius::Md, cx)
            .border_1()
            .p(rems(0.625))
            .map(|this| {
                if selected {
                    this.border_color(with_alpha(palette.primary, 0.5))
                        .bg(with_alpha(palette.primary, 0.05))
                } else {
                    this.border_color(palette.border)
                        .bg(with_alpha(palette.card, 0.7))
                        .hover(|style| {
                            style
                                .border_color(with_alpha(palette.primary, 0.3))
                                .bg(with_alpha(palette.secondary, 0.4))
                        })
                }
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                this.select_repository(clicked.clone(), window, cx);
            }))
            .child(
                div()
                    .flex()
                    .items_start()
                    .justify_between()
                    .gap_3()
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .min_w_0()
                                    .child(div().flex_shrink_0().child(image.unwrap_or_else(
                                        || {
                                            github_icon(px(20.))
                                                .text_color(palette.muted_foreground)
                                                .into_any_element()
                                        },
                                    )))
                                    .child(
                                        div()
                                            .min_w_0()
                                            .truncate()
                                            .tw_text_sm()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(palette.foreground)
                                            .child(SharedString::from(
                                                repo.full_name.clone().unwrap_or_default(),
                                            )),
                                    )
                                    .when(repo.private == Some(true), |this| {
                                        this.child(
                                            mergeon_ui::badge("Private")
                                                .variant(BadgeVariant::Secondary),
                                        )
                                    })
                                    .when_some(
                                        owner_login.filter(|_| is_foreign),
                                        |this, login| {
                                            this.child(
                                                mergeon_ui::badge(login)
                                                    .variant(BadgeVariant::Outline),
                                            )
                                        },
                                    ),
                            )
                            .child(
                                div()
                                    .mt_1()
                                    // Una línea de `text-xs` (16px), haya o no
                                    // descripción.
                                    .h(rems(1.))
                                    .truncate()
                                    .tw_text_xs()
                                    .text_color(palette.muted_foreground)
                                    .children(
                                        repo.description
                                            .clone()
                                            .filter(|text| !text.is_empty())
                                            .map(SharedString::from),
                                    ),
                            ),
                    )
                    .child(
                        nowrap(format_updated_ago(
                            repo.updated_at.as_deref().unwrap_or_default(),
                        ))
                        .radius(Radius::Md, cx)
                        .border_1()
                        .border_color(palette.border)
                        .bg(with_alpha(palette.background, 0.8))
                        .px_2()
                        .py_1()
                        .tw_text_xs()
                        .tw_font_medium()
                        .text_color(palette.muted_foreground),
                    ),
            )
            .child(
                div()
                    .mt_2()
                    .flex()
                    .overflow_hidden()
                    .gap(rems(0.375))
                    .tw_text_xs()
                    .text_color(palette.muted_foreground)
                    .when_some(
                        repo.language
                            .clone()
                            .filter(|language| !language.is_empty()),
                        |this, language| {
                            this.child(
                                chip()
                                    .text_color(palette.foreground)
                                    .child(SharedString::from(language)),
                            )
                        },
                    )
                    .child(
                        chip()
                            .flex()
                            .items_center()
                            .gap(space)
                            // La estrella es un emoji: se pide a la fuente de
                            // emojis de la plataforma, como hace el navegador.
                            .child(div().font_family(EMOJI_FONT).child("⭐"))
                            .child(SharedString::from(repo.stars.unwrap_or(0).to_string())),
                    )
                    .child(
                        chip().child(SharedString::from(format!("⑂ {}", repo.forks.unwrap_or(0)))),
                    ),
            );
        // `space-y-2` entre tarjetas.
        div().pb_2().child(card).into_any_element()
    }

    /// Las tarjetas de las filas `range` de la lista virtual.
    fn render_repository_rows(
        &mut self,
        range: Range<usize>,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let account = self.shown_account();
        let indices: Vec<usize> = self.shown_results.get(range).unwrap_or_default().to_vec();
        indices
            .into_iter()
            .filter_map(|index| self.all_repositories.get(index).cloned())
            .map(|item| self.render_repository_card(&item, account.as_ref(), cx))
            .collect()
    }

    /// Un estado (cargando, falta instalar la app, sin resultados) o, con
    /// `None`, nada o la lista: lo que haya que listar queda en
    /// `shown_results` y lo pinta la lista virtual de `render`.
    fn render_results(
        &mut self,
        account: Option<&GitHubAccount>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        self.shown_results.clear();
        let is_loading = self.shown_model.loading;
        let query = self.search.read(cx).value();
        let repositories = filter_repository_indices(&self.all_repositories, &query);

        if is_loading {
            return Some(
                mergeon_ui::empty_state("Loading repositories")
                    .description("Fetching repositories from all available installations.")
                    .icon(Lucide::LoaderCircle)
                    .spin_icon(true)
                    .into_any_element(),
            );
        }

        let account = account?;
        if account.requires_installation == Some(true) {
            let has_install_url = account
                .install_url
                .as_deref()
                .is_some_and(|url| !url.is_empty());
            let refreshing = self.is_refreshing_account;
            return Some(
                div()
                    .id("github-picker-install-required")
                    .test_support()
                    .child(
                        mergeon_ui::empty_state("GitHub App not installed")
                            .description(
                                "Install the GitHub App first so GitHub can ask which account \
                                 and repositories to grant.",
                            )
                            .icon(Lucide::Puzzle)
                            .when(has_install_url, |this| {
                                this.child(
                                    mergeon_ui::button("github-picker-install")
                                        .label("Install GitHub App")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.open_installation_page(cx);
                                        })),
                                )
                            })
                            .child(
                                mergeon_ui::button("github-picker-refresh")
                                    .outline()
                                    .loading(refreshing)
                                    .disabled(refreshing)
                                    .label("Refresh Installations")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.refresh_selected_account(cx);
                                    })),
                            ),
                    )
                    .into_any_element(),
            );
        }

        if repositories.is_empty() {
            return Some(
                div()
                    .id("github-picker-empty")
                    .test_support()
                    .child(
                        mergeon_ui::empty_state("No repositories found")
                            .description(
                                "Only repositories covered by a GitHub App installation appear \
                                 here. To see a repository owned by someone else, its owner must \
                                 install the GitHub App and grant access to that repository.",
                            )
                            .icon(Lucide::PackageSearch),
                    )
                    .into_any_element(),
            );
        }

        // Los resultados se pintan en la lista virtual (ver `render`).
        self.shown_results = repositories;
        None
    }

    /// Slot `footer`: destino del clon y acciones.
    fn render_footer(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let is_cloning = self.is_cloning;
        let can_clone =
            !is_cloning && self.selected_repo.is_some() && !self.clone_path().is_empty();

        // `.picker-footer`
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .gap_3()
            .child(
                card_section(cx)
                    .child(field_label("Clone destination", cx).mb_2())
                    .child(
                        div()
                            .flex()
                            .gap_3()
                            .child(mergeon_ui::input(&self.destination).readonly(true).flex_1())
                            .child(
                                mergeon_ui::button("github-picker-browse")
                                    .outline()
                                    .disabled(is_cloning)
                                    .label("Browse…")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.select_clone_location(window, cx);
                                    })),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_3()
                    .child(
                        mergeon_ui::button("github-picker-cancel")
                            .ghost()
                            .label("Cancel")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.handle_close(window, cx);
                            })),
                    )
                    .child(
                        mergeon_ui::button("github-picker-clone")
                            .loading(is_cloning)
                            .disabled(!can_clone)
                            .label(if is_cloning {
                                "Cloning..."
                            } else {
                                "Clone Repository"
                            })
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.handle_clone(window, cx);
                            })),
                    ),
            )
            .into_any_element()
    }

    /// El diálogo de `mergeon_ui::dialog` que muestra esta vista.
    pub fn dialog_id(&self) -> Option<DialogId> {
        self.dialog_id
    }
}

impl Render for GitHubRepositoryPicker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        let account = self.shown_account();
        let can_search = account
            .as_ref()
            .is_some_and(|account| account.requires_installation != Some(true));
        let account_section = self.render_account_section(account.as_ref(), window, cx);
        let results = self.render_results(account.as_ref(), cx);
        let palette = *cx.palette();
        let max_height = px(RESULTS_MAX_HEIGHT);
        let listed = results.is_none() && !self.shown_results.is_empty();
        let app_installations = account
            .as_ref()
            .is_some_and(|account| !account.installations.is_empty());

        // `.picker-content`
        div()
            .flex()
            .flex_col()
            .min_h_0()
            .gap_3()
            .child(account_section)
            .when(can_search, |this| {
                this.child(
                    card_section(cx)
                        .child(field_label("Search repositories", cx).mb_2())
                        .child(mergeon_ui::input(&self.search)),
                )
            })
            .child(
                // `.picker-results`: `max-height: 320px; overflow-y: auto`
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .min_h_0()
                    .max_h(max_height)
                    .child(
                        div()
                            .id("github-picker-results")
                            .test_support()
                            .min_h_0()
                            .children(results)
                            .when(listed, |this| {
                                // Lista virtual: solo las tarjetas a la vista.
                                this.child(
                                    uniform_list(
                                        "github-picker-repositories",
                                        self.shown_results.len(),
                                        cx.processor(|this, range, _, cx| {
                                            this.render_repository_rows(range, cx)
                                        }),
                                    )
                                    .track_scroll(&self.results_scroll)
                                    .with_sizing_behavior(ListSizingBehavior::Infer)
                                    .max_h(max_height),
                                )
                            }),
                    )
                    .when(listed, |this| this.vertical_scrollbar(&self.results_scroll)),
            )
            .when(listed && app_installations, |this| {
                this.child(
                    div()
                        .flex_shrink_0()
                        .px_1()
                        .tw_text_xs()
                        .text_color(palette.muted_foreground)
                        .child(
                            "Missing a repository you collaborate on? This account is \
                             connected through a GitHub App, which only sees repositories \
                             from its installations, so its owner must install and grant it.",
                        ),
                )
            })
    }
}

/// Lo que el `watch(visible)` del original hacía al cerrarse: las peticiones
/// en vuelo se abandonan.
fn closed(cx: &mut App) {
    HomeDialogs::forget(TITLE, cx);
    AppModel::global(cx).update(cx, |model, cx| model.cancel_repository_requests(cx));
}

/// Abre el selector "Clone From GitHub" con la cuenta activa. Si ya está
/// abierto devuelve el que hay.
///
/// Un clon terminado se entrega a `AppModel::handle_repository_cloned` y
/// cierra el selector; "Add Account" abre [`open_github_account_dialog`].
pub fn open_github_repository_picker(
    window: &mut Window,
    cx: &mut App,
) -> Entity<GitHubRepositoryPicker> {
    if let Some(view) = HomeDialogs::open_view::<GitHubRepositoryPicker>(TITLE, window, cx) {
        return view;
    }

    let view = cx.new(|cx| GitHubRepositoryPicker::new(window, cx));
    let id = AppDialog::new()
        .title(TITLE)
        .description(
            "Choose an authenticated account and clone a repository into your local workspace.",
        )
        .width(DialogWidth::Xl3)
        .close_on_backdrop(false)
        .body({
            let view = view.clone();
            move |_, _| view.clone()
        })
        .footer({
            let view = view.downgrade();
            move |_, cx| {
                view.update(cx, |view, cx| view.render_footer(cx))
                    .unwrap_or_else(|_| div().into_any_element())
            }
        })
        .on_close(|_, cx| closed(cx))
        .open(window, cx);

    view.update(cx, |view, _| view.dialog_id = Some(id));
    HomeDialogs::register(TITLE, id, Some(view.clone().into_any()), cx);
    view
}
