//! Selector rápido de workspaces (`SwitchWorkspace`, por defecto `mod+alt+w`):
//! un diálogo ligero orientado a teclado donde `0` activa `All` y `1`–`9`
//! cambian al workspace correspondiente en una sola pulsación.
//!
//! También acepta flechas (`↑`/`↓`), `k`/`j`, `Home`/`End`, `Enter`, `Esc` y
//! clic con el ratón.

use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, Context, EventEmitter, FocusHandle, Focusable, FontWeight, KeyDownEvent,
    SharedString, TestSupportExt as _, Window, div, px,
};
use mergeon_ui::{ActivePaletteExt as _, IconTone, Lucide};

/// Alto de una fila del selector.
const ROW_HEIGHT: f32 = 36.;

/// Resumen de un workspace para el selector rápido.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceItem {
    pub id: String,
    pub name: String,
    pub repo_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceSwitcherEvent {
    /// Cambia al workspace elegido (`None` = "All").
    Select(Option<String>),
    /// Abre el flujo para crear un nuevo workspace.
    CreateNew,
}

pub struct WorkspaceSwitcher {
    workspaces: Vec<WorkspaceItem>,
    active_workspace_id: Option<String>,
    total_open: usize,
    selected: usize,
    triggered: bool,
    focus_handle: FocusHandle,
}

impl EventEmitter<WorkspaceSwitcherEvent> for WorkspaceSwitcher {}

