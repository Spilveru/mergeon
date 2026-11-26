//! Sistema de diseño de Mergeon sobre GPUI Kit: tema, paleta, íconos y los
//! componentes base de la app (el port de `src/components/ui/` y de las recetas
//! de `src/lib/ui/variants.js`).
//!
//! Catálogo y equivalencias con los componentes Vue: `native/docs/ui-components.md`.
//!
//! ```ignore
//! gpui_kit::application()
//!     .with_assets(mergeon_ui::Assets)
//!     .run(|cx| {
//!         mergeon_ui::init(cx); // antes de abrir ventanas
//!         gpui_kit::open_window(options, cx, |window, cx| cx.new(|cx| Shell::new(window, cx)))
//!             .expect("no se pudo abrir la ventana");
//!     });
//! ```

mod alert;
mod assets;
mod badge;
mod button;
mod checkbox;
pub mod dialog;
mod empty_state;
mod hover_card;
mod icon;
mod input;
mod kbd;
pub mod menu;
mod radio;
mod root;
mod select;
mod spinner;
pub mod style;
mod tabs;
pub mod theme;
pub mod toast;

pub use alert::{Alert, AlertVariant, alert};
pub use assets::Assets;
pub use badge::{
    Badge, BadgeSize, BadgeVariant, badge, status_badge, status_badge_style, status_label,
};
pub use button::{Button, ButtonSize, ButtonVariant, button};
pub use checkbox::{Checkbox, checkbox};
pub use empty_state::{EmptyState, empty_state};
/// Catálogo completo de íconos Lucide (los mismos que usa la app Electron):
/// `Icon::new(Lucide::GitBranch)`, `mergeon_ui::icon(Lucide::Download)`.
pub use gpui_kit::assets::IconName as Lucide;
pub use hover_card::{HoverCard, HoverCardAlign, hover_card, tooltip, tooltip_with_shortcut};
pub use icon::{AppIcon, IconTone, icon};
pub use input::{TextField, input, textarea};
pub use kbd::{Kbd, kbd};
pub use radio::{RadioGroup, RadioOption, radio_group};
pub use select::{Select, SelectOption, select};
pub use spinner::{Spinner, SpinnerColor, SpinnerSize, spinner};
pub use style::{MergeonStyled, Radius};
pub use tabs::{TabItem, Tabs, tabs};
pub use theme::{ActivePaletteExt, Palette, ThemePreference};

/// Estados de los campos de texto, reexportados de gpui-kit para que una vista
/// los cree sin importar otra ruta: `cx.new(|cx| InputState::new(window, cx))`.
pub use gpui_kit::component::input::{InputEvent, InputState, TextareaState};

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{App, Global};

struct UiInitialized;

impl Global for UiInitialized {}

/// Inicializa gpui-kit, las fuentes, el tema y la capa de Mergeon de las
/// ventanas (texto base, diálogos y toasts). Debe llamarse una vez, antes de
/// abrir ventanas con `gpui_kit::open_window`; puede volver a llamarse tras
/// `cx.clear_key_bindings()` para re-registrar los bindings sin perder el tema
/// activo ni recargar fuentes.
pub fn init(cx: &mut App) {
    let existing_dark = cx
        .try_global::<UiInitialized>()
        .map(|_| cx.theme().mode.is_dark());
    gpui_kit::init(cx);
    button::init(cx);
    if existing_dark.is_none() {
        assets::register_fonts(cx);
        root::init(cx);
        cx.set_global(UiInitialized);
    }
    theme::init(cx);
    if let Some(dark) = existing_dark {
        theme::apply(
            if dark {
                ThemePreference::Dark
            } else {
                ThemePreference::Light
            },
            cx,
        );
    }
}
