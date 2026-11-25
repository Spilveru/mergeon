//! Selector (`src/components/ui/select/Select.vue`, `selectTriggerClass`).
//!
//! Es controlado, como el `v-model` del original: recibe el valor actual y
//! avisa del nuevo con `on_change`, sin estado que guardar en la vista. El
//! disparador es propio (la receta comparte el lenguaje de los inputs) y la
//! lista es el menú de Mergeon ([`crate::menu`]) sobre el `PopupMenu` de
//! gpui-kit, que pone la navegación con teclado, el cierre y la posición.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::base::{Button as BaseButton, ElementExt as _, Selectable};
use gpui_kit::component::menu::DropdownMenu;
use gpui_kit::component::{Icon, Sizable as _, Size, StyledExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, ElementId, InteractiveElement, Interactivity, IntoElement, MouseButton,
    ParentElement, Pixels, RenderOnce, Role, SharedString, StatefulInteractiveElement as _,
    StyleRefinement, Styled, Window, div, px, rems,
};

use crate::Lucide;
use crate::menu::{self, MenuItem};
use crate::style::{DISABLED_OPACITY, Radius, input_focus_overlay};
use crate::theme::ActivePaletteExt as _;

/// Una opción: `{ value, label }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectOption {
    value: SharedString,
    label: SharedString,
}

impl SelectOption {
    pub fn new(value: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
        }
    }

    pub fn value(&self) -> &SharedString {
        &self.value
    }

    pub fn label(&self) -> &SharedString {
        &self.label
    }
}

/// Una cadena es una opción cuyo valor y etiqueta coinciden.
impl<T: Into<SharedString>> From<T> for SelectOption {
    fn from(value: T) -> Self {
        let value: SharedString = value.into();
        Self::new(value.clone(), value)
    }
}

type ChangeHandler = Rc<dyn Fn(&SharedString, &mut Window, &mut App)>;

/// Selector de Mergeon. Se crea con [`select`].
#[derive(IntoElement)]
pub struct Select {
    id: ElementId,
    items: Vec<SelectOption>,
    value: Option<SharedString>,
    placeholder: SharedString,
    full_width: bool,
    disabled: bool,
    on_change: Option<ChangeHandler>,
    style: StyleRefinement,
}

/// Selector controlado.
///
/// ```ignore
/// mergeon_ui::select("remote")
///     .items(["origin", "upstream"])
///     .value(self.remote.clone())
///     .on_change(cx.listener(|this, value: &SharedString, _, cx| {
///         this.remote = value.clone();
///         cx.notify();
///     }))
/// ```
pub fn select(id: impl Into<ElementId>) -> Select {
    Select {
        id: id.into(),
        items: Vec::new(),
        value: None,
        placeholder: "Select…".into(),
        full_width: true,
        disabled: false,
        on_change: None,
        style: StyleRefinement::default(),
    }
}

impl Select {
    /// Opciones: cadenas o [`SelectOption`] (`{ value, label }`).
    pub fn items<T: Into<SelectOption>>(mut self, items: impl IntoIterator<Item = T>) -> Self {
        self.items = items.into_iter().map(Into::into).collect();
        self
    }

    /// Valor seleccionado (`modelValue`). Si no coincide con ninguna opción se
    /// muestra el placeholder.
    pub fn value(mut self, value: impl Into<SharedString>) -> Self {
        self.value = Some(value.into());
        self
    }

