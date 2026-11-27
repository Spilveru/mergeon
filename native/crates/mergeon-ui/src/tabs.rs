//! Control segmentado (`src/components/ui/tabs/Tabs.vue`, `tabsListClass` /
//! `tabClass`): lista sobre superficie apagada y pestaña seleccionada elevada.
//!
//! El `TabBar` segmentado de gpui-kit fija alturas (24/18px) y cuerpos de texto
//! distintos a los de la receta (26px, `text-xs`), así que la presentación es
//! propia sobre los primitivos de `gpui_kit::base`.

use std::rc::Rc;

use gpui_kit::base::{Button as BaseButton, Tabs as BaseTabs};
use gpui_kit::component::{Icon, Sizable as _, Size, StyledExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, ElementId, FontWeight, InteractiveElement as _, IntoElement, MouseButton,
    ParentElement as _, RenderOnce, Role, SharedString, StatefulInteractiveElement as _,
    StyleRefinement, Styled, Window, div, px, rems, transparent_black,
};

use crate::style::{DISABLED_OPACITY, MergeonStyled as _, Radius, focus_ring_overlay};
use crate::theme::ActivePaletteExt as _;

/// Una pestaña: `{ key, label, disabled?, icon? }`.
#[derive(Clone)]
pub struct TabItem {
    key: SharedString,
    label: SharedString,
    icon: Option<Icon>,
    disabled: bool,
}

impl TabItem {
    pub fn new(key: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            icon: None,
            disabled: false,
        }
    }

    /// Ícono de 14px antes de la etiqueta.
    pub fn icon(mut self, icon: impl Into<Icon>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn key(&self) -> &SharedString {
        &self.key
    }
}

type ChangeHandler = Rc<dyn Fn(&SharedString, &mut Window, &mut App)>;

/// Control segmentado de Mergeon. Se crea con [`tabs`].
#[derive(IntoElement)]
pub struct Tabs {
    id: ElementId,
    items: Vec<TabItem>,
    selected: Option<SharedString>,
    full_width: bool,
    on_change: Option<ChangeHandler>,
    style: StyleRefinement,
}

/// Control segmentado controlado: se le pasa la clave seleccionada y avisa del
/// cambio con `on_change`.
///
/// ```ignore
/// mergeon_ui::tabs("commit-tabs")
///     .item(TabItem::new("changes", "Changes"))
///     .item(TabItem::new("tree", "Repo tree").icon(Lucide::FolderTree))
///     .selected(self.tab.clone())
///     .on_change(cx.listener(|this, key: &SharedString, _, cx| {
///         this.tab = key.clone();
///         cx.notify();
///     }))
/// ```
pub fn tabs(id: impl Into<ElementId>) -> Tabs {
    Tabs {
        id: id.into(),
        items: Vec::new(),
        selected: None,
        full_width: false,
        on_change: None,
        style: StyleRefinement::default(),
    }
}

impl Tabs {
    pub fn item(mut self, item: TabItem) -> Self {
        self.items.push(item);
        self
    }

    pub fn items(mut self, items: impl IntoIterator<Item = TabItem>) -> Self {
        self.items.extend(items);
        self
    }

    /// Clave de la pestaña seleccionada (`modelValue`).
    pub fn selected(mut self, key: impl Into<SharedString>) -> Self {
        self.selected = Some(key.into());
        self
    }

    /// Reparte el ancho disponible entre las pestañas; por defecto el control
    /// mide lo que su contenido.
    pub fn full_width(mut self, full_width: bool) -> Self {
        self.full_width = full_width;
        self
    }

    pub fn on_change(
        mut self,
        handler: impl Fn(&SharedString, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_change = Some(Rc::new(handler));
        self
    }
}

impl Styled for Tabs {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for Tabs {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let palette = *cx.palette();
        let radius = Radius::Xs.px(cx);
        let full_width = self.full_width;
        let count = self.items.len();

        let tabs = self.items.into_iter().enumerate().map(|(index, item)| {
            let selected = self.selected.as_ref() == Some(&item.key);
            let id = ElementId::Name(format!("tab-{}", item.key).into());
            let focus_handle = window
                .use_keyed_state(id.clone(), cx, |_, cx| cx.focus_handle())
                .read(cx)
                .clone();
            let focused = !item.disabled && focus_handle.is_focused(window);
            let on_change = self.on_change.clone();
            let key = item.key.clone();

            BaseButton::new(id)
                .role(Role::Tab)
                .disabled(item.disabled)
                .selected(selected)
                .track_focus(&focus_handle)
                .aria_selected(selected)
                .aria_position_in_set(index + 1)
                .aria_size_of_set(count)
                .relative()
                .when(full_width, |this| this.flex_1())
                .h(px(22.))
                .px(px(10.))
                .gap(rems(0.375))
                .rounded(radius)
                // Borde transparente: mantiene la caja y da sitio al anillo de foco.
                .border_1()
                .border_color(transparent_black())
                .tw_text_xs()
                .font_weight(FontWeight::MEDIUM)
                .whitespace_nowrap()
                .text_color(palette.text_2)
                .when(!item.disabled, |this| {
                    this.key_context(crate::button::BUTTON_KEY_CONTEXT)
                        .cursor_pointer()
                        .hover(|style| style.text_color(palette.text_1).bg(palette.bg_hover))
                        .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                })
                .when(item.disabled, |this| this.opacity(DISABLED_OPACITY))
                .styles(|styles| {
                    // Señal: el segmento activo se invierte a tinta.
                    styles.selected(|style| style.bg(palette.sel_bg).text_color(palette.sel_fg))
                })
                .when_some(on_change, |this, on_change| {
                    this.on_click(move |_, window, cx| on_change(&key, window, cx))
                })
                .when_some(item.icon, |this, icon| {
                    this.child(icon.with_size(Size::Size(px(14.))))
                })
                .child(item.label)
                .when(focused, |this| this.child(focus_ring_overlay(radius, cx)))
        });
        let tabs: Vec<_> = tabs.collect();

        // `w-fit` salvo `fullWidth`.
        div()
            .flex()
            .max_w_full()
            .when(full_width, |this| this.w_full())
            .refine_style(&self.style)
            .child(
                BaseTabs::new(self.id)
                    .flex()
                    .items_center()
                    .when(full_width, |this| this.w_full())
                    .gap(px(1.))
                    .p(px(1.))
                    .radius(Radius::Lg, cx)
                    // Señal: segmentado con borde base sobre fondo base.
                    .border_1()
                    .border_color(palette.border_base)
                    .bg(palette.bg_base)
                    .children(tabs),
            )
    }
}
