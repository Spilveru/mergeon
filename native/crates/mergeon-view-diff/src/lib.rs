//! Visores de diff y de contenido de archivo, compartidos por las vistas de commits y de cambios.

mod blame_viewer;
mod canvas;
mod change_markers;
mod diff_panel;
mod diff_viewer;
mod editor_line;
mod file_content_viewer;
mod file_tree_row;
mod find;
pub mod highlight;
mod image_diff;
mod text;
mod typography;

pub use blame_viewer::{BlameLine, BlameState, BlameViewer, BlameViewerEvent};
pub use change_markers::ChangeMarker;
pub use diff_panel::{
    DiffPanel, DiffSide, SPLIT_STAGING_NOTE, ignore_whitespace_tooltip, panel_status_label,
    view_toggle_label, whole_file_tooltip,
};
pub use diff_viewer::{
    DiffViewer, DiffViewerEvent, HunkAction, HunkRequest, LineAnchor, LinesRequest, PatchAction,
    ScrollPosition,
};
pub use editor_line::nearest_new_line;
pub use file_content_viewer::{FileContent, FileContentViewer, FileContentViewerEvent};
pub use file_tree_row::{
    CHANGE_BAR_BLOCKS, ChangeBlock, FileStats, FileTreeRow, bar_pattern, change_bars, file_stats,
};
pub use find::{
    CONTEXT as VIEWER_KEY_CONTEXT, FindInViewer, FindStatus, NextChange, PreviousChange,
};
pub use image_diff::{
    IMAGE_EXTENSIONS, ImageDiff, ImageDiffViewer, ImageVersion, image_dimensions,
    image_format_for_path, image_text_toggle, is_image_path, is_svg_path,
};
pub use mergeon_model::diff_preferences::DiffViewMode;
pub use mergeon_model::patch::PatchDirection;
pub use typography::code_font_family;

/// Punto de entrada común de los crates de vistas: registra las teclas de los
/// visores (`mod+f` busca dentro del visor con el foco en él, F7 / Shift+F7 y
/// Alt+↓ / Alt+↑ saltan entre bloques de cambios). Hay que llamarlo al
/// arrancar: sin él, `mod+f` sigue siendo "Search commits" también dentro
/// del visor.
pub fn init(cx: &mut gpui_kit::App) {
    find::init(cx);
}
