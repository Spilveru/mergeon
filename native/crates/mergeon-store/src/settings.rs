//! Ajustes: normalización (la de `store.js`, sobre JSON) y sus tipos.
//!
//! La normalización trabaja sobre `serde_json::Value` para conservar lo que el
//! original conserva: cualquier clave que no conoce pasa tal cual (por ejemplo
//! `autoFetchIntervalMinutes`, que el renderer guarda y `store.js` no declara).

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

use crate::schema::{default_commit_view_settings, default_settings, default_sidebar_sections};

/// `DEFAULT_SETTINGS.maxRecentRepos`.
pub const DEFAULT_MAX_RECENT_REPOS: usize = 10;

/// Intervalo de auto-fetch cuando no hay uno guardado (`useAppSettings.js`).
pub const DEFAULT_AUTO_FETCH_INTERVAL_MINUTES: u32 = 10;

/// Veracidad de JavaScript (`Boolean(value)`).
pub(crate) fn js_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|number| number != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// `value?.key`: solo los objetos tienen propiedades con nombre.
pub(crate) fn prop<'a>(value: Option<&'a Value>, key: &str) -> Option<&'a Value> {
    value
        .and_then(Value::as_object)
        .and_then(|map| map.get(key))
}

/// `{ ...target, ...source }` cuando `source` es un objeto (un array también lo
/// es para `typeof`, y se esparce por índices).
fn spread(target: &mut Map<String, Value>, source: Option<&Value>) {
    match source {
        Some(Value::Object(entries)) => {
            for (key, value) in entries {
                target.insert(key.clone(), value.clone());
            }
        }
        Some(Value::Array(items)) => {
            for (index, value) in items.iter().enumerate() {
                target.insert(index.to_string(), value.clone());
            }
        }
        _ => {}
    }
}

