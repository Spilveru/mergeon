//! Preferencias (`src/components/repository/PreferencesDialog.vue`): identidad
//! de git, auto-fetch y tema. En la app solo se ve incrustado en Settings
//! (Appearance y Git); como diálogo muestra las tres secciones.

use gpui_kit::base::{ObservedElement, TestSupportExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Div, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, Role, SharedString, Stateful, StatefulInteractiveElement as _,
    Styled as _, Subscription, Window, div, px, rems,
};
use mergeon_app::AppModel;
use mergeon_app::backend::editor::editor_definition;
use mergeon_app::backend::{
    DetectedEditor, ExternalEditorSettings, IdentityValues, SetIdentityOptions,
};
use mergeon_ui::dialog::{self, AppDialog, DialogId, DialogWidth};
use mergeon_ui::{
    ActivePaletteExt as _, InputEvent, InputState, MergeonStyled as _, RadioOption, SelectOption,
};

use crate::parts::HomeDialogs;

const TITLE: &str = "Preferences";

/// Qué secciones muestra el panel (el prop `section`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PreferencesSection {
    /// Identidad, auto-fetch y tema.
    #[default]
    All,
    /// Solo el tema.
    Appearance,
    /// Identidad y auto-fetch.
    Git,
}

impl PreferencesSection {
    /// Traduce el valor del prop: `appearance`, `git` o cualquier otro (`all`).
    pub fn parse(section: &str) -> Self {
        match section {
            "appearance" => Self::Appearance,
            "git" => Self::Git,
            _ => Self::All,
        }
    }
}

/// `INTERVAL_OPTIONS`
const INTERVALS: [u32; 4] = [5, 10, 15, 30];

/// Las opciones del selector de editor externo: "Automatic", los editores
/// detectados (y el elegido aunque ya no se encuentre), el programa por
/// defecto y un comando propio.
pub fn editor_options(detected: &[DetectedEditor], chosen: &str) -> Vec<SelectOption> {
    let automatic = match detected.first() {
        Some(editor) => format!("Automatic ({})", editor.name),
        None => "Automatic (system default)".to_string(),
    };
    let mut options = vec![SelectOption::new(ExternalEditorSettings::AUTO, automatic)];
    options.extend(
        detected
            .iter()
            .map(|editor| SelectOption::new(editor.id.clone(), editor.name.clone())),
    );
    let special = [
        ExternalEditorSettings::AUTO,
        ExternalEditorSettings::SYSTEM,
        ExternalEditorSettings::CUSTOM,
    ];
    if !special.contains(&chosen) && !detected.iter().any(|editor| editor.id == chosen) {
        let name = editor_definition(chosen).map_or(chosen, |editor| editor.name);
        options.push(SelectOption::new(
            chosen.to_string(),
            format!("{name} (not found)"),
        ));
    }
    options.push(SelectOption::new(
        ExternalEditorSettings::SYSTEM,
        "System default app",
    ));
    options.push(SelectOption::new(
        ExternalEditorSettings::CUSTOM,
        "Custom command…",
    ));
    options
}