    /// Texto cuando no hay selección (`"Select…"` por defecto).
    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    /// Ocupa todo el ancho del contenedor (por defecto sí, como en el original).
    pub fn full_width(mut self, full_width: bool) -> Self {
        self.full_width = full_width;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn on_change(
        mut self,
        handler: impl Fn(&SharedString, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_change = Some(Rc::new(handler));
        self
    }

    /// Etiqueta que muestra el disparador para el valor actual.
    pub fn selected_label(&self) -> Option<&SharedString> {
        let value = self.value.as_ref()?;
        self.items
            .iter()
            .find(|item| &item.value == value)
            .map(|item| &item.label)
    }
}

/// El estilo externo se aplica al contenedor (ancho, márgenes).
impl Styled for Select {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

/// Entradas de menú de un select: una por opción, con el check en la activa.
pub(crate) fn menu_items(
    items: &[SelectOption],
    value: Option<&SharedString>,
    on_change: Option<ChangeHandler>,
) -> Vec<MenuItem> {
    items
        .iter()
        .map(|item| {
            let entry = MenuItem::new(item.label.clone())
                .checked(value == Some(&item.value))
                .list_box();
            match on_change.clone() {
                Some(on_change) => {
                    let value = item.value.clone();
                    entry.on_click(move |window, cx| on_change(&value, window, cx))
                }
                None => entry,
            }
        })
        .collect()
}

impl RenderOnce for Select {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let label = self.selected_label().cloned();
        // El ancho del disparador, medido al pintarlo, es el de la lista.
        let width = window
            .use_keyed_state(self.id.clone(), cx, |_, _| Rc::new(Cell::new(px(0.))))
            .read(cx)
            .clone();

        let trigger = SelectTrigger {
            base: BaseButton::new(self.id.clone()),
            id: self.id,
            label: label.clone().unwrap_or(self.placeholder),
            disabled: self.disabled,
            open: false,
            width: width.clone(),
        };

        let wrapper = div()
            .when(self.full_width, |this| this.w_full())
            .min_w_0()
            .refine_style(&self.style);

        if self.disabled {
            return wrapper.child(trigger).into_any_element();
        }

        let items = Rc::new(self.items);
        let value = self.value;
        let on_change = self.on_change;
        wrapper
            .child(trigger.dropdown_menu(move |popup, window, cx| {
                // `width: trigger; min-width: 8rem`
                let width = width.get().max(px(128.));
                let entries = menu_items(&items, value.as_ref(), on_change.clone());
                menu::build(popup, entries, window, cx)
                    .min_w(width)
                    .max_w(width)
            }))
            .into_any_element()
    }
}

/// El botón del select. Existe como tipo para poder ser el disparador de un
/// menú desplegable de gpui-kit.
#[derive(IntoElement)]
struct SelectTrigger {
    id: ElementId,
    base: BaseButton,
    label: SharedString,
    disabled: bool,
    open: bool,
    width: Rc<Cell<Pixels>>,
}

impl Selectable for SelectTrigger {
    fn selected(mut self, selected: bool) -> Self {
        self.open = selected;
        self
    }

    fn is_selected(&self) -> bool {
        self.open
    }
}

impl Styled for SelectTrigger {
    fn style(&mut self) -> &mut StyleRefinement {
        self.base.style()
    }
}

impl InteractiveElement for SelectTrigger {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.base.interactivity()
    }
}

impl ParentElement for SelectTrigger {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.base.extend(elements);
    }
}

impl DropdownMenu for SelectTrigger {}

impl RenderOnce for SelectTrigger {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let palette = *cx.palette();
        let radius = Radius::Lg.px(cx);
        let focus_handle = window
            .use_keyed_state(self.id.clone(), cx, |_, cx| cx.focus_handle())
            .read(cx)
            .clone();
        // El clic no mueve el foco: un disparador con foco lo recibió del
        // teclado, que es cuando el original pinta el anillo.
        let focused = !self.disabled && focus_handle.is_focused(window);
        let width = self.width;

        self.base
            .role(Role::ComboBox)
            .disabled(self.disabled)
            .selected(self.open)
            .track_focus(&focus_handle)
            .accessibility_label(self.label.clone())
            .aria_expanded(self.open)
            .relative()
            .w_full()
            .h_7()
            .gap_2()
            .justify_between()
            .rounded(radius)
            .border_1()
            .border_color(palette.input)
            // Señal: como el input, fondo base y borde `border-control`.
            .bg(palette.bg_base)
            // `pl-[calc(--spacing(3)-1px)] pr-8`; el chevron va a 0.65rem del borde.
            .pl(rems(11. / 16.))
            .pr(rems(0.65))
            .text_size(rems(13. / 16.))
            .text_color(palette.foreground)
            .when(!self.disabled, |this| {
                this.cursor_pointer()
                    .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
            })
            .when(self.disabled, |this| this.opacity(DISABLED_OPACITY))
            .on_prepaint(move |bounds, _, _| width.set(bounds.size.width))
            .child(div().flex_1().min_w_0().truncate().child(self.label))
            .child(
                Icon::new(Lucide::ChevronDown)
                    .with_size(Size::Size(px(16.)))
                    .text_color(palette.muted_foreground),
            )
            .when(focused, |this| this.child(input_focus_overlay(radius, cx)))
    }
}
