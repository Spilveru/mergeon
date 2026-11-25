//! Diálogo «Create Branch» (`src/components/repository/CreateBranchDialog.vue`),
//! tal como lo abre el sidebar desde «New Branch...».

use gpui_kit::prelude::*;
use gpui_kit::{
    App, Context, Entity, EventEmitter, SharedString, Subscription, Window, div, px, rems,
};
use mergeon_app::LocalChangesMode;
use mergeon_ui::dialog::{self, AppDialog, DialogId, DialogWidth};
use mergeon_ui::{
    ActivePaletteExt as _, InputEvent, InputState, Lucide, MergeonStyled as _, RadioOption,
};

use super::parts::{branch_chip, caption, icon_badge, local_changes_box, local_changes_mode};

/// Las props de `CreateBranchDialog.vue`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct CreateBranchOptions {
    /// Nombre visible del punto de partida.
    pub base_label: SharedString,
    pub has_local_changes: bool,
    pub suggested_name: SharedString,
}

impl Default for CreateBranchOptions {
    fn default() -> Self {
        Self {
            base_label: "HEAD".into(),
            has_local_changes: false,
            suggested_name: SharedString::default(),
        }
    }
}

impl CreateBranchOptions {
    pub fn new(base_label: impl Into<SharedString>) -> Self {
        Self {
            base_label: base_label.into(),
            ..Self::default()
        }
    }

    pub fn has_local_changes(mut self, has_local_changes: bool) -> Self {
        self.has_local_changes = has_local_changes;
        self
    }

    pub fn suggested_name(mut self, name: impl Into<SharedString>) -> Self {
        self.suggested_name = name.into();
        self
    }
}

/// `emit('submit', { branchName, checkoutAfterCreate, localChangesMode })` y
/// `emit('close')`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreateBranchDialogEvent {
    Submit {
        branch_name: String,
        checkout_after_create: bool,
        local_changes_mode: LocalChangesMode,
    },
    Close,
}

/// Estado del diálogo. Se abre con [`open_create_branch_dialog`].
pub struct CreateBranchDialog {
    options: CreateBranchOptions,
    branch_name: Entity<InputState>,
    checkout_after_create: bool,
    local_changes_mode: LocalChangesMode,
    dialog_id: Option<DialogId>,
    _subscription: Subscription,
}

impl EventEmitter<CreateBranchDialogEvent> for CreateBranchDialog {}

impl CreateBranchDialog {
    fn new(options: CreateBranchOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let branch_name = cx.new(|cx| {
            let mut state = InputState::new(window, cx).placeholder("Enter Branch Name");
            if !options.suggested_name.is_empty() {
                state.set_value(options.suggested_name.clone(), window, cx);
            }
            state
        });
        // `@keydown.enter.prevent="handleSubmit"`; y cada tecla puede cambiar
        // si el botón está habilitado.
        let subscription = cx.subscribe_in(
            &branch_name,
            window,
            |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } => this.submit(window, cx),
                InputEvent::Change => cx.notify(),
                _ => {}
            },
        );

        Self {
            options,
            branch_name,
            checkout_after_create: true,
            local_changes_mode: LocalChangesMode::Keep,
            dialog_id: None,
            _subscription: subscription,
        }
    }

    /// El campo del nombre, para escribir en él desde fuera (tests, galería).
    pub fn branch_name_input(&self) -> &Entity<InputState> {
        &self.branch_name
    }

    pub fn branch_name(&self, cx: &App) -> String {
        self.branch_name.read(cx).value().trim().to_string()
    }

    /// `canSubmit`.
    pub fn is_submittable(&self, cx: &App) -> bool {
        !self.branch_name(cx).is_empty()
    }

    pub fn checkout_after_create(&self) -> bool {
        self.checkout_after_create
    }

    pub fn set_checkout_after_create(&mut self, checked: bool, cx: &mut Context<Self>) {
        self.checkout_after_create = checked;
        cx.notify();
    }

    pub fn set_local_changes_mode(&mut self, mode: LocalChangesMode, cx: &mut Context<Self>) {
        self.local_changes_mode = mode;
        cx.notify();
    }

    /// Texto del botón de confirmar.
    pub fn confirm_label(&self) -> &'static str {
        if self.checkout_after_create {
            "Create and Checkout"
        } else {
            "Create Branch"
        }
    }

    fn close_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.dialog_id.take() {
            dialog::close_id(window, cx, id);
        }
    }

    /// Cierra sin avisar a nadie (lo llama quien lo abrió: otro diálogo del
    /// mismo tipo o un cambio de repositorio).
    pub fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.dialog_id.take() {
            dialog::close_id(window, cx, id);
        }
    }

    /// `handleSubmit`. Sin nombre no hace nada.
    pub fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.is_submittable(cx) {
            return;
        }
        let branch_name = self.branch_name(cx);
        self.close_dialog(window, cx);
        cx.emit(CreateBranchDialogEvent::Submit {
            branch_name,
            checkout_after_create: self.checkout_after_create,
            local_changes_mode: self.local_changes_mode,
        });
    }

    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_dialog(window, cx);
        cx.emit(CreateBranchDialogEvent::Close);
    }

    fn dismissed(&mut self, cx: &mut Context<Self>) {
        self.dialog_id = None;
        cx.emit(CreateBranchDialogEvent::Close);
    }
}

