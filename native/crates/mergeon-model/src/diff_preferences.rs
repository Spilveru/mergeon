//! Cómo prefiere el usuario leer un diff: en una columna o en dos, si los
//! cambios de solo espacios cuentan (`src/composables/useDiffPreferences.js`)
//! y si se ve el archivo entero o solo los cambios ("Whole file", nuevo).
//!
//! En el original el estado vivía en el módulo, no dentro de la función, a
//! propósito: es la misma preferencia mire donde mire. Tenerla por componente es
//! lo que hizo que el detalle de un commit ganara "ignorar espacios" y los
//! cambios locales no. Aquí es un único [`DiffPreferences`] que la aplicación
//! comparte entre vistas.
//!
//! Leer y guardar los ajustes queda fuera: los métodos que cambian algo
//! devuelven el parche de ajustes que hay que persistir.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::jsstr::js_truthy;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiffViewMode {
    #[default]
    Unified,
    Split,
}

impl DiffViewMode {
    pub fn as_str(self) -> &'static str {
        match self {
            DiffViewMode::Unified => "unified",
            DiffViewMode::Split => "split",
        }
    }

    /// Un valor guardado que no significa nada se queda en una columna.
    pub fn parse(text: &str) -> Self {
        if text == "split" {
            DiffViewMode::Split
        } else {
            DiffViewMode::Unified
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffPreferences {
    pub diff_view_mode: DiffViewMode,
    pub ignore_whitespace: bool,
    /// "Whole file": el diff con el archivo entero como contexto, no solo las
    /// líneas alrededor de los cambios.
    pub whole_file: bool,
    /// Ya se leyeron los ajustes en esta sesión.
    #[serde(skip)]
    loaded: bool,
}

impl DiffPreferences {
    pub fn new() -> Self {
        Self::default()
    }

    /// Se lee una vez por sesión: la segunda vista que monte encuentra lo cargado.
    pub fn should_load(&self, force: bool) -> bool {
        !self.loaded || force
    }

    /// Adopta lo guardado. `settings` es el objeto de ajustes completo, tal
    /// como lo devuelve el almacenamiento.
    pub fn load_from_settings(&mut self, settings: &Value) {
        let commit_view = settings.get("commitView");
        let stored = |key: &str| commit_view.and_then(|commit_view| commit_view.get(key));

        self.diff_view_mode = DiffViewMode::parse(
            stored("diffViewMode")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        );
        self.ignore_whitespace = stored("ignoreWhitespace").is_some_and(js_truthy);
        self.whole_file = stored("wholeFile").is_some_and(js_truthy);
        self.loaded = true;
    }

    /// Cambia entre una y dos columnas; devuelve lo que hay que guardar.
    pub fn toggle_diff_view(&mut self) -> Value {
        self.diff_view_mode = match self.diff_view_mode {
            DiffViewMode::Unified => DiffViewMode::Split,
            DiffViewMode::Split => DiffViewMode::Unified,
        };
        self.settings_patch()
    }

    /// Cambia si los espacios cuentan; devuelve lo que hay que guardar.
    pub fn toggle_ignore_whitespace(&mut self) -> Value {
        self.ignore_whitespace = !self.ignore_whitespace;
        self.settings_patch()
    }

    /// Cambia entre ver solo los cambios y el archivo entero; devuelve lo que
    /// hay que guardar.
    pub fn toggle_whole_file(&mut self) -> Value {
        self.whole_file = !self.whole_file;
        self.settings_patch()
    }

    /// Parche para `updateSettings`.
    pub fn settings_patch(&self) -> Value {
        json!({
            "commitView": {
                "diffViewMode": self.diff_view_mode.as_str(),
                "ignoreWhitespace": self.ignore_whitespace,
                "wholeFile": self.whole_file
            }
        })
    }

    /// Para que un test empiece de cero.
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_on_one_column_counting_whitespace() {
        let preferences = DiffPreferences::new();

        assert_eq!(preferences.diff_view_mode, DiffViewMode::Unified);
        assert!(!preferences.ignore_whitespace);
    }

    // Lo que motivó extraer esto: el detalle de un commit ganó "ignorar espacios"
    // y los cambios locales no, porque cada vista tenía su propio estado. Aquí
    // las dos vistas leen el mismo valor.
    #[test]
    fn is_the_same_preference_wherever_you_look() {
        let mut shared = DiffPreferences::new();

        shared.toggle_diff_view();

        let (commit_view, local_changes) = (&shared, &shared);
        assert_eq!(commit_view.diff_view_mode, DiffViewMode::Split);
        assert_eq!(local_changes.diff_view_mode, DiffViewMode::Split);
    }

    #[test]
    fn remembers_the_choice_between_sessions() {
        let mut preferences = DiffPreferences::new();

        assert_eq!(
            preferences.toggle_diff_view(),
            json!({ "commitView": { "diffViewMode": "split", "ignoreWhitespace": false, "wholeFile": false } })
        );
        assert_eq!(
            preferences.toggle_ignore_whitespace(),
            json!({ "commitView": { "diffViewMode": "split", "ignoreWhitespace": true, "wholeFile": false } })
        );
        assert_eq!(
            preferences.toggle_whole_file(),
            json!({ "commitView": { "diffViewMode": "split", "ignoreWhitespace": true, "wholeFile": true } })
        );
        assert_eq!(
            preferences.toggle_diff_view(),
            json!({ "commitView": { "diffViewMode": "unified", "ignoreWhitespace": true, "wholeFile": true } })
        );
    }

    #[test]
    fn picks_up_what_was_saved() {
        let mut preferences = DiffPreferences::new();

        preferences.load_from_settings(&json!({
            "commitView": { "diffViewMode": "split", "ignoreWhitespace": true, "wholeFile": true }
        }));

        assert_eq!(preferences.diff_view_mode, DiffViewMode::Split);
        assert!(preferences.ignore_whitespace);
        assert!(preferences.whole_file);

        // Sin la clave (ajustes de antes de "Whole file"): solo los cambios.
        preferences.load_from_settings(&json!({ "commitView": { "diffViewMode": "split" } }));
        assert!(!preferences.whole_file);
    }

    // La segunda vista que monte encuentra lo ya cargado en vez de releerlo.
    #[test]
    fn reads_the_settings_once_per_session() {
        let mut preferences = DiffPreferences::new();
        assert!(preferences.should_load(false));

        preferences.load_from_settings(&json!({ "commitView": {} }));

        assert!(!preferences.should_load(false));
        assert!(preferences.should_load(true));
        preferences.reset();
        assert!(preferences.should_load(false));
    }

    #[test]
    fn ignores_a_stored_value_that_means_nothing() {
        let mut preferences = DiffPreferences::new();
        preferences.toggle_diff_view();

        preferences.load_from_settings(&json!({ "commitView": { "diffViewMode": "diagonal" } }));

        assert_eq!(preferences.diff_view_mode, DiffViewMode::Unified);
        assert!(!preferences.ignore_whitespace);
    }

    // Sin almacenamiento la app sigue funcionando, solo que sin memoria.
    #[test]
    fn holds_up_when_there_is_nowhere_to_save() {
        let mut preferences = DiffPreferences::new();

        preferences.load_from_settings(&Value::Null);
        preferences.toggle_diff_view();

        assert_eq!(preferences.diff_view_mode, DiffViewMode::Split);
    }

    #[test]
    fn reads_whitespace_like_javascript_reads_a_flag() {
        let mut preferences = DiffPreferences::new();

        preferences.load_from_settings(&json!({ "commitView": { "ignoreWhitespace": 1 } }));
        assert!(preferences.ignore_whitespace);
        preferences.load_from_settings(&json!({ "commitView": { "ignoreWhitespace": "" } }));
        assert!(!preferences.ignore_whitespace);
        assert_eq!(
            serde_json::to_value(preferences).unwrap(),
            json!({ "diffViewMode": "unified", "ignoreWhitespace": false, "wholeFile": false })
        );
    }
}
