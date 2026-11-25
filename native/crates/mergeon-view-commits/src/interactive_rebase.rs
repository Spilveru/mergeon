//! Diálogo de rebase interactivo
//! (`src/components/repository/InteractiveRebaseDialog.vue`): el todo-list
//! editable. Recibe los commits del rango en orden de aplicación (el más viejo
//! primero) y entrega el todo final; quien ejecuta el rebase es quien lo abre.

use std::collections::HashMap;
use std::ops::Range;

use gpui_kit::base::{Button as BaseButton, Selectable, TestSupportExt as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::menu::DropdownMenu;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Size};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, Context, Entity, EventEmitter, Hsla, InteractiveElement, Interactivity,
    ListSizingBehavior, MouseButton, ParentElement, Role, SharedString, StyleRefinement, Styled,
    Subscription, UniformListScrollHandle, Window, div, px, relative, rems, uniform_list,
};
use mergeon_backend::RebaseTodoEntry;
use mergeon_ui::dialog::{self, AppDialog, DialogId, DialogWidth};
use mergeon_ui::menu::{MenuItem, MenuTriggerExt as _};
use mergeon_ui::theme::with_alpha;
use mergeon_ui::{ActivePaletteExt as _, IconTone, InputEvent, InputState, Lucide, Radius};
use mergeon_view_diff::code_font_family;

/// Qué hace el rebase con un commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum RebaseAction {
    #[default]
    Pick,
    Reword,
    Squash,
    Fixup,
    Drop,
}

impl RebaseAction {
    /// `ACTIONS`, en el orden del selector.
    pub const ALL: [RebaseAction; 5] = [
        RebaseAction::Pick,
        RebaseAction::Reword,
        RebaseAction::Squash,
        RebaseAction::Fixup,
        RebaseAction::Drop,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            RebaseAction::Pick => "pick",
            RebaseAction::Reword => "reword",
            RebaseAction::Squash => "squash",
            RebaseAction::Fixup => "fixup",
            RebaseAction::Drop => "drop",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|action| action.as_str() == name)
    }

    /// Se funde con el commit anterior: no puede ser la primera fila (git no
    /// tiene con qué fusionarla).
    pub fn melds_into_previous(self) -> bool {
        matches!(self, RebaseAction::Squash | RebaseAction::Fixup)
    }
}

/// Un commit del rango (`{ hash, message }`).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RebaseCommit {
    pub hash: String,
    pub message: String,
}

impl RebaseCommit {
    pub fn new(hash: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            hash: hash.into(),
            message: message.into(),
        }
    }
}

/// Una fila del todo-list.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RebaseRow {
    pub hash: String,
    pub action: RebaseAction,
    pub original_message: String,
    /// Mensaje nuevo; solo se usa con `reword`.
    pub message: String,
}

/// `emit('submit', todo)` y `emit('close')`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InteractiveRebaseDialogEvent {
    Submit(Vec<RebaseTodoEntry>),
    Close,
}

/// `availableActions(index)`: la primera fila no ofrece squash ni fixup.
pub fn available_actions(index: usize) -> Vec<RebaseAction> {
    RebaseAction::ALL
        .into_iter()
        .filter(|action| index != 0 || !action.melds_into_previous())
        .collect()
}

/// Texto de la descripción del diálogo.
pub fn rebase_description(commit_count: usize) -> String {
    format!(
        "This rewrites the history of the current branch: {commit_count} commit{}, oldest first, applied top to bottom.",
        if commit_count == 1 { "" } else { "s" }
    )
}

/// Estado del diálogo. Se abre con [`open_interactive_rebase_dialog`].
pub struct InteractiveRebaseDialog {
    rows: Vec<RebaseRow>,
    busy: bool,
    /// Campo del mensaje de cada fila en `reword`, por hash.
    inputs: HashMap<String, Entity<InputState>>,
    dialog_id: Option<DialogId>,
    scroll_handle: UniformListScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<InteractiveRebaseDialogEvent> for InteractiveRebaseDialog {}

impl InteractiveRebaseDialog {
    fn new(commits: &[RebaseCommit]) -> Self {
        Self {
            rows: commits
                .iter()
                .map(|commit| RebaseRow {
                    hash: commit.hash.clone(),
                    action: RebaseAction::Pick,
                    original_message: commit.message.clone(),
                    message: commit.message.clone(),
                })
                .collect(),
            busy: false,
            inputs: HashMap::new(),
            dialog_id: None,
            scroll_handle: UniformListScrollHandle::new(),
            _subscriptions: Vec::new(),
        }
    }