fn object_of(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

/// `Number.isFinite(value) && value > 0 ? value : null`.
fn positive_number_or_null(value: Option<&Value>) -> Value {
    match value {
        Some(Value::Number(number)) if number.as_f64().is_some_and(|number| number > 0.0) => {
            Value::Number(number.clone())
        }
        _ => Value::Null,
    }
}

pub(crate) fn normalize_sidebar_width(sidebar_width: Option<&Value>) -> Value {
    positive_number_or_null(sidebar_width)
}

pub(crate) fn normalize_sidebar_sections(sidebar_sections: Option<&Value>) -> Value {
    let mut sections = object_of(default_sidebar_sections());
    spread(&mut sections, sidebar_sections);
    Value::Object(sections)
}

pub(crate) fn normalize_commit_view_settings(commit_view: Option<&Value>) -> Value {
    let history_width = positive_number_or_null(prop(commit_view, "historyWidth"));
    let file_list_width = positive_number_or_null(prop(commit_view, "fileListWidth"));
    let diff_view_mode = match prop(commit_view, "diffViewMode") {
        Some(Value::String(mode)) if mode == "split" => "split",
        _ => "unified",
    };
    let ignore_whitespace = prop(commit_view, "ignoreWhitespace").is_some_and(js_truthy);

    let mut normalized = object_of(default_commit_view_settings());
    spread(&mut normalized, commit_view);
    normalized.insert("historyWidth".into(), history_width);
    normalized.insert("fileListWidth".into(), file_list_width);
    normalized.insert("diffViewMode".into(), Value::String(diff_view_mode.into()));
    normalized.insert("ignoreWhitespace".into(), Value::Bool(ignore_whitespace));
    // Las preferencias de "Local Changes" son opcionales (sin la clave valen
    // las de diseño): solo se limpian si están.
    if let Some(width) = prop(commit_view, LOCAL_CHANGES_WIDTH_KEY) {
        normalized.insert(
            LOCAL_CHANGES_WIDTH_KEY.into(),
            positive_number_or_null(Some(width)),
        );
    }
    if let Some(layout) = prop(commit_view, CHANGES_LIST_LAYOUT_KEY) {
        let layout = match layout {
            Value::String(layout) if layout == "flat" => "flat",
            _ => "tree",
        };
        normalized.insert(CHANGES_LIST_LAYOUT_KEY.into(), Value::String(layout.into()));
    }
    if let Some(dismissed) = prop(commit_view, CHANGES_HINTS_DISMISSED_KEY) {
        normalized.insert(
            CHANGES_HINTS_DISMISSED_KEY.into(),
            Value::Bool(js_truthy(dismissed)),
        );
    }
    Value::Object(normalized)
}

/// `settings.commitView.localChangesWidth`: ancho de la columna de la lista y
/// la caja de commit en "Local Changes". Sin la clave, el de diseño.
pub const LOCAL_CHANGES_WIDTH_KEY: &str = "localChangesWidth";
/// `settings.commitView.changesListLayout`: `"tree"` (por defecto) o `"flat"`.
pub const CHANGES_LIST_LAYOUT_KEY: &str = "changesListLayout";
/// `settings.commitView.changesHintsDismissed`: se cerró la caja de atajos de
/// la lista de cambios.
pub const CHANGES_HINTS_DISMISSED_KEY: &str = "changesHintsDismissed";

/// Clave de [`Settings::skip_action_dialogs`] dentro de `settings`.
pub const SKIP_ACTION_DIALOGS_KEY: &str = "skipActionDialogs";
/// Clave de [`Settings::action_dialogs`] dentro de `settings`.
pub const ACTION_DIALOGS_KEY: &str = "actionDialogs";
/// Clave de [`Settings::action_defaults`] dentro de `settings`.
pub const ACTION_DEFAULTS_KEY: &str = "actionDefaults";
/// Clave de [`Settings::keyboard_shortcuts`] dentro de `settings`.
pub const KEYBOARD_SHORTCUTS_KEY: &str = "keyboardShortcuts";

fn is_false(value: &bool) -> bool {
    !*value
}

pub(crate) fn normalize_keyboard_shortcuts(value: &Value) -> BTreeMap<String, String> {
    let Some(map) = value.as_object() else {
        return BTreeMap::new();
    };
    let mut normalized = BTreeMap::new();
    for (key, value) in map {
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        let Some(combo) = value.as_str() else {
            continue;
        };
        normalized.insert(key.to_string(), combo.trim().to_ascii_lowercase());
    }
    normalized
}

pub(crate) fn normalize_settings(settings: Option<&Value>) -> Value {
    let mut normalized = object_of(default_settings());
    spread(&mut normalized, settings);
    normalized.insert(
        "sidebarWidth".into(),
        normalize_sidebar_width(prop(settings, "sidebarWidth")),
    );
    normalized.insert(
        "sidebarSections".into(),
        normalize_sidebar_sections(prop(settings, "sidebarSections")),
    );
    normalized.insert(
        "commitView".into(),
        normalize_commit_view_settings(prop(settings, "commitView")),
    );
    // Opcional, como `autoFetchIntervalMinutes`: sin la clave vale "auto". Si
    // está, se guarda siempre como objeto completo (un valor viejo o roto se
    // migra a su forma tolerante).
    if let Some(editor) = prop(settings, EXTERNAL_EDITOR_KEY) {
        normalized.insert(
            EXTERNAL_EDITOR_KEY.into(),
            serde_json::to_value(ExternalEditorSettings::from_value(editor)).unwrap_or(Value::Null),
        );
    }
    let has_skip = prop(settings, SKIP_ACTION_DIALOGS_KEY).is_some();
    let has_dialogs = prop(settings, ACTION_DIALOGS_KEY).is_some();
    if has_skip || has_dialogs {
        let raw_skip = prop(settings, SKIP_ACTION_DIALOGS_KEY).is_some_and(js_truthy);
        let raw_dialogs = prop(settings, ACTION_DIALOGS_KEY)
            .map(ActionDialogs::from_value)
            .unwrap_or_default();
        let (skip, dialogs) = ActionDialogs::from_modes(
            raw_dialogs.resolve(RemoteAction::Fetch, raw_skip),
            raw_dialogs.resolve(RemoteAction::Pull, raw_skip),
            raw_dialogs.resolve(RemoteAction::Push, raw_skip),
        );
        if skip {
            normalized.insert(SKIP_ACTION_DIALOGS_KEY.into(), Value::Bool(true));
        } else {
            normalized.remove(SKIP_ACTION_DIALOGS_KEY);
        }
        if dialogs.is_empty() {
            normalized.remove(ACTION_DIALOGS_KEY);
        } else {
            normalized.insert(
                ACTION_DIALOGS_KEY.into(),
                serde_json::to_value(dialogs).unwrap_or(Value::Null),
            );
        }
    }
    if let Some(action_defaults) = prop(settings, ACTION_DEFAULTS_KEY) {
        let cleaned = ActionDefaults::from_value(action_defaults);
        if cleaned.is_default() {
            normalized.remove(ACTION_DEFAULTS_KEY);
        } else {
            normalized.insert(
                ACTION_DEFAULTS_KEY.into(),
                serde_json::to_value(cleaned).unwrap_or(Value::Null),
            );
        }
    }
    if let Some(shortcuts) = prop(settings, KEYBOARD_SHORTCUTS_KEY) {
        let cleaned = normalize_keyboard_shortcuts(shortcuts);
        if cleaned.is_empty() {
            normalized.remove(KEYBOARD_SHORTCUTS_KEY);
        } else {
            normalized.insert(
                KEYBOARD_SHORTCUTS_KEY.into(),
                serde_json::to_value(cleaned).unwrap_or(Value::Null),
            );
        }
    }
    Value::Object(normalized)
}

/// El cuerpo de `updateSettings`: mezcla `updates` sobre los ajustes actuales
/// (ya normalizados). `sidebarSections` y `commitView` se mezclan por dentro en
/// vez de reemplazarse enteros.
pub(crate) fn merge_settings_update(current: &Value, updates: &Value) -> Value {
    let mut merged = object_of(current.clone());
    spread(&mut merged, Some(updates));

    let current_sections = current.get("sidebarSections").cloned().unwrap_or_default();
    let sections = match prop(Some(updates), "sidebarSections").filter(|value| js_truthy(value)) {
        Some(section_updates) => {
            let mut combined = object_of(current_sections);
            spread(&mut combined, Some(section_updates));
            normalize_sidebar_sections(Some(&Value::Object(combined)))
        }
        None => current_sections,
    };
    merged.insert("sidebarSections".into(), sections);

    let current_commit_view = current.get("commitView").cloned().unwrap_or_default();
    let commit_view = match prop(Some(updates), "commitView").filter(|value| js_truthy(value)) {
        Some(commit_view_updates) => {
            let mut combined = object_of(current_commit_view);
            spread(&mut combined, Some(commit_view_updates));
            normalize_commit_view_settings(Some(&Value::Object(combined)))
        }
        None => current_commit_view,
    };
    merged.insert("commitView".into(), commit_view);

    normalize_settings(Some(&Value::Object(merged)))
}

/// Cuántos recientes conservar: `recent.slice(0, maxRecentRepos)` con
/// `store.get('settings.maxRecentRepos', 10)`.
pub(crate) fn recent_repositories_limit(settings: Option<&Value>, length: usize) -> usize {
    match prop(settings, "maxRecentRepos") {
        Some(Value::Number(number)) => {
            let end = number.as_f64().unwrap_or(0.0).trunc();
            if end < 0.0 {
                // Un final negativo de `slice` cuenta desde el final.
                (length as f64 + end).max(0.0) as usize
            } else {
                end.min(length as f64) as usize
            }
        }
        // Sin número utilizable se aplica el default. (El original solo lo hace
        // cuando la clave falta; con otro tipo dejaría que `slice` lo convirtiera.)
        _ => DEFAULT_MAX_RECENT_REPOS.min(length),
    }
}

// JSON no distingue 280 de 280.0, pero el archivo queda como lo escribe el
// original (`JSON.stringify` imprime los enteros sin decimales).
fn js_number(number: f64) -> Value {
    if number.fract() == 0.0 && number.abs() < 9_007_199_254_740_992.0 {
        Value::from(number as i64)
    } else {
        Value::from(number)
    }
}

fn serialize_number<S: Serializer>(number: &Option<f64>, serializer: S) -> Result<S::Ok, S::Error> {
    number.map(js_number).serialize(serializer)
}

fn serialize_number_update<S: Serializer>(
    number: &Option<Option<f64>>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    number.flatten().map(js_number).serialize(serializer)
}

fn number_of(value: Option<&Value>) -> Option<f64> {
    value.and_then(Value::as_f64)
}

fn bool_or(value: Option<&Value>, default: bool) -> bool {
    value.map_or(default, js_truthy)
}

fn take_extra(value: Option<&Value>, known: &[&str]) -> Map<String, Value> {
    value
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .filter(|(key, _)| !known.contains(&key.as_str()))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// Tema de la interfaz (`settings.theme`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    Dark,
    #[default]
    Light,
}

impl Theme {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }
}

/// Cómo se prefiere leer un diff (`settings.commitView.diffViewMode`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiffViewMode {
    #[default]
    Unified,
    Split,
}

impl DiffViewMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unified => "unified",
            Self::Split => "split",
        }
    }
}

/// Secciones desplegadas de la barra lateral (`settings.sidebarSections`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SidebarSections {
    pub starred: bool,
    pub branches: bool,
    pub remotes: bool,
    pub tags: bool,
    pub stashes: bool,
    pub submodules: bool,
    /// Claves que este tipo no conoce; se conservan.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for SidebarSections {
    fn default() -> Self {
        Self {
            starred: true,
            branches: true,
            remotes: false,
            tags: false,
            stashes: false,
            submodules: false,
            extra: Map::new(),
        }
    }
}

impl SidebarSections {
    const KNOWN: [&'static str; 6] = [
        "starred",
        "branches",
        "remotes",
        "tags",
        "stashes",
        "submodules",
    ];