impl Focusable for WorkspaceSwitcher {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl WorkspaceSwitcher {
    pub fn new(
        workspaces: Vec<WorkspaceItem>,
        active_workspace_id: Option<String>,
        total_open: usize,
        cx: &mut Context<Self>,
    ) -> Self {
        let selected = active_workspace_id
            .as_deref()
            .and_then(|active_id| workspaces.iter().position(|ws| ws.id == active_id))
            .map_or(0, |index| index + 1);
        Self {
            workspaces,
            active_workspace_id,
            total_open,
            selected,
            triggered: false,
            focus_handle: cx.focus_handle(),
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn selected_index(&self) -> usize {
        self.selected
    }

    /// Total de filas navegables: `All` + cada workspace + `New workspace…`.
    fn row_count(&self) -> usize {
        self.workspaces.len() + 2
    }

    fn move_by(&mut self, delta: isize, cx: &mut Context<Self>) {
        let count = self.row_count();
        if count == 0 {
            return;
        }
        let next = (self.selected as isize + delta).rem_euclid(count as isize) as usize;
        if next != self.selected {
            self.selected = next;
            cx.notify();
        }
    }

    /// Confirma la fila actualmente resaltada (`Enter`).
    pub fn confirm(&mut self, cx: &mut Context<Self>) {
        self.activate_row(self.selected, cx);
    }

    fn activate_row(&mut self, row: usize, cx: &mut Context<Self>) {
        if self.triggered {
            return;
        }
        let event = if row == 0 {
            WorkspaceSwitcherEvent::Select(None)
        } else if let Some(workspace) = self.workspaces.get(row - 1) {
            WorkspaceSwitcherEvent::Select(Some(workspace.id.clone()))
        } else if row == self.workspaces.len() + 1 {
            WorkspaceSwitcherEvent::CreateNew
        } else {
            return;
        };
        self.triggered = true;
        cx.emit(event);
    }

    fn handle_key_down(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let modifiers = event.keystroke.modifiers;
        if modifiers.control || modifiers.alt || modifiers.platform {
            return;
        }
        let key = event.keystroke.key.trim().to_ascii_lowercase();
        let digit = match key.as_str() {
            "0" => Some(0usize),
            "1" => Some(1),
            "2" => Some(2),
            "3" => Some(3),
            "4" => Some(4),
            "5" => Some(5),
            "6" => Some(6),
            "7" => Some(7),
            "8" => Some(8),
            "9" => Some(9),
            _ => event
                .keystroke
                .key_char
                .as_deref()
                .and_then(|ch| ch.bytes().next().filter(|_| ch.len() == 1))
                .filter(u8::is_ascii_digit)
                .map(|b| (b - b'0') as usize),
        };
        if let Some(number) = digit {
            cx.stop_propagation();
            if number == 0 || number <= self.workspaces.len() {
                self.activate_row(number, cx);
            }
            return;
        }
        if modifiers.shift {
            return;
        }
        match key.as_str() {
            "up" | "k" => {
                cx.stop_propagation();
                self.move_by(-1, cx);
            }
            "down" | "j" => {
                cx.stop_propagation();
                self.move_by(1, cx);
            }
            "home" => {
                cx.stop_propagation();
                if self.selected != 0 {
                    self.selected = 0;
                    cx.notify();
                }
            }
            "end" => {
                cx.stop_propagation();
                let last = self.row_count().saturating_sub(1);
                if self.selected != last {
                    self.selected = last;
                    cx.notify();
                }
            }
            _ => {}
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn render_row(
        &self,
        id: SharedString,
        row_index: usize,
        icon: Lucide,
        label: SharedString,
        detail: Option<SharedString>,
        active: bool,
        hotkey: Option<SharedString>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = *cx.palette();
        let selected = row_index == self.selected;
        let (text_color, muted_color) = if selected {
            (palette.sel_fg, palette.sel_fg_2)
        } else {
            (palette.text_1, palette.text_3)
        };
        let check_color = if selected {
            palette.sel_fg
        } else {
            palette.primary
        };

        div()
            .id(id)
            .test_support()
            .relative()
            .w_full()
            .flex()
            .items_center()
            .gap(px(10.))
            .h(px(ROW_HEIGHT))
            .px(px(12.))
            .cursor_pointer()
            .text_color(text_color)
            .when(selected, |row| row.bg(palette.sel_bg))
            .when(!selected, |row| {
                row.hover(move |row| row.bg(palette.bg_hover))
            })
            .when(selected, |row| {
                row.child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .bottom_0()
                        .w(px(2.))
                        .bg(palette.primary),
                )
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                this.activate_row(row_index, cx);
            }))
            .child(
                div()
                    .flex_none()
                    .text_color(muted_color)
                    .child(mergeon_ui::icon(icon).size(px(15.)).tone(IconTone::Current)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_size(px(13.))
                    .when(active, |el| el.font_weight(FontWeight::MEDIUM))
                    .child(label),
            )
            .when_some(detail, |row, detail| {
                row.child(
                    div()
                        .flex_none()
                        .text_size(px(12.))
                        .text_color(muted_color)
                        .child(detail),
                )
            })
            .when(active, |row| {
                row.child(
                    div().flex_none().text_color(check_color).child(
                        mergeon_ui::icon(Lucide::Check)
                            .size(px(14.))
                            .tone(IconTone::Current),
                    ),
                )
            })
            .children(hotkey.map(mergeon_ui::kbd))
            .into_any_element()
    }
}

fn repo_count_label(count: usize) -> SharedString {
    if count == 1 {
        "1 repo".into()
    } else {
        format!("{count} repos").into()
    }
}

impl Render for WorkspaceSwitcher {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();

        let mut rows = Vec::with_capacity(self.workspaces.len() + 1);
        rows.push(self.render_row(
            "workspace-switcher-all".into(),
            0,
            Lucide::Folders,
            "All".into(),
            Some(repo_count_label(self.total_open)),
            self.active_workspace_id.is_none(),
            Some("0".into()),
            cx,
        ));
        for (index, workspace) in self.workspaces.iter().enumerate() {
            let is_active = self.active_workspace_id.as_deref() == Some(workspace.id.as_str());
            let hotkey = (index < 9).then(|| SharedString::from((index + 1).to_string()));
            rows.push(self.render_row(
                SharedString::from(format!("workspace-switcher-item:{}", workspace.id)),
                index + 1,
                Lucide::Folder,
                workspace.name.clone().into(),
                Some(repo_count_label(workspace.repo_count)),
                is_active,
                hotkey,
                cx,
            ));
        }

        let new_row_index = self.workspaces.len() + 1;
        let new_row = self.render_row(
            "workspace-switcher-new".into(),
            new_row_index,
            Lucide::Plus,
            "New workspace…".into(),
            None,
            false,
            None,
            cx,
        );

        let hint = |key: &'static str, label: &'static str| {
            div()
                .flex()
                .items_center()
                .gap(px(5.))
                .child(mergeon_ui::kbd(key))
                .child(label)
        };

        div()
            .id("workspace-switcher-dialog")
            .test_support()
            .key_context("WorkspaceSwitcher")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::handle_key_down))
            .flex()
            .flex_col()
            .w_full()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(12.))
                    .py(px(10.))
                    .border_b_1()
                    .border_color(palette.border)
                    .child(
                        mergeon_ui::icon(Lucide::Folders)
                            .size(px(14.))
                            .tone(IconTone::Muted),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(palette.text_1)
                            .child("Switch workspace"),
                    ),
            )
            .child(
                div()
                    .id("workspace-switcher-list")
                    .flex()
                    .flex_col()
                    .max_h(px(360.))
                    .overflow_y_scroll()
                    .py(px(4.))
                    .children(rows),
            )
            .child(
                div()
                    .py(px(4.))
                    .border_t_1()
                    .border_color(palette.border)
                    .child(new_row),
            )
            .child(
                div()
                    .id("workspace-switcher-hints")
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .px(px(12.))
                    .py(px(8.))
                    .border_t_1()
                    .border_color(palette.border)
                    .bg(palette.bg_inset)
                    .text_size(px(11.5))
                    .text_color(palette.text_3)
                    .child(hint("0–9", "switch"))
                    .child(hint("↑↓", "navigate"))
                    .child(hint("↵", "select"))
                    .child(hint("Esc", "close")),
            )
    }
}
