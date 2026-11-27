//! Diálogo de checkout (`src/components/repository/CheckoutBranchDialog.vue`):
//! cambiar a una rama local, crear la rama local que sigue a una remota, o
//! —desde el historial— elegir entre varios destinos.

use gpui_kit::prelude::*;
use gpui_kit::{App, Context, Entity, EventEmitter, SharedString, Window, div, px, rems};
use mergeon_app::LocalChangesMode;
use mergeon_ui::dialog::{self, AppDialog, DialogId, DialogWidth};
use mergeon_ui::{AlertVariant, Lucide, RadioOption};

use super::parts::{branch_chip, caption, icon_badge, local_changes_box, local_changes_mode};
use crate::logic::local_branch_name;

/// Un destino posible del checkout
/// (`{ value, label, description?, warning?, confirmLabel? }`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct CheckoutTargetOption {
    pub value: SharedString,
    pub label: SharedString,
    pub description: Option<SharedString>,
    /// Aviso que se muestra mientras este destino está elegido (HEAD
    /// desanclado, por ejemplo).
    pub warning: Option<SharedString>,
    /// Texto del botón de confirmar para este destino.
    pub confirm_label: Option<SharedString>,
}

impl CheckoutTargetOption {
    pub fn new(value: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
            ..Self::default()
        }
    }

    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn warning(mut self, warning: impl Into<SharedString>) -> Self {
        self.warning = Some(warning.into());
        self
    }

    pub fn confirm_label(mut self, label: impl Into<SharedString>) -> Self {
        self.confirm_label = Some(label.into());
        self
    }
}

/// Las props de `CheckoutBranchDialog.vue`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct CheckoutBranchOptions {
    /// Nombre visible del destino.
    pub target_label: SharedString,
    pub has_local_changes: bool,
    /// `origin/feature` cuando se va a crear la rama local que la sigue; vacío
    /// si el destino es una rama local.
    pub remote_branch: SharedString,
    /// Con dos o más se pinta un selector (doble clic sobre un commit con
    /// varias ramas locales encima).
    pub target_options: Vec<CheckoutTargetOption>,
    /// Vacíos = los textos por defecto de cambiar de rama.
    pub title: SharedString,
    pub description: SharedString,
}

impl Default for CheckoutBranchOptions {
    fn default() -> Self {
        Self {
            target_label: "HEAD".into(),
            has_local_changes: false,
            remote_branch: SharedString::default(),
            target_options: Vec::new(),
            title: SharedString::default(),
            description: SharedString::default(),
        }
    }
}

impl CheckoutBranchOptions {
    pub fn new(target_label: impl Into<SharedString>) -> Self {
        Self {
            target_label: target_label.into(),
            ..Self::default()
        }
    }

    pub fn has_local_changes(mut self, has_local_changes: bool) -> Self {
        self.has_local_changes = has_local_changes;
        self
    }

    pub fn remote_branch(mut self, remote_branch: impl Into<SharedString>) -> Self {
        self.remote_branch = remote_branch.into();
        self
    }

    pub fn target_options(mut self, options: Vec<CheckoutTargetOption>) -> Self {
        self.target_options = options;
        self
    }

    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = title.into();
        self
    }

    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = description.into();
        self
    }

    /// `resolvedTitle`.
    pub fn resolved_title(&self) -> SharedString {
        if !self.title.is_empty() {
            self.title.clone()
        } else if self.remote_branch.is_empty() {
            "Switch Branch".into()
        } else {
            "Track Remote Branch".into()
        }
    }

    /// `resolvedDescription`.
    pub fn resolved_description(&self) -> SharedString {
        if !self.description.is_empty() {
            self.description.clone()
        } else if self.remote_branch.is_empty() {
            "Switch to another branch".into()
        } else {
            "Create a local branch that tracks the remote branch and switch to it".into()
        }
    }
}

/// Lo que emite el diálogo (`emit('submit', ...)` / `emit('close')`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckoutBranchDialogEvent {
    /// Se confirmó. `target` es `None` cuando no había destinos que elegir (quien
    /// abrió el diálogo ya sabe a dónde va).
    Submit {
        local_changes_mode: LocalChangesMode,
        target: Option<SharedString>,
    },
    /// Se cerró sin confirmar.
    Close,
}