    /// Lectura tolerante: lo que falte o no sea un objeto toma su default.
    pub fn from_value(value: &Value) -> Self {
        let value = Some(value);
        let defaults = Self::default();
        Self {
            starred: bool_or(prop(value, "starred"), defaults.starred),
            branches: bool_or(prop(value, "branches"), defaults.branches),
            remotes: bool_or(prop(value, "remotes"), defaults.remotes),
            tags: bool_or(prop(value, "tags"), defaults.tags),
            stashes: bool_or(prop(value, "stashes"), defaults.stashes),
            submodules: bool_or(prop(value, "submodules"), defaults.submodules),
            extra: take_extra(value, &Self::KNOWN),
        }
    }
}

/// Preferencias de la vista de commits y de diffs (`settings.commitView`).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitViewSettings {
    /// Ancho del panel de historial. `None` = el de diseño.
    #[serde(serialize_with = "serialize_number")]
    pub history_width: Option<f64>,
    /// Lista de archivos frente al diff dentro de una pestaña de archivo.
    #[serde(serialize_with = "serialize_number")]
    pub file_list_width: Option<f64>,
    /// Cómo se prefiere leer un diff. Es del usuario, no de cada sesión.
    pub diff_view_mode: DiffViewMode,
    pub ignore_whitespace: bool,
    /// Claves que este tipo no conoce; se conservan.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl CommitViewSettings {
    const KNOWN: [&'static str; 4] = [
        "historyWidth",
        "fileListWidth",
        "diffViewMode",
        "ignoreWhitespace",
    ];

    /// Lectura tolerante: lo que falte o tenga otro tipo toma su default.
    pub fn from_value(value: &Value) -> Self {
        let value = Some(value);
        Self {
            history_width: number_of(prop(value, "historyWidth")).filter(|width| *width > 0.0),
            file_list_width: number_of(prop(value, "fileListWidth")).filter(|width| *width > 0.0),
            diff_view_mode: match prop(value, "diffViewMode").and_then(Value::as_str) {
                Some("split") => DiffViewMode::Split,
                _ => DiffViewMode::Unified,
            },
            ignore_whitespace: bool_or(prop(value, "ignoreWhitespace"), false),
            extra: take_extra(value, &Self::KNOWN),
        }
    }
}

/// Clave de [`ExternalEditorSettings`] dentro de `settings`.
pub const EXTERNAL_EDITOR_KEY: &str = "externalEditor";

/// Editor externo elegido (`settings.externalEditor`).
///
/// `editor` es `"auto"` (el primero que se detecte; si no hay ninguno, el
/// programa por defecto del sistema), `"system"`, `"custom"` o el id de un
/// editor conocido (`"vscode"`, `"zed"`…; el catálogo es del backend). Con
/// `"custom"` se lanza `custom_command` con `custom_args`, donde `{path}`,
/// `{line}` y `{column}` se sustituyen al abrir.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalEditorSettings {
    pub editor: String,
    pub custom_command: String,
    pub custom_args: String,
}

impl Default for ExternalEditorSettings {
    fn default() -> Self {
        Self {
            editor: Self::AUTO.to_string(),
            custom_command: String::new(),
            custom_args: String::new(),
        }
    }
}

impl ExternalEditorSettings {
    pub const AUTO: &'static str = "auto";
    pub const SYSTEM: &'static str = "system";
    pub const CUSTOM: &'static str = "custom";

    /// Un editor conocido (o `auto` / `system`), sin comando propio.
    pub fn editor(editor: impl Into<String>) -> Self {
        Self {
            editor: editor.into(),
            ..Self::default()
        }
    }

    /// Un comando propio con sus argumentos.
    pub fn custom(command: impl Into<String>, args: impl Into<String>) -> Self {
        Self {
            editor: Self::CUSTOM.to_string(),
            custom_command: command.into(),
            custom_args: args.into(),
        }
    }

    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    pub fn is_custom(&self) -> bool {
        self.editor == Self::CUSTOM
    }

    /// Lectura tolerante: un texto suelto es el id del editor (`"code"`), lo
    /// que no sea texto se ignora y un editor vacío es `auto`.
    pub fn from_value(value: &Value) -> Self {
        let text = |key: &str| {
            prop(Some(value), key)
                .and_then(Value::as_str)
                .map(str::trim)
                .unwrap_or_default()
                .to_string()
        };
        let (editor, custom_command, custom_args) = match value {
            Value::String(editor) => (editor.trim().to_string(), String::new(), String::new()),
            Value::Object(_) => (
                text("editor"),
                text("customCommand"),
                // Los argumentos se guardan tal cual (pueden llevar comillas).
                prop(Some(value), "customArgs")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            ),
            _ => Default::default(),
        };
        Self {
            editor: if editor.is_empty() {
                Self::AUTO.to_string()
            } else {
                editor
            },
            custom_command,
            custom_args,
        }
    }
}

/// Cuándo abrir el diálogo de opciones de una acción remota (Fetch / Pull / Push).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ActionDialogMode {
    /// Desde barra de herramientas y paleta abre diálogo; el atajo corre directo.
    #[default]
    Toolbar,
    /// Abre diálogo siempre (incluyendo atajos de teclado).
    Always,
    /// Corre directo cuando hay valores seguros (barra, paleta y atajos).
    Never,
}

impl ActionDialogMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Toolbar => "toolbar",
            Self::Always => "always",
            Self::Never => "never",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "toolbar" => Some(Self::Toolbar),
            "always" => Some(Self::Always),
            "never" => Some(Self::Never),
            _ => None,
        }
    }
}

/// Acción remota con diálogo de opciones configurable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteAction {
    Fetch,
    Pull,
    Push,
}

/// Overrides por acción para el modo de diálogo (`settings.actionDialogs`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionDialogs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fetch: Option<ActionDialogMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pull: Option<ActionDialogMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub push: Option<ActionDialogMode>,
}

impl ActionDialogs {
    pub fn is_empty(&self) -> bool {
        self.fetch.is_none() && self.pull.is_none() && self.push.is_none()
    }

    pub fn resolve(&self, action: RemoteAction, skip_action_dialogs: bool) -> ActionDialogMode {
        let override_mode = match action {
            RemoteAction::Fetch => self.fetch,
            RemoteAction::Pull => self.pull,
            RemoteAction::Push => self.push,
        };
        override_mode.unwrap_or(if skip_action_dialogs {
            ActionDialogMode::Never
        } else {
            ActionDialogMode::Toolbar
        })
    }

    pub fn from_modes(
        fetch: ActionDialogMode,
        pull: ActionDialogMode,
        push: ActionDialogMode,
    ) -> (bool, Self) {
        let never_count = [fetch, pull, push]
            .into_iter()
            .filter(|m| *m == ActionDialogMode::Never)
            .count();
        let base = if never_count >= 2 {
            ActionDialogMode::Never
        } else {
            ActionDialogMode::Toolbar
        };
        let skip_action_dialogs = base == ActionDialogMode::Never;
        let dialogs = Self {
            fetch: (fetch != base).then_some(fetch),
            pull: (pull != base).then_some(pull),
            push: (push != base).then_some(push),
        };
        (skip_action_dialogs, dialogs)
    }

