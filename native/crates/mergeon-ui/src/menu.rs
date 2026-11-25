//! Menús contextuales y desplegables (`src/components/ui/context-menu/ContextMenu.vue`,
//! receta `menuItemVariants`).
//!
//! ```ignore
//! use mergeon_ui::menu::{MenuItem, MenuTriggerExt as _};
//!
//! div()
//!     .id(("branch", branch.name.clone()))
//!     .child(branch.name.clone())
//!     .context_menu_items(move |_, _| {
//!         vec![
//!             MenuItem::new("Checkout").icon(Lucide::GitBranch).on_click(|window, cx| { /* ... */ }),
//!             MenuItem::separator(),
//!             MenuItem::new("Delete").icon(Lucide::Trash).danger().shortcut("Del"),
//!         ]
//!     })
//! ```
//!
//! La navegación con flechas, Enter / Escape, el cierre al hacer clic fuera, el
//! ajuste a la ventana y los submenús son los del `PopupMenu` de gpui-kit; cada
//! fila se dibuja aquí con la receta de Mergeon (ícono de 16px, atajo en `Kbd`,
//! variante destructiva).
//!
//! Un atajo de una sola letra (`.shortcut("B")`) además funciona con el menú
//! abierto: pulsar la letra ejecuta la entrada y cierra el menú, como en el
//! diseño Señal. Los atajos con modificadores solo se muestran.

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::base::actions::Cancel;
use gpui_kit::component::menu::{
    ContextMenu, ContextMenuExt, DropdownMenu, PopupMenu, PopupMenuItem,
};
use gpui_kit::component::{ActiveTheme as _, Icon, IconNamed, Side};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Context, DismissEvent, Div, Focusable as _, InteractiveElement, IntoElement,
    ParentElement, SharedString, Styled, Window, div, px, rems,
};

use crate::icon::{AppIcon, IconTone};
use crate::style::MergeonStyled as _;
use crate::theme::ActivePaletteExt as _;

type Handler = Rc<dyn Fn(&mut Window, &mut App)>;

/// Una entrada de menú: `{ label, icon?, shortcut?, danger?, disabled?, action }`
/// o un separador (`{ type: 'separator' }`).
#[derive(Clone)]
pub struct MenuItem {
    entry: Entry,
}

#[derive(Clone)]
enum Entry {
    Separator,
    Label(SharedString),
    /// `.mlab` de Señal: mono pequeño en mayúsculas, apagado.
    Header(SharedString),
    Item {
        label: SharedString,
        icon: Option<AppIcon>,
        shortcut: Option<SharedString>,
        danger: bool,
        disabled: bool,
        checked: bool,
        /// Fila de `.list-box-item` (select): 13px, y peso medio si es la activa.
        list_box: bool,
        handler: Option<Handler>,
    },
    Submenu {
        label: SharedString,
        icon: Option<AppIcon>,
        disabled: bool,
        items: Rc<Vec<MenuItem>>,
    },
}

