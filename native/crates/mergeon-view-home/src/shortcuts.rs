//! Panel de atajos de teclado personalizables (Settings → Shortcuts).
//!
//! Muestra todos los atajos personalizables agrupados por categoría, con
//! búsqueda, grabación al hacer clic (captura modificadores + tecla, Escape
//! cancela, Backspace/Delete desvincula), avisos de conflicto y restauración
//! individual o global.

use std::collections::BTreeMap;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, Focusable, FontWeight,
    InteractiveElement as _, IntoElement, KeyDownEvent, Keystroke, ParentElement as _, Render,
    Role, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div,
    px, relative, rems,
};
use mergeon_app::AppModel;
use mergeon_model::keyboard_shortcuts::{
    CUSTOMIZABLE_SHORTCUTS, ShortcutAction, ShortcutCategory, ShortcutDefinition, combo_label,
    conflicts_for, effective_combo_for, find_conflicts, is_customized, is_mac_platform,
    normalize_combo,
};
use mergeon_ui::{ActivePaletteExt as _, InputEvent, InputState, Lucide, Radius};

/// Contexto de teclado del grabador de atajos: mientras está activo, los
/// atajos globales de la cáscara no se disparan.
pub const SHORTCUT_RECORDER_CONTEXT: &str = "ShortcutRecorder";

fn is_modifier_only_key(key: &str) -> bool {
    matches!(
        key,
        "control"
            | "ctrl"
            | "shift"
            | "alt"
            | "option"
            | "platform"
            | "meta"
            | "super"
            | "cmd"
            | "command"
            | "fn"
            | "function"
            | "capslock"
    )
}

/// Convierte un `Keystroke` de GPUI al formato de combo de `mergeon-model`
/// (`mod+shift+f`, `ctrl+shift+down`, `f5`…). Devuelve `None` si la tecla sola
/// no es un combo válido para un atajo global.
pub fn combo_from_keystroke(keystroke: &Keystroke, is_mac: bool) -> Option<String> {
    let key = keystroke.key.trim().to_ascii_lowercase();
    if key.is_empty() || is_modifier_only_key(&key) {
        return None;
    }

    let mut parts: Vec<&str> = Vec::with_capacity(5);
    if is_mac {
        if keystroke.modifiers.platform {
            parts.push("mod");
        }
        if keystroke.modifiers.control {
            parts.push("ctrl");
        }
    } else if keystroke.modifiers.control {
        parts.push("mod");
    }
    if keystroke.modifiers.alt {
        parts.push("alt");
    }
    if keystroke.modifiers.shift {
        parts.push("shift");
    }
    parts.push(&key);
    normalize_combo(&parts.join("+"))
}

fn matches_filter(
    def: &ShortcutDefinition,
    overrides: &BTreeMap<String, String>,
    query: &str,
    is_mac: bool,
) -> bool {
    let query = query.trim().to_ascii_lowercase();
    if query.is_empty() {
        return true;
    }
    if def.label.to_ascii_lowercase().contains(&query)
        || def.category.title().to_ascii_lowercase().contains(&query)
        || def.id.to_ascii_lowercase().contains(&query)
    {
        return true;
    }
    if let Some(combo) = effective_combo_for(def.action, overrides) {
        if combo.to_ascii_lowercase().contains(&query)
            || combo_label(&combo, is_mac)
                .to_ascii_lowercase()
                .contains(&query)
        {
            return true;
        }
    } else if "unassigned".contains(&query) {
        return true;
    }
    false
}

/// Panel de Settings → Shortcuts.
pub struct ShortcutsPanel {
    model: Entity<AppModel>,
    pub(crate) search: Entity<InputState>,
    recording: Option<ShortcutAction>,
    recording_error: Option<SharedString>,
    recorder_focus: FocusHandle,
    /// Interceptor activo únicamente mientras se está grabando un atajo, para
    /// capturar también combinaciones que ya están enlazadas en el keymap
    /// global (p. ej. al reasignar o provocar un conflicto).
    recording_interceptor: Option<Subscription>,
    _subscriptions: Vec<Subscription>,
}

impl Focusable for ShortcutsPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.recorder_focus.clone()
    }
}

