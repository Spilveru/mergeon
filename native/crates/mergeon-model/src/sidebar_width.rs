//! Ancho de la barra lateral (ramas, remotos, tags, stashes) frente a la vista
//! de commits (`src/composables/useSidebarWidth.js`). Es una preferencia del
//! usuario, no de cada repositorio: un único estado que comparten todas las
//! pestañas de repositorio, y se guarda al soltar el divisor. Mismo patrón que
//! `file_list_width`.

use serde_json::{Value, json};

use crate::resizable_width::{ResizableWidth, WidthLimits, saved_width, width_setting};

pub const DEFAULT_SIDEBAR_WIDTH: f64 = 260.0;
const MIN_WIDTH: f64 = 200.0;
const MAX_WIDTH: f64 = 520.0;
// Lo mínimo que se deja a la vista de commits (lista + detalle) para que el
// divisor no la aplaste.
const MIN_CONTENT_WIDTH: f64 = 480.0;

pub const SIDEBAR_WIDTH_LIMITS: WidthLimits = WidthLimits {
    default_width: DEFAULT_SIDEBAR_WIDTH,
    min_width: MIN_WIDTH,
    max_width: MAX_WIDTH,
    min_content_width: MIN_CONTENT_WIDTH,
};

pub fn clamp_sidebar_width(value: f64, container_width: f64) -> f64 {
    SIDEBAR_WIDTH_LIMITS.clamp(value, container_width)
}

/// Estado del divisor de la barra lateral, en su ancho inicial.
pub const fn sidebar_width() -> ResizableWidth {
    ResizableWidth::new(SIDEBAR_WIDTH_LIMITS)
}

/// El ancho guardado en los ajustes (`settings.sidebarWidth`), si es un número.
pub fn saved_sidebar_width(settings: &Value) -> Option<f64> {
    saved_width(settings.get("sidebarWidth"))
}

/// Parche para `updateSettings` con el ancho actual.
pub fn sidebar_width_settings(width: f64) -> Value {
    json!({ "sidebarWidth": width_setting(width) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loaded() -> ResizableWidth {
        let mut sidebar = sidebar_width();
        if sidebar.begin_load() {
            sidebar.apply_saved(saved_sidebar_width(&json!({ "sidebarWidth": 320 })));
        }
        sidebar
    }

    #[test]
    fn keeps_the_sidebar_between_its_limits_and_always_leaves_room_for_the_commits() {
        assert_eq!(clamp_sidebar_width(50.0, 0.0), 200.0);
        assert_eq!(clamp_sidebar_width(2000.0, 0.0), 520.0);
        assert_eq!(clamp_sidebar_width(500.0, 900.0), 420.0);
        assert_eq!(clamp_sidebar_width(f64::NAN, 0.0), DEFAULT_SIDEBAR_WIDTH);
    }

    #[test]
    fn restores_the_saved_width_shared_by_every_repository_tab() {
        let mut sidebar = loaded();

        assert_eq!(sidebar.width, 320.0);
        // La segunda pestaña encuentra el mismo estado y no vuelve a leer.
        assert!(!sidebar.begin_load());
    }

    #[test]
    fn follows_the_mouse_while_dragging_and_saves_once_on_release() {
        let mut sidebar = loaded();

        sidebar.start();
        sidebar.drag(348.0, 48.0, 1400.0);
        assert_eq!(sidebar.width, 300.0);
        assert!(sidebar.resizing);

        assert!(sidebar.stop());
        assert!(!sidebar.resizing);
        assert_eq!(
            sidebar_width_settings(sidebar.width),
            json!({ "sidebarWidth": 300 })
        );
        assert!(!sidebar.stop());
    }

    #[test]
    fn nudges_with_the_keyboard_and_resets_on_demand() {
        let mut sidebar = loaded();

        sidebar.nudge(16.0, 0.0);
        assert_eq!(sidebar.width, 336.0);

        sidebar.reset();
        assert_eq!(sidebar.width, DEFAULT_SIDEBAR_WIDTH);
        assert_eq!(
            sidebar_width_settings(sidebar.width),
            json!({ "sidebarWidth": DEFAULT_SIDEBAR_WIDTH as i64 })
        );
    }

    #[test]
    fn starts_at_the_default_when_nothing_usable_was_saved() {
        assert_eq!(sidebar_width().width, DEFAULT_SIDEBAR_WIDTH);
        assert_eq!(saved_sidebar_width(&json!({})), None);
        assert_eq!(
            saved_sidebar_width(&json!({ "sidebarWidth": "wide" })),
            None
        );
        assert_eq!(saved_sidebar_width(&Value::Null), None);
    }
}