    pub fn rows(&self) -> &[RebaseRow] {
        &self.rows
    }

    pub fn is_busy(&self) -> bool {
        self.busy
    }

    /// El rebase está en curso: nada se puede tocar hasta que termine.
    pub fn set_busy(&mut self, busy: bool, cx: &mut Context<Self>) {
        if self.busy != busy {
            self.busy = busy;
            cx.notify();
        }
    }

    /// `moveRow(index, delta)`.
    pub fn move_row(&mut self, index: usize, delta: isize, cx: &mut Context<Self>) {
        let Some(target) = index.checked_add_signed(delta) else {
            return;
        };
        if index >= self.rows.len() || target >= self.rows.len() {
            return;
        }
        self.rows.swap(index, target);
        // La primera fila no puede quedar en squash/fixup tras el reorden.
        if self.rows[0].action.melds_into_previous() {
            self.rows[0].action = RebaseAction::Pick;
        }
        cx.notify();
    }

    /// Cambia la acción de una fila (el `<select>` del original).
    pub fn set_action(
        &mut self,
        index: usize,
        action: RebaseAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(row) = self.rows.get_mut(index) else {
            return;
        };
        if !available_actions(index).contains(&action) || row.action == action {
            return;
        }
        row.action = action;
        if action == RebaseAction::Reword {
            self.ensure_input(index, window, cx);
        }
        cx.notify();
    }

    /// Escribe el mensaje nuevo de una fila en `reword`.
    pub fn set_message(
        &mut self,
        index: usize,
        message: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(row) = self.rows.get_mut(index) else {
            return;
        };
        row.message = message.to_string();
        if let Some(input) = self.inputs.get(&row.hash).cloned() {
            let message = message.to_string();
            input.update(cx, |state, cx| state.set_value(message, window, cx));
        }
        cx.notify();
    }

    /// El campo de texto de una fila en `reword`, si ya existe.
    pub fn message_input(&self, index: usize) -> Option<&Entity<InputState>> {
        self.inputs.get(&self.rows.get(index)?.hash)
    }

    /// `summary`: "2 pick · 1 squash".
    pub fn summary(&self) -> String {
        RebaseAction::ALL
            .into_iter()
            .filter_map(|action| {
                let count = self.rows.iter().filter(|row| row.action == action).count();
                (count > 0).then(|| format!("{count} {}", action.as_str()))
            })
            .collect::<Vec<_>>()
            .join(" · ")
    }

    /// `canSubmit`: hay filas, la primera no se funde con nada y queda al menos
    /// un commit sin descartar.
    pub fn can_submit(&self) -> bool {
        self.rows
            .first()
            .is_some_and(|row| !row.action.melds_into_previous())
            && self.rows.iter().any(|row| row.action != RebaseAction::Drop)
    }

    /// El todo que se entregaría ahora mismo.
    pub fn todo(&self) -> Vec<RebaseTodoEntry> {
        self.rows
            .iter()
            .map(|row| RebaseTodoEntry {
                action: row.action.as_str().to_string(),
                hash: row.hash.clone(),
                message: (row.action == RebaseAction::Reword).then(|| row.message.clone()),
            })
            .collect()
    }

    /// `handleSubmit`. El diálogo sigue abierto: lo cierra quien ejecuta el
    /// rebase cuando termina.
    pub fn submit(&mut self, cx: &mut Context<Self>) {
        if !self.can_submit() || self.busy {
            return;
        }
        cx.emit(InteractiveRebaseDialogEvent::Submit(self.todo()));
    }

    /// Cierra el diálogo y avisa (`emit('close')`).
    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dismiss(window, cx);
        cx.emit(InteractiveRebaseDialogEvent::Close);
    }