impl MenuItem {
    /// Entrada normal con la etiqueta dada.
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            entry: Entry::Item {
                label: label.into(),
                icon: None,
                shortcut: None,
                danger: false,
                disabled: false,
                checked: false,
                list_box: false,
                handler: None,
            },
        }
    }

    /// Opción de un select: la fila `.list-box-item` de `globals.css`.
    pub fn list_box(mut self) -> Self {
        if let Entry::Item { list_box, .. } = &mut self.entry {
            *list_box = true;
        }
        self
    }

    /// Línea separadora.
    pub fn separator() -> Self {
        Self {
            entry: Entry::Separator,
        }
    }

    /// Rótulo no interactivo, para titular un grupo.
    pub fn label(label: impl Into<SharedString>) -> Self {
        Self {
            entry: Entry::Label(label.into()),
        }
    }

    /// Cabecera del menú (`.mlab`): texto mono de 10px en mayúsculas y tono
    /// apagado, p. ej. `"c4e1a07 · fix(cart)"`. Va como primera entrada.
    pub fn header(text: impl Into<SharedString>) -> Self {
        Self {
            entry: Entry::Header(text.into()),
        }
    }

    /// Entrada que abre otro menú. El original no los usa; están para los
    /// menús que crezcan.
    pub fn submenu(label: impl Into<SharedString>, items: Vec<MenuItem>) -> Self {
        Self {
            entry: Entry::Submenu {
                label: label.into(),
                icon: None,
                disabled: false,
                items: Rc::new(items),
            },
        }
    }

    /// Ícono de 16px al inicio de la fila.
    pub fn icon(self, icon: impl IconNamed) -> Self {
        self.app_icon(AppIcon::new(icon))
    }

    /// Como [`Self::icon`], con un [`AppIcon`] ya armado (un SVG propio, por
    /// ejemplo). El tono lo pone la fila.
    pub fn app_icon(mut self, icon: AppIcon) -> Self {
        match &mut self.entry {
            Entry::Item { icon: slot, .. } | Entry::Submenu { icon: slot, .. } => {
                *slot = Some(icon);
            }
            _ => {}
        }
        self
    }

    /// Atajo que se muestra a la derecha (`"Ctrl+C"`). Con modificadores solo
    /// informa (el atajo se registra aparte); una sola letra o cifra (`"B"`)
    /// además ejecuta la entrada si se pulsa con el menú abierto.
    pub fn shortcut(mut self, shortcut: impl Into<SharedString>) -> Self {
        if let Entry::Item { shortcut: slot, .. } = &mut self.entry {
            *slot = Some(shortcut.into());
        }
        self
    }

    /// Texto secundario a la derecha, en el hueco del atajo (la ruta de un
    /// repositorio, el contexto de un documento). A diferencia de
    /// [`Self::shortcut`], nunca es una tecla de acceso.
    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        let detail: SharedString = detail.into();
        if let Entry::Item { shortcut: slot, .. } = &mut self.entry {
            // Un solo carácter sería una tecla de acceso: se le añade un
            // espacio de ancho cero (que `trim` no quita).
            *slot = Some(if access_key(&detail).is_some() {
                format!("{detail}\u{200B}").into()
            } else {
                detail
            });
        }
        self
    }

    /// Variante destructiva (`danger: true`).
    pub fn danger(mut self) -> Self {
        if let Entry::Item { danger, .. } = &mut self.entry {
            *danger = true;
        }
        self
    }

    pub fn disabled(mut self, value: bool) -> Self {
        match &mut self.entry {
            Entry::Item { disabled, .. } | Entry::Submenu { disabled, .. } => *disabled = value,
            _ => {}
        }
        self
    }

    /// Marca la entrada con un check a la derecha (la opción activa de un select).
    pub fn checked(mut self, value: bool) -> Self {
        if let Entry::Item { checked, .. } = &mut self.entry {
            *checked = value;
        }
        self
    }

    /// Acción de la entrada. El menú se cierra después de ejecutarla.
    pub fn on_click(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        if let Entry::Item { handler: slot, .. } = &mut self.entry {
            *slot = Some(Rc::new(handler));
        }
        self
    }

    fn is_disabled(&self) -> bool {
        match &self.entry {
            Entry::Item { disabled, .. } | Entry::Submenu { disabled, .. } => *disabled,
            _ => true,
        }
    }

    /// Si es un separador.
    pub fn is_separator(&self) -> bool {
        matches!(self.entry, Entry::Separator)
    }

    /// Texto de la entrada; vacío en un separador.
    pub fn text(&self) -> SharedString {
        match &self.entry {
            Entry::Separator => SharedString::default(),
            Entry::Label(label)
            | Entry::Header(label)
            | Entry::Item { label, .. }
            | Entry::Submenu { label, .. } => label.clone(),
        }
    }

    /// La tecla que ejecuta la entrada con el menú abierto: su atajo, si es
    /// una sola letra o cifra (en minúscula).
    pub fn access_key(&self) -> Option<SharedString> {
        match &self.entry {
            Entry::Item {
                shortcut: Some(shortcut),
                ..
            } => access_key(shortcut),
            _ => None,
        }
    }

    /// Ejecuta la acción de la entrada si está habilitada. Devuelve si lo hizo.
    pub fn activate(&self, window: &mut Window, cx: &mut App) -> bool {
        match &self.entry {
            Entry::Item {
                disabled: false,
                handler: Some(handler),
                ..
            } => {
                handler(window, cx);
                true
            }
            _ => false,
        }
    }
}

/// `min-width: 12rem; max-width: 22rem` de `.context-menu-popover`.
const MENU_MIN_WIDTH: f32 = 192.;
const MENU_MAX_WIDTH: f32 = 352.;

