//! Ajustes de la aplicación: `useTheme`, `useAppSettings`,
//! `useDiffPreferences`, `useSidebarWidth`, `useFileListWidth`, el ancho del
//! historial de `CommitView.vue` y las secciones desplegadas de `Sidebar.vue`.
//!
//! En el original cada uno era estado de módulo con su propia lectura
//! perezosa; aquí es un único estado dentro de [`AppModel`]. Las reglas (qué
//! vale como ancho, cuándo toca guardar, qué se persiste) siguen en
//! `mergeon-model`; este módulo lee, guarda y avisa.
//!
//! Un fallo al guardar no se queda en el log: se avisa con
//! `reportPersistenceError`, como en el original.

use std::collections::BTreeMap;

use gpui_kit::{Context, Task};
use mergeon_backend::Store;
use mergeon_model::diff_preferences::DiffPreferences;
use mergeon_model::file_list_width::{
    file_list_width, file_list_width_settings, saved_file_list_width,
};
use mergeon_model::keyboard_shortcuts::{
    ShortcutAction, default_combo_for, normalize_overrides, normalize_shortcut_override,
};
use mergeon_model::resizable_width::ResizableWidth;
use mergeon_model::sidebar_width::{saved_sidebar_width, sidebar_width, sidebar_width_settings};
use mergeon_store::{
    ActionDefaults, DEFAULT_AUTO_FETCH_INTERVAL_MINUTES, EXTERNAL_EDITOR_KEY,
    ExternalEditorSettings, KEYBOARD_SHORTCUTS_KEY, SKIP_ACTION_DIALOGS_KEY, Settings,
    SettingsUpdate, SidebarSections, Theme,
};
use serde_json::{Value, json};

use crate::tasks::flow;
use crate::{AppEvent, AppModel};

#[derive(Debug)]
pub(crate) struct SettingsState {
    /// `isDark` de `useTheme`: oscuro por defecto.
    pub is_dark: bool,
    pub auto_fetch_enabled: bool,
    pub auto_fetch_interval_minutes: u32,
    /// Notificar al escritorio las operaciones de red largas que terminan sin
    /// foco (`desktopNotifications`; encendido si no está guardado).
    pub desktop_notifications: bool,
    app_settings_loaded: bool,
    pub diff_preferences: DiffPreferences,
    pub sidebar_width: ResizableWidth,
    pub file_list_width: ResizableWidth,
    /// Ancho guardado del panel de historial; `None` es el de diseño.
    pub history_width: Option<f64>,
    pub sidebar_sections: SidebarSections,
    /// El editor externo elegido (lo que muestra Settings → Git).
    pub external_editor: ExternalEditorSettings,
    /// Ejecutar Fetch / Pull / Push directamente sin diálogo cuando hay valores
    /// por defecto seguros (`settings.skipActionDialogs`).
    pub skip_action_dialogs: bool,
    /// Últimas opciones seguras confirmadas en los diálogos de Fetch / Pull / Push
    /// (`settings.actionDefaults`).
    pub action_defaults: ActionDefaults,
    /// Atajos de teclado personalizados (`settings.keyboardShortcuts`).
    pub keyboard_shortcuts: BTreeMap<String, String>,
    /// Preferencias de la pestaña "Local Changes".
    pub local_changes: LocalChangesPrefs,
    local_changes_loaded: bool,
}

/// Preferencias de la pestaña "Local Changes" (dentro de
/// `settings.commitView`). Sin guardar valen las de diseño.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LocalChangesPrefs {
    /// Ancho de la columna de la lista y la caja de commit (`localChangesWidth`).
    pub width: Option<f64>,
    /// Lista plana en vez de árbol (`changesListLayout: "flat"`).
    pub flat_list: bool,
    /// Se cerró la caja de atajos sobre la caja de commit
    /// (`changesHintsDismissed`).
    pub hints_dismissed: bool,
}

const LOCAL_CHANGES_WIDTH_KEY: &str = "localChangesWidth";
const CHANGES_LIST_LAYOUT_KEY: &str = "changesListLayout";
const CHANGES_HINTS_DISMISSED_KEY: &str = "changesHintsDismissed";

