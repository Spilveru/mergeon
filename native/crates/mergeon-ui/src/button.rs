//! Botón de Mergeon (`src/components/ui/button/Button.vue`, receta
//! `buttonVariants` de `src/lib/ui/variants.js`).
//!
//! El comportamiento (foco, Enter/Espacio, rol accesible, bloqueo al
//! deshabilitar) es el de `gpui_kit::base::Button`; aquí solo vive la
//! presentación. El `Button` de `gpui_kit::component` no sirve de base porque
//! fija por tamaño alturas, separaciones y cuerpo de texto distintos a los de la
//! receta (`h-6/7/8/9`, `gap-1/1.5/2`, `text-xs/sm`).

use std::rc::Rc;

use gpui_kit::base::{Button as BaseButton, Selectable};
use gpui_kit::component::menu::DropdownMenu;
use gpui_kit::component::{
    ActiveTheme as _, Disableable, Icon, Sizable as _, Size, StyledExt as _,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, BoxShadow, ClickEvent, ElementId, Hsla, InteractiveElement, Interactivity,
    IntoElement, KeyBinding, MouseButton, NoAction, ParentElement, RenderOnce, SharedString,
    StatefulInteractiveElement as _, StyleRefinement, Styled, Window, black, div, rems,
    transparent_black, white,
};

use crate::spinner::{Spinner, SpinnerSize};
use crate::style::{DISABLED_OPACITY, Radius, control_shadow, focus_ring_overlay};
use crate::theme::{ActivePaletteExt as _, with_alpha};

/// Contexto de teclado de los botones de Mergeon. Suprime el `Enter → Confirm`
/// de un diálogo padre para que Enter active el botón enfocado en vez de
/// disparar la acción por defecto del diálogo.
pub(crate) const BUTTON_KEY_CONTEXT: &str = "MergeonButton";

pub(crate) fn init(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("enter", NoAction, Some(BUTTON_KEY_CONTEXT))]);
}

/// Variantes de `buttonVariants`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ButtonVariant {
    /// Acción principal: fondo `--primary`. En Vue también `primary`.
    #[default]
    Default,
    /// Fondo `--secondary`, sin borde.
    Secondary,
    /// Borde `--input` sobre fondo de popover. La variante de casi toda la app.
    Outline,
    /// Sin fondo hasta el hover. En Vue también `tertiary`.
    Ghost,
    /// Texto que se subraya al pasar el cursor.
    Link,
    /// Acción destructiva: fondo `--destructive`. En Vue también `danger`.
    Destructive,
    /// Como `Outline` con texto destructivo. En Vue también `danger-soft`.
    DestructiveOutline,
}

impl ButtonVariant {
    /// Traduce los nombres que acepta `Button.vue` (incluidos los históricos de
    /// HeroUI); un nombre desconocido cae en `Default`, igual que allí.
    pub fn parse(name: &str) -> Self {
        match name {
            "secondary" => Self::Secondary,
            "outline" => Self::Outline,
            "ghost" | "tertiary" => Self::Ghost,
            "link" => Self::Link,
            "destructive" | "danger" => Self::Destructive,
            "destructive-outline" | "danger-soft" => Self::DestructiveOutline,
            _ => Self::Default,
        }
    }
}

/// Tamaños de `buttonVariants`. Un botón sin etiqueta usa el cuadrado
/// equivalente (`icon-xs`, `icon-sm`, `icon`, `icon-lg`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ButtonSize {
    /// `h-6`, `text-xs`.
    Xs,
    /// `h-7`.
    Sm,
    /// `h-8`. En Vue también `md`.
    #[default]
    Default,
    /// `h-9`.
    Lg,
}

impl ButtonSize {
    /// Traduce `xs | sm | default | md | lg`; lo demás cae en `Default`.
    pub fn parse(name: &str) -> Self {
        match name {
            "xs" => Self::Xs,
            "sm" => Self::Sm,
            "lg" => Self::Lg,
            _ => Self::Default,
        }
    }

    /// Alto en unidades de Tailwind (1 unidad = 0.25rem). Señal: botón de 28px
    /// (sm 24, lg 32).
    fn height(self) -> f32 {
        match self {
            Self::Xs => 5.5,
            Self::Sm => 6.,
            Self::Default => 7.,
            Self::Lg => 8.,
        }
    }

    /// Padding horizontal (12px, sm 8px); el borde de 1px completa el espaciado.
    fn padding_x(self) -> f32 {
        match self {
            Self::Xs => 6.,
            Self::Sm => 7.,
            Self::Default => 11.,
            Self::Lg => 13.,
        }
    }

