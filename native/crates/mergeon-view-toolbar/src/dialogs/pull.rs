//! Diálogo Pull (`src/components/repository/PullDialog.vue`).

use gpui_kit::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Subscription, Window, div, px, rems,
};
use mergeon_app::{AppModel, PullRequest};
use mergeon_ui::dialog::{self, AppDialog, DialogId, DialogWidth};
use mergeon_ui::{ActivePaletteExt as _, Lucide, MergeonStyled as _, Radius};

use super::{action_field, action_layout, is_open};

const TITLE: &str = "Pull";

/// Formulario del diálogo: de qué remoto y rama traer, y cómo integrarlo.
pub(crate) struct PullDialog {
    model: Entity<AppModel>,
    selected_remote: SharedString,
    selected_remote_branch: SharedString,
    rebase_instead_of_merge: bool,
    stash_and_reapply: bool,
    /// Los valores por defecto con los que se rellenó el formulario; si
    /// cambian con el diálogo abierto, se vuelve a rellenar.
    defaults: (String, String),
    /// Las ramas ofrecidas la última vez, para saber cuándo cambian.
    branch_options: Vec<SharedString>,
    dialog_id: Option<DialogId>,
    _observer: Subscription,
}

/// `remoteBranchOptions`: las ramas conocidas del remoto elegido o, si no se
/// conoce ninguna, la rama actual.
fn remote_branch_options(model: &AppModel, remote: &str) -> Vec<SharedString> {
    let branches = model
        .repositories()
        .active_repository_remotes()
        .iter()
        .find(|entry| entry.name == remote)
        .map(|entry| entry.branches.as_slice())
        .unwrap_or_default();
    if !branches.is_empty() {
        return branches
            .iter()
            .map(|branch| SharedString::from(branch.clone()))
            .collect();
    }

    let current = model.active_repo_current_branch();
    if current.is_empty() {
        Vec::new()
    } else {
        vec![SharedString::from(current.to_string())]
    }
}

impl PullDialog {
    fn new(cx: &mut Context<Self>) -> Self {
        let model = AppModel::global(cx);
        let observer = cx.observe(&model, |this: &mut Self, _, cx| {
            this.sync_with_model(cx);
            cx.notify();
        });
        let mut this = Self {
            model,
            selected_remote: SharedString::default(),
            selected_remote_branch: SharedString::default(),
            rebase_instead_of_merge: false,
            stash_and_reapply: false,
            defaults: Default::default(),
            branch_options: Vec::new(),
            dialog_id: None,
            _observer: observer,
        };
        this.reset(cx);
        this
    }

    fn current_defaults(&self, cx: &App) -> (String, String) {
        let model = self.model.read(cx);
        (
            model.default_remote_name(),
            model.default_remote_branch_name(),
        )
    }

    /// Lo que hacía el `watch` de `visible` y de los valores por defecto.
    fn reset(&mut self, cx: &App) {
        let defaults = self.current_defaults(cx);
        self.selected_remote = defaults.0.clone().into();
        // `defaultRemoteBranch || currentBranch || ''`: el modelo ya cae en la
        // rama actual cuando no hay upstream.
        self.selected_remote_branch = defaults.1.clone().into();
        self.rebase_instead_of_merge = false;
        self.stash_and_reapply = false;
        self.defaults = defaults;
        self.branch_options = remote_branch_options(self.model.read(cx), &self.selected_remote);
    }

    /// El `watch` de `remoteBranchOptions`: si las ramas ofrecidas cambian y
    /// la elegida ya no está, se toma la primera.
    fn sync_branch_options(&mut self, cx: &App) {
        let options = remote_branch_options(self.model.read(cx), &self.selected_remote);
        if options == self.branch_options {
            return;
        }
        if !options.contains(&self.selected_remote_branch) {
            self.selected_remote_branch = options.first().cloned().unwrap_or_default();
        }
        self.branch_options = options;
    }

    fn sync_with_model(&mut self, cx: &App) {
        if self.current_defaults(cx) != self.defaults {
            self.reset(cx);
        } else {
            self.sync_branch_options(cx);
        }
    }

    /// `canSubmit`.
    fn can_submit(&self) -> bool {
        !self.selected_remote.is_empty() && !self.selected_remote_branch.is_empty()
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.dialog_id.take() {
            dialog::close_id(window, cx, id);
        }
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_submit() {
            return;
        }
        let request = PullRequest {
            remote: self.selected_remote.to_string(),
            branch: Some(self.selected_remote_branch.to_string()),
            rebase: self.rebase_instead_of_merge,
            autostash: self.stash_and_reapply,
        };
        self.close(window, cx);
        self.model
            .update(cx, |model, cx| model.handle_pull_submit(request, cx))
            .detach();
    }

