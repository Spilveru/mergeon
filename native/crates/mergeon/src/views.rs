//! Reparto de los diálogos que piden los flujos del modelo.

use gpui_kit::{App, Window};
use mergeon_app::shell::DialogRequest;

/// Reparte el diálogo que pide un flujo del modelo entre los crates de vistas.
pub fn open_dialog(request: &DialogRequest, window: &mut Window, cx: &mut App) -> bool {
    if mergeon_view_toolbar::handle_dialog_request(request, window, cx) {
        return true;
    }
    if mergeon_view_home::handle_dialog_request(request, window, cx) {
        return true;
    }
    log::warn!("diálogo sin vista: {request:?}");
    false
}