    fn gap(self) -> f32 {
        match self {
            Self::Xs => 4.,
            Self::Sm | Self::Default => 6.,
            Self::Lg => 8.,
        }
    }

    fn radius(self) -> Radius {
        match self {
            Self::Xs => Radius::Md,
            _ => Radius::Lg,
        }
    }

    fn icon_size(self) -> Size {
        match self {
            Self::Xs => Size::Small,
            _ => Size::Medium,
        }
    }
}

type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// Botón de Mergeon. Se crea con [`button`].
#[derive(IntoElement)]
pub struct Button {
    id: ElementId,
    base: BaseButton,
    variant: ButtonVariant,
    size: ButtonSize,
    full_width: bool,
    disabled: bool,
    loading: bool,
    selected: bool,
    label: Option<SharedString>,
    icon: Option<Icon>,
    trailing_icon: Option<Icon>,
    children: Vec<AnyElement>,
    tooltip: Option<SharedString>,
    on_click: Option<ClickHandler>,
}

/// Crea un botón con la variante `Default` (primaria) y tamaño `Default`.
///
/// ```ignore
/// mergeon_ui::button("fetch")
///     .outline()
///     .sm()
///     .icon(Lucide::Download)
///     .label("Fetch")
///     .on_click(cx.listener(|this, _, window, cx| this.fetch(window, cx)))
/// ```
pub fn button(id: impl Into<ElementId>) -> Button {
    Button::new(id)
}

impl Button {
    pub fn new(id: impl Into<ElementId>) -> Self {
        let id = id.into();
        Self {
            base: BaseButton::new(id.clone()),
            id,
            variant: ButtonVariant::default(),
            size: ButtonSize::default(),
            full_width: false,
            disabled: false,
            loading: false,
            selected: false,
            label: None,
            icon: None,
            trailing_icon: None,
            children: Vec::new(),
            tooltip: None,
            on_click: None,
        }
    }

    pub fn variant(mut self, variant: ButtonVariant) -> Self {
        self.variant = variant;
        self
    }

    pub fn secondary(self) -> Self {
        self.variant(ButtonVariant::Secondary)
    }

    pub fn outline(self) -> Self {
        self.variant(ButtonVariant::Outline)
    }

    pub fn ghost(self) -> Self {
        self.variant(ButtonVariant::Ghost)
    }

    pub fn link(self) -> Self {
        self.variant(ButtonVariant::Link)
    }

    pub fn destructive(self) -> Self {
        self.variant(ButtonVariant::Destructive)
    }

    pub fn destructive_outline(self) -> Self {
        self.variant(ButtonVariant::DestructiveOutline)
    }

    pub fn size(mut self, size: ButtonSize) -> Self {
        self.size = size;
        self
    }

    pub fn xs(self) -> Self {
        self.size(ButtonSize::Xs)
    }

    pub fn sm(self) -> Self {
        self.size(ButtonSize::Sm)
    }

    pub fn lg(self) -> Self {
        self.size(ButtonSize::Lg)
    }

    /// Texto del botón. Sin etiqueta ni hijos el botón es cuadrado (`icon-*`).
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Ícono a la izquierda de la etiqueta (16px; 14px en `Xs`).
    pub fn icon(mut self, icon: impl Into<Icon>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    /// Ícono a la derecha de la etiqueta (p. ej. el chevron de un desplegable).
    pub fn trailing_icon(mut self, icon: impl Into<Icon>) -> Self {
        self.trailing_icon = Some(icon.into());
        self
    }

    /// `fullWidth`: ocupa todo el ancho del contenedor.
    pub fn full_width(mut self, full_width: bool) -> Self {
        self.full_width = full_width;
        self
    }

    /// Muestra el spinner y bloquea el botón, como `:loading` en Vue.
    pub fn loading(mut self, loading: bool) -> Self {
        self.loading = loading;
        self
    }

    /// Texto de ayuda al pasar el cursor (el `title` del original).
    pub fn tooltip(mut self, tooltip: impl Into<SharedString>) -> Self {
        self.tooltip = Some(tooltip.into());
        self
    }

    /// Acción al activar con el puntero, Enter o Espacio.
    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }

    /// Deshabilita el botón: no responde y se atenúa (`disabled:opacity-64`).
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Mantiene el aspecto de hover, p. ej. mientras el menú que abre el botón
    /// está desplegado.
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    fn is_icon_only(&self) -> bool {
        self.label.is_none() && self.children.is_empty()
    }
}

