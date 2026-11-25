//! Grupo de opciones (`src/components/ui/radio-group/RadioGroup.vue`): filas
//! con borde, punto, etiqueta y descripción opcional.
//!
//! El `RadioGroup` de gpui-kit es una lista de radios sueltos; aquí cada opción
//! es una tarjeta, así que la presentación es propia sobre el `Radio` de
//! `gpui_kit::base` (activación, foco y rol accesible).

use std::rc::Rc;

use gpui_kit::base::{Radio as BaseRadio, RadioGroup as BaseRadioGroup};
use gpui_kit::component::StyledExt as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, ElementId, FocusHandle, InteractiveElement as _, IntoElement, KeyDownEvent,
    ParentElement as _, RenderOnce, SharedString, StatefulInteractiveElement as _, StyleRefinement,
    Styled, Window, div, rems,
};

use crate::style::{DISABLED_OPACITY, MergeonStyled as _, Radius, focus_ring_overlay};
use crate::theme::ActivePaletteExt as _;

/// Una opción: `{ value, label, description?, disabled? }`.
#[derive(Clone)]
pub struct RadioOption {
    value: SharedString,
    label: SharedString,
    description: Option<SharedString>,
    disabled: bool,
}

impl RadioOption {
    pub fn new(value: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
            description: None,
            disabled: false,
        }
    }

    /// Segunda línea en `text-xs` apagado.
    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn value(&self) -> &SharedString {
        &self.value
    }
}

/// Una cadena es una opción cuyo valor y etiqueta coinciden.
impl<T: Into<SharedString>> From<T> for RadioOption {
    fn from(value: T) -> Self {
        let value: SharedString = value.into();
        Self::new(value.clone(), value)
    }
}

type ChangeHandler = Rc<dyn Fn(&SharedString, &mut Window, &mut App)>;

/// Grupo de opciones de Mergeon. Se crea con [`radio_group`].
#[derive(IntoElement)]
pub struct RadioGroup {
    id: ElementId,
    options: Vec<RadioOption>,
    selected: Option<SharedString>,
    disabled: bool,
    label: Option<SharedString>,
    on_change: Option<ChangeHandler>,
    style: StyleRefinement,
}

/// Grupo controlado: se le pasa el valor seleccionado y avisa del cambio con
/// `on_change`. El grupo es una sola parada de tabulación; dentro se navega con
/// las flechas.
///
/// ```ignore
/// mergeon_ui::radio_group("merge-mode")
///     .option(RadioOption::new("merge", "Merge").description("Create a merge commit"))
///     .option(RadioOption::new("rebase", "Rebase"))
///     .selected(self.mode.clone())
///     .on_change(cx.listener(|this, value: &SharedString, _, cx| {
///         this.mode = value.clone();
///         cx.notify();
///     }))
/// ```
pub fn radio_group(id: impl Into<ElementId>) -> RadioGroup {
    RadioGroup {
        id: id.into(),
        options: Vec::new(),
        selected: None,
        disabled: false,
        label: None,
        on_change: None,
        style: StyleRefinement::default(),
    }
}

impl RadioGroup {
    pub fn option(mut self, option: impl Into<RadioOption>) -> Self {
        self.options.push(option.into());
        self
    }

    pub fn options<T: Into<RadioOption>>(mut self, options: impl IntoIterator<Item = T>) -> Self {
        self.options.extend(options.into_iter().map(Into::into));
        self
    }

    /// Valor seleccionado (`modelValue`).
    pub fn selected(mut self, value: impl Into<SharedString>) -> Self {
        self.selected = Some(value.into());
        self
    }

    /// Deshabilita todas las opciones.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Nombre accesible del grupo (`aria-label`).
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
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

impl Styled for RadioGroup {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

/// Índice de la opción a la que salta una flecha: la siguiente o la anterior
/// habilitada, dando la vuelta. `None` si no hay ninguna habilitada.
pub(crate) fn next_enabled(
    enabled: &[bool],
    current: Option<usize>,
    forward: bool,
) -> Option<usize> {
    let indexes: Vec<usize> = enabled
        .iter()
        .enumerate()
        .filter_map(|(index, enabled)| enabled.then_some(index))
        .collect();
    if indexes.is_empty() {
        return None;
    }
    let position = current
        .and_then(|current| indexes.iter().position(|index| *index == current))
        .unwrap_or(0);
    let step = if forward { 1 } else { indexes.len() - 1 };
    Some(indexes[(position + step) % indexes.len()])
}

impl RenderOnce for RadioGroup {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let palette = *cx.palette();
        let radius = Radius::Lg.px(cx);
        let group_disabled = self.disabled;
        let count = self.options.len();

        let enabled: Vec<bool> = self
            .options
            .iter()
            .map(|option| !(group_disabled || option.disabled))
            .collect();
        let selected_index = self.selected.as_ref().and_then(|value| {
            self.options
                .iter()
                .position(|option| &option.value == value)
        });
        // La parada de tabulación es la opción seleccionada o, sin selección, la
        // primera habilitada.
        let tab_stop = selected_index
            .filter(|index| enabled[*index])
            .or_else(|| enabled.iter().position(|enabled| *enabled));