/// Estado del diálogo de checkout. Se abre con [`open_checkout_branch_dialog`].
pub struct CheckoutBranchDialog {
    options: CheckoutBranchOptions,
    local_changes_mode: LocalChangesMode,
    selected_target: Option<SharedString>,
    dialog_id: Option<DialogId>,
}

impl EventEmitter<CheckoutBranchDialogEvent> for CheckoutBranchDialog {}

impl CheckoutBranchDialog {
    fn new(options: CheckoutBranchOptions) -> Self {
        let selected_target = options
            .target_options
            .first()
            .map(|option| option.value.clone());
        Self {
            options,
            local_changes_mode: LocalChangesMode::Keep,
            selected_target,
            dialog_id: None,
        }
    }

    pub fn options(&self) -> &CheckoutBranchOptions {
        &self.options
    }

    pub fn local_changes_mode(&self) -> LocalChangesMode {
        self.local_changes_mode
    }

    /// `hasTargetChoice`.
    pub fn has_target_choice(&self) -> bool {
        self.options.target_options.len() > 1
    }

    /// `activeOption`: el destino elegido o, si no hay elección, el primero.
    pub fn active_option(&self) -> Option<&CheckoutTargetOption> {
        self.options
            .target_options
            .iter()
            .find(|option| Some(&option.value) == self.selected_target.as_ref())
            .or_else(|| self.options.target_options.first())
    }

    /// `resolvedConfirmLabel`: el botón dice lo que va a pasar con el destino
    /// elegido.
    pub fn confirm_label(&self) -> SharedString {
        self.active_option()
            .and_then(|option| option.confirm_label.clone())
            .filter(|label| !label.is_empty())
            .unwrap_or_else(|| {
                if self.options.remote_branch.is_empty() {
                    "Checkout".into()
                } else {
                    "Create and Checkout".into()
                }
            })
    }

    pub fn set_local_changes_mode(&mut self, mode: LocalChangesMode, cx: &mut Context<Self>) {
        self.local_changes_mode = mode;
        cx.notify();
    }

    pub fn select_target(&mut self, value: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.selected_target = Some(value.into());
        cx.notify();
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

    /// `handleSubmit`: cierra y avisa de lo elegido.
    pub fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.dialog_id.take() else {
            return;
        };
        dialog::close_id(window, cx, id);
        cx.emit(CheckoutBranchDialogEvent::Submit {
            local_changes_mode: self.local_changes_mode,
            target: self.active_option().map(|option| option.value.clone()),
        });
    }

    /// `handleClose`.
    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_dialog(window, cx);
        cx.emit(CheckoutBranchDialogEvent::Close);
    }

    fn dismissed(&mut self, cx: &mut Context<Self>) {
        self.dialog_id = None;
        cx.emit(CheckoutBranchDialogEvent::Close);
    }
}

impl Render for CheckoutBranchDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let options = &self.options;
        let has_local_changes = options.has_local_changes;
        let tracks_remote = !options.remote_branch.is_empty();

        let target = if self.has_target_choice() {
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(caption("Checkout:", cx))
                .child(
                    mergeon_ui::radio_group("checkout-target")
                        .label("Checkout target")
                        .options(options.target_options.iter().map(|option| {
                            let radio =
                                RadioOption::new(option.value.clone(), option.label.clone());
                            match &option.description {
                                Some(description) => radio.description(description.clone()),
                                None => radio,
                            }
                        }))
                        .selected(self.selected_target.clone().unwrap_or_default())
                        .on_change(cx.listener(|this, value: &SharedString, _, cx| {
                            this.select_target(value.clone(), cx);
                        })),
                )
        } else {
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(caption(
                    if tracks_remote {
                        "Create local branch:"
                    } else {
                        "Switch to:"
                    },
                    cx,
                ))
                .child(branch_chip(
                    if tracks_remote {
                        // Como el backend (`checkout-remote-branch`), hasta el primer `/`.
                        SharedString::from(local_branch_name(&options.remote_branch, &[]))
                    } else {
                        options.target_label.clone()
                    },
                    cx,
                ))
                .when(tracks_remote, |this| {
                    this.child(caption(
                        format!(
                            "It will track the remote branch '{}'.",
                            options.remote_branch
                        ),
                        cx,
                    ))
                })
        };

        // El aviso de HEAD desanclado depende del destino elegido.
        let warning = self
            .active_option()
            .and_then(|option| option.warning.clone())
            .filter(|warning| !warning.is_empty());

        let local_changes = mergeon_ui::radio_group("checkout-local-changes")
            .label("Local changes")
            .option(RadioOption::new("keep", "Keep changes"))
            .option(RadioOption::new("stash", "Stash and reapply"))
            .option(RadioOption::new("discard", "Discard changes"))
            .selected(self.local_changes_mode.as_str())
            .disabled(!has_local_changes)
            .on_change(cx.listener(|this, value: &SharedString, _, cx| {
                this.set_local_changes_mode(local_changes_mode(value), cx);
            }));

        div()
            .flex()
            .gap_3()
            .child(div().w(px(56.)).flex_shrink_0().child(icon_badge(
                Lucide::GitBranch,
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
                    .child(target)
                    .when_some(warning, |this, warning| {
                        this.child(mergeon_ui::alert(AlertVariant::Warning).description(warning))
                    })
                    .child(local_changes_box(has_local_changes, local_changes, cx)),
            )
    }
}