    /// Cierra el diálogo sin avisar: lo usa quien lo abrió al terminar el
    /// rebase.
    pub fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.dialog_id.take() {
            dialog::close_id(window, cx, id);
        }
    }

    fn closed_from_outside(&mut self, cx: &mut Context<Self>) {
        if self.dialog_id.take().is_some() {
            cx.emit(InteractiveRebaseDialogEvent::Close);
        }
    }

    fn ensure_input(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get(index) else {
            return;
        };
        if self.inputs.contains_key(&row.hash) {
            return;
        }
        let hash = row.hash.clone();
        let message = row.message.clone();
        let input = cx.new(|cx| {
            let mut state = InputState::new(window, cx).placeholder("New commit message");
            state.set_value(message, window, cx);
            state
        });
        self._subscriptions.push(cx.subscribe(&input, {
            let hash = hash.clone();
            move |this, input, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    let value = input.read(cx).value().to_string();
                    if let Some(row) = this.rows.iter_mut().find(|row| row.hash == hash) {
                        row.message = value;
                    }
                }
            }
        }));
        self.inputs.insert(hash, input);
    }

    /// Las filas `range`, cada una con el hueco que la separa de la
    /// siguiente (`gap: 0.25rem`).
    fn render_rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let last = self.rows.len().saturating_sub(1);
        range
            .filter_map(|index| {
                let row = self.rows.get(index)?;
                Some(
                    div()
                        .when(index < last, |this| this.pb(rems(0.25)))
                        .child(self.render_row(index, row, cx))
                        .into_any_element(),
                )
            })
            .collect()
    }

    fn render_row(&self, index: usize, row: &RebaseRow, cx: &mut Context<Self>) -> AnyElement {
        let palette = *cx.palette();
        let busy = self.busy;
        let last = self.rows.len().saturating_sub(1);
        let key = row.hash.clone();

        let move_button = |id: &'static str, icon: Lucide, title: &'static str, disabled: bool| {
            BaseButton::new(SharedString::from(format!("{id}:{key}")))
                .accessibility_label(title)
                .disabled(disabled)
                .w(rems(1.15))
                .h(rems(0.9))
                .rounded(Radius::Sm.px(cx))
                .tooltip(mergeon_ui::tooltip(title))
                .map(|button| {
                    if disabled {
                        button.opacity(0.3)
                    } else {
                        button
                            .cursor_pointer()
                            .hover(move |style| style.bg(palette.sidebar_row_hover))
                    }
                })
                .child(mergeon_ui::icon(icon).size(px(13.)).tone(IconTone::Muted))
        };

        let action_color: Hsla = match row.action {
            RebaseAction::Drop => palette.destructive.into(),
            RebaseAction::Squash | RebaseAction::Fixup => palette.warning.into(),
            RebaseAction::Reword => palette.primary.into(),
            RebaseAction::Pick => palette.foreground.into(),
        };
        let current = row.action;
        let view = cx.entity().downgrade();
        let select = ActionTrigger {
            base: BaseButton::new(SharedString::from(format!("rebase-action:{}", row.hash))),
            label: row.action.as_str().into(),
            color: action_color,
            disabled: busy,
            open: false,
        };
        let select = if busy {
            select.into_any_element()
        } else {
            select
                .dropdown_menu_items(move |_, _| {
                    available_actions(index)
                        .into_iter()
                        .map(|action| {
                            let view = view.clone();
                            MenuItem::new(action.as_str())
                                .checked(action == current)
                                .list_box()
                                .on_click(move |window, cx| {
                                    let _ = view.update(cx, |dialog, cx| {
                                        dialog.set_action(index, action, window, cx);
                                    });
                                })
                        })
                        .collect()
                })
                .into_any_element()
        };

        let message = match (row.action, self.inputs.get(&row.hash)) {
            (RebaseAction::Reword, Some(input)) => div()
                .flex_1()
                .min_w_0()
                .child(
                    Input::new(input)
                        .with_size(Size::Small)
                        .disabled(busy)
                        .h(px(26.))
                        .px(rems(0.45))
                        .rounded(Radius::Md.px(cx))
                        .bg(palette.background)
                        .text_size(rems(0.8125)),
                )
                .into_any_element(),
            _ => {
                let dropped = row.action == RebaseAction::Drop;
                div()
                    .id(SharedString::from(format!("rebase-message:{}", row.hash)))
                    .test_support()
                    .aria_label(SharedString::from(row.original_message.clone()))
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(rems(0.8125))
                    .text_color(if dropped {
                        palette.muted_foreground
                    } else {
                        palette.foreground
                    })
                    .when(dropped, |message| message.line_through())
                    .child(row.original_message.clone())
                    .into_any_element()
            }
        };

        div()
            .id(SharedString::from(format!("rebase-row:{}", row.hash)))
            .test_support()
            .aria_label(SharedString::from(format!(
                "{} {}",
                row.action.as_str(),
                short_hash(&row.hash)
            )))
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(rems(0.5))
            .py(rems(0.3))
            .px(rems(0.4))
            .border_1()
            .border_color(with_alpha(palette.border, 0.7))
            .rounded(Radius::Lg.px(cx))
            .bg(with_alpha(palette.card, 0.6))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(
                        move_button(
                            "rebase-up",
                            Lucide::ChevronUp,
                            "Move up",
                            index == 0 || busy,
                        )
                        .on_click(cx.listener(move |dialog, _, _, cx| {
                            dialog.move_row(index, -1, cx);
                        })),
                    )
                    .child(
                        move_button(
                            "rebase-down",
                            Lucide::ChevronDown,
                            "Move down",
                            index == last || busy,
                        )
                        .on_click(cx.listener(move |dialog, _, _, cx| {
                            dialog.move_row(index, 1, cx);
                        })),
                    ),
            )
            .child(div().w(px(92.)).flex_shrink_0().child(select))
            .child(
                div()
                    .flex_shrink_0()
                    .font_family(code_font_family(cx))
                    .text_size(rems(0.73))
                    .text_color(palette.muted_foreground)
                    .child(short_hash(&row.hash).to_string()),
            )
            .child(message)
            .into_any_element()
    }
}

