//! Diálogo Create Branch (`src/components/repository/CreateBranchDialog.vue`).

use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, TestSupportExt as _, Window, div, px, rems,
};
use mergeon_app::{AppModel, CreateBranchSubmission, LocalChangesMode};
use mergeon_ui::dialog::{self, AppDialog, DialogId, DialogWidth};
use mergeon_ui::theme::with_alpha;
use mergeon_ui::{
    ActivePaletteExt as _, IconTone, InputEvent, InputState, Lucide, MergeonStyled as _,
    RadioOption, Radius,
};

use super::{action_badge, is_open};

const TITLE: &str = "Create Branch";

/// El `md:` de Tailwind: desde este ancho de ventana el formulario va en
/// columnas; por debajo, apilado.
const MD_BREAKPOINT: Pixels = px(768.);

type SubmitHandler = Rc<dyn Fn(CreateBranchSubmission, &mut Window, &mut App)>;

/// Con qué se abre el diálogo (las propiedades del componente original).
///
/// ```ignore
/// open_create_branch_dialog(
///     CreateBranchDialogOptions::new("main")
///         .has_local_changes(true)
///         .on_submit(|submission, _, cx| { /* crear la rama */ }),
///     window,
///     cx,
/// );
/// ```
#[derive(Clone)]
pub struct CreateBranchDialogOptions {
    base_label: SharedString,
    has_local_changes: bool,
    suggested_name: SharedString,
    on_submit: Option<SubmitHandler>,
}

impl CreateBranchDialogOptions {
    /// `base_label` es lo que se lee junto a "Create branch at:" (una rama,
    /// un tag o un commit).
    pub fn new(base_label: impl Into<SharedString>) -> Self {
        Self {
            base_label: base_label.into(),
            has_local_changes: false,
            suggested_name: SharedString::default(),
            on_submit: None,
        }
    }

    /// Hay cambios sin commitear: se puede elegir qué hacer con ellos.
    pub fn has_local_changes(mut self, has_local_changes: bool) -> Self {
        self.has_local_changes = has_local_changes;
        self
    }

    /// Nombre con el que arranca el campo (`suggestedName`).
    pub fn suggested_name(mut self, name: impl Into<SharedString>) -> Self {
        self.suggested_name = name.into();
        self
    }

    /// `@submit`. Sin manejador, el diálogo llama a
    /// `AppModel::handle_create_branch_submit`, que crea la rama desde la
    /// referencia que preparó `AppModel::handle_new_branch`.
    pub fn on_submit(
        mut self,
        handler: impl Fn(CreateBranchSubmission, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_submit = Some(Rc::new(handler));
        self
    }
}

/// `localChangeOptions`.
const LOCAL_CHANGE_OPTIONS: [(LocalChangesMode, &str); 3] = [
    (LocalChangesMode::Keep, "Don't change"),
    (LocalChangesMode::Stash, "Stash and reapply"),
    (LocalChangesMode::Discard, "Discard"),
];

fn local_changes_mode(value: &str) -> LocalChangesMode {
    LOCAL_CHANGE_OPTIONS
        .into_iter()
        .map(|(mode, _)| mode)
        .find(|mode| mode.as_str() == value)
        .unwrap_or(LocalChangesMode::Keep)
}

/// Formulario del diálogo.
pub(crate) struct CreateBranchDialog {
    base_label: SharedString,
    has_local_changes: bool,
    branch_name: Entity<InputState>,
    checkout_after_create: bool,
    local_changes_mode: LocalChangesMode,
    on_submit: Option<SubmitHandler>,
    dialog_id: Option<DialogId>,
    _subscription: Subscription,
}

impl CreateBranchDialog {
    fn new(
        options: CreateBranchDialogOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let branch_name = cx.new(|cx| {
            let mut state = InputState::new(window, cx).placeholder("Enter Branch Name");
            if !options.suggested_name.is_empty() {
                state.set_value(options.suggested_name.clone(), window, cx);
            }
            state
        });
        let subscription = cx.subscribe_in(
            &branch_name,
            window,
            |this, _, event: &InputEvent, window, cx| match event {
                // `canSubmit` depende de lo escrito.
                InputEvent::Change => cx.notify(),
                // `@keydown.enter.prevent="handleSubmit"`
                InputEvent::PressEnter { .. } => this.submit(window, cx),
                _ => {}
            },
        );

        Self {
            base_label: options.base_label,
            has_local_changes: options.has_local_changes,
            branch_name,
            checkout_after_create: true,
            local_changes_mode: LocalChangesMode::Keep,
            on_submit: options.on_submit,
            dialog_id: None,
            _subscription: subscription,
        }
    }