/// Añade las entradas a un `PopupMenu` de gpui-kit con la receta de Mergeon.
/// Sirve dentro de cualquier constructor de menú de gpui-kit
/// (`.context_menu(..)`, `.dropdown_menu(..)`).
pub fn build(
    menu: PopupMenu,
    items: impl IntoIterator<Item = MenuItem>,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let items: Vec<MenuItem> = items.into_iter().collect();
    listen_access_keys(&menu, &items, cx);
    let menu = menu
        .min_w(px(MENU_MIN_WIDTH))
        .max_w(px(MENU_MAX_WIDTH))
        .check_side(Side::Right);
    append(menu, items, window, cx)
}

/// `"B"` → `"b"`: solo una letra o cifra sin modificadores es tecla de acceso.
fn access_key(shortcut: &str) -> Option<SharedString> {
    let mut chars = shortcut.trim().chars();
    let key = chars.next()?;
    (chars.next().is_none() && key.is_alphanumeric())
        .then(|| key.to_lowercase().collect::<String>().into())
}

/// Con el menú abierto (y enfocado), la letra de una entrada la ejecuta: se
/// cierra el menú como con Escape (el foco vuelve a donde estaba) y después
/// corre la acción, que puede abrir un diálogo y llevarse el foco.
///
/// El oyente se suelta cuando el menú se cierra.
fn listen_access_keys(menu: &PopupMenu, items: &[MenuItem], cx: &mut Context<PopupMenu>) {
    let keys: Vec<(SharedString, MenuItem)> = items
        .iter()
        .filter_map(|item| Some((item.access_key()?, item.clone())))
        .collect();
    if keys.is_empty() {
        return;
    }
    let focus = menu.focus_handle(cx);
    let subscription = cx.intercept_keystrokes(move |event, window, cx| {
        let keystroke = &event.keystroke;
        if keystroke.modifiers.modified() || !focus.is_focused(window) {
            return;
        }
        let pressed = keystroke.key.to_lowercase();
        let Some((_, item)) = keys.iter().find(|(key, _)| key.as_ref() == pressed) else {
            return;
        };
        cx.stop_propagation();
        if item.is_disabled() {
            return;
        }
        focus.dispatch_action(&Cancel, window, cx);
        item.activate(window, cx);
    });
    // Cerrado el menú (con la letra, Escape, un clic o un clic fuera) la
    // tecla vuelve a ser de la ventana: gpui-kit conserva el menú hasta el
    // siguiente y, si nada tenía el foco antes, el foco se queda en él.
    let subscription = RefCell::new(Some(subscription));
    cx.subscribe_self(move |_, _: &DismissEvent, _| {
        subscription.borrow_mut().take();
    })
    .detach();
}

fn append(
    menu: PopupMenu,
    items: impl IntoIterator<Item = MenuItem>,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    items.into_iter().fold(menu, |menu, item| match item.entry {
        Entry::Separator => menu.separator(),
        Entry::Header(text) => menu.item(
            PopupMenuItem::element(move |_, cx| {
                div()
                    .min_w_0()
                    .pt(px(2.))
                    .truncate()
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_size(px(10.))
                    .line_height(px(14.))
                    .text_color(cx.palette().text_3)
                    .child(SharedString::from(text.to_uppercase()))
            })
            .disabled(true),
        ),
        Entry::Label(label) => menu.item(
            PopupMenuItem::element(move |_, cx| {
                div()
                    .tw_text_xs()
                    .text_color(cx.palette().muted_foreground)
                    .child(label.clone())
            })
            .disabled(true),
        ),
        Entry::Item {
            label,
            icon,
            shortcut,
            danger,
            disabled,
            checked,
            list_box,
            handler,
        } => {
            let row = PopupMenuItem::element(move |_, cx| {
                item_row(
                    label.clone(),
                    icon.clone(),
                    shortcut.clone(),
                    danger,
                    checked,
                    cx,
                )
                .when(list_box, |this| {
                    this.text_size(rems(0.8125))
                        .when(checked, |this| this.tw_font_medium())
                })
            })
            .disabled(disabled);
            menu.item(match handler {
                Some(handler) => row.on_click(move |_, window, cx| handler(window, cx)),
                None => row,
            })
        }
        Entry::Submenu {
            label,
            icon,
            disabled,
            items,
        } => {
            let menu = menu.submenu_with_icon(
                icon.map(|icon| icon.tone(IconTone::Muted).into_icon(cx)),
                label,
                window,
                cx,
                move |submenu, window, cx| build(submenu, items.iter().cloned(), window, cx),
            );
            if disabled {
                log::debug!("los submenús deshabilitados no están soportados por gpui-kit");
            }
            menu
        }
    })
}