        let handles: Vec<FocusHandle> = self
            .options
            .iter()
            .map(|option| {
                window
                    .use_keyed_state(
                        ElementId::Name(format!("{:?}-{}", self.id, option.value).into()),
                        cx,
                        |_, cx| cx.focus_handle(),
                    )
                    .read(cx)
                    .clone()
            })
            .collect();

        let radios = self
            .options
            .iter()
            .enumerate()
            .map(|(index, option)| {
                let checked = selected_index == Some(index);
                let disabled = !enabled[index];
                let focus_handle = handles[index].clone();
                let focused = !disabled && focus_handle.is_focused(window);
                let on_change = self.on_change.clone();
                let value = option.value.clone();

                BaseRadio::new(ElementId::Name(format!("radio-{}", option.value).into()))
                    .checked(checked)
                    .disabled(disabled)
                    .track_focus(&focus_handle)
                    .tab_stop(tab_stop == Some(index))
                    .set_position(index + 1, count)
                    .accessibility_label(option.label.clone())
                    .relative()
                    .flex()
                    .items_center()
                    .w_full()
                    .gap(rems(0.625))
                    .rounded(radius)
                    .border_1()
                    .px_3()
                    .py_2()
                    .tw_text_sm()
                    .text_color(palette.foreground)
                    .border_color(palette.border_base)
                    .bg(palette.bg_base)
                    .when(!disabled, |this| {
                        this.cursor_pointer().when(!checked, |this| {
                            this.hover(|style| style.bg(palette.accent))
                        })
                    })
                    .styles(|styles| {
                        styles
                            .checked(|style| {
                                // Señal: la opción elegida, con borde en tinta.
                                style.border_color(palette.border_ink).bg(palette.bg_inset)
                            })
                            .disabled(|style| style.opacity(DISABLED_OPACITY))
                    })
                    .when_some(on_change, |this, on_change| {
                        this.on_change(move |_, _, window, cx| on_change(&value, window, cx))
                    })
                    .child(
                        div()
                            .flex()
                            .flex_shrink_0()
                            .items_center()
                            .justify_center()
                            .size_4()
                            .rounded_full()
                            .border_1()
                            .map(|dot| {
                                if checked {
                                    dot.border_color(palette.border_ink)
                                        .bg(palette.border_ink)
                                        .child(
                                            div()
                                                .size(rems(0.375))
                                                .rounded_full()
                                                .bg(palette.bg_base),
                                        )
                                } else {
                                    dot.border_color(palette.border_control).bg(palette.bg_base)
                                }
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .min_w_0()
                            .child(div().truncate().child(option.label.clone()))
                            .when_some(option.description.clone(), |this, description| {
                                this.child(
                                    div()
                                        .truncate()
                                        .tw_text_xs()
                                        .text_color(palette.muted_foreground)
                                        .child(description),
                                )
                            }),
                    )
                    .when(focused, |this| this.child(focus_ring_overlay(radius, cx)))
            })
            .collect::<Vec<_>>();

        let values: Vec<SharedString> = self.options.iter().map(|o| o.value.clone()).collect();
        let on_change = self.on_change.clone();

        BaseRadioGroup::new(self.id)
            .flex()
            .flex_col()
            .gap(rems(0.375))
            .when_some(self.label, |this, label| this.aria_label(label))
            .on_key_down(move |event: &KeyDownEvent, window, cx| {
                let forward = match event.keystroke.key.as_str() {
                    "down" | "right" => true,
                    "up" | "left" => false,
                    _ => return,
                };
                cx.stop_propagation();
                let Some(next) = next_enabled(&enabled, selected_index, forward) else {
                    return;
                };
                if let Some(on_change) = &on_change {
                    on_change(&values[next], window, cx);
                }
                handles[next].focus(window, cx);
            })
            .refine_style(&self.style)
            .children(radios)
    }
}

#[cfg(test)]
mod tests {
    use super::next_enabled;

    #[test]
    fn arrows_skip_disabled_options_and_wrap_around() {
        let enabled = [true, false, true];

        assert_eq!(next_enabled(&enabled, Some(0), true), Some(2));
        assert_eq!(next_enabled(&enabled, Some(2), true), Some(0));
        assert_eq!(next_enabled(&enabled, Some(0), false), Some(2));
    }

    #[test]
    fn without_a_selection_the_arrows_start_from_the_first_enabled_option() {
        let enabled = [false, true, true];

        assert_eq!(next_enabled(&enabled, None, true), Some(2));
        assert_eq!(next_enabled(&enabled, None, false), Some(2));
    }

    #[test]
    fn a_fully_disabled_group_has_nowhere_to_go() {
        assert_eq!(next_enabled(&[false, false], Some(0), true), None);
    }
}