    fn trimmed_name(&self, cx: &App) -> String {
        self.branch_name.read(cx).value().trim().to_string()
    }

    /// `canSubmit`.
    fn can_submit(&self, cx: &App) -> bool {
        !self.trimmed_name(cx).is_empty()
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.dialog_id.take() {
            dialog::close_id(window, cx, id);
        }
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_submit(cx) {
            return;
        }
        let submission = CreateBranchSubmission {
            branch_name: self.trimmed_name(cx),
            checkout_after_create: self.checkout_after_create,
            local_changes_mode: self.local_changes_mode,
        };
        let on_submit = self.on_submit.clone();
        self.close(window, cx);
        match on_submit {
            Some(on_submit) => on_submit(submission, window, cx),
            None => AppModel::global(cx)
                .update(cx, |model, cx| {
                    model.handle_create_branch_submit(submission, cx)
                })
                .detach(),
        }
    }

    fn footer(form: &Entity<Self>, cx: &App) -> impl IntoElement + use<> {
        let state = form.read(cx);
        let can_submit = state.can_submit(cx);
        let label = if state.checkout_after_create {
            "Create and Checkout"
        } else {
            "Create Branch"
        };
        let cancel = form.clone();
        let submit = form.clone();
        div()
            .flex()
            .justify_end()
            .gap_2()
            .child(
                mergeon_ui::button("create-branch-cancel")
                    .ghost()
                    .sm()
                    .label("Cancel")
                    .on_click(move |_, window, cx| {
                        cancel.update(cx, |form, cx| form.close(window, cx));
                    }),
            )
            .child(
                mergeon_ui::button("create-branch-submit")
                    .sm()
                    .label(label)
                    .disabled(!can_submit)
                    .on_click(move |_, window, cx| {
                        submit.update(cx, |form, cx| form.submit(window, cx));
                    }),
            )
    }
}

impl Render for CreateBranchDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let wide = window.viewport_size().width >= MD_BREAKPOINT;
        let label = |text: &'static str| {
            div()
                .when(wide, |this| this.w(px(112.)).flex_shrink_0())
                .tw_text_sm()
                .tw_font_medium()
                .text_color(palette.foreground)
                .child(text)
        };
        // `<label for="create-branch-name">`: un clic en la etiqueta enfoca el campo.
        let name_label = label("Branch name:")
            .id("create-branch-name-label")
            .test_support()
            .on_click(cx.listener(|this, _, window, cx| {
                this.branch_name
                    .update(cx, |state, cx| state.focus(window, cx));
            }));
        // `grid gap-2 md:grid-cols-[112px_minmax(0,1fr)]`
        let pair = || div().flex().gap_2().when(!wide, |this| this.flex_col());

        div()
            .flex()
            .gap_3()
            .when(!wide, |this| this.flex_col())
            .child(
                div()
                    .flex()
                    .items_start()
                    .when(wide, |this| this.w(px(56.)).flex_shrink_0())
                    .when(!wide, |this| this.justify_center())
                    .child(action_badge(Lucide::GitBranchPlus, 2.85, cx)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    // `space-y-2.5`
                    .gap(rems(0.625))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_start()
                            .gap_1()
                            .child(
                                div()
                                    .tw_text_xs()
                                    .text_color(palette.muted_foreground)
                                    .child("Create branch at:"),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .max_w_full()
                                    .radius(Radius::Lg, cx)
                                    .border_1()
                                    .border_color(palette.border)
                                    .bg(palette.background)
                                    .px(rems(0.625))
                                    .py(rems(0.375))
                                    .tw_text_sm()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(palette.foreground)
                                    .child(
                                        mergeon_ui::icon(Lucide::GitBranch)
                                            .size(px(14.))
                                            .tone(IconTone::Muted),
                                    )
                                    .child(div().truncate().child(self.base_label.clone())),
                            ),
                    )
                    .child(
                        pair()
                            .when(wide, |this| this.items_center())
                            .child(name_label)
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
                            .items_start()
                            .gap_2()
                            .child(
                                mergeon_ui::checkbox("create-branch-checkout")
                                    .label("Check out after create")
                                    .checked(self.checkout_after_create)
                                    .on_change(cx.listener(|this, checked: &bool, _, cx| {
                                        this.checkout_after_create = *checked;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .w_full()
                                    .gap_2()
                                    .radius(Radius::Xl, cx)
                                    .border_1()
                                    .border_color(with_alpha(palette.border, 0.8))
                                    .p(rems(0.625))
                                    .when(!self.has_local_changes, |this| this.opacity(0.6))
                                    .child(
                                        pair().child(label("Local changes:")).child(
                                            div().flex_1().min_w_0().child(
                                                mergeon_ui::radio_group(
                                                    "create-branch-local-changes",
                                                )
                                                .label("Local changes")
                                                .options(LOCAL_CHANGE_OPTIONS.into_iter().map(
                                                    |(mode, label)| {
                                                        RadioOption::new(mode.as_str(), label)
                                                    },
                                                ))
                                                .selected(self.local_changes_mode.as_str())
                                                .disabled(!self.has_local_changes)
                                                .on_change(cx.listener(
                                                    |this, value: &SharedString, _, cx| {
                                                        this.local_changes_mode =
                                                            local_changes_mode(value);
                                                        cx.notify();
                                                    },
                                                )),
                                            ),
                                        ),
                                    )
                                    .when(!self.has_local_changes, |this| {
                                        this.child(
                                            div()
                                                .tw_text_xs()
                                                .text_color(palette.muted_foreground)
                                                .child(
                                                    "No local changes detected in the working tree.",
                                                ),
                                        )
                                    }),
                            ),
                    ),
            )
    }
}

/// Abre el diálogo Create Branch. Devuelve `None` si ya estaba abierto.
pub fn open_create_branch_dialog(
    options: CreateBranchDialogOptions,
    window: &mut Window,
    cx: &mut App,
) -> Option<DialogId> {
    if is_open(TITLE, window, cx) {
        return None;
    }

    let form = cx.new(|cx| CreateBranchDialog::new(options, window, cx));
    let id = AppDialog::new()
        .title(TITLE)
        .description("Create new branch")
        .width(DialogWidth::Lg)
        .body({
            let form = form.clone();
            move |_, _| form.clone()
        })
        .footer({
            let form = form.clone();
            move |_, cx| CreateBranchDialog::footer(&form, cx)
        })
        .open(window, cx);
    form.update(cx, |form, cx| {
        form.dialog_id = Some(id);
        // `autofocus`: se abre para escribir el nombre.
        form.branch_name
            .update(cx, |state, cx| state.focus(window, cx));
    });
    Some(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn radio_values_map_back_to_their_mode() {
        for (mode, _) in LOCAL_CHANGE_OPTIONS {
            assert_eq!(local_changes_mode(mode.as_str()), mode);
        }
        assert_eq!(local_changes_mode("unknown"), LocalChangesMode::Keep);
    }
}