/// Contenido de una fila: ícono, etiqueta y atajo o check.
fn item_row(
    label: SharedString,
    icon: Option<AppIcon>,
    shortcut: Option<SharedString>,
    danger: bool,
    checked: bool,
    cx: &App,
) -> Div {
    let palette = cx.palette();
    // El ítem activo se pinta en tinta (`accent` del tema de gpui-kit): ícono y
    // atajo heredan el color del texto y se apagan con opacidad, así se leen
    // igual sobre el menú y sobre la fila invertida.
    let quiet = |child: AnyElement| div().flex().items_center().opacity(0.62).child(child);
    div()
        .flex()
        .flex_1()
        .min_w_0()
        .items_center()
        .gap(px(9.))
        .text_size(rems(13. / 16.))
        // `danger-text`; la fila normal hereda el color del menú.
        .when(danger, |this| {
            this.text_color(palette.destructive_foreground)
        })
        .when_some(icon, |this, icon| {
            this.child(quiet(
                icon.size(px(14.))
                    .tone(IconTone::Current)
                    .into_any_element(),
            ))
        })
        .child(div().flex_1().min_w_0().truncate().child(label))
        .when_some(shortcut, |this, shortcut| {
            this.child(
                quiet(
                    div()
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_size(px(11.))
                        .child(shortcut)
                        .into_any_element(),
                )
                .ml_auto()
                .pl_3(),
            )
        })
        .when(checked, |this| {
            this.child(
                AppIcon::from_icon(Icon::default().data(CHECK_SVG))
                    .size(rems(0.7).to_pixels(px(16.)))
                    .tone(IconTone::Current)
                    .ml_auto(),
            )
        })
}

/// El check de trazo grueso que dibujan `Select.vue` y `Checkbox.vue`.
pub(crate) const CHECK_SVG: &[u8] = include_bytes!("../assets/icons/check.svg");

/// Atajos para colgar un menú de Mergeon de cualquier elemento.
pub trait MenuTriggerExt: Sized {
    /// Menú contextual (clic derecho) con las entradas que devuelve `items`,
    /// que se evalúa cada vez que el menú se abre.
    fn context_menu_items(
        self,
        items: impl Fn(&mut Window, &mut App) -> Vec<MenuItem> + 'static,
    ) -> ContextMenu<Self>
    where
        Self: InteractiveElement + ParentElement + Styled;

    /// Menú desplegable (clic) bajo el elemento, alineado a su borde izquierdo.
    fn dropdown_menu_items(
        self,
        items: impl Fn(&mut Window, &mut App) -> Vec<MenuItem> + 'static,
    ) -> impl IntoElement
    where
        Self: DropdownMenu;
}

impl<E: Sized> MenuTriggerExt for E {
    #[track_caller]
    fn context_menu_items(
        self,
        items: impl Fn(&mut Window, &mut App) -> Vec<MenuItem> + 'static,
    ) -> ContextMenu<Self>
    where
        Self: InteractiveElement + ParentElement + Styled,
    {
        self.context_menu(move |menu, window, cx| {
            let entries = items(window, cx);
            build(menu, entries, window, cx)
        })
    }

    fn dropdown_menu_items(
        self,
        items: impl Fn(&mut Window, &mut App) -> Vec<MenuItem> + 'static,
    ) -> impl IntoElement
    where
        Self: DropdownMenu,
    {
        self.dropdown_menu(move |menu, window, cx| {
            let entries = items(window, cx);
            build(menu, entries, window, cx)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_detail_shows_where_the_shortcut_goes_but_is_never_an_access_key() {
        assert_eq!(MenuItem::new("main.rs").detail("x").access_key(), None);
        assert_eq!(MenuItem::new("demo").detail("~/src").access_key(), None);
        assert_eq!(
            MenuItem::new("Copy").shortcut("C").access_key().as_deref(),
            Some("c")
        );
    }
}