/// `row.hash.slice(0, 7)`.
fn short_hash(hash: &str) -> &str {
    let end = hash
        .char_indices()
        .nth(7)
        .map_or(hash.len(), |(index, _)| index);
    &hash[..end]
}

impl Render for InteractiveRebaseDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        // Lista virtual: el rango puede tener cientos de commits y solo se
        // construyen las filas a la vista.
        let rows = uniform_list(
            "rebase-rows",
            self.rows.len(),
            cx.processor(|this, range, _, cx| this.render_rows(range, cx)),
        )
        .with_sizing_behavior(ListSizingBehavior::Infer)
        .track_scroll(&self.scroll_handle)
        .min_h_0()
        .max_h(px(420.))
        .pr(rems(0.25));
        // La lista virtual mide todas sus filas: sin un contenedor que la
        // recorte, su alto completo estiraría el diálogo.
        let rows = div()
            .id("rebase-list")
            .test_support()
            .flex()
            .flex_col()
            .max_h(px(420.))
            .overflow_hidden()
            .child(rows);

        div()
            .flex()
            .flex_col()
            .gap(rems(0.6))
            .line_height(relative(1.5))
            .child(rows)
            .child(
                div()
                    .id("rebase-summary")
                    .test_support()
                    .aria_label(SharedString::from(self.summary()))
                    .text_size(rems(0.75))
                    .text_color(palette.muted_foreground)
                    .child(self.summary()),
            )
    }
}

/// El `<select>` de una fila: 92×26, del color de su acción.
#[derive(IntoElement)]
struct ActionTrigger {
    base: BaseButton,
    label: SharedString,
    color: Hsla,
    disabled: bool,
    open: bool,
}

impl Selectable for ActionTrigger {
    fn selected(mut self, selected: bool) -> Self {
        self.open = selected;
        self
    }

    fn is_selected(&self) -> bool {
        self.open
    }
}

impl Styled for ActionTrigger {
    fn style(&mut self) -> &mut StyleRefinement {
        self.base.style()
    }
}

impl InteractiveElement for ActionTrigger {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.base.interactivity()
    }
}