/// Qué hará "Open in editor" con la opción elegida, en una línea.
fn editor_hint(detected: &[DetectedEditor], chosen: &str) -> String {
    let found = |id: &str| detected.iter().find(|editor| editor.id == id);
    match chosen {
        ExternalEditorSettings::AUTO => match detected.first() {
            Some(editor) => format!("Opens files with {} ({}).", editor.name, editor.executable),
            None => "No editor detected: files open with the system's default app.".to_string(),
        },
        ExternalEditorSettings::SYSTEM => {
            "Files open with the system's default app (without jumping to the line).".to_string()
        }
        ExternalEditorSettings::CUSTOM => {
            "{path}, {line} and {column} are replaced when opening; without {path}, the path goes last."
                .to_string()
        }
        id => match found(id) {
            Some(editor) => format!("Runs {}.", editor.executable),
            None => "Not found on this system: install it or choose another editor.".to_string(),
        },
    }
}
/// El formulario de preferencias.
pub struct PreferencesPanel {
    model: Entity<AppModel>,
    section: PreferencesSection,
    identity_scope: SharedString,
    pub(crate) name: Entity<InputState>,
    pub(crate) email: Entity<InputState>,
    global_identity: IdentityValues,
    local_identity: IdentityValues,
    saving_identity: bool,
    // Mientras carga, los inputs quedan bloqueados: el prefill asíncrono no debe
    // pisar lo que el usuario ya empezó a escribir.
    loading_identity: bool,
    identity_request: u64,
    active_repo_path: Option<String>,
    /// Editores instalados (se buscan al abrir el panel).
    editors: Vec<DetectedEditor>,
    /// La opción del selector de editor. Puede ser "custom" antes de que el
    /// comando se guarde.
    editor_choice: SharedString,
    /// Lo último que se vio guardado, para rellenar el formulario si cambia.
    saved_editor: ExternalEditorSettings,
    pub(crate) editor_command: Entity<InputState>,
    pub(crate) editor_args: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

impl PreferencesPanel {
    pub fn new(section: PreferencesSection, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let model = AppModel::global(cx);
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("Your name"));
        let email = cx.new(|cx| InputState::new(window, cx).placeholder("you@example.com"));
        let editor_command = cx.new(|cx| {
            InputState::new(window, cx).placeholder("/usr/bin/vim or C:\\Tools\\editor.exe")
        });
        let editor_args =
            cx.new(|cx| InputState::new(window, cx).placeholder("--goto {path}:{line}:{column}"));
        let subscriptions = vec![
            cx.observe_in(&model, window, |this, _, window, cx| {
                // `watch(() => repositoriesStore.activeRepository?.path)`
                let path = this.current_repo_path(cx);
                if path != this.active_repo_path {
                    this.active_repo_path = path;
                    this.load_identity(window, cx);
                }
                this.sync_saved_editor(window, cx);
                cx.notify();
            }),
            cx.subscribe(&editor_command, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }),
            cx.subscribe(&editor_args, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }),
            cx.subscribe(&name, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }),
            cx.subscribe(&email, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }),
        ];

        let mut this = Self {
            model,
            section,
            identity_scope: "global".into(),
            name,
            email,
            global_identity: IdentityValues::default(),
            local_identity: IdentityValues::default(),
            saving_identity: false,
            loading_identity: false,
            identity_request: 0,
            active_repo_path: None,
            editors: Vec::new(),
            editor_choice: ExternalEditorSettings::AUTO.into(),
            saved_editor: ExternalEditorSettings::default(),
            editor_command,
            editor_args,
            _subscriptions: subscriptions,
        };
        this.active_repo_path = this.current_repo_path(cx);
        // `watch(visible, { immediate: true })`
        this.load_identity(window, cx);
        this.saved_editor = this.model.read(cx).external_editor().clone();
        this.apply_saved_editor(window, cx);
        this.detect_editors(window, cx);
        this
    }

    // ------------------------------------------------------------------
    // Editor externo
    // ------------------------------------------------------------------

    /// La opción elegida en el selector de editor.
    pub fn editor_choice(&self) -> &SharedString {
        &self.editor_choice
    }

    /// Los editores instalados que se encontraron.
    pub fn detected_editors(&self) -> &[DetectedEditor] {
        &self.editors
    }

    fn detect_editors(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let detect = self.model.update(cx, |model, cx| model.detect_editors(cx));
        cx.spawn_in(window, async move |this, cx| {
            let editors = detect.await;
            this.update(cx, |this, cx| {
                this.editors = editors;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Lleva al formulario el editor guardado.
    fn apply_saved_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let saved = self.saved_editor.clone();
        self.editor_choice = saved.editor.clone().into();
        self.editor_command.update(cx, |state, cx| {
            state.set_value(saved.custom_command.clone(), window, cx);
        });
        self.editor_args.update(cx, |state, cx| {
            state.set_value(saved.custom_args.clone(), window, cx);
        });
    }

    /// Si el editor guardado cambió (se terminó de leer, o lo cambió otra
    /// vista), el formulario lo adopta.
    fn sync_saved_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let saved = self.model.read(cx).external_editor().clone();
        if saved != self.saved_editor {
            self.saved_editor = saved;
            self.apply_saved_editor(window, cx);
        }
    }

    /// Elige una opción del selector. Un editor (o "Automatic", o el programa
    /// por defecto) se guarda al momento; "Custom command…" se guarda así si ya
    /// había un comando guardado y, si no, espera a que se pulse "Save editor".
    pub fn choose_editor(&mut self, value: SharedString, cx: &mut Context<Self>) {
        self.editor_choice = value.clone();
        cx.notify();
        if value.as_ref() == ExternalEditorSettings::CUSTOM
            && self.saved_editor.custom_command.trim().is_empty()
        {
            return;
        }
        // El comando propio guardado se conserva, para poder volver a él.
        let editor = ExternalEditorSettings {
            editor: value.to_string(),
            ..self.saved_editor.clone()
        };
        self.saved_editor = editor.clone();
        self.model
            .update(cx, |model, cx| model.save_external_editor(editor, cx))
            .detach();
    }

    /// "Save editor": guarda el comando propio con sus argumentos.
    pub fn save_custom_editor(&mut self, cx: &mut Context<Self>) {
        let command = self.editor_command.read(cx).value().trim().to_string();
        if command.is_empty() {
            return;
        }
        let editor =
            ExternalEditorSettings::custom(command, self.editor_args.read(cx).value().to_string());
        self.editor_choice = ExternalEditorSettings::CUSTOM.into();
        self.saved_editor = editor.clone();
        self.model
            .update(cx, |model, cx| model.save_external_editor(editor, cx))
            .detach();
        cx.notify();
    }

    /// Sección visible.
    pub fn section(&self) -> PreferencesSection {
        self.section
    }

    /// Cambia la sección visible (Settings reutiliza el panel entre
    /// Appearance y Git).
    pub fn set_section(&mut self, section: PreferencesSection, cx: &mut Context<Self>) {
        if self.section != section {
            self.section = section;
            cx.notify();
        }
    }

    /// Ámbito de la identidad que se edita: `global` o `local`.
    pub fn identity_scope(&self) -> &str {
        &self.identity_scope
    }

    /// Nombre escrito en el formulario.
    pub fn identity_name(&self, cx: &App) -> SharedString {
        self.name.read(cx).value()
    }

    /// Correo escrito en el formulario.
    pub fn identity_email(&self, cx: &App) -> SharedString {
        self.email.read(cx).value()
    }

    /// Si la identidad se está leyendo.
    pub fn is_loading_identity(&self) -> bool {
        self.loading_identity
    }

    fn current_repo_path(&self, cx: &App) -> Option<String> {
        self.model
            .read(cx)
            .active_repository()
            .map(|repo| repo.path.clone())
    }

    /// Los `placeholder` de nombre y correo: en el ámbito del repositorio, un
    /// campo vacío hereda (y enseña) el valor global.
    pub fn identity_placeholders(&self) -> (SharedString, SharedString) {
        let local = self.identity_scope.as_ref() == "local";
        let inherited = |value: &Option<String>, fallback: &'static str| -> SharedString {
            match value.as_deref().filter(|value| local && !value.is_empty()) {
                Some(value) => SharedString::from(value.to_string()),
                None => fallback.into(),
            }
        };
        (
            inherited(&self.global_identity.name, "Your name"),
            inherited(&self.global_identity.email, "you@example.com"),
        )
    }

    /// `applyScopeToForm`: lleva al formulario la identidad del ámbito elegido.
    fn apply_scope_to_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let local = self.identity_scope.as_ref() == "local";
        let source = if local {
            self.local_identity.clone()
        } else {
            self.global_identity.clone()
        };
        let (name_placeholder, email_placeholder) = self.identity_placeholders();

        self.name.update(cx, |state, cx| {
            state.set_placeholder(name_placeholder, window, cx);
            state.set_value(source.name.unwrap_or_default(), window, cx);
        });
        self.email.update(cx, |state, cx| {
            state.set_placeholder(email_placeholder, window, cx);
            state.set_value(source.email.unwrap_or_default(), window, cx);
        });
        cx.notify();
    }

    pub(crate) fn set_identity_scope(
        &mut self,
        scope: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.identity_scope == scope {
            return;
        }
        self.identity_scope = scope;
        // `watch(identityScope, applyScopeToForm)`
        self.apply_scope_to_form(window, cx);
    }

    /// `loadIdentity`.
    fn load_identity(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.identity_request += 1;
        let request = self.identity_request;

        let Some(repo_path) = self.current_repo_path(cx) else {
            self.identity_scope = "global".into();
            self.global_identity = IdentityValues::default();
            self.local_identity = IdentityValues::default();
            self.loading_identity = false;
            self.apply_scope_to_form(window, cx);
            return;
        };

        self.loading_identity = true;
        cx.notify();

        let model = self.model.clone();
        let read = model.update(cx, |model, cx| model.get_identity(&repo_path, cx));
        cx.spawn_in(window, async move |this, cx| {
            let identity = read.await;
            let failure = this
                .update_in(cx, |this, window, cx| {
                    if request != this.identity_request {
                        return None;
                    }
                    this.loading_identity = false;
                    cx.notify();
                    match identity {
                        Ok(identity) => {
                            this.global_identity = identity.global;
                            this.local_identity = identity.local;
                            this.apply_scope_to_form(window, cx);
                            None
                        }
                        Err(error) => Some(error.to_string()),
                    }
                })
                .ok()
                .flatten();

            if let Some(message) = failure {
                model.update(cx, |model, cx| {
                    model.notify_error(
                        if message.is_empty() {
                            "Failed to read the git identity"
                        } else {
                            &message
                        },
                        cx,
                    );
                });
            }
        })
        .detach();
    }

    /// `saveIdentity`.
    fn save_identity(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo_path) = self.current_repo_path(cx) else {
            self.model.update(cx, |model, cx| {
                model.notify_warning("Open a repository to change the git identity.", cx);
            });
            return;
        };

        self.saving_identity = true;
        cx.notify();

        let options = SetIdentityOptions {
            scope: self.identity_scope.to_string(),
            name: Some(self.name.read(cx).value().to_string()),
            email: Some(self.email.read(cx).value().to_string()),
        };
        let saving = self
            .model
            .update(cx, |model, cx| model.set_identity(&repo_path, options, cx));
        cx.spawn_in(window, async move |this, cx| {
            let result = saving.await;
            this.update_in(cx, |this, window, cx| {
                if result.success {
                    this.load_identity(window, cx);
                }
                this.saving_identity = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // ------------------------------------------------------------------
    // Render
    // ------------------------------------------------------------------

    /// `.prefs__section`
    fn section_block(
        id: &'static str,
        title: &'static str,
        cx: &App,
    ) -> ObservedElement<Stateful<Div>> {
        div()
            .id(id)
            .test_support()
            .flex()
            .flex_col()
            .gap(rems(0.5))
            .child(
                // `.prefs__title`
                div()
                    .text_size(rems(0.8125))
                    .line_height(gpui_kit::relative(1.5))
                    .tw_font_medium()
                    .text_color(cx.palette().foreground)
                    .child(title),
            )
    }

    /// `.prefs__hint`
    fn hint(text: &'static str, cx: &App) -> Div {
        div()
            .text_size(rems(0.75))
            .line_height(gpui_kit::relative(1.5))
            .text_color(cx.palette().muted_foreground)
            .child(text)
    }

    /// `.prefs__field`: rótulo de 70px y el control en el resto.
    fn field(label: &'static str, control: impl IntoElement, cx: &App) -> Div {
        div()
            .flex()
            .items_center()
            .gap(rems(0.5))
            .child(
                div()
                    .w(px(70.))
                    .flex_shrink_0()
                    .text_size(rems(0.8125))
                    .line_height(gpui_kit::relative(1.5))
                    .text_color(cx.palette().foreground)
                    .child(label),
            )
            .child(div().flex_1().min_w_0().child(control))
    }
}

impl PreferencesPanel {
    /// "External editor": con qué se abre "Open in editor".
    fn render_editor_section(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let choice = self.editor_choice.clone();
        let custom = choice.as_ref() == ExternalEditorSettings::CUSTOM;
        let has_command = !self.editor_command.read(cx).value().trim().is_empty();
        let custom_changed = custom && {
            let saved = &self.saved_editor;
            !saved.is_custom()
                || saved.custom_command != self.editor_command.read(cx).value().trim()
                || saved.custom_args != self.editor_args.read(cx).value().as_ref()
        };

        Self::section_block("prefs-editor-section", "External editor", cx)
            .child(Self::hint(
                "Used by \"Open in editor\" on files and repositories.",
                cx,
            ))
            .child(Self::field(
                "Editor",
                mergeon_ui::select("prefs-editor")
                    .items(editor_options(&self.editors, &choice))
                    .value(choice.clone())
                    .on_change(cx.listener(|this, value: &SharedString, window, cx| {
                        this.choose_editor(value.clone(), cx);
                        // Con "Custom command…" lo siguiente es escribirlo: el
                        // cursor va al campo (el primer clic en el campo recién
                        // aparecido se perdía, QA 0.12.0).
                        if value.as_ref() == ExternalEditorSettings::CUSTOM {
                            this.editor_command
                                .update(cx, |state, cx| state.focus(window, cx));
                        }
                    })),
                cx,
            ))
            .child({
                let hint = editor_hint(&self.editors, &choice);
                div()
                    .id("prefs-editor-hint")
                    .role(Role::Status)
                    .aria_label(hint.clone())
                    .test_support()
                    .text_size(rems(0.75))
                    .line_height(gpui_kit::relative(1.5))
                    .text_color(cx.palette().muted_foreground)
                    .child(hint)
            })
            .when(custom, |this| {
                this.child(Self::field(
                    "Command",
                    mergeon_ui::input(&self.editor_command),
                    cx,
                ))
                .child(Self::field(
                    "Arguments",
                    mergeon_ui::input(&self.editor_args),
                    cx,
                ))
                .child(
                    div().flex().justify_end().child(
                        mergeon_ui::button("prefs-save-editor")
                            .sm()
                            .disabled(!has_command || !custom_changed)
                            .label("Save editor")
                            .on_click(cx.listener(|this, _, _, cx| this.save_custom_editor(cx))),
                    ),
                )
            })
    }
}

impl Render for PreferencesPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.model.read(cx);
        let has_repository = model.active_repository().is_some();
        let auto_fetch_enabled = model.auto_fetch_enabled();
        let auto_fetch_interval = model.auto_fetch_interval_minutes();
        let skip_action_dialogs = model.skip_action_dialogs();
        let desktop_notifications = model.desktop_notifications_enabled();
        let theme = if model.is_dark() { "dark" } else { "light" };

        let section = self.section;
        let local = self.identity_scope.as_ref() == "local";
        let fields_disabled = !has_repository || self.loading_identity || self.saving_identity;
        let inherits = local
            && self.name.read(cx).value().is_empty()
            && self.email.read(cx).value().is_empty();
        let saving = self.saving_identity;
        let loading = self.loading_identity;

        // `.prefs`
        div()
            .id("preferences")
            .test_support()
            .flex()
            .flex_col()
            .gap(rems(1.1))
            .when(section != PreferencesSection::Appearance, |this| {
                this.child(
                    Self::section_block("prefs-identity", "Git identity", cx)
                        .child(Self::hint("Used as author of your commits.", cx))
                        .when(!has_repository, |this| {
                            this.child(Self::hint(
                                "Open a repository to configure Git identity.",
                                cx,
                            ))
                        })
                        .child(
                            mergeon_ui::radio_group("prefs-identity-scope")
                                .option(RadioOption::new("global", "Global (all repositories)"))
                                .option(RadioOption::new("local", "This repository"))
                                .selected(self.identity_scope.clone())
                                .on_change(cx.listener(
                                    |this, value: &SharedString, window, cx| {
                                        this.set_identity_scope(value.clone(), window, cx);
                                    },
                                )),
                        )
                        .child(Self::field(
                            "Name",
                            mergeon_ui::input(&self.name).disabled(fields_disabled),
                            cx,
                        ))
                        .child(Self::field(
                            "Email",
                            mergeon_ui::input(&self.email).disabled(fields_disabled),
                            cx,
                        ))
                        .when(inherits, |this| {
                            this.child(
                                Self::hint("Empty fields inherit the global identity.", cx)
                                    .id("prefs-inherit-hint")
                                    .test_support(),
                            )
                        })
                        .child(
                            // `.prefs__actions`
                            div().flex().justify_end().child(
                                mergeon_ui::button("prefs-save-identity")
                                    .sm()
                                    .loading(saving)
                                    .disabled(!has_repository || loading)
                                    .label("Save identity")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.save_identity(window, cx);
                                    })),
                            ),
                        ),
                )
                .child(
                    Self::section_block("prefs-auto-fetch-section", "Auto-fetch", cx)
                        .child(
                            div().flex().child(
                                mergeon_ui::checkbox("prefs-auto-fetch")
                                    .label("Fetch automatically in the background")
                                    .checked(auto_fetch_enabled)
                                    .on_change(cx.listener(move |this, checked: &bool, _, cx| {
                                        // Auto-fetch y tema se aplican al vuelo (sin botón Save).
                                        let checked = *checked;
                                        this.model
                                            .update(cx, |model, cx| {
                                                model.save_auto_fetch_settings(
                                                    checked,
                                                    f64::from(auto_fetch_interval),
                                                    cx,
                                                )
                                            })
                                            .detach();
                                    })),
                            ),
                        )
                        .child(Self::field(
                            "Interval",
                            mergeon_ui::select("prefs-auto-fetch-interval")
                                .items(INTERVALS.map(|minutes| {
                                    SelectOption::new(
                                        minutes.to_string(),
                                        format!("Every {minutes} minutes"),
                                    )
                                }))
                                .value(auto_fetch_interval.to_string())
                                .disabled(!auto_fetch_enabled)
                                .on_change(cx.listener(
                                    move |this, value: &SharedString, _, cx| {
                                        let Ok(minutes) = value.parse::<u32>() else {
                                            return;
                                        };
                                        this.model
                                            .update(cx, |model, cx| {
                                                model.save_auto_fetch_settings(
                                                    auto_fetch_enabled,
                                                    f64::from(minutes),
                                                    cx,
                                                )
                                            })
                                            .detach();
                                    },
                                )),
                            cx,
                        )),
                )
                .child(
                    Self::section_block("prefs-notifications-section", "Notifications", cx)
                        .child(
                            div().flex().child(
                                mergeon_ui::checkbox("prefs-desktop-notifications")
                                    .label("Notify when a long fetch, pull, push or clone finishes")
                                    .checked(desktop_notifications)
                                    .on_change(cx.listener(|this, checked: &bool, _, cx| {
                                        let checked = *checked;
                                        this.model
                                            .update(cx, |model, cx| {
                                                model.set_desktop_notifications_enabled(checked, cx)
                                            })
                                            .detach();
                                    })),
                            ),
                        )
                        .child(Self::hint(
                            "A system notification, only while Mergeon is in the background.",
                            cx,
                        )),
                )
                .child(self.render_editor_section(cx))
                .child(
                    Self::section_block("prefs-actions-section", "Git actions", cx)
                        .child(
                            div().flex().child(
                                mergeon_ui::checkbox("prefs-skip-action-dialogs")
                                    .label(
                                        "Toolbar buttons run Fetch, Pull, and Push immediately (skip options dialog)",
                                    )
                                    .checked(skip_action_dialogs)
                                    .on_change(cx.listener(|this, checked: &bool, _, cx| {
                                        let checked = *checked;
                                        this.model
                                            .update(cx, |model, cx| {
                                                model.set_skip_action_dialogs(checked, cx)
                                            })
                                            .detach();
                                    })),
                            ),
                        )
                        .child(Self::hint(
                            "Keyboard shortcuts always run Fetch, Pull, and Push directly using your last confirmed options (falling back to the dialog when no safe target exists). Shift+click, right-click, or the Command Palette still opens options.",
                            cx,
                        )),
                )
            })
            .when(section != PreferencesSection::Git, |this| {
                this.child(
                    Self::section_block("prefs-theme-section", "Theme", cx).child(
                        mergeon_ui::radio_group("prefs-theme")
                            .option(RadioOption::new("dark", "Dark"))
                            .option(RadioOption::new("light", "Light"))
                            .selected(theme)
                            .on_change(cx.listener(|this, value: &SharedString, _, cx| {
                                let dark = value.as_ref() == "dark";
                                this.model
                                    .update(cx, |model, cx| model.set_dark(dark, cx))
                                    .detach();
                            })),
                    ),
                )
            })
    }
}

/// Abre Preferences como diálogo, con las tres secciones.
pub fn open_preferences_dialog(window: &mut Window, cx: &mut App) -> DialogId {
    if let Some((id, _)) = HomeDialogs::open_dialog(TITLE, window, cx) {
        return id;
    }

    let view = cx.new(|cx| PreferencesPanel::new(PreferencesSection::All, window, cx));
    let id = AppDialog::new()
        .title(TITLE)
        .width(DialogWidth::Lg)
        .body({
            let view = view.clone();
            move |_, _| view.clone()
        })
        .footer(|_, _| {
            div().flex().justify_end().gap_2().child(
                mergeon_ui::button("preferences-close")
                    .ghost()
                    .sm()
                    .label("Close")
                    .on_click(|_, window, cx| {
                        if let Some((id, _)) = HomeDialogs::open_dialog(TITLE, window, cx) {
                            dialog::close_id(window, cx, id);
                        }
                        HomeDialogs::forget(TITLE, cx);
                    }),
            )
        })
        .on_close(|_, cx| HomeDialogs::forget(TITLE, cx))
        .open(window, cx);
    HomeDialogs::register(TITLE, id, Some(view.into_any()), cx);
    id
}