impl Render for CreateBranchDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let has_local_changes = self.options.has_local_changes;
        let palette = *cx.palette();

        let local_changes = mergeon_ui::radio_group("create-branch-local-changes")
            .label("Local changes")
            .option(RadioOption::new("keep", "Don't change"))
            .option(RadioOption::new("stash", "Stash and reapply"))
            .option(RadioOption::new("discard", "Discard"))
            .selected(self.local_changes_mode.as_str())
            .disabled(!has_local_changes)
            .on_change(cx.listener(|this, value: &SharedString, _, cx| {
                this.set_local_changes_mode(local_changes_mode(value), cx);
            }));

        div()
            .flex()
            .gap_3()
            .child(div().w(px(56.)).flex_shrink_0().child(icon_badge(
                Lucide::GitBranchPlus,
                2.85,
                30.,
                cx,
            )))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(rems(0.625))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(caption("Create branch at:", cx))
                            .child(branch_chip(self.options.base_label.clone(), cx)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .w(px(112.))
                                    .flex_shrink_0()
                                    .tw_text_sm()
                                    .tw_font_medium()
                                    .text_color(palette.foreground)
                                    .child("Branch name:"),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(mergeon_ui::input(&self.branch_name)),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div().flex().child(
                                    mergeon_ui::checkbox("create-branch-checkout")
                                        .label("Check out after create")
                                        .checked(self.checkout_after_create)
                                        .on_change(cx.listener(|this, checked: &bool, _, cx| {
                                            this.set_checkout_after_create(*checked, cx);
                                        })),
                                ),
                            )
                            .child(local_changes_box(has_local_changes, local_changes, cx)),
                    ),
            )
    }
}

/// Abre el diálogo «Create Branch» con el foco en el nombre. Quien llama se
/// suscribe al [`CreateBranchDialogEvent`] de la entidad devuelta.
pub fn open_create_branch_dialog(
    options: CreateBranchOptions,
    window: &mut Window,
    cx: &mut App,
) -> Entity<CreateBranchDialog> {
    let view = cx.new(|cx| CreateBranchDialog::new(options, window, cx));

    let id = AppDialog::new()
        .title("Create Branch")
        .description("Create new branch")
        .width(DialogWidth::Lg)
        .body({
            let view = view.clone();
            move |_, _| view.clone()
        })
        .footer({
            let view = view.clone();
            move |_, cx| {
                let (confirm_label, submittable) = {
                    let state = view.read(cx);
                    (state.confirm_label(), state.is_submittable(cx))
                };
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        mergeon_ui::button("create-branch-cancel")
                            .ghost()
                            .sm()
                            .label("Cancel")
                            .on_click({
                                let view = view.clone();
                                move |_, window, cx| {
                                    view.update(cx, |view, cx| view.close(window, cx));
                                }
                            }),
                    )
                    .child(
                        mergeon_ui::button("create-branch-submit")
                            .sm()
                            .disabled(!submittable)
                            .label(confirm_label)
                            .on_click({
                                let view = view.clone();
                                move |_, window, cx| {
                                    view.update(cx, |view, cx| view.submit(window, cx));
                                }
                            }),
                    )
            }
        })
        .on_close({
            let view = view.clone();
            move |_, cx| view.update(cx, |view, cx| view.dismissed(cx))
        })
        .open(window, cx);

    view.update(cx, |view, cx| {
        view.dialog_id = Some(id);
        // `autofocus` del campo.
        view.branch_name
            .update(cx, |state, cx| state.focus(window, cx));
    });
    view
}