impl ParentElement for ActionTrigger {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.base.extend(elements);
    }
}

impl DropdownMenu for ActionTrigger {}

impl RenderOnce for ActionTrigger {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let palette = *cx.palette();
        let dark = cx.theme().mode.is_dark();
        self.base
            .role(Role::ComboBox)
            .disabled(self.disabled)
            .selected(self.open)
            .accessibility_label(self.label.clone())
            .aria_expanded(self.open)
            .w_full()
            .h(px(26.))
            .justify_between()
            .border_1()
            .border_color(palette.border)
            .rounded(Radius::Md.px(cx))
            .bg(palette.background)
            .px(rems(0.35))
            .text_size(rems(0.75))
            .text_color(self.color)
            .when(!self.disabled, |this| {
                this.cursor_pointer()
                    .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
            })
            .when(self.disabled, |this| this.opacity(0.64))
            .child(div().pl(px(4.)).child(self.label))
            .child(
                Icon::new(Lucide::ChevronDown)
                    .with_size(Size::Size(px(14.)))
                    // La flecha del `<select>` nativo sigue al color del tema,
                    // no al de la acción.
                    .text_color(if dark {
                        palette.foreground
                    } else {
                        palette.muted_foreground
                    }),
            )
    }
}

/// Abre el diálogo de rebase interactivo. Quien llama se suscribe a los
/// [`InteractiveRebaseDialogEvent`] de la entidad devuelta, y lo cierra con
/// [`InteractiveRebaseDialog::dismiss`] cuando el rebase termina.
pub fn open_interactive_rebase_dialog(
    commits: Vec<RebaseCommit>,
    window: &mut Window,
    cx: &mut App,
) -> Entity<InteractiveRebaseDialog> {
    let description = rebase_description(commits.len());
    let view = cx.new(|_| InteractiveRebaseDialog::new(&commits));

    let id = AppDialog::new()
        .title("Interactive rebase")
        .description(description)
        .width(DialogWidth::Xl2)
        .body({
            let view = view.clone();
            move |_, _| view.clone()
        })
        .footer({
            let view = view.clone();
            move |_, cx| {
                let (busy, can_submit) = {
                    let dialog = view.read(cx);
                    (dialog.is_busy(), dialog.can_submit())
                };
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        mergeon_ui::button("rebase-cancel")
                            .ghost()
                            .sm()
                            .disabled(busy)
                            .label("Cancel")
                            .on_click({
                                let view = view.clone();
                                move |_, window, cx| {
                                    view.update(cx, |dialog, cx| dialog.close(window, cx));
                                }
                            }),
                    )
                    .child(
                        mergeon_ui::button("rebase-submit")
                            .destructive_outline()
                            .sm()
                            .disabled(!can_submit || busy)
                            .loading(busy)
                            .label("Rebase")
                            .on_click({
                                let view = view.clone();
                                move |_, _, cx| {
                                    view.update(cx, |dialog, cx| dialog.submit(cx));
                                }
                            }),
                    )
            }
        })
        .on_close({
            let view = view.clone();
            move |_, cx| view.update(cx, |dialog, cx| dialog.closed_from_outside(cx))
        })
        .open(window, cx);

    view.update(cx, |dialog, _| dialog.dialog_id = Some(id));
    view
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_row_cannot_squash_or_fixup() {
        let names = |index| -> Vec<&'static str> {
            available_actions(index)
                .into_iter()
                .map(RebaseAction::as_str)
                .collect()
        };
        assert_eq!(names(0), ["pick", "reword", "drop"]);
        assert_eq!(names(1), ["pick", "reword", "squash", "fixup", "drop"]);
    }

    #[test]
    fn describes_how_many_commits_are_rewritten() {
        assert_eq!(
            rebase_description(1),
            "This rewrites the history of the current branch: 1 commit, oldest first, applied top to bottom."
        );
        assert!(rebase_description(4).contains("4 commits,"));
    }

    #[test]
    fn parses_action_names() {
        assert_eq!(RebaseAction::parse("fixup"), Some(RebaseAction::Fixup));
        assert_eq!(RebaseAction::parse("edit"), None);
    }
}