    /// Lectura tolerante: ignora valores desconocidos o que no sean cadenas.
    pub fn from_value(value: &Value) -> Self {
        let read = |key: &str| {
            prop(Some(value), key)
                .and_then(Value::as_str)
                .and_then(ActionDialogMode::parse)
        };
        Self {
            fetch: read("fetch"),
            pull: read("pull"),
            push: read("push"),
        }
    }
}

/// Opciones recordadas del diálogo de Fetch (`settings.actionDefaults.fetch`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchActionDefaults {
    pub all_remotes: bool,
}

impl Default for FetchActionDefaults {
    fn default() -> Self {
        Self { all_remotes: true }
    }
}

/// Opciones recordadas del diálogo de Pull (`settings.actionDefaults.pull`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullActionDefaults {
    pub rebase: bool,
    pub autostash: bool,
}

/// Opciones recordadas del diálogo de Push (`settings.actionDefaults.push`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushActionDefaults {
    pub push_tags: bool,
}

/// Opciones recordadas para Fetch / Pull / Push (`settings.actionDefaults`).
///
/// Solo guarda preferencias seguras (`allRemotes`, `rebase`, `autostash`,
/// `pushTags`); nunca persiste `force`, `remote` ni `branch`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionDefaults {
    #[serde(default)]
    pub fetch: FetchActionDefaults,
    #[serde(default)]
    pub pull: PullActionDefaults,
    #[serde(default)]
    pub push: PushActionDefaults,
}

impl ActionDefaults {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Lectura tolerante: lo que falte o no sea un objeto toma su default
    /// (`fetch.allRemotes = true`, el resto `false`). Nunca persiste ni lee
    /// `force`, `remote` ni `branch`.
    pub fn from_value(value: &Value) -> Self {
        let value = Some(value);
        let fetch = prop(value, "fetch");
        let pull = prop(value, "pull");
        let push = prop(value, "push");
        let defaults = Self::default();
        Self {
            fetch: FetchActionDefaults {
                all_remotes: bool_or(prop(fetch, "allRemotes"), defaults.fetch.all_remotes),
            },
            pull: PullActionDefaults {
                rebase: bool_or(prop(pull, "rebase"), defaults.pull.rebase),
                autostash: bool_or(prop(pull, "autostash"), defaults.pull.autostash),
            },
            push: PushActionDefaults {
                push_tags: bool_or(prop(push, "pushTags"), defaults.push.push_tags),
            },
        }
    }
}

/// Ajustes de la aplicación (`settings`), ya normalizados.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub theme: Theme,
    pub auto_fetch: bool,
    /// No está en los defaults de `store.js`: lo guarda el diálogo de
    /// preferencias. Ver [`Settings::effective_auto_fetch_interval_minutes`].
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_number"
    )]
    pub auto_fetch_interval_minutes: Option<f64>,
    pub max_recent_repos: usize,
    /// Ancho de la barra lateral (ramas, remotos, tags). `None` = el de diseño.
    #[serde(serialize_with = "serialize_number")]
    pub sidebar_width: Option<f64>,
    pub sidebar_sections: SidebarSections,
    pub commit_view: CommitViewSettings,
    /// Editor externo. No está en los defaults: sin la clave vale `auto`.
    #[serde(skip_serializing_if = "ExternalEditorSettings::is_default")]
    pub external_editor: ExternalEditorSettings,
    /// Ejecutar Fetch / Pull / Push sin diálogo de confirmación cuando existen
    /// valores seguros por defecto.
    #[serde(skip_serializing_if = "is_false")]
    pub skip_action_dialogs: bool,
    /// Overrides por acción para el modo de diálogo de Fetch / Pull / Push.
    #[serde(default, skip_serializing_if = "ActionDialogs::is_empty")]
    pub action_dialogs: ActionDialogs,
    /// Últimas opciones seguras confirmadas en los diálogos de Fetch / Pull / Push.
    #[serde(skip_serializing_if = "ActionDefaults::is_default")]
    pub action_defaults: ActionDefaults,
    /// Atajos de teclado personalizados (`action_id -> combo`, `""` = sin asignar).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub keyboard_shortcuts: BTreeMap<String, String>,
    /// Claves que este tipo no conoce; se conservan.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: Theme::Light,
            auto_fetch: true,
            auto_fetch_interval_minutes: None,
            max_recent_repos: DEFAULT_MAX_RECENT_REPOS,
            sidebar_width: None,
            sidebar_sections: SidebarSections::default(),
            commit_view: CommitViewSettings::default(),
            external_editor: ExternalEditorSettings::default(),
            skip_action_dialogs: false,
            action_dialogs: ActionDialogs::default(),
            action_defaults: ActionDefaults::default(),
            keyboard_shortcuts: BTreeMap::new(),
            extra: Map::new(),
        }
    }
}

impl Settings {
    const KNOWN: [&'static str; 12] = [
        "theme",
        "autoFetch",
        "autoFetchIntervalMinutes",
        "maxRecentRepos",
        "sidebarWidth",
        "sidebarSections",
        "commitView",
        EXTERNAL_EDITOR_KEY,
        SKIP_ACTION_DIALOGS_KEY,
        ACTION_DIALOGS_KEY,
        ACTION_DEFAULTS_KEY,
        KEYBOARD_SHORTCUTS_KEY,
    ];

    /// Lectura tolerante de unos ajustes en JSON. Interpreta cada campo como lo
    /// hace quien lo consume en el renderer: el tema es oscuro solo si vale
    /// `"dark"` (`useTheme.js`) y el auto-fetch está activo salvo que valga
    /// `false` (`useAppSettings.js`).
    pub fn from_value(value: &Value) -> Self {
        let value = Some(value);
        let raw_skip = bool_or(prop(value, SKIP_ACTION_DIALOGS_KEY), false);
        let raw_dialogs = prop(value, ACTION_DIALOGS_KEY)
            .map(ActionDialogs::from_value)
            .unwrap_or_default();
        let (skip_action_dialogs, action_dialogs) = ActionDialogs::from_modes(
            raw_dialogs.resolve(RemoteAction::Fetch, raw_skip),
            raw_dialogs.resolve(RemoteAction::Pull, raw_skip),
            raw_dialogs.resolve(RemoteAction::Push, raw_skip),
        );
        Self {
            theme: match prop(value, "theme") {
                // Sin ajuste guardado (instalación nueva): el tema claro de Señal.
                None => Theme::Light,
                Some(Value::String(theme)) if theme == "dark" => Theme::Dark,
                Some(_) => Theme::Light,
            },
            auto_fetch: prop(value, "autoFetch") != Some(&Value::Bool(false)),
            auto_fetch_interval_minutes: number_of(prop(value, "autoFetchIntervalMinutes")),
            max_recent_repos: match number_of(prop(value, "maxRecentRepos")) {
                Some(limit) => limit.trunc().max(0.0) as usize,
                None => DEFAULT_MAX_RECENT_REPOS,
            },
            sidebar_width: number_of(prop(value, "sidebarWidth")).filter(|width| *width > 0.0),
            sidebar_sections: prop(value, "sidebarSections")
                .map(SidebarSections::from_value)
                .unwrap_or_default(),
            commit_view: prop(value, "commitView")
                .map(CommitViewSettings::from_value)
                .unwrap_or_default(),
            external_editor: prop(value, EXTERNAL_EDITOR_KEY)
                .map(ExternalEditorSettings::from_value)
                .unwrap_or_default(),
            skip_action_dialogs,
            action_dialogs,
            action_defaults: prop(value, ACTION_DEFAULTS_KEY)
                .map(ActionDefaults::from_value)
                .unwrap_or_default(),
            keyboard_shortcuts: prop(value, KEYBOARD_SHORTCUTS_KEY)
                .map(normalize_keyboard_shortcuts)
                .unwrap_or_default(),
            extra: take_extra(value, &Self::KNOWN),
        }
    }