impl LocalChangesPrefs {
    /// Lee las preferencias de los ajustes en JSON (`settings.commitView`).
    pub fn from_settings(settings: &Value) -> Self {
        let commit_view = settings.get("commitView");
        let key = |key: &str| commit_view.and_then(|commit_view| commit_view.get(key));
        Self {
            width: key(LOCAL_CHANGES_WIDTH_KEY)
                .and_then(Value::as_f64)
                .filter(|width| width.is_finite() && *width > 0.0),
            flat_list: key(CHANGES_LIST_LAYOUT_KEY).and_then(Value::as_str) == Some("flat"),
            hints_dismissed: key(CHANGES_HINTS_DISMISSED_KEY) == Some(&Value::Bool(true)),
        }
    }
}

impl Default for SettingsState {
    fn default() -> Self {
        Self {
            is_dark: true,
            auto_fetch_enabled: true,
            auto_fetch_interval_minutes: DEFAULT_AUTO_FETCH_INTERVAL_MINUTES,
            desktop_notifications: true,
            app_settings_loaded: false,
            diff_preferences: DiffPreferences::new(),
            sidebar_width: sidebar_width(),
            file_list_width: file_list_width(),
            history_width: None,
            sidebar_sections: SidebarSections::default(),
            external_editor: ExternalEditorSettings::default(),
            skip_action_dialogs: false,
            action_defaults: ActionDefaults::default(),
            keyboard_shortcuts: BTreeMap::new(),
            local_changes: LocalChangesPrefs::default(),
            local_changes_loaded: false,
        }
    }
}

impl SettingsState {
    /// Como una ventana recién cargada. El tema se conserva hasta releerlo,
    /// para no dar un destello.
    pub fn reset(&mut self) {
        let is_dark = self.is_dark;
        *self = Self {
            is_dark,
            ..Self::default()
        };
    }
}

/// `normalizeInterval` de `useAppSettings`.
pub fn normalize_auto_fetch_interval(minutes: f64) -> u32 {
    if minutes.is_finite() && minutes >= 1.0 {
        // `Math.round`.
        (minutes + 0.5).floor().min(f64::from(u32::MAX)) as u32
    } else {
        DEFAULT_AUTO_FETCH_INTERVAL_MINUTES
    }
}

fn settings_value(settings: &Settings) -> Value {
    serde_json::to_value(settings).unwrap_or(Value::Null)
}

impl AppModel {
    /// `storage.getSettings()`.
    pub fn load_settings(&self, cx: &mut Context<Self>) -> Task<Settings> {
        self.storage(cx, Store::get_settings)
    }

