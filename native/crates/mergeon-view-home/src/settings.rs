//! La pestaña Settings (`src/components/layout/SettingsView.vue`): Appearance,
//! Git, Accounts y Help / About.

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, EventEmitter, FontWeight,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, ScrollHandle,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, point, px, relative,
};
use mergeon_app::AppModel;
use mergeon_ui::{ActivePaletteExt as _, Lucide, Radius};

use crate::github_accounts::{GitHubAccountsEvent, GitHubAccountsManager};
use crate::help::{about_panel, shortcuts_panel, update_settings};
use crate::parts::{lucide, plain_button};
use crate::preferences::{PreferencesPanel, PreferencesSection};

/// Las secciones de Settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SettingsSection {
    #[default]
    Appearance,
    Git,
    Accounts,
    Help,
}

impl SettingsSection {
    pub const ALL: [SettingsSection; 4] = [
        SettingsSection::Appearance,
        SettingsSection::Git,
        SettingsSection::Accounts,
        SettingsSection::Help,
    ];

    /// La clave que usa `AppModel::open_settings` (`appearance`, `git`,
    /// `accounts`, `help`).
    pub fn key(self) -> &'static str {
        match self {
            Self::Appearance => "appearance",
            Self::Git => "git",
            Self::Accounts => "accounts",
            Self::Help => "help",
        }
    }

    /// La sección de una clave; una clave desconocida cae en Help, como el
    /// `v-else` del original.
    pub fn parse(key: &str) -> Self {
        match key {
            "appearance" => Self::Appearance,
            "git" => Self::Git,
            "accounts" => Self::Accounts,
            _ => Self::Help,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Appearance => "Appearance",
            Self::Git => "Git",
            Self::Accounts => "Accounts",
            Self::Help => "Help / About",
        }
    }

    /// La sección de `PreferencesPanel` que muestra, si es una de las suyas.
    fn preferences(self) -> Option<PreferencesSection> {
        match self {
            Self::Appearance => Some(PreferencesSection::Appearance),
            Self::Git => Some(PreferencesSection::Git),
            Self::Accounts | Self::Help => None,
        }
    }

    fn icon(self) -> Lucide {
        match self {
            Self::Appearance => Lucide::Palette,
            Self::Git => Lucide::GitBranch,
            Self::Accounts => Lucide::Users,
            Self::Help => Lucide::CircleQuestionMark,
        }
    }
}

/// Lo que Settings pide a la cáscara.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsEvent {
    /// "Back" (`emit('close')`): volver a lo que se veía antes, sin cerrar la
    /// pestaña. En `App.vue` es `showPreferences = false`.
    Close,
}

/// La vista de Settings. La sección inicial y los cambios posteriores llegan
/// de `AppModel::settings_section` (`AppModel::open_settings`).
pub struct SettingsView {
    model: Entity<AppModel>,
    section: SettingsSection,
    /// El último `initialSection` visto, para reaccionar solo cuando cambia.
    initial_section: String,
    preferences: Entity<PreferencesPanel>,
    accounts: Entity<GitHubAccountsManager>,
    body_scroll: ScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<SettingsEvent> for SettingsView {}

impl SettingsView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let model = AppModel::global(cx);
        let initial_section = model.read(cx).settings_section().to_string();
        let section = SettingsSection::parse(&initial_section);
        let preferences = cx.new(|cx| {
            PreferencesPanel::new(
                section
                    .preferences()
                    .unwrap_or(PreferencesSection::Appearance),
                window,
                cx,
            )
        });
        let accounts = cx.new(|cx| GitHubAccountsManager::new(true, cx));

        let subscriptions = vec![
            cx.observe_in(&model, window, |this, model, window, cx| {
                // `watch(() => props.initialSection)`
                let initial_section = model.read(cx).settings_section().to_string();
                if initial_section != this.initial_section {
                    this.set_section(SettingsSection::parse(&initial_section), window, cx);
                    this.initial_section = initial_section;
                }
                cx.notify();
            }),
            cx.subscribe(&accounts, |this, _, event: &GitHubAccountsEvent, cx| {
                if *event == GitHubAccountsEvent::AddAccount {
                    // `@add-account="emit('add-account')"` → `addAccountFromSettings`
                    this.model
                        .update(cx, |model, cx| model.add_account_from_settings(cx));
                }
            }),
        ];