    /// Minutos entre auto-fetch, con la regla de `useAppSettings.js`: un número
    /// finito y >= 1 se redondea; cualquier otra cosa es el default (10).
    pub fn effective_auto_fetch_interval_minutes(&self) -> u32 {
        match self.auto_fetch_interval_minutes {
            Some(minutes) if minutes.is_finite() && minutes >= 1.0 => {
                (minutes + 0.5).floor().min(f64::from(u32::MAX)) as u32
            }
            _ => DEFAULT_AUTO_FETCH_INTERVAL_MINUTES,
        }
    }
}

macro_rules! deserialize_from_value {
    ($($name:ident),+) => {
        $(impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                Value::deserialize(deserializer).map(|value| Self::from_value(&value))
            }
        })+
    };
}

deserialize_from_value!(
    Settings,
    SidebarSections,
    CommitViewSettings,
    ExternalEditorSettings
);

/// Cambios parciales para [`crate::Store::update_settings`]: solo se tocan los
/// campos con valor.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsUpdate {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub theme: Option<Theme>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_fetch: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_fetch_interval_minutes: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_recent_repos: Option<usize>,
    /// `Some(None)` vuelve al ancho de diseño.
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_number_update"
    )]
    pub sidebar_width: Option<Option<f64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sidebar_sections: Option<SidebarSectionsUpdate>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit_view: Option<CommitViewUpdate>,
    /// Reemplaza el editor externo entero.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_editor: Option<ExternalEditorSettings>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skip_action_dialogs: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action_dialogs: Option<ActionDialogs>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action_defaults: Option<ActionDefaults>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keyboard_shortcuts: Option<BTreeMap<String, String>>,
    /// Otras claves a guardar tal cual.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Cambios parciales de `settings.sidebarSections`.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct SidebarSectionsUpdate {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub starred: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branches: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remotes: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stashes: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub submodules: Option<bool>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl From<SidebarSections> for SidebarSectionsUpdate {
    fn from(sections: SidebarSections) -> Self {
        Self {
            starred: Some(sections.starred),
            branches: Some(sections.branches),
            remotes: Some(sections.remotes),
            tags: Some(sections.tags),
            stashes: Some(sections.stashes),
            submodules: Some(sections.submodules),
            extra: sections.extra,
        }
    }
}

