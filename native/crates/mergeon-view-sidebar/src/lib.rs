//! Sidebar del repositorio activo: navegación, filtro, secciones (Starred,
//! Branches, Pull requests, Remotes, Tags, Stashes, Submodules), búsqueda
//! de commits y los diálogos que abre. Es el port de `src/components/layout/Sidebar*.vue`.
//!
//! API y eventos: `native/docs/view-sidebar.md`.
//!
//! ```ignore
//! mergeon_view_sidebar::init(cx);
//! let sidebar = cx.new(|cx| Sidebar::new(window, cx));
//! cx.subscribe(&sidebar, |_, _, event: &SidebarEvent, cx| { /* ... */ }).detach();
//!
//! // en `render`, dentro de la fila que comparte con la vista de commits:
//! div().flex().child(sidebar.clone()).child(SidebarResizer::new(&sidebar))
//! ```

mod actions;
pub mod dialogs;
mod flows;
pub mod logic;
pub mod menus;
pub mod pull_requests;
mod render_sections;
mod resizer;
mod search_filters;
mod section;
mod sidebar;
mod virtual_rows;
pub mod worktrees;

pub use menus::{BranchAction, MenuEntry, SidebarCommand};
pub use pull_requests::{PullRequestCommand, PullRequestMenuEntry, PullRequestsSection};
pub use resizer::SidebarResizer;
pub use sidebar::{
    CommitSearchResult, CommitSelectionRequest, ComparisonRequest, PanelMode, Section, Sidebar,
    SidebarEvent,
};

/// Inicialización del crate. El sidebar no registra acciones ni atajos
/// propios (el de la búsqueda, `mod+f`, es de `AppModel`); la función existe
/// para que la cáscara inicialice todas las vistas igual.
pub fn init(_cx: &mut gpui_kit::App) {}