    /// `storage.updateSettings(patch)`; si falla, avisa con
    /// `reportPersistenceError(failure_context, error)`.
    pub(crate) fn persist_settings(
        &mut self,
        patch: Value,
        failure_context: &'static str,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        let write = self.storage(cx, move |store| {
            store
                .update_settings_json(&patch)
                .map(|_| ())
                .map_err(|error| error.to_string())
        });
        flow(cx, async move |this, cx| {
            if let Err(error) = write.await {
                this.update(cx, |this, cx| {
                    this.report_persistence_error(failure_context, &error, cx);
                });
            }
        })
    }

    // ------------------------------------------------------------------
    // useTheme
    // ------------------------------------------------------------------

    /// `isDark`.
    pub fn is_dark(&self) -> bool {
        self.settings.is_dark
    }

    /// `applyTheme`: la cáscara aplica el tema a la ventana (y, en Windows, a
    /// los controles de la barra de título) al recibir el evento.
    fn apply_theme(&mut self, cx: &mut Context<Self>) {
        cx.emit(AppEvent::ThemeChanged {
            dark: self.settings.is_dark,
        });
        cx.notify();
    }

    /// `loadTheme`: adopta el tema guardado y lo aplica.
    pub fn load_theme(&mut self, cx: &mut Context<Self>) -> Task<()> {
        let read = self.load_settings(cx);
        flow(cx, async move |this, cx| {
            let settings = read.await;
            this.update(cx, |this, cx| {
                this.settings.is_dark = settings.theme == Theme::Dark;
                this.apply_theme(cx);
            });
        })
    }

    /// Cambia el tema, lo aplica y lo guarda (el `watch(isDark)` del
    /// original).
    pub fn set_dark(&mut self, dark: bool, cx: &mut Context<Self>) -> Task<()> {
        if self.settings.is_dark == dark {
            return Task::ready(());
        }

        self.settings.is_dark = dark;
        self.apply_theme(cx);

        // Guardar en electron-store
        self.persist_settings(
            json!({ "theme": if dark { "dark" } else { "light" } }),
            "Failed to save the theme setting",
            cx,
        )
    }

    /// `toggleTheme`.
    pub fn toggle_theme(&mut self, cx: &mut Context<Self>) -> Task<()> {
        self.set_dark(!self.settings.is_dark, cx)
    }

    // ------------------------------------------------------------------
    // useAppSettings
    // ------------------------------------------------------------------

    /// `autoFetchEnabled`.
    pub fn auto_fetch_enabled(&self) -> bool {
        self.settings.auto_fetch_enabled
    }

    /// `autoFetchIntervalMinutes`.
    pub fn auto_fetch_interval_minutes(&self) -> u32 {
        self.settings.auto_fetch_interval_minutes
    }

    fn set_auto_fetch(&mut self, enabled: bool, interval_minutes: u32, cx: &mut Context<Self>) {
        let changed = self.settings.auto_fetch_enabled != enabled
            || self.settings.auto_fetch_interval_minutes != interval_minutes;
        self.settings.auto_fetch_enabled = enabled;
        self.settings.auto_fetch_interval_minutes = interval_minutes;
        if changed {
            // `watch([autoFetchEnabled, autoFetchIntervalMinutes], scheduleAutoFetch)`
            self.schedule_auto_fetch(cx);
            cx.notify();
        }
    }

    /// `loadAppSettings({ force })`: se lee una vez por sesión.
    pub fn load_app_settings(&mut self, force: bool, cx: &mut Context<Self>) -> Task<()> {
        if self.settings.app_settings_loaded && !force {
            return Task::ready(());
        }

        let read = self.load_settings(cx);
        flow(cx, async move |this, cx| {
            let settings = read.await;
            this.update(cx, |this, cx| {
                this.set_auto_fetch(
                    settings.auto_fetch,
                    settings.effective_auto_fetch_interval_minutes(),
                    cx,
                );
                this.settings.external_editor = settings.external_editor;
                this.settings.skip_action_dialogs = settings.skip_action_dialogs;
                this.settings.action_defaults = settings.action_defaults;
                let shortcuts = normalize_overrides(&settings.keyboard_shortcuts);
                let shortcuts_changed = shortcuts != this.settings.keyboard_shortcuts;
                this.settings.keyboard_shortcuts = shortcuts;
                this.settings.desktop_notifications = settings
                    .extra
                    .get(crate::desktop_notifications::DESKTOP_NOTIFICATIONS_KEY)
                    != Some(&Value::Bool(false));
                this.settings.app_settings_loaded = true;
                if shortcuts_changed {
                    cx.emit(AppEvent::ShortcutsChanged);
                }
                cx.notify();
            });
        })
    }

    /// `saveAutoFetchSettings({ enabled, intervalMinutes })`.
    pub fn save_auto_fetch_settings(
        &mut self,
        enabled: bool,
        interval_minutes: f64,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        let interval_minutes = normalize_auto_fetch_interval(interval_minutes);
        self.set_auto_fetch(enabled, interval_minutes, cx);

        self.persist_settings(
            json!({
                "autoFetch": enabled,
                "autoFetchIntervalMinutes": interval_minutes
            }),
            "Failed to save auto-fetch settings",
            cx,
        )
    }

    // ------------------------------------------------------------------
    // Ejecución directa de Fetch / Pull / Push (`settings.skipActionDialogs`)
    // ------------------------------------------------------------------

    /// Si Fetch / Pull / Push corren sin diálogo cuando hay valores seguros.
    pub fn skip_action_dialogs(&self) -> bool {
        self.settings.skip_action_dialogs
    }

    /// Activa o desactiva la ejecución directa sin diálogo y la guarda.
    pub fn set_skip_action_dialogs(&mut self, enabled: bool, cx: &mut Context<Self>) -> Task<()> {
        if self.settings.skip_action_dialogs == enabled {
            return Task::ready(());
        }
        self.settings.skip_action_dialogs = enabled;
        cx.notify();
        self.persist_settings(
            json!({ SKIP_ACTION_DIALOGS_KEY: enabled }),
            "Failed to save the direct actions setting",
            cx,
        )
    }

    // ------------------------------------------------------------------
    // Opciones recordadas de Fetch / Pull / Push (`settings.actionDefaults`)
    // ------------------------------------------------------------------

    /// Últimas opciones seguras confirmadas para Fetch / Pull / Push.
    pub fn action_defaults(&self) -> ActionDefaults {
        self.settings.action_defaults
    }

    fn persist_action_defaults(&mut self, cx: &mut Context<Self>) -> Task<()> {
        cx.notify();
        let update = SettingsUpdate {
            action_defaults: Some(self.settings.action_defaults),
            ..Default::default()
        };
        let patch = serde_json::to_value(&update).unwrap_or(Value::Null);
        self.persist_settings(patch, "Failed to save action defaults", cx)
    }

    /// Recuerda las opciones seguras de Fetch y las persiste si cambiaron.
    pub fn remember_fetch_defaults(
        &mut self,
        all_remotes: bool,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        if self.settings.action_defaults.fetch.all_remotes == all_remotes {
            return Task::ready(());
        }
        self.settings.action_defaults.fetch.all_remotes = all_remotes;
        self.persist_action_defaults(cx)
    }

    /// Recuerda las opciones seguras de Pull y las persiste si cambiaron.
    pub fn remember_pull_defaults(
        &mut self,
        rebase: bool,
        autostash: bool,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        let pull = &mut self.settings.action_defaults.pull;
        if pull.rebase == rebase && pull.autostash == autostash {
            return Task::ready(());
        }
        pull.rebase = rebase;
        pull.autostash = autostash;
        self.persist_action_defaults(cx)
    }

    /// Recuerda las opciones seguras de Push y las persiste si cambiaron.
    pub fn remember_push_defaults(&mut self, push_tags: bool, cx: &mut Context<Self>) -> Task<()> {
        if self.settings.action_defaults.push.push_tags == push_tags {
            return Task::ready(());
        }
        self.settings.action_defaults.push.push_tags = push_tags;
        self.persist_action_defaults(cx)
    }

    // ------------------------------------------------------------------
    // Atajos de teclado personalizados (`settings.keyboardShortcuts`)
    // ------------------------------------------------------------------

    /// Mapa `action_id -> combo` con los atajos personalizados del usuario.
    pub fn keyboard_shortcuts(&self) -> &BTreeMap<String, String> {
        &self.settings.keyboard_shortcuts
    }

    fn persist_keyboard_shortcuts(&mut self, cx: &mut Context<Self>) -> Task<()> {
        cx.emit(AppEvent::ShortcutsChanged);
        cx.notify();
        let shortcuts =
            serde_json::to_value(&self.settings.keyboard_shortcuts).unwrap_or(Value::Null);
        self.persist_settings(
            json!({ KEYBOARD_SHORTCUTS_KEY: shortcuts }),
            "Failed to save keyboard shortcuts",
            cx,
        )
    }

    /// Asigna un combo personalizado a `action` (`""` para desvincularlo).
    /// Si coincide con el atajo por defecto, elimina el override.
    pub fn set_keyboard_shortcut(
        &mut self,
        action: ShortcutAction,
        combo: &str,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        let id = action.id();
        if id.is_empty() {
            return Task::ready(());
        }
        let Some(normalized) = normalize_shortcut_override(combo) else {
            return Task::ready(());
        };
        let is_default = !normalized.is_empty() && default_combo_for(action) == Some(&normalized);
        let changed = if is_default {
            self.settings.keyboard_shortcuts.remove(id).is_some()
        } else if self.settings.keyboard_shortcuts.get(id) == Some(&normalized) {
            false
        } else {
            self.settings
                .keyboard_shortcuts
                .insert(id.to_string(), normalized);
            true
        };
        if !changed {
            return Task::ready(());
        }
        self.persist_keyboard_shortcuts(cx)
    }

    /// Restaura un atajo individual a su valor por defecto.
    pub fn reset_keyboard_shortcut(
        &mut self,
        action: ShortcutAction,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        let id = action.id();
        if id.is_empty() || self.settings.keyboard_shortcuts.remove(id).is_none() {
            return Task::ready(());
        }
        self.persist_keyboard_shortcuts(cx)
    }

    /// Restaura todos los atajos personalizados a sus valores por defecto.
    pub fn reset_all_keyboard_shortcuts(&mut self, cx: &mut Context<Self>) -> Task<()> {
        if self.settings.keyboard_shortcuts.is_empty() {
            return Task::ready(());
        }
        self.settings.keyboard_shortcuts.clear();
        self.persist_keyboard_shortcuts(cx)
    }

    // ------------------------------------------------------------------
    // Editor externo
    // ------------------------------------------------------------------

    /// El editor externo elegido (`settings.externalEditor`).
    pub fn external_editor(&self) -> &ExternalEditorSettings {
        &self.settings.external_editor
    }

    /// Elige el editor externo: lo usa desde ya [`AppModel::open_in_editor`] y
    /// se guarda.
    pub fn save_external_editor(
        &mut self,
        editor: ExternalEditorSettings,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        self.settings.external_editor = editor.clone();
        cx.notify();
        let patch = json!({
            EXTERNAL_EDITOR_KEY: serde_json::to_value(&editor).unwrap_or(Value::Null)
        });
        let write = self.blocking(cx, move |backend| {
            backend.shell().set_external_editor(editor);
            backend
                .storage()
                .update_settings_json(&patch)
                .map(|_| ())
                .map_err(|error| error.to_string())
        });
        flow(cx, async move |this, cx| {
            if let Err(error) = write.await {
                this.update(cx, |this, cx| {
                    this.report_persistence_error("Failed to save the external editor", &error, cx);
                });
            }
        })
    }

    // ------------------------------------------------------------------
    // useDiffPreferences
    // ------------------------------------------------------------------

    /// Cómo se prefiere leer un diff (`diffViewMode`, `ignoreWhitespace` y
    /// `wholeFile`).
    pub fn diff_preferences(&self) -> &DiffPreferences {
        &self.settings.diff_preferences
    }

    /// `loadDiffPreferences({ force })`: se lee una vez por sesión; la segunda
    /// vista que monte encuentra lo cargado.
    pub fn load_diff_preferences(&mut self, force: bool, cx: &mut Context<Self>) -> Task<()> {
        if !self.settings.diff_preferences.should_load(force) {
            return Task::ready(());
        }

        let read = self.load_settings(cx);
        flow(cx, async move |this, cx| {
            let settings = read.await;
            this.update(cx, |this, cx| {
                this.settings
                    .diff_preferences
                    .load_from_settings(&settings_value(&settings));
                cx.notify();
            });
        })
    }

    /// `toggleDiffView`.
    pub fn toggle_diff_view(&mut self, cx: &mut Context<Self>) -> Task<()> {
        let patch = self.settings.diff_preferences.toggle_diff_view();
        cx.notify();
        self.persist_settings(patch, "Failed to save diff preferences", cx)
    }

    /// `toggleIgnoreWhitespace`.
    pub fn toggle_ignore_whitespace(&mut self, cx: &mut Context<Self>) -> Task<()> {
        let patch = self.settings.diff_preferences.toggle_ignore_whitespace();
        cx.notify();
        self.persist_settings(patch, "Failed to save diff preferences", cx)
    }

    /// "Whole file" / solo los cambios (`commitView.wholeFile`). Cambia lo que
    /// se le pide a git: quien muestra un diff lo vuelve a pedir.
    pub fn toggle_whole_file(&mut self, cx: &mut Context<Self>) -> Task<()> {
        let patch = self.settings.diff_preferences.toggle_whole_file();
        cx.notify();
        self.persist_settings(patch, "Failed to save diff preferences", cx)
    }

    // ------------------------------------------------------------------
    // useSidebarWidth
    // ------------------------------------------------------------------

    /// Ancho de la barra lateral y si hay un arrastre en curso.
    pub fn sidebar_width(&self) -> &ResizableWidth {
        &self.settings.sidebar_width
    }

    /// `load()` de `useSidebarWidth`: adopta el ancho guardado, la primera vez
    /// que se pide.
    pub fn load_sidebar_width(&mut self, cx: &mut Context<Self>) -> Task<()> {
        if !self.settings.sidebar_width.begin_load() {
            return Task::ready(());
        }

        let read = self.load_settings(cx);
        flow(cx, async move |this, cx| {
            let settings = read.await;
            this.update(cx, |this, cx| {
                this.settings
                    .sidebar_width
                    .apply_saved(saved_sidebar_width(&settings_value(&settings)));
                cx.notify();
            });
        })
    }

    fn persist_sidebar_width(&mut self, cx: &mut Context<Self>) -> Task<()> {
        let patch = sidebar_width_settings(self.settings.sidebar_width.width);
        self.persist_settings(patch, "Failed to save the sidebar width", cx)
    }

    /// `start`: empieza el arrastre del divisor.
    pub fn start_sidebar_resize(&mut self, cx: &mut Context<Self>) {
        self.settings.sidebar_width.start();
        cx.notify();
    }

    /// `move`: el puntero se movió durante el arrastre. `origin_x` es el borde
    /// izquierdo del contenedor de la barra y `container_width` su ancho.
    ///
    /// No avisa a las vistas: un aviso del modelo por cada movimiento del
    /// ratón hacía que todos sus observadores se reevaluaran. Devuelve si el
    /// ancho cambió; quien arrastra repinta la ventana (`window.refresh()`) y
    /// el aviso llega al soltar ([`AppModel::stop_sidebar_resize`]).
    #[must_use = "true significa que hay que repintar"]
    pub fn drag_sidebar_resize(
        &mut self,
        pointer_x: f64,
        origin_x: f64,
        container_width: f64,
    ) -> bool {
        let before = self.settings.sidebar_width.width;
        self.settings
            .sidebar_width
            .drag(pointer_x, origin_x, container_width);
        self.settings.sidebar_width.width != before
    }

    /// `stop`: termina el arrastre y guarda el ancho una vez.
    pub fn stop_sidebar_resize(&mut self, cx: &mut Context<Self>) -> Task<()> {
        if !self.settings.sidebar_width.stop() {
            return Task::ready(());
        }
        cx.notify();
        self.persist_sidebar_width(cx)
    }

    /// `nudge`: flechas sobre el divisor enfocado.
    pub fn nudge_sidebar_width(
        &mut self,
        delta: f64,
        container_width: f64,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        self.settings.sidebar_width.nudge(delta, container_width);
        cx.notify();
        self.persist_sidebar_width(cx)
    }

    /// `reset`: doble clic sobre el divisor, vuelve al ancho inicial.
    pub fn reset_sidebar_width(&mut self, cx: &mut Context<Self>) -> Task<()> {
        self.settings.sidebar_width.reset();
        cx.notify();
        self.persist_sidebar_width(cx)
    }

    // ------------------------------------------------------------------
    // useFileListWidth
    // ------------------------------------------------------------------

    /// Ancho de la lista de archivos frente al diff y si hay un arrastre en
    /// curso.
    pub fn file_list_width(&self) -> &ResizableWidth {
        &self.settings.file_list_width
    }

    /// `load()` de `useFileListWidth`.
    pub fn load_file_list_width(&mut self, cx: &mut Context<Self>) -> Task<()> {
        if !self.settings.file_list_width.begin_load() {
            return Task::ready(());
        }

        let read = self.load_settings(cx);
        flow(cx, async move |this, cx| {
            let settings = read.await;
            this.update(cx, |this, cx| {
                this.settings
                    .file_list_width
                    .apply_saved(saved_file_list_width(&settings_value(&settings)));
                cx.notify();
            });
        })
    }

    /// `save` de `useFileListWidth`: guarda el ancho actual.
    pub fn save_file_list_width(&mut self, cx: &mut Context<Self>) -> Task<()> {
        let patch = file_list_width_settings(self.settings.file_list_width.width);
        self.persist_settings(patch, "Failed to save the file list width", cx)
    }

    /// `start`.
    pub fn start_file_list_resize(&mut self, cx: &mut Context<Self>) {
        self.settings.file_list_width.start();
        cx.notify();
    }

    /// `move`. Como [`AppModel::drag_sidebar_resize`]: no avisa; devuelve si
    /// el ancho cambió y quien arrastra repinta su vista.
    #[must_use = "true significa que hay que repintar"]
    pub fn drag_file_list_resize(
        &mut self,
        pointer_x: f64,
        origin_x: f64,
        container_width: f64,
    ) -> bool {
        let before = self.settings.file_list_width.width;
        self.settings
            .file_list_width
            .drag(pointer_x, origin_x, container_width);
        self.settings.file_list_width.width != before
    }

    /// `stop`.
    pub fn stop_file_list_resize(&mut self, cx: &mut Context<Self>) -> Task<()> {
        if !self.settings.file_list_width.stop() {
            return Task::ready(());
        }
        cx.notify();
        self.save_file_list_width(cx)
    }

    /// `nudge`.
    pub fn nudge_file_list_width(
        &mut self,
        delta: f64,
        container_width: f64,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        self.settings.file_list_width.nudge(delta, container_width);
        cx.notify();
        self.save_file_list_width(cx)
    }

    /// `reset`.
    pub fn reset_file_list_width(&mut self, cx: &mut Context<Self>) -> Task<()> {
        self.settings.file_list_width.reset();
        cx.notify();
        self.save_file_list_width(cx)
    }

    // ------------------------------------------------------------------
    // Ancho del historial (`CommitView.vue`)
    // ------------------------------------------------------------------

    /// Ancho guardado del panel de historial; `None` si no hay ninguno (o no
    /// se ha leído) y vale el de diseño. Los límites los pone la vista.
    pub fn history_width(&self) -> Option<f64> {
        self.settings.history_width
    }

    /// Lee `settings.commitView.historyWidth`.
    pub fn load_history_width(&mut self, cx: &mut Context<Self>) -> Task<Option<f64>> {
        let read = self.load_settings(cx);
        flow(cx, async move |this, cx| {
            let settings = read.await;
            this.update(cx, |this, cx| {
                this.settings.history_width = settings
                    .commit_view
                    .history_width
                    .filter(|width| width.is_finite());
                cx.notify();
                this.settings.history_width
            })
        })
    }

    /// `persistHistoryWidth(width)`.
    pub fn persist_history_width(&mut self, width: f64, cx: &mut Context<Self>) -> Task<()> {
        self.settings.history_width = Some(width);
        cx.notify();
        self.persist_settings(
            json!({ "commitView": { "historyWidth": width } }),
            "Failed to save the history panel width",
            cx,
        )
    }

    // ------------------------------------------------------------------
    // "Local Changes"
    // ------------------------------------------------------------------

    /// Ancho de la columna izquierda, lista plana o árbol y si se cerraron
    /// los atajos de la pestaña "Local Changes".
    pub fn local_changes_prefs(&self) -> LocalChangesPrefs {
        self.settings.local_changes
    }

    /// Lee las preferencias de "Local Changes", la primera vez que se piden.
    pub fn load_local_changes_prefs(&mut self, cx: &mut Context<Self>) -> Task<()> {
        if self.settings.local_changes_loaded {
            return Task::ready(());
        }
        self.settings.local_changes_loaded = true;
        let read = self.load_settings(cx);
        flow(cx, async move |this, cx| {
            let settings = read.await;
            this.update(cx, |this, cx| {
                let prefs = LocalChangesPrefs::from_settings(&settings_value(&settings));
                if prefs != this.settings.local_changes {
                    this.settings.local_changes = prefs;
                    cx.notify();
                }
            });
        })
    }

    fn persist_local_changes(&mut self, patch: Value, cx: &mut Context<Self>) -> Task<()> {
        cx.notify();
        self.persist_settings(
            json!({ "commitView": patch }),
            "Failed to save the Local Changes layout",
            cx,
        )
    }

    /// Guarda el ancho de la columna de "Local Changes"; `None` vuelve al de
    /// diseño.
    pub fn save_local_changes_width(
        &mut self,
        width: Option<f64>,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        let width = width.filter(|width| width.is_finite() && *width > 0.0);
        if self.settings.local_changes.width == width {
            return Task::ready(());
        }
        self.settings.local_changes.width = width;
        // Como los demás anchos: un entero se guarda sin decimales.
        let value = match width {
            Some(width) if width.fract() == 0.0 && width < 9.0e15 => Value::from(width as i64),
            Some(width) => Value::from(width),
            None => Value::Null,
        };
        self.persist_local_changes(json!({ LOCAL_CHANGES_WIDTH_KEY: value }), cx)
    }

    /// Lista plana o árbol en la lista de cambios.
    pub fn set_changes_list_flat(&mut self, flat: bool, cx: &mut Context<Self>) -> Task<()> {
        if self.settings.local_changes.flat_list == flat {
            return Task::ready(());
        }
        self.settings.local_changes.flat_list = flat;
        let layout = if flat { "flat" } else { "tree" };
        self.persist_local_changes(json!({ CHANGES_LIST_LAYOUT_KEY: layout }), cx)
    }

    /// Cierra (para siempre) la caja de atajos de la lista de cambios.
    pub fn dismiss_changes_hints(&mut self, cx: &mut Context<Self>) -> Task<()> {
        if self.settings.local_changes.hints_dismissed {
            return Task::ready(());
        }
        self.settings.local_changes.hints_dismissed = true;
        self.persist_local_changes(json!({ CHANGES_HINTS_DISMISSED_KEY: true }), cx)
    }

    // ------------------------------------------------------------------
    // Secciones de la barra lateral (`Sidebar.vue`)
    // ------------------------------------------------------------------

    /// Qué secciones de la barra lateral están desplegadas.
    pub fn sidebar_sections(&self) -> &SidebarSections {
        &self.settings.sidebar_sections
    }

    /// Lee `settings.sidebarSections` (lo que falte toma su valor por
    /// defecto).
    pub fn load_sidebar_sections(&mut self, cx: &mut Context<Self>) -> Task<()> {
        let read = self.load_settings(cx);
        flow(cx, async move |this, cx| {
            let settings = read.await;
            this.update(cx, |this, cx| {
                this.settings.sidebar_sections = settings.sidebar_sections;
                cx.notify();
            });
        })
    }

    /// Despliega o pliega secciones y guarda el resultado
    /// (`persistSidebarSections`).
    pub fn update_sidebar_sections(
        &mut self,
        cx: &mut Context<Self>,
        update: impl FnOnce(&mut SidebarSections),
    ) -> Task<()> {
        update(&mut self.settings.sidebar_sections);
        cx.notify();
        let sections = serde_json::to_value(&self.settings.sidebar_sections).unwrap_or(Value::Null);
        self.persist_settings(
            json!({ "sidebarSections": sections }),
            "Failed to save the sidebar layout",
            cx,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_the_auto_fetch_interval_like_the_original() {
        assert_eq!(normalize_auto_fetch_interval(15.0), 15);
        assert_eq!(normalize_auto_fetch_interval(2.5), 3);
        assert_eq!(normalize_auto_fetch_interval(1.4), 1);
        // Lo que no es un número de minutos válido vuelve al valor por defecto.
        assert_eq!(normalize_auto_fetch_interval(0.0), 10);
        assert_eq!(normalize_auto_fetch_interval(-3.0), 10);
        assert_eq!(normalize_auto_fetch_interval(f64::NAN), 10);
        assert_eq!(normalize_auto_fetch_interval(f64::INFINITY), 10);
    }
}