/// Cambios parciales de `settings.commitView`.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitViewUpdate {
    /// `Some(None)` vuelve al ancho de diseño.
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_number_update"
    )]
    pub history_width: Option<Option<f64>>,
    /// `Some(None)` vuelve al ancho de diseño.
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_number_update"
    )]
    pub file_list_width: Option<Option<f64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff_view_mode: Option<DiffViewMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ignore_whitespace: Option<bool>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn normalize_settings_fills_defaults_and_keeps_unknown_keys() {
        assert_eq!(normalize_settings(None), default_settings());
        assert_eq!(normalize_settings(Some(&json!("nope"))), default_settings());

        let normalized = normalize_settings(Some(&json!({
            "theme": "light",
            "autoFetchIntervalMinutes": 15,
            "sidebarWidth": -3,
            "sidebarSections": { "tags": true, "custom": 1 },
            "commitView": { "historyWidth": 320, "diffViewMode": "weird", "ignoreWhitespace": 1, "other": "x" }
        })));

        assert_eq!(
            normalized,
            json!({
                "theme": "light",
                "autoFetch": true,
                "autoFetchIntervalMinutes": 15,
                "maxRecentRepos": 10,
                "sidebarWidth": null,
                "sidebarSections": {
                    "starred": true,
                    "branches": true,
                    "remotes": false,
                    "tags": true,
                    "stashes": false,
                    "submodules": false,
                    "custom": 1
                },
                "commitView": {
                    "historyWidth": 320,
                    "fileListWidth": null,
                    "diffViewMode": "unified",
                    "ignoreWhitespace": true,
                    "other": "x"
                }
            })
        );
    }

    #[test]
    fn widths_must_be_positive_numbers() {
        assert_eq!(normalize_sidebar_width(Some(&json!(260))), json!(260));
        assert_eq!(normalize_sidebar_width(Some(&json!(260.5))), json!(260.5));
        assert_eq!(normalize_sidebar_width(Some(&json!(0))), json!(null));
        assert_eq!(normalize_sidebar_width(Some(&json!("260"))), json!(null));
        assert_eq!(normalize_sidebar_width(None), json!(null));
    }

    #[test]
    fn merge_update_combines_nested_sections_instead_of_replacing_them() {
        let current = normalize_settings(Some(&json!({
            "sidebarSections": { "tags": true },
            "commitView": { "historyWidth": 300, "diffViewMode": "split" }
        })));

        let merged = merge_settings_update(
            &current,
            &json!({
                "theme": "light",
                "sidebarSections": { "remotes": true },
                "commitView": { "fileListWidth": 200 }
            }),
        );

        assert_eq!(merged["theme"], "light");
        assert_eq!(merged["sidebarSections"]["tags"], true);
        assert_eq!(merged["sidebarSections"]["remotes"], true);
        assert_eq!(merged["commitView"]["historyWidth"], 300);
        assert_eq!(merged["commitView"]["fileListWidth"], 200);
        assert_eq!(merged["commitView"]["diffViewMode"], "split");

        // Un valor "falsy" en esas claves no las toca.
        let untouched = merge_settings_update(
            &current,
            &json!({ "sidebarSections": null, "commitView": 0 }),
        );
        assert_eq!(untouched, current);

        // Unos updates que no son un objeto no cambian nada.
        assert_eq!(merge_settings_update(&current, &json!(null)), current);
    }

    #[test]
    fn recent_limit_follows_slice_semantics() {
        assert_eq!(recent_repositories_limit(None, 20), 10);
        assert_eq!(recent_repositories_limit(Some(&json!({})), 3), 3);
        assert_eq!(
            recent_repositories_limit(Some(&json!({ "maxRecentRepos": 5 })), 20),
            5
        );
        assert_eq!(
            recent_repositories_limit(Some(&json!({ "maxRecentRepos": 2.9 })), 20),
            2
        );
        assert_eq!(
            recent_repositories_limit(Some(&json!({ "maxRecentRepos": -1 })), 3),
            2
        );
        assert_eq!(
            recent_repositories_limit(Some(&json!({ "maxRecentRepos": 0 })), 3),
            0
        );
        assert_eq!(
            recent_repositories_limit(Some(&json!({ "maxRecentRepos": "many" })), 20),
            10
        );
    }

    #[test]
    fn typed_settings_read_leniently() {
        assert_eq!(
            Settings::from_value(&default_settings()),
            Settings::default()
        );
        assert_eq!(Settings::from_value(&json!(null)), Settings::default());

        let settings = Settings::from_value(&json!({
            "theme": "solarized",
            "autoFetch": false,
            "autoFetchIntervalMinutes": 14.5,
            "maxRecentRepos": "x",
            "sidebarWidth": 300,
            "sidebarSections": { "starred": false, "pinned": true },
            "commitView": { "diffViewMode": "split", "historyWidth": 0 },
            "future": { "a": 1 }
        }));

        assert_eq!(settings.theme, Theme::Light);
        assert!(!settings.auto_fetch);
        assert_eq!(settings.effective_auto_fetch_interval_minutes(), 15);
        assert_eq!(settings.max_recent_repos, 10);
        assert_eq!(settings.sidebar_width, Some(300.0));
        assert!(!settings.sidebar_sections.starred);
        assert!(settings.sidebar_sections.branches);
        assert_eq!(settings.sidebar_sections.extra["pinned"], true);
        assert_eq!(settings.commit_view.diff_view_mode, DiffViewMode::Split);
        assert_eq!(settings.commit_view.history_width, None);
        assert_eq!(settings.extra["future"], json!({ "a": 1 }));

        // Serializa con los nombres del JSON y vuelve a leerse igual.
        let json = serde_json::to_value(&settings).unwrap();
        assert_eq!(json["autoFetch"], false);
        assert_eq!(json["commitView"]["diffViewMode"], "split");
        assert_eq!(json["sidebarSections"]["pinned"], true);
        let again: Settings = serde_json::from_value(json).unwrap();
        assert_eq!(again.theme, Theme::Light);
        assert_eq!(again.extra, settings.extra);
    }

    #[test]
    fn auto_fetch_interval_defaults_like_the_renderer() {
        let mut settings = Settings::default();
        assert_eq!(settings.effective_auto_fetch_interval_minutes(), 10);
        settings.auto_fetch_interval_minutes = Some(0.4);
        assert_eq!(settings.effective_auto_fetch_interval_minutes(), 10);
        settings.auto_fetch_interval_minutes = Some(1.0);
        assert_eq!(settings.effective_auto_fetch_interval_minutes(), 1);
        settings.auto_fetch_interval_minutes = Some(2.5);
        assert_eq!(settings.effective_auto_fetch_interval_minutes(), 3);
    }

    #[test]
    fn updates_only_serialize_what_was_set() {
        let update = SettingsUpdate {
            theme: Some(Theme::Light),
            sidebar_width: Some(None),
            commit_view: Some(CommitViewUpdate {
                file_list_width: Some(Some(240.0)),
                ..Default::default()
            }),
            ..Default::default()
        };

        assert_eq!(
            serde_json::to_value(&update).unwrap(),
            json!({
                "theme": "light",
                "sidebarWidth": null,
                "commitView": { "fileListWidth": 240 }
            })
        );

        // Un ancho con decimales se guarda con ellos.
        let fractional = SettingsUpdate {
            sidebar_width: Some(Some(260.5)),
            ..Default::default()
        };
        assert_eq!(
            serde_json::to_value(&fractional).unwrap(),
            json!({ "sidebarWidth": 260.5 })
        );
    }

    #[test]
    fn local_changes_preferences_are_optional_and_cleaned_when_present() {
        // Sin las claves no aparecen (los ajustes de siempre no cambian).
        let plain = normalize_commit_view_settings(Some(&json!({})));
        for key in [
            LOCAL_CHANGES_WIDTH_KEY,
            CHANGES_LIST_LAYOUT_KEY,
            CHANGES_HINTS_DISMISSED_KEY,
        ] {
            assert!(plain.get(key).is_none(), "{key}");
        }

        let normalized = normalize_commit_view_settings(Some(&json!({
            "localChangesWidth": 420,
            "changesListLayout": "flat",
            "changesHintsDismissed": 1
        })));
        assert_eq!(normalized[LOCAL_CHANGES_WIDTH_KEY], json!(420));
        assert_eq!(normalized[CHANGES_LIST_LAYOUT_KEY], json!("flat"));
        assert_eq!(normalized[CHANGES_HINTS_DISMISSED_KEY], json!(true));

        let broken = normalize_commit_view_settings(Some(&json!({
            "localChangesWidth": -1,
            "changesListLayout": "weird",
            "changesHintsDismissed": null
        })));
        assert_eq!(broken[LOCAL_CHANGES_WIDTH_KEY], json!(null));
        assert_eq!(broken[CHANGES_LIST_LAYOUT_KEY], json!("tree"));
        assert_eq!(broken[CHANGES_HINTS_DISMISSED_KEY], json!(false));

        // Un parche de una sola preferencia no pisa las demás.
        let current = normalize_settings(Some(&json!({
            "commitView": { "localChangesWidth": 420, "changesListLayout": "flat" }
        })));
        let merged = merge_settings_update(
            &current,
            &json!({ "commitView": { "changesHintsDismissed": true } }),
        );
        assert_eq!(merged["commitView"][LOCAL_CHANGES_WIDTH_KEY], json!(420));
        assert_eq!(merged["commitView"][CHANGES_LIST_LAYOUT_KEY], json!("flat"));
        assert_eq!(
            merged["commitView"][CHANGES_HINTS_DISMISSED_KEY],
            json!(true)
        );
        // Y llegan intactos a los ajustes leídos (como claves que el tipo no declara).
        let settings = Settings::from_value(&merged);
        assert_eq!(
            settings.commit_view.extra.get(LOCAL_CHANGES_WIDTH_KEY),
            Some(&json!(420))
        );
    }

    #[test]
    fn the_external_editor_is_optional_and_read_leniently() {
        // Sin la clave: automático, y no aparece al serializar ni al normalizar.
        assert_eq!(
            Settings::default().external_editor,
            ExternalEditorSettings::editor("auto")
        );
        assert!(normalize_settings(None).get(EXTERNAL_EDITOR_KEY).is_none());
        let json = serde_json::to_value(Settings::default()).unwrap();
        assert!(json.get(EXTERNAL_EDITOR_KEY).is_none());

        let read = |value: Value| ExternalEditorSettings::from_value(&value);
        assert_eq!(
            read(json!({ "editor": "zed" })),
            ExternalEditorSettings::editor("zed")
        );
        // Un texto suelto (formato viejo o escrito a mano) es el id del editor.
        assert_eq!(
            read(json!(" code ")),
            ExternalEditorSettings::editor("code")
        );
        // Lo que no se entiende vuelve a automático, sin perder lo demás.
        assert_eq!(read(json!(42)), ExternalEditorSettings::default());
        assert_eq!(
            read(json!({ "editor": "" })),
            ExternalEditorSettings::default()
        );
        assert_eq!(
            read(
                json!({ "editor": "custom", "customCommand": 7, "customArgs": "-n {line} \"{path}\"" })
            ),
            ExternalEditorSettings::custom("", "-n {line} \"{path}\"")
        );
    }

    #[test]
    fn normalizing_migrates_a_broken_external_editor_to_its_full_form() {
        let normalized = normalize_settings(Some(&json!({ "externalEditor": "subl" })));
        assert_eq!(
            normalized[EXTERNAL_EDITOR_KEY],
            json!({ "editor": "subl", "customCommand": "", "customArgs": "" })
        );
        let normalized = normalize_settings(Some(&json!({ "externalEditor": null })));
        assert_eq!(normalized[EXTERNAL_EDITOR_KEY]["editor"], "auto");
    }

    #[test]
    fn an_external_editor_update_replaces_the_whole_choice() {
        let current = normalize_settings(Some(&json!({
            "externalEditor": { "editor": "custom", "customCommand": "/opt/ed", "customArgs": "{path}" }
        })));
        let update = SettingsUpdate {
            external_editor: Some(ExternalEditorSettings::editor("vscode")),
            ..Default::default()
        };
        let merged = merge_settings_update(&current, &serde_json::to_value(&update).unwrap());
        let settings = Settings::from_value(&merged);
        assert_eq!(
            settings.external_editor,
            ExternalEditorSettings::editor("vscode")
        );
        assert_eq!(
            serde_json::to_value(&settings).unwrap()[EXTERNAL_EDITOR_KEY],
            json!({ "editor": "vscode", "customCommand": "", "customArgs": "" })
        );
    }

    #[test]
    fn skip_action_dialogs_and_keyboard_shortcuts_are_optional_and_normalized() {
        assert!(!Settings::default().skip_action_dialogs);
        assert!(Settings::default().keyboard_shortcuts.is_empty());
        let default_json = serde_json::to_value(Settings::default()).unwrap();
        assert!(default_json.get(SKIP_ACTION_DIALOGS_KEY).is_none());
        assert!(default_json.get(KEYBOARD_SHORTCUTS_KEY).is_none());

        let normalized = normalize_settings(Some(&json!({
            "skipActionDialogs": true,
            "keyboardShortcuts": {
                " fetch ": "  Mod+Shift+Down ",
                "pull": "",
                "": "mod+x",
                "bad": 123
            }
        })));
        assert_eq!(normalized[SKIP_ACTION_DIALOGS_KEY], json!(true));
        assert_eq!(
            normalized[KEYBOARD_SHORTCUTS_KEY],
            json!({ "fetch": "mod+shift+down", "pull": "" })
        );

        let settings = Settings::from_value(&normalized);
        assert!(settings.skip_action_dialogs);
        assert_eq!(
            settings.keyboard_shortcuts.get("fetch").map(String::as_str),
            Some("mod+shift+down")
        );
        assert_eq!(
            settings.keyboard_shortcuts.get("pull").map(String::as_str),
            Some("")
        );
        assert!(!settings.extra.contains_key(SKIP_ACTION_DIALOGS_KEY));
        assert!(!settings.extra.contains_key(KEYBOARD_SHORTCUTS_KEY));

        // Resetting back to defaults omits the keys cleanly.
        let cleared = merge_settings_update(
            &normalized,
            &json!({ "skipActionDialogs": false, "keyboardShortcuts": {} }),
        );
        assert!(cleared.get(SKIP_ACTION_DIALOGS_KEY).is_none());
        assert!(cleared.get(KEYBOARD_SHORTCUTS_KEY).is_none());
    }

    #[test]
    fn action_defaults_are_omitted_at_default_round_trip_and_normalize_leniently() {
        // Default Settings omits actionDefaults so parity JSON stays identical.
        assert_eq!(
            Settings::default().action_defaults,
            ActionDefaults::default()
        );
        assert!(ActionDefaults::default().is_default());
        assert!(ActionDefaults::default().fetch.all_remotes);
        assert!(!ActionDefaults::default().pull.rebase);
        assert!(!ActionDefaults::default().pull.autostash);
        assert!(!ActionDefaults::default().push.push_tags);
        let default_json = serde_json::to_value(Settings::default()).unwrap();
        assert!(default_json.get(ACTION_DEFAULTS_KEY).is_none());
        assert!(normalize_settings(None).get(ACTION_DEFAULTS_KEY).is_none());
        assert_eq!(default_json, default_settings());

        // Round-trip non-default ActionDefaults through SettingsUpdate and merge_settings_update.
        let custom = ActionDefaults {
            fetch: FetchActionDefaults { all_remotes: false },
            pull: PullActionDefaults {
                rebase: true,
                autostash: true,
            },
            push: PushActionDefaults { push_tags: true },
        };
        assert!(!custom.is_default());
        let update = SettingsUpdate {
            action_defaults: Some(custom),
            ..Default::default()
        };
        let merged = merge_settings_update(
            &normalize_settings(None),
            &serde_json::to_value(&update).unwrap(),
        );
        assert_eq!(
            merged[ACTION_DEFAULTS_KEY],
            json!({
                "fetch": { "allRemotes": false },
                "pull": { "rebase": true, "autostash": true },
                "push": { "pushTags": true }
            })
        );
        let read_back = Settings::from_value(&merged);
        assert_eq!(read_back.action_defaults, custom);
        assert!(!read_back.extra.contains_key(ACTION_DEFAULTS_KEY));
        let reserialized = serde_json::to_value(&read_back).unwrap();
        assert_eq!(
            reserialized[ACTION_DEFAULTS_KEY],
            merged[ACTION_DEFAULTS_KEY]
        );

        // Resetting back to defaults removes the actionDefaults key cleanly.
        let reset = SettingsUpdate {
            action_defaults: Some(ActionDefaults::default()),
            ..Default::default()
        };
        let cleared = merge_settings_update(&merged, &serde_json::to_value(&reset).unwrap());
        assert!(cleared.get(ACTION_DEFAULTS_KEY).is_none());
        assert_eq!(
            Settings::from_value(&cleared).action_defaults,
            ActionDefaults::default()
        );

        // Lenient normalization of partial/malformed JSON and ignoring unsafe/unknown keys.
        let malformed = normalize_settings(Some(&json!({
            "actionDefaults": {
                "fetch": "not-an-object",
                "pull": { "rebase": 1, "branch": "main" },
                "push": { "pushTags": "yes", "force": true, "remote": "origin" }
            }
        })));
        assert_eq!(
            malformed[ACTION_DEFAULTS_KEY],
            json!({
                "fetch": { "allRemotes": true },
                "pull": { "rebase": true, "autostash": false },
                "push": { "pushTags": true }
            })
        );
        let parsed = Settings::from_value(&malformed).action_defaults;
        assert!(parsed.fetch.all_remotes);
        assert!(parsed.pull.rebase);
        assert!(!parsed.pull.autostash);
        assert!(parsed.push.push_tags);

        // Completely invalid value normalizes to defaults and omits the key.
        let bogus = normalize_settings(Some(&json!({ "actionDefaults": 123 })));
        assert!(bogus.get(ACTION_DEFAULTS_KEY).is_none());
        assert_eq!(
            ActionDefaults::from_value(&json!(null)),
            ActionDefaults::default()
        );
        assert_eq!(
            ActionDefaults::from_value(&json!({})),
            ActionDefaults::default()
        );
    }

    #[test]
    fn action_dialogs_canonicalize_migrate_and_parse_leniently() {
        // Default Settings omits both skipActionDialogs and actionDialogs.
        let default_json = serde_json::to_value(Settings::default()).unwrap();
        assert!(default_json.get(SKIP_ACTION_DIALOGS_KEY).is_none());
        assert!(default_json.get(ACTION_DIALOGS_KEY).is_none());
        assert_eq!(default_json, default_settings());
        assert!(normalize_settings(None).get(ACTION_DIALOGS_KEY).is_none());
        assert_eq!(
            ActionDialogs::default().resolve(RemoteAction::Fetch, false),
            ActionDialogMode::Toolbar
        );
        assert_eq!(
            ActionDialogs::default().resolve(RemoteAction::Pull, false),
            ActionDialogMode::Toolbar
        );
        assert_eq!(
            ActionDialogs::default().resolve(RemoteAction::Push, false),
            ActionDialogMode::Toolbar
        );

        // Legacy skipActionDialogs: true -> all Never, actionDialogs omitted.
        let legacy = normalize_settings(Some(&json!({ "skipActionDialogs": true })));
        assert_eq!(legacy[SKIP_ACTION_DIALOGS_KEY], json!(true));
        assert!(legacy.get(ACTION_DIALOGS_KEY).is_none());
        let parsed_legacy = Settings::from_value(&legacy);
        assert!(parsed_legacy.skip_action_dialogs);
        assert!(parsed_legacy.action_dialogs.is_empty());
        for action in [RemoteAction::Fetch, RemoteAction::Pull, RemoteAction::Push] {
            assert_eq!(
                parsed_legacy
                    .action_dialogs
                    .resolve(action, parsed_legacy.skip_action_dialogs),
                ActionDialogMode::Never
            );
        }

        // All 3 set to Never via actionDialogs canonicalizes to skipActionDialogs: true only.
        let all_never = normalize_settings(Some(&json!({
            "actionDialogs": { "fetch": "never", "pull": "never", "push": "never" }
        })));
        assert_eq!(all_never[SKIP_ACTION_DIALOGS_KEY], json!(true));
        assert!(all_never.get(ACTION_DIALOGS_KEY).is_none());

        // push: Always only -> {"actionDialogs":{"push":"always"}}, skipActionDialogs omitted.
        let (skip, dialogs) = ActionDialogs::from_modes(
            ActionDialogMode::Toolbar,
            ActionDialogMode::Toolbar,
            ActionDialogMode::Always,
        );
        assert!(!skip);
        assert_eq!(
            dialogs,
            ActionDialogs {
                fetch: None,
                pull: None,
                push: Some(ActionDialogMode::Always),
            }
        );
        let push_always = normalize_settings(Some(&json!({
            "actionDialogs": { "push": "always" }
        })));
        assert!(push_always.get(SKIP_ACTION_DIALOGS_KEY).is_none());
        assert_eq!(push_always[ACTION_DIALOGS_KEY], json!({ "push": "always" }));
        let parsed_push_always = Settings::from_value(&push_always);
        assert!(!parsed_push_always.skip_action_dialogs);
        assert_eq!(parsed_push_always.action_dialogs, dialogs);
        assert!(!parsed_push_always.extra.contains_key(ACTION_DIALOGS_KEY));

        // 2x Never + 1x Always -> {"skipActionDialogs":true,"actionDialogs":{"push":"always"}}.
        let (skip, dialogs) = ActionDialogs::from_modes(
            ActionDialogMode::Never,
            ActionDialogMode::Never,
            ActionDialogMode::Always,
        );
        assert!(skip);
        assert_eq!(
            dialogs,
            ActionDialogs {
                fetch: None,
                pull: None,
                push: Some(ActionDialogMode::Always),
            }
        );
        let two_never = normalize_settings(Some(&json!({
            "actionDialogs": { "fetch": "never", "pull": "never", "push": "always" }
        })));
        assert_eq!(two_never[SKIP_ACTION_DIALOGS_KEY], json!(true));
        assert_eq!(two_never[ACTION_DIALOGS_KEY], json!({ "push": "always" }));
        let parsed_two_never = Settings::from_value(&two_never);
        assert!(parsed_two_never.skip_action_dialogs);
        assert_eq!(
            parsed_two_never
                .action_dialogs
                .resolve(RemoteAction::Fetch, parsed_two_never.skip_action_dialogs),
            ActionDialogMode::Never
        );
        assert_eq!(
            parsed_two_never
                .action_dialogs
                .resolve(RemoteAction::Pull, parsed_two_never.skip_action_dialogs),
            ActionDialogMode::Never
        );
        assert_eq!(
            parsed_two_never
                .action_dialogs
                .resolve(RemoteAction::Push, parsed_two_never.skip_action_dialogs),
            ActionDialogMode::Always
        );

        // Resetting back to all Toolbar via SettingsUpdate removes both keys.
        let reset = SettingsUpdate {
            skip_action_dialogs: Some(false),
            action_dialogs: Some(ActionDialogs::default()),
            ..Default::default()
        };
        let cleared = merge_settings_update(&two_never, &serde_json::to_value(&reset).unwrap());
        assert!(cleared.get(SKIP_ACTION_DIALOGS_KEY).is_none());
        assert!(cleared.get(ACTION_DIALOGS_KEY).is_none());

        // Lenient parsing of unknown/invalid values.
        assert_eq!(
            ActionDialogMode::parse("toolbar"),
            Some(ActionDialogMode::Toolbar)
        );
        assert_eq!(
            ActionDialogMode::parse("always"),
            Some(ActionDialogMode::Always)
        );
        assert_eq!(
            ActionDialogMode::parse("never"),
            Some(ActionDialogMode::Never)
        );
        assert_eq!(ActionDialogMode::parse("sometimes"), None);
        assert_eq!(ActionDialogMode::Toolbar.as_str(), "toolbar");
        assert_eq!(ActionDialogMode::Always.as_str(), "always");
        assert_eq!(ActionDialogMode::Never.as_str(), "never");

        let lenient = normalize_settings(Some(&json!({
            "actionDialogs": { "fetch": "bogus", "pull": 42, "push": "always" }
        })));
        assert!(lenient.get(SKIP_ACTION_DIALOGS_KEY).is_none());
        assert_eq!(lenient[ACTION_DIALOGS_KEY], json!({ "push": "always" }));

        let all_bogus = normalize_settings(Some(&json!({
            "actionDialogs": { "fetch": "unknown", "pull": null }
        })));
        assert!(all_bogus.get(SKIP_ACTION_DIALOGS_KEY).is_none());
        assert!(all_bogus.get(ACTION_DIALOGS_KEY).is_none());
    }
}
