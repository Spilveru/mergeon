//! Campos de texto (`src/components/ui/input/Input.vue`,
//! `src/components/ui/textarea/Textarea.vue`, receta `inputVariants`).
//!
//! La edición es la de gpui-kit (`Input` / `Textarea` sobre `InputState` /
//! `TextareaState`, que guarda la vista); aquí se ajusta la superficie a la
//! receta y se dibuja el foco de Mergeon (`ring-[3px] ring-ring/24`), más tenue
//! que el anillo de gpui-kit.

use gpui_kit::component::input::{Input, InputState, Textarea, TextareaState};
use gpui_kit::component::{Disableable, StyledExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Entity, FocusHandle, Focusable as _, IntoElement, ParentElement as _,
    RenderOnce, StyleRefinement, Styled, Window, div, rems,
};

use crate::style::{Radius, input_focus_overlay};

/// `leading-relaxed` sobre `text-sm`: 14px × 1.625.
const TEXTAREA_LINE: f32 = 22.75;
use crate::theme::ActivePaletteExt as _;

enum Field {
    Input(Entity<InputState>),
    Textarea(Entity<TextareaState>),
}

/// Campo de texto de Mergeon. Se crea con [`input`] o [`textarea`].
#[derive(IntoElement)]
pub struct TextField {
    field: Field,
    disabled: bool,
    readonly: bool,
    cleanable: bool,
    mask_toggle: bool,
    rows: usize,
    prefix: Option<AnyElement>,
    suffix: Option<AnyElement>,
    style: StyleRefinement,
}

/// Campo de una línea sobre un `InputState` que guarda la vista.
///
/// ```ignore
/// // en `new`: cx.new(|cx| InputState::new(window, cx).placeholder("Branch name"))
/// mergeon_ui::input(&self.branch_name)
/// ```
pub fn input(state: &Entity<InputState>) -> TextField {
    TextField::new(Field::Input(state.clone()))
}

/// Área de texto sobre un `TextareaState` que guarda la vista. Mide cuatro
/// filas, como el original; se cambia con [`TextField::rows`].
///
/// ```ignore
/// // en `new`: cx.new(|cx| TextareaState::new(window, cx).placeholder("Commit message"))
/// mergeon_ui::textarea(&self.message).rows(6)
/// ```
pub fn textarea(state: &Entity<TextareaState>) -> TextField {
    TextField::new(Field::Textarea(state.clone()))
}

impl TextField {
    fn new(field: Field) -> Self {
        Self {
            field,
            disabled: false,
            readonly: false,
            cleanable: false,
            mask_toggle: false,
            rows: 4,
            prefix: None,
            suffix: None,
            style: StyleRefinement::default(),
        }
    }

    /// Filas visibles de un área de texto (`rows`, 4 por defecto). El alto
    /// nunca baja de los 96px del original; el texto que no cabe hace scroll.
    pub fn rows(mut self, rows: usize) -> Self {
        self.rows = rows.max(1);
        self
    }

    /// Deshabilitado: no se puede enfocar ni editar.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Solo lectura: se puede enfocar, seleccionar y copiar, pero no editar.
    pub fn readonly(mut self, readonly: bool) -> Self {
        self.readonly = readonly;
        self
    }

    /// Botón para vaciar el campo cuando tiene texto (solo una línea).
    pub fn cleanable(mut self, cleanable: bool) -> Self {
        self.cleanable = cleanable;
        self
    }

    /// Botón para mostrar u ocultar una contraseña (solo una línea).
    pub fn mask_toggle(mut self) -> Self {
        self.mask_toggle = true;
        self
    }

    /// Elemento al inicio del campo, p. ej. un ícono de búsqueda (solo una línea).
    pub fn prefix(mut self, prefix: impl IntoElement) -> Self {
        self.prefix = Some(prefix.into_any_element());
        self
    }

    /// Elemento al final del campo (solo una línea).
    pub fn suffix(mut self, suffix: impl IntoElement) -> Self {
        self.suffix = Some(suffix.into_any_element());
        self
    }

    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match &self.field {
            Field::Input(state) => state.read(cx).focus_handle(cx),
            Field::Textarea(state) => state.read(cx).focus_handle(cx),
        }
    }
}

impl Disableable for TextField {
    fn disabled(self, disabled: bool) -> Self {
        TextField::disabled(self, disabled)
    }
}

/// El estilo externo se aplica al contenedor (ancho, márgenes). El campo ocupa
/// todo su ancho, como `fullWidth` en el original.
impl Styled for TextField {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for TextField {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let palette = *cx.palette();
        let radius = Radius::Lg.px(cx);
        let focused = !self.disabled && self.focus_handle(cx).contains_focused(window, cx);
        // Señal: fondo base y borde `border-control`, sin sombra.
        let background = palette.bg_base;

        let field = match self.field {
            Field::Input(state) => Input::new(&state)
                // El foco lo dibuja `input_focus_overlay`.
                .focus_bordered(false)
                .disabled(self.disabled)
                .readonly(self.readonly)
                .cleanable(self.cleanable)
                .when(self.mask_toggle, |this| this.mask_toggle())
                .when_some(self.prefix, |this, prefix| this.prefix(prefix))
                .when_some(self.suffix, |this, suffix| this.suffix(suffix))
                // `px-[calc(--spacing(3)-1px)]`
                .px(rems(11. / 16.))
                .bg(background)
                .rounded(radius)
                .into_any_element(),
            Field::Textarea(state) => Textarea::new(&state)
                .disabled(self.disabled)
                .readonly(self.readonly)
                // `rows` líneas `leading-relaxed` + `py-2` + el borde, con el
                // `min-h-[96px]` de la receta.
                .h(rems(
                    (self.rows as f32 * TEXTAREA_LINE + 18.).max(96.) / 16.,
                ))
                .line_height(rems(TEXTAREA_LINE / 16.))
                .bg(background)
                .rounded(radius)
                .into_any_element(),
        };

        // gpui-kit ya atenúa el campo deshabilitado (texto apagado y fondo a la
        // mitad); sumarle `opacity-64` lo dejaría casi invisible.
        div()
            .relative()
            .w_full()
            .min_w_0()
            .refine_style(&self.style)
            .child(field)
            .when(focused, |this| this.child(input_focus_overlay(radius, cx)))
    }
}