impl Disableable for Button {
    fn disabled(self, disabled: bool) -> Self {
        Button::disabled(self, disabled)
    }
}

impl Selectable for Button {
    fn selected(self, selected: bool) -> Self {
        Button::selected(self, selected)
    }

    fn is_selected(&self) -> bool {
        self.selected
    }
}

impl Styled for Button {
    fn style(&mut self) -> &mut StyleRefinement {
        self.base.style()
    }
}

impl ParentElement for Button {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl InteractiveElement for Button {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.base.interactivity()
    }
}

impl DropdownMenu for Button {}

/// Colores y sombras de una variante en el tema activo.
struct Recipe {
    bg: Hsla,
    border: Hsla,
    fg: Hsla,
    icon: Hsla,
    shadow: Vec<BoxShadow>,
    hover_bg: Hsla,
    hover_border: Hsla,
    active_bg: Hsla,
    active_shadow: Vec<BoxShadow>,
    hover_underline: bool,
}

impl ButtonVariant {
    /// Recetas de Señal: primario naranja con texto tinta (nunca blanco),
    /// secundario de fondo base con `border-base`, fantasma, peligro rojo con
    /// texto blanco y peligro-outline. Al presionar desaparece la sombra.
    fn recipe(self, cx: &App) -> Recipe {
        let palette = *cx.palette();
        let dark = cx.theme().mode.is_dark();
        let clear = transparent_black();
        let shadow_1 = control_shadow(black().opacity(if dark { 0.3 } else { 0.04 }));

        let outline = |fg: Hsla, icon: Hsla, hover_bg: Hsla, hover_border: Hsla| Recipe {
            bg: palette.bg_base.into(),
            border: palette.border_base.into(),
            fg,
            icon,
            shadow: shadow_1.clone(),
            hover_bg,
            hover_border,
            active_bg: palette.bg_pressed.into(),
            active_shadow: Vec::new(),
            hover_underline: false,
        };

        match self {
            Self::Default => Recipe {
                bg: palette.primary.into(),
                border: palette.primary_active.into(),
                fg: palette.primary_foreground.into(),
                icon: palette.primary_foreground.into(),
                shadow: shadow_1.clone(),
                hover_bg: palette.primary_hover.into(),
                hover_border: palette.primary_active.into(),
                active_bg: palette.primary_active.into(),
                active_shadow: Vec::new(),
                hover_underline: false,
            },
            Self::Destructive => Recipe {
                bg: palette.destructive.into(),
                border: palette.destructive_hover.into(),
                fg: white(),
                icon: white(),
                shadow: shadow_1.clone(),
                hover_bg: palette.destructive_hover.into(),
                hover_border: palette.destructive_hover.into(),
                active_bg: palette.destructive_hover.into(),
                active_shadow: Vec::new(),
                hover_underline: false,
            },
            Self::Outline => outline(
                palette.text_1.into(),
                palette.text_2.into(),
                palette.bg_hover.into(),
                palette.border_base.into(),
            ),
            Self::DestructiveOutline => outline(
                palette.destructive_foreground.into(),
                palette.destructive_foreground.into(),
                palette.destructive_soft.into(),
                with_alpha(palette.destructive, 0.5),
            ),
            Self::Secondary => Recipe {
                bg: palette.bg_hover.into(),
                border: clear,
                fg: palette.text_1.into(),
                icon: palette.text_2.into(),
                shadow: Vec::new(),
                hover_bg: palette.bg_pressed.into(),
                hover_border: clear,
                active_bg: palette.bg_pressed.into(),
                active_shadow: Vec::new(),
                hover_underline: false,
            },
            Self::Ghost => Recipe {
                bg: clear,
                border: clear,
                fg: palette.text_1.into(),
                icon: palette.text_2.into(),
                shadow: Vec::new(),
                hover_bg: palette.bg_hover.into(),
                hover_border: clear,
                active_bg: palette.bg_pressed.into(),
                active_shadow: Vec::new(),
                hover_underline: false,
            },
            Self::Link => Recipe {
                bg: clear,
                border: clear,
                fg: palette.primary_text.into(),
                icon: palette.primary_text.into(),
                shadow: Vec::new(),
                hover_bg: clear,
                hover_border: clear,
                active_bg: clear,
                active_shadow: Vec::new(),
                hover_underline: true,
            },
        }
    }
}

