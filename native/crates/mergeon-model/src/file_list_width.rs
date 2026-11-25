//! Ancho de la lista de archivos dentro de una pestaña de archivo, frente al
//! diff (`src/composables/useFileListWidth.js`). Es una preferencia del usuario,
//! no de cada pestaña: un único estado para que todas las pestañas (cambios
//! locales, diff de un commit, árbol) muestren el mismo reparto, y se guarda al
//! soltar el divisor.

use serde_json::{Value, json};

use crate::resizable_width::{ResizableWidth, WidthLimits, saved_width, width_setting};

pub const DEFAULT_FILE_LIST_WIDTH: f64 = 240.0;
const MIN_WIDTH: f64 = 160.0;
const MAX_WIDTH: f64 = 640.0;
// Lo mínimo que se deja al diff para que el divisor no lo haga desaparecer.
const MIN_CONTENT_WIDTH: f64 = 240.0;

pub const FILE_LIST_WIDTH_LIMITS: WidthLimits = WidthLimits {
    default_width: DEFAULT_FILE_LIST_WIDTH,
    min_width: MIN_WIDTH,
    max_width: MAX_WIDTH,
    min_content_width: MIN_CONTENT_WIDTH,
};

pub fn clamp_file_list_width(value: f64, container_width: f64) -> f64 {
    FILE_LIST_WIDTH_LIMITS.clamp(value, container_width)
}

/// Estado del divisor de la lista de archivos, en su ancho inicial.
pub const fn file_list_width() -> ResizableWidth {
    ResizableWidth::new(FILE_LIST_WIDTH_LIMITS)
}

/// El ancho guardado en los ajustes (`settings.commitView.fileListWidth`), si
/// es un número.
pub fn saved_file_list_width(settings: &Value) -> Option<f64> {
    saved_width(
        settings
            .get("commitView")
            .and_then(|commit_view| commit_view.get("fileListWidth")),
    )
}

/// Parche para `updateSettings` con el ancho actual.
pub fn file_list_width_settings(width: f64) -> Value {
    json!({ "commitView": { "fileListWidth": width_setting(width) } })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loaded() -> ResizableWidth {
        let mut list = file_list_width();
        if list.begin_load() {
            list.apply_saved(saved_file_list_width(
                &json!({ "commitView": { "fileListWidth": 320 } }),
            ));
        }
        list
    }

    #[test]
    fn keeps_the_list_between_its_limits_and_always_leaves_room_for_the_diff() {
        assert_eq!(clamp_file_list_width(50.0, 0.0), 160.0);
        assert_eq!(clamp_file_list_width(2000.0, 0.0), 640.0);
        assert_eq!(clamp_file_list_width(600.0, 700.0), 460.0);
        assert_eq!(
            clamp_file_list_width(f64::NAN, 0.0),
            DEFAULT_FILE_LIST_WIDTH
        );
    }

    #[test]
    fn restores_the_saved_width_shared_by_every_file_tab() {
        let mut list = loaded();

        assert_eq!(list.width, 320.0);
        assert!(!list.begin_load());
    }

    #[test]
    fn follows_the_pointer_while_dragging_and_saves_once_on_release() {
        let mut list = loaded();

        list.start();
        list.drag(400.0, 100.0, 1000.0);
        list.drag(450.0, 100.0, 1000.0);
        assert_eq!(list.width, 350.0);

        assert!(list.stop());
        // Soltado el divisor, el puntero ya no mueve nada.
        list.drag(600.0, 100.0, 1000.0);

        assert_eq!(list.width, 350.0);
        assert_eq!(
            file_list_width_settings(list.width),
            json!({ "commitView": { "fileListWidth": 350 } })
        );
    }

    #[test]
    fn does_not_save_when_the_view_unmounts_without_a_drag() {
        let mut list = loaded();

        assert!(!list.stop());
    }

    #[test]
    fn nudges_with_the_keyboard_and_resets_on_double_click() {
        let mut list = loaded();

        list.nudge(16.0, 0.0);
        assert_eq!(list.width, 336.0);
        list.reset();
        assert_eq!(list.width, DEFAULT_FILE_LIST_WIDTH);
        assert_eq!(
            file_list_width_settings(list.width),
            json!({ "commitView": { "fileListWidth": DEFAULT_FILE_LIST_WIDTH as i64 } })
        );
    }

    #[test]
    fn has_no_saved_width_without_a_number() {
        assert_eq!(saved_file_list_width(&json!({ "commitView": {} })), None);
        assert_eq!(saved_file_list_width(&json!({})), None);
        assert_eq!(
            saved_file_list_width(&json!({ "commitView": { "fileListWidth": null } })),
            None
        );
    }
}