impl ShortcutsPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let model = AppModel::global(cx);
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Filter shortcuts…"));
        let recorder_focus = cx.focus_handle();
        let subscriptions = vec![
            cx.observe(&model, |_, _, cx| cx.notify()),
            cx.on_blur(&recorder_focus, window, |this, _, cx| {
                this.cancel_recording(cx);
            }),
            cx.subscribe(&search, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.cancel_recording(cx);
                    cx.notify();
                }
            }),
        ];
        Self {
            model,
            search,
            recording: None,
            recording_error: None,
            recorder_focus,
            recording_interceptor: None,
            _subscriptions: subscriptions,
        }
    }

    /// Acción que se está grabando ahora mismo, si la hay.
    pub fn recording(&self) -> Option<ShortcutAction> {
        self.recording
    }

    /// Inicia (o cancela si ya era la misma) la grabación de un atajo.
    pub fn toggle_recording(
        &mut self,
        action: ShortcutAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.recording == Some(action) {
            self.stop_recording();
            cx.notify();
            return;
        }
        self.recording = Some(action);
        self.recording_error = None;
        let weak = cx.weak_entity();
        self.recording_interceptor = Some(cx.intercept_keystrokes(move |event, _, cx| {
            let handled = weak
                .update(cx, |this, cx| this.record_keystroke(&event.keystroke, cx))
                .unwrap_or(false);
            if handled {
                cx.stop_propagation();
            }
        }));
        window.focus(&self.recorder_focus, cx);
        cx.notify();
    }

    fn stop_recording(&mut self) {
        self.recording = None;
        self.recording_error = None;
        self.recording_interceptor = None;
    }

    /// Cancela la grabación en curso.
    pub fn cancel_recording(&mut self, cx: &mut Context<Self>) {
        if self.recording.is_some()
            || self.recording_error.is_some()
            || self.recording_interceptor.is_some()
        {
            self.stop_recording();
            cx.notify();
        }
    }

    fn record_keystroke(&mut self, keystroke: &Keystroke, cx: &mut Context<Self>) -> bool {
        let Some(action) = self.recording else {
            return false;
        };
        let key = keystroke.key.trim().to_ascii_lowercase();
        let no_modifiers = !keystroke.modifiers.modified();

        if is_modifier_only_key(&key) {
            return true;
        }
        if key == "escape" && no_modifiers {
            self.stop_recording();
            cx.notify();
            return true;
        }
        if matches!(key.as_str(), "backspace" | "delete") && no_modifiers {
            self.stop_recording();
            self.model
                .update(cx, |model, cx| model.set_keyboard_shortcut(action, "", cx))
                .detach();
            cx.notify();
            return true;
        }

        let is_mac = is_mac_platform();
        match combo_from_keystroke(keystroke, is_mac) {
            Some(combo) => {
                self.stop_recording();
                self.model
                    .update(cx, |model, cx| {
                        model.set_keyboard_shortcut(action, &combo, cx)
                    })
                    .detach();
                cx.notify();
            }
            None => {
                let modifier = if is_mac {
                    "Cmd, Ctrl, Alt"
                } else {
                    "Ctrl, Alt"
                };
                self.recording_error = Some(
                    format!(
                        "Include {modifier}, or use a function key (F1–F12). Press Esc to cancel."
                    )
                    .into(),
                );
                cx.notify();
            }
        }
        true
    }

    fn handle_record_key_down(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.record_keystroke(&event.keystroke, cx) {
            cx.stop_propagation();
        }
    }

    fn render_row(
        &self,
        def: &ShortcutDefinition,
        overrides: &BTreeMap<String, String>,
        is_mac: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = *cx.palette();
        let action = def.action;
        let id = def.id;
        let is_recording = self.recording == Some(action);
        let customized = is_customized(action, overrides);
        let combo = effective_combo_for(action, overrides);
        let conflicts = combo
            .as_deref()
            .map(|combo| conflicts_for(action, combo, overrides, is_mac))
            .unwrap_or_default();

        let badge_text: SharedString = if is_recording {
            "Press keys…".into()
        } else if let Some(combo) = &combo {
            combo_label(combo, is_mac).into()
        } else {
            "Unassigned".into()
        };

        let conflict_text: Option<SharedString> = (!conflicts.is_empty()).then(|| {
            let names: Vec<&'static str> = conflicts.iter().map(|other| other.label()).collect();
            SharedString::from(format!("Conflicts with {}", names.join(", ")))
        });

        let record_id = SharedString::from(format!("shortcut-record-{id}"));
        let clear_id = SharedString::from(format!("shortcut-clear-{id}"));
        let reset_id = SharedString::from(format!("shortcut-reset-{id}"));
        let row_id = SharedString::from(format!("shortcut-row-{id}"));
        let conflict_id = SharedString::from(format!("shortcut-conflict-{id}"));

        let record_button = div()
            .id(record_id)
            .test_support()
            .role(Role::Button)
            .aria_label(badge_text.clone())
            .flex()
            .items_center()
            .justify_center()
            .min_w(px(96.))
            .h(px(26.))
            .px(px(8.))
            .rounded(Radius::Md.px(cx))
            .border_1()
            .cursor_pointer()
            .text_size(rems(0.75))
            .when(is_recording, |el| {
                el.border_color(palette.primary)
                    .bg(palette.accent)
                    .text_color(palette.foreground)
                    .font_weight(FontWeight::MEDIUM)
            })
            .when(!is_recording && !conflicts.is_empty(), |el| {
                el.border_color(palette.warning)
                    .bg(palette.bg_base)
                    .hover(|style| style.bg(palette.bg_hover))
            })
            .when(!is_recording && conflicts.is_empty(), |el| {
                el.border_color(if customized {
                    palette.primary
                } else {
                    palette.border_base
                })
                .bg(palette.bg_base)
                .hover(|style| style.bg(palette.bg_hover))
            })
            .child(if is_recording {
                div().child(badge_text).into_any_element()
            } else if combo.is_some() {
                mergeon_ui::kbd(badge_text).into_any_element()
            } else {
                div()
                    .text_color(palette.muted_foreground)
                    .child(badge_text)
                    .into_any_element()
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                this.toggle_recording(action, window, cx);
            }));

        div()
            .id(row_id)
            .test_support()
            .flex()
            .items_center()
            .justify_between()
            .gap(rems(0.75))
            .px(rems(0.5))
            .py(rems(0.35))
            .rounded(Radius::Md.px(cx))
            .hover(|style| style.bg(palette.sidebar_row_hover))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(rems(0.8125))
                            .line_height(relative(1.4))
                            .text_color(palette.foreground)
                            .child(def.label),
                    )
                    .when_some(conflict_text, |el, message| {
                        el.child(
                            div()
                                .id(conflict_id)
                                .test_support()
                                .role(Role::Status)
                                .aria_label(message.clone())
                                .text_size(rems(0.7))
                                .line_height(relative(1.3))
                                .text_color(palette.warning)
                                .child(message),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap(px(4.))
                    .child(record_button)
                    .child(
                        mergeon_ui::button(clear_id)
                            .ghost()
                            .xs()
                            .icon(Lucide::X)
                            .tooltip("Unbind shortcut")
                            .disabled(combo.is_none())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.cancel_recording(cx);
                                this.model
                                    .update(cx, |model, cx| {
                                        model.set_keyboard_shortcut(action, "", cx)
                                    })
                                    .detach();
                            })),
                    )
                    .child(
                        mergeon_ui::button(reset_id)
                            .ghost()
                            .xs()
                            .icon(Lucide::RotateCcw)
                            .tooltip("Reset to default")
                            .disabled(!customized)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.cancel_recording(cx);
                                this.model
                                    .update(cx, |model, cx| {
                                        model.reset_keyboard_shortcut(action, cx)
                                    })
                                    .detach();
                            })),
                    ),
            )
            .into_any_element()
    }
}