    fn footer(form: &Entity<Self>, cx: &App) -> impl IntoElement + use<> {
        let can_submit = form.read(cx).can_submit();
        let cancel = form.clone();
        let submit = form.clone();
        div()
            .flex()
            .justify_end()
            .gap_2()
            .child(
                mergeon_ui::button("pull-cancel")
                    .ghost()
                    .sm()
                    .label("Cancel")
                    .on_click(move |_, window, cx| {
                        cancel.update(cx, |form, cx| form.close(window, cx));
                    }),
            )
            .child(
                mergeon_ui::button("pull-submit")
                    .sm()
                    .label("Pull")
                    .disabled(!can_submit)
                    .on_click(move |_, window, cx| {
                        submit.update(cx, |form, cx| form.submit(window, cx));
                    }),
            )
    }
}

impl Render for PullDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.model.read(cx);
        let remotes: Vec<SharedString> = model
            .repositories()
            .active_repository_remotes()
            .iter()
            .map(|remote| SharedString::from(remote.name.clone()))
            .collect();
        let current_branch: SharedString = model.active_repo_current_branch().to_string().into();
        let has_local_changes = model.active_repo_has_local_changes();
        let palette = *cx.palette();

        action_layout(
            Lucide::Download,
            [
                action_field(
                    "Remote:",
                    mergeon_ui::select("pull-remote")
                        .items(remotes)
                        .value(self.selected_remote.clone())
                        .on_change(cx.listener(|this, value: &SharedString, _, cx| {
                            this.selected_remote = value.clone();
                            this.sync_branch_options(cx);
                            cx.notify();
                        })),
                    cx,
                )
                .into_any_element(),
                action_field(
                    "Branch:",
                    mergeon_ui::select("pull-branch")
                        .items(self.branch_options.clone())
                        .value(self.selected_remote_branch.clone())
                        .on_change(cx.listener(|this, value: &SharedString, _, cx| {
                            this.selected_remote_branch = value.clone();
                            cx.notify();
                        })),
                    cx,
                )
                .into_any_element(),
                action_field(
                    "Into:",
                    // `.git-action-dialog__inline-value`
                    div()
                        .flex()
                        .items_center()
                        .w_full()
                        .h(px(32.))
                        .px(rems(0.75))
                        .border_1()
                        .border_color(palette.border)
                        .radius(Radius::Lg, cx)
                        .bg(palette.background)
                        .text_color(palette.foreground)
                        .text_size(rems(0.8125))
                        .child(current_branch),
                    cx,
                )
                .into_any_element(),
                div()
                    .flex()
                    .flex_col()
                    .items_start()
                    .gap_2()
                    .child(
                        mergeon_ui::checkbox("pull-rebase")
                            .label("Rebase instead of merge")
                            .checked(self.rebase_instead_of_merge)
                            .on_change(cx.listener(|this, checked: &bool, _, cx| {
                                this.rebase_instead_of_merge = *checked;
                                cx.notify();
                            })),
                    )
                    .child(
                        mergeon_ui::checkbox("pull-autostash")
                            .label("Stash and reapply local changes")
                            .checked(self.stash_and_reapply)
                            .disabled(!has_local_changes)
                            .on_change(cx.listener(|this, checked: &bool, _, cx| {
                                this.stash_and_reapply = *checked;
                                cx.notify();
                            })),
                    )
                    .into_any_element(),
            ],
            cx,
        )
    }
}

/// Abre el diálogo Pull del repositorio activo. Al enviarlo llama a
/// `AppModel::handle_pull_submit`. Devuelve `None` si ya estaba abierto.
pub fn open_pull_dialog(window: &mut Window, cx: &mut App) -> Option<DialogId> {
    if is_open(TITLE, window, cx) {
        return None;
    }

    let form = cx.new(PullDialog::new);
    let id = AppDialog::new()
        .title(TITLE)
        .description("Pull remote changes and merge them into your local branch")
        .width(DialogWidth::Lg)
        .body({
            let form = form.clone();
            move |_, _| form.clone()
        })
        .footer({
            let form = form.clone();
            move |_, cx| PullDialog::footer(&form, cx)
        })
        .open(window, cx);
    form.update(cx, |form, _| form.dialog_id = Some(id));
    Some(id)
}