impl RenderOnce for Button {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let icon_only = self.is_icon_only();
        let inert = self.disabled || self.loading;
        let size = self.size;
        let recipe = self.variant.recipe(cx);
        let radius = size.radius().px(cx);

        let focus_handle = window
            .use_keyed_state(self.id.clone(), cx, |_, cx| cx.focus_handle())
            .read(cx)
            .clone();
        // El clic no mueve el foco (ver `on_mouse_down`), así que un botón con
        // foco lo recibió del teclado: equivale a `:focus-visible`.
        let focused = !inert && focus_handle.is_focused(window);

        let mut base = self.base;
        // La receta va primero y el estilo de quien llama la refina después.
        let instance_style = std::mem::take(base.style());

        // `[&_svg]:-mx-0.5`: el ícono invade 2px a cada lado de su hueco. Un
        // margen negativo hace que GPUI mida mal el ancho del botón, así que el
        // hueco se estrecha y el ícono lo desborda centrado.
        let icon_px = match size {
            ButtonSize::Xs => 14.,
            _ => 16.,
        };
        let icon_color = if self.disabled && !self.loading {
            cx.palette().text_disabled.into()
        } else {
            recipe.icon
        };
        let icon = |icon: Icon| {
            div()
                .flex()
                .flex_shrink_0()
                .items_center()
                .justify_center()
                .h(rems(icon_px / 16.))
                .w(rems((icon_px - if icon_only { 0. } else { 4. }) / 16.))
                .child(icon.with_size(size.icon_size()).text_color(icon_color))
        };

        base.disabled(inert)
            .selected(self.selected)
            .track_focus(&focus_handle)
            .relative()
            .flex_shrink_0()
            .h(rems(size.height() * 0.25))
            .map(|this| {
                if icon_only {
                    this.w(rems(size.height() * 0.25))
                } else {
                    this.px(rems(size.padding_x() / 16.))
                }
            })
            .when(self.full_width, |this| this.w_full())
            .gap(rems(size.gap() / 16.))
            .rounded(radius)
            .border_1()
            .border_color(recipe.border)
            .bg(recipe.bg)
            .text_color(recipe.fg)
            // Señal: el primario va en 600; el resto, en 500.
            .font_weight(if self.variant == ButtonVariant::Default {
                gpui_kit::FontWeight::SEMIBOLD
            } else {
                gpui_kit::FontWeight::MEDIUM
            })
            .map(|this| match size {
                ButtonSize::Xs => this.text_xs(),
                _ => this.text_size(rems(13. / 16.)),
            })
            .whitespace_nowrap()
            .when(!inert, |this| {
                this.key_context(BUTTON_KEY_CONTEXT)
                    .cursor_pointer()
                    .shadow(recipe.shadow.clone())
                    .hover(|style| {
                        let style = style.bg(recipe.hover_bg).border_color(recipe.hover_border);
                        if recipe.hover_underline {
                            style.underline()
                        } else {
                            style
                        }
                    })
                    .active(|style| {
                        style
                            .bg(recipe.active_bg)
                            .border_color(recipe.hover_border)
                            .shadow(recipe.active_shadow.clone())
                    })
                    .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
            })
            // Señal: deshabilitado = `bg-inset` + `text-disabled`, sin sombra
            // (no un primario desvaído). Cargando conserva sus colores.
            .when(self.disabled && !self.loading, |this| {
                let palette = cx.palette();
                this.bg(palette.bg_inset)
                    .border_color(palette.border)
                    .text_color(palette.text_disabled)
            })
            .when(self.loading, |this| this.opacity(DISABLED_OPACITY))
            .styles(|styles| {
                styles.selected(|style| style.bg(recipe.hover_bg).border_color(recipe.hover_border))
            })
            .when_some(self.on_click, |this, on_click| {
                this.on_click(move |event, window, cx| on_click(event, window, cx))
            })
            .when_some(self.tooltip, |this, tooltip| {
                this.tooltip(crate::hover_card::tooltip(tooltip))
            })
            .when(self.loading, |this| {
                this.child(Spinner::new().size(SpinnerSize::Sm).color_value(recipe.fg))
            })
            .when_some(self.icon, |this, leading| this.child(icon(leading)))
            .when_some(self.label, |this, label| this.child(div().child(label)))
            .children(self.children)
            .when_some(self.trailing_icon, |this, trailing| {
                this.child(icon(trailing))
            })
            .when(focused, |this| this.child(focus_ring_overlay(radius, cx)))
            .refine_style(&instance_style)
    }
}