        Self {
            model,
            section,
            initial_section,
            preferences,
            accounts,
            body_scroll: ScrollHandle::new(),
            _subscriptions: subscriptions,
        }
    }

    /// Sección visible.
    pub fn section(&self) -> SettingsSection {
        self.section
    }

    /// Cambia de sección (el clic en la navegación).
    pub fn set_section(
        &mut self,
        section: SettingsSection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.section == section {
            return;
        }
        let was_preferences = self.section.preferences().is_some();
        self.section = section;
        self.body_scroll.set_offset(point(px(0.), px(0.)));
        if let Some(preferences_section) = section.preferences() {
            if was_preferences {
                // Appearance y Git son el mismo formulario con otro `section`.
                self.preferences.update(cx, |preferences, cx| {
                    preferences.set_section(preferences_section, cx);
                });
            } else {
                // El `v-if` del original lo monta de nuevo: drafts frescos.
                self.preferences =
                    cx.new(|cx| PreferencesPanel::new(preferences_section, window, cx));
            }
        }
        cx.notify();
    }

    fn render_navigation(
        &self,
        compact: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let palette = *cx.palette();
        let radius = Radius::Md.px(cx);
        let items = SettingsSection::ALL.map(|item| {
            let active = self.section == item;
            plain_button(item.key(), radius, window, cx)
                .accessibility_label(item.label())
                .w_full()
                .justify_start()
                .gap(px(10.))
                .p(px(10.))
                .text_size(px(14.))
                .line_height(relative(1.5))
                .text_color(palette.muted_foreground)
                .selected(active)
                .when(active, |this| {
                    this.text_color(palette.foreground).bg(palette.accent)
                })
                .hover(|style| style.text_color(palette.foreground).bg(palette.accent))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.set_section(item, window, cx);
                }))
                .child(lucide(item.icon(), px(17.)))
                .child(item.label())
        });
        div()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .w(px(if compact { 150. } else { 180. }))
            .px(px(if compact { 8. } else { 12. }))
            .py(px(if compact { 12. } else { 16. }))
            .border_r_1()
            .border_color(palette.border)
            .children(items)
    }

    fn render_body(&self, cx: &Context<Self>) -> AnyElement {
        // `.settings-body > * { max-width: 720px }`
        let content = div().max_w(px(720.));
        match self.section {
            SettingsSection::Appearance | SettingsSection::Git => {
                content.child(self.preferences.clone()).into_any_element()
            }
            SettingsSection::Accounts => content.child(self.accounts.clone()).into_any_element(),
            SettingsSection::Help => content
                // `.help-content`: ocupa el ancho disponible (hasta 720 px); con
                // `items_start` cada bloque medía solo lo de su contenido y la
                // lista de atajos quedaba en una columna angosta.
                .w_full()
                .flex()
                .flex_col()
                .gap(px(28.))
                .child(about_panel(cx))
                .child(update_settings(cx))
                .child(shortcuts_panel(cx))
                .child(
                    // `.window-actions`
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(8.))
                        .child(
                            mergeon_ui::button("settings-minimize")
                                .outline()
                                .label("Minimize window")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.model
                                        .update(cx, |model, cx| model.request_minimize(cx));
                                })),
                        )
                        .child(
                            mergeon_ui::button("settings-quit")
                                .outline()
                                .label("Exit Mergeon")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.model.update(cx, |model, cx| model.request_quit(cx));
                                })),
                        ),
                )
                .into_any_element(),
        }
    }
}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        // `@media (max-width: 700px)`
        let compact = window.viewport_size().width <= px(700.);
        let navigation = self.render_navigation(compact, window, cx);

        // `.settings-view`
        div()
            .id("settings")
            .test_support()
            .flex()
            .flex_col()
            .size_full()
            .min_w_0()
            .min_h_0()
            .bg(palette.background)
            .text_color(palette.foreground)
            .child(
                // `header`
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap(px(20.))
                    .px(px(24.))
                    .py(px(16.))
                    .border_b_1()
                    .border_color(palette.border)
                    .child(
                        mergeon_ui::button("settings-back")
                            .ghost()
                            .icon(Lucide::ArrowLeft)
                            .label("Back")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(SettingsEvent::Close))),
                    )
                    .child(
                        div()
                            .text_size(px(20.))
                            .line_height(relative(1.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Settings"),
                    ),
            )
            .child(
                // `.settings-layout`
                div().flex().flex_1().min_h_0().child(navigation).child(
                    div()
                        .relative()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .min_h_0()
                        .child(
                            // `.settings-body`
                            div()
                                .id("settings-body")
                                .test_support()
                                .size_full()
                                .overflow_y_scroll()
                                .track_scroll(&self.body_scroll)
                                .map(|this| {
                                    if compact {
                                        this.px(px(16.)).py(px(20.))
                                    } else {
                                        this.p(px(28.))
                                    }
                                })
                                .child(self.render_body(cx)),
                        )
                        .vertical_scrollbar(&self.body_scroll),
                ),
            )
    }
}