impl Render for ShortcutsPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let is_mac = is_mac_platform();
        let overrides = self.model.read(cx).keyboard_shortcuts().clone();
        let query = self.search.read(cx).value().to_string();
        let conflicts = find_conflicts(&overrides, is_mac);
        let has_overrides = !overrides.is_empty();

        let mut sections: Vec<AnyElement> = Vec::new();
        for category in ShortcutCategory::ALL {
            let matching: Vec<&ShortcutDefinition> = CUSTOMIZABLE_SHORTCUTS
                .iter()
                .filter(|def| def.category == category)
                .filter(|def| matches_filter(def, &overrides, &query, is_mac))
                .collect();
            if matching.is_empty() {
                continue;
            }
            let rows: Vec<AnyElement> = matching
                .into_iter()
                .map(|def| self.render_row(def, &overrides, is_mac, cx))
                .collect();
            sections.push(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .mb(rems(0.2))
                            .text_size(rems(0.75))
                            .line_height(relative(1.5))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(palette.muted_foreground)
                            .child(category.title()),
                    )
                    .children(rows)
                    .into_any_element(),
            );
        }

        let conflict_summary = (!conflicts.is_empty()).then(|| {
            let count = conflicts.len();
            SharedString::from(if count == 1 {
                "1 shortcut conflict detected. Rebind or clear one of the conflicting actions."
                    .to_string()
            } else {
                format!(
                    "{count} shortcut conflicts detected. Rebind or clear the conflicting actions."
                )
            })
        });

        div()
            .id("shortcuts-panel")
            .test_support()
            .track_focus(&self.recorder_focus)
            .when(self.recording.is_some(), |el| {
                el.key_context(SHORTCUT_RECORDER_CONTEXT)
            })
            .capture_key_down(cx.listener(Self::handle_record_key_down))
            .flex()
            .flex_col()
            .gap(rems(1.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(rems(0.5))
                    .child(
                        div()
                            .id("shortcuts-search")
                            .test_support()
                            .flex_1()
                            .min_w_0()
                            .child(mergeon_ui::input(&self.search)),
                    )
                    .child(
                        mergeon_ui::button("shortcuts-reset-all")
                            .outline()
                            .sm()
                            .label("Reset all to defaults")
                            .disabled(!has_overrides)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.cancel_recording(cx);
                                this.model
                                    .update(cx, |model, cx| {
                                        model.reset_all_keyboard_shortcuts(cx)
                                    })
                                    .detach();
                            })),
                    ),
            )
            .child(
                div()
                    .text_size(rems(0.75))
                    .line_height(relative(1.5))
                    .text_color(palette.muted_foreground)
                    .child(
                        "Click a shortcut to record a new key combination. Press Esc to cancel, or Backspace / Delete to unbind.",
                    ),
            )
            .when_some(conflict_summary, |el, summary| {
                el.child(
                    div()
                        .id("shortcuts-conflict-banner")
                        .test_support()
                        .role(Role::Alert)
                        .aria_label(summary.clone())
                        .px(px(10.))
                        .py(px(8.))
                        .rounded(Radius::Md.px(cx))
                        .border_1()
                        .border_color(palette.warning)
                        .text_size(rems(0.75))
                        .line_height(relative(1.4))
                        .text_color(palette.warning)
                        .child(summary),
                )
            })
            .when_some(self.recording_error.clone(), |el, error| {
                el.child(
                    div()
                        .id("shortcuts-recording-error")
                        .test_support()
                        .role(Role::Status)
                        .aria_label(error.clone())
                        .text_size(rems(0.75))
                        .line_height(relative(1.4))
                        .text_color(palette.destructive)
                        .child(error),
                )
            })
            .when(sections.is_empty(), |el| {
                el.child(
                    div()
                        .id("shortcuts-empty")
                        .test_support()
                        .py(rems(1.))
                        .text_size(rems(0.8125))
                        .text_color(palette.muted_foreground)
                        .child("No matching shortcuts."),
                )
            })
            .children(sections)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::Modifiers;

    fn stroke(key: &str, modifiers: Modifiers) -> Keystroke {
        Keystroke {
            modifiers,
            key: key.to_string(),
            key_char: None,
        }
    }

    #[test]
    fn converts_gpui_keystrokes_to_normalized_combos() {
        assert_eq!(
            combo_from_keystroke(
                &stroke(
                    "down",
                    Modifiers {
                        control: true,
                        shift: true,
                        ..Default::default()
                    }
                ),
                false
            ),
            Some("mod+shift+down".to_string())
        );
        assert_eq!(
            combo_from_keystroke(
                &stroke(
                    "down",
                    Modifiers {
                        control: true,
                        shift: true,
                        ..Default::default()
                    }
                ),
                true
            ),
            Some("ctrl+shift+down".to_string())
        );
        assert_eq!(
            combo_from_keystroke(
                &stroke(
                    "f",
                    Modifiers {
                        platform: true,
                        shift: true,
                        ..Default::default()
                    }
                ),
                true
            ),
            Some("mod+shift+f".to_string())
        );
        assert_eq!(
            combo_from_keystroke(&stroke("f5", Modifiers::none()), false),
            Some("f5".to_string())
        );
        assert_eq!(
            combo_from_keystroke(&stroke("a", Modifiers::none()), false),
            None
        );
        assert_eq!(
            combo_from_keystroke(&stroke("shift", Modifiers::none()), false),
            None
        );
    }
}