/// Abre el diálogo de checkout. Quien llama se suscribe al
/// [`CheckoutBranchDialogEvent`] de la entidad devuelta.
pub fn open_checkout_branch_dialog(
    options: CheckoutBranchOptions,
    window: &mut Window,
    cx: &mut App,
) -> Entity<CheckoutBranchDialog> {
    let title = options.resolved_title();
    let description = options.resolved_description();
    let view = cx.new(|_| CheckoutBranchDialog::new(options));

    let id = AppDialog::new()
        .title(title)
        .description(description)
        .width(DialogWidth::Lg)
        .body({
            let view = view.clone();
            move |_, _| view.clone()
        })
        .footer({
            let view = view.clone();
            move |_, cx| {
                let confirm_label = view.read(cx).confirm_label();
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        mergeon_ui::button("checkout-cancel")
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
                        mergeon_ui::button("checkout-submit")
                            .sm()
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
        .on_confirm({
            let view = view.clone();
            move |window, cx| {
                view.update(cx, |view, cx| view.submit(window, cx));
                false
            }
        })
        .on_close({
            let view = view.clone();
            move |_, cx| view.update(cx, |view, cx| view.dismissed(cx))
        })
        .open(window, cx);

    view.update(cx, |view, _| view.dialog_id = Some(id));
    view
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_texts_depend_on_whether_a_remote_branch_is_tracked() {
        let local = CheckoutBranchOptions::new("feature");
        assert_eq!(local.resolved_title(), "Switch Branch");
        assert_eq!(local.resolved_description(), "Switch to another branch");

        let remote = CheckoutBranchOptions::new("origin/feature").remote_branch("origin/feature");
        assert_eq!(remote.resolved_title(), "Track Remote Branch");
        assert_eq!(
            remote.resolved_description(),
            "Create a local branch that tracks the remote branch and switch to it"
        );

        // El historial de commits sobreescribe los dos textos.
        let custom = CheckoutBranchOptions::new("d6cf210")
            .title("Checkout commit")
            .description("Check out this commit");
        assert_eq!(custom.resolved_title(), "Checkout commit");
        assert_eq!(custom.resolved_description(), "Check out this commit");
    }

    #[test]
    fn the_confirm_label_follows_the_chosen_target() {
        let mut dialog =
            CheckoutBranchDialog::new(CheckoutBranchOptions::new("d6cf210").target_options(vec![
                CheckoutTargetOption::new("main", "main"),
                CheckoutTargetOption::new("d6cf210", "Commit d6cf210")
                    .warning("You will be in detached HEAD state.")
                    .confirm_label("Checkout commit"),
            ]));

        assert!(dialog.has_target_choice());
        assert_eq!(dialog.confirm_label(), "Checkout");
        assert_eq!(dialog.active_option().and_then(|o| o.warning.clone()), None);

        dialog.selected_target = Some("d6cf210".into());
        assert_eq!(dialog.confirm_label(), "Checkout commit");
        assert_eq!(
            dialog.active_option().and_then(|o| o.warning.clone()),
            Some("You will be in detached HEAD state.".into())
        );
    }

    #[test]
    fn a_single_target_is_not_a_choice() {
        let dialog = CheckoutBranchDialog::new(
            CheckoutBranchOptions::new("main")
                .target_options(vec![CheckoutTargetOption::new("main", "main")]),
        );

        assert!(!dialog.has_target_choice());
        assert_eq!(
            dialog.active_option().map(|option| option.value.clone()),
            Some("main".into())
        );
    }
}
