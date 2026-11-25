//! Diálogo Push (`src/components/repository/PushDialog.vue`).

use gpui_kit::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Subscription, Window, div,
};
use mergeon_app::{AppModel, PushRequest};
use mergeon_model::repository_read_model::{BranchItem, RemoteItem};
use mergeon_ui::dialog::{self, AppDialog, DialogId, DialogWidth};
use mergeon_ui::{Lucide, SelectOption};

use super::{action_field, action_layout, is_open};

const TITLE: &str = "Push";

/// Un destino del push: `value` es `<remoto>/<rama>` y `label` lo que se lee.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PushDestination {
    pub value: String,
    pub label: String,
}

/// `destinationOptions`: primero el upstream de la rama elegida, luego la
/// misma rama en cada remoto.
pub(crate) fn push_destination_options(
    branches: &[BranchItem],
    remotes: &[RemoteItem],
    selected_local_branch: &str,
) -> Vec<PushDestination> {
    let mut options: Vec<PushDestination> = Vec::new();
    let upstream = branches
        .iter()
        .filter(|branch| !branch.is_remote)
        .find(|branch| branch.name == selected_local_branch)
        .and_then(|branch| branch.upstream.as_deref())
        .filter(|upstream| !upstream.is_empty());

    if let Some(upstream) = upstream {
        options.push(PushDestination {
            value: upstream.to_string(),
            label: format!("default ({upstream})"),
        });
    }

    for remote in remotes {
        let value = format!("{}/{selected_local_branch}", remote.name);
        if !options.iter().any(|option| option.value == value) {
            options.push(PushDestination {
                label: value.clone(),
                value,
            });
        }
    }

    options
}

/// Formulario del diálogo: qué rama local empujar y a dónde.
pub(crate) struct PushDialog {
    model: Entity<AppModel>,
    selected_local_branch: SharedString,
    selected_destination: SharedString,
    push_all_tags: bool,
    force_push: bool,
    /// Los destinos ofrecidos la última vez, para saber cuándo cambian.
    destinations: Vec<PushDestination>,
    dialog_id: Option<DialogId>,
    _observer: Subscription,
}

impl PushDialog {
    fn new(cx: &mut Context<Self>) -> Self {
        let model = AppModel::global(cx);
        let observer = cx.observe(&model, |this: &mut Self, _, cx| {
            this.sync_destinations(cx);
            cx.notify();
        });
        // `currentBranch || branches[0]?.name || ''`: la rama actual nunca
        // llega vacía (sin rama es `HEAD`).
        let selected_local_branch = model.read(cx).active_repo_current_branch().to_string();
        let mut this = Self {
            model,
            selected_local_branch: selected_local_branch.into(),
            selected_destination: SharedString::default(),
            push_all_tags: false,
            force_push: false,
            destinations: Vec::new(),
            dialog_id: None,
            _observer: observer,
        };
        this.destinations = this.destination_options(cx);
        this.select_first_destination();
        this
    }

    fn destination_options(&self, cx: &App) -> Vec<PushDestination> {
        let repositories = self.model.read(cx).repositories();
        push_destination_options(
            repositories.active_repository_branches(),
            repositories.active_repository_remotes(),
            &self.selected_local_branch,
        )
    }

    fn select_first_destination(&mut self) {
        self.selected_destination = self
            .destinations
            .first()
            .map(|option| SharedString::from(option.value.clone()))
            .unwrap_or_default();
    }

    /// El `watch` de `destinationOptions`: cada vez que cambian se vuelve al
    /// primero (el upstream, si lo hay).
    fn sync_destinations(&mut self, cx: &App) {
        let options = self.destination_options(cx);
        if options != self.destinations {
            self.destinations = options;
            self.select_first_destination();
        }
    }

    /// `canSubmit`.
    fn can_submit(&self) -> bool {
        !self.selected_local_branch.is_empty() && !self.selected_destination.is_empty()
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
        let request = PushRequest {
            branch: self.selected_local_branch.to_string(),
            destination: self.selected_destination.to_string(),
            push_tags: self.push_all_tags,
            force: self.force_push,
        };
        self.close(window, cx);
        self.model
            .update(cx, |model, cx| model.handle_push_submit(request, cx))
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
                mergeon_ui::button("push-cancel")
                    .ghost()
                    .sm()
                    .label("Cancel")
                    .on_click(move |_, window, cx| {
                        cancel.update(cx, |form, cx| form.close(window, cx));
                    }),
            )
            .child(
                mergeon_ui::button("push-submit")
                    .sm()
                    .label("Push")
                    .disabled(!can_submit)
                    .on_click(move |_, window, cx| {
                        submit.update(cx, |form, cx| form.submit(window, cx));
                    }),
            )
    }
}

impl Render for PushDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let local_branches: Vec<SharedString> = self
            .model
            .read(cx)
            .repositories()
            .active_repository_branches()
            .iter()
            .filter(|branch| !branch.is_remote)
            .map(|branch| SharedString::from(branch.name.clone()))
            .collect();
        let destinations: Vec<SelectOption> = self
            .destinations
            .iter()
            .map(|option| SelectOption::new(option.value.clone(), option.label.clone()))
            .collect();

        action_layout(
            Lucide::Upload,
            [
                action_field(
                    "Branch:",
                    mergeon_ui::select("push-branch")
                        .items(local_branches)
                        .value(self.selected_local_branch.clone())
                        .on_change(cx.listener(|this, value: &SharedString, _, cx| {
                            this.selected_local_branch = value.clone();
                            this.sync_destinations(cx);
                            cx.notify();
                        })),
                    cx,
                )
                .into_any_element(),
                action_field(
                    "To:",
                    mergeon_ui::select("push-destination")
                        .items(destinations)
                        .value(self.selected_destination.clone())
                        .on_change(cx.listener(|this, value: &SharedString, _, cx| {
                            this.selected_destination = value.clone();
                            cx.notify();
                        })),
                    cx,
                )
                .into_any_element(),
                div()
                    .flex()
                    .flex_col()
                    .items_start()
                    .gap_2()
                    .child(
                        mergeon_ui::checkbox("push-tags")
                            .label("Push all tags")
                            .checked(self.push_all_tags)
                            .on_change(cx.listener(|this, checked: &bool, _, cx| {
                                this.push_all_tags = *checked;
                                cx.notify();
                            })),
                    )
                    .child(
                        mergeon_ui::checkbox("push-force")
                            .label("Force push")
                            .checked(self.force_push)
                            .on_change(cx.listener(|this, checked: &bool, _, cx| {
                                this.force_push = *checked;
                                cx.notify();
                            })),
                    )
                    .into_any_element(),
            ],
            cx,
        )
    }
}

/// Abre el diálogo Push del repositorio activo. Al enviarlo llama a
/// `AppModel::handle_push_submit`. Devuelve `None` si ya estaba abierto.
pub fn open_push_dialog(window: &mut Window, cx: &mut App) -> Option<DialogId> {
    if is_open(TITLE, window, cx) {
        return None;
    }

    let form = cx.new(PushDialog::new);
    let id = AppDialog::new()
        .title(TITLE)
        .description("Push your local changes to remote repository")
        .width(DialogWidth::Lg)
        .body({
            let form = form.clone();
            move |_, _| form.clone()
        })
        .footer({
            let form = form.clone();
            move |_, cx| PushDialog::footer(&form, cx)
        })
        .open(window, cx);
    form.update(cx, |form, _| form.dialog_id = Some(id));
    Some(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn branch(name: &str, upstream: Option<&str>) -> BranchItem {
        BranchItem {
            name: name.to_string(),
            upstream: upstream.map(str::to_string),
            ..Default::default()
        }
    }

    fn remote(name: &str) -> RemoteItem {
        RemoteItem {
            name: name.to_string(),
            ..Default::default()
        }
    }

    fn values(options: &[PushDestination]) -> Vec<(&str, &str)> {
        options
            .iter()
            .map(|option| (option.value.as_str(), option.label.as_str()))
            .collect()
    }

    #[test]
    fn offers_the_upstream_first_and_then_the_branch_on_each_remote() {
        let options = push_destination_options(
            &[branch("main", Some("origin/main"))],
            &[remote("origin"), remote("fork")],
            "main",
        );

        // `origin/main` no se repite: ya es el destino por defecto.
        assert_eq!(
            values(&options),
            [
                ("origin/main", "default (origin/main)"),
                ("fork/main", "fork/main"),
            ]
        );
    }

    #[test]
    fn a_branch_without_upstream_goes_to_its_namesake_on_each_remote() {
        let options = push_destination_options(
            &[branch("feature/login", None)],
            &[remote("origin")],
            "feature/login",
        );

        assert_eq!(
            values(&options),
            [("origin/feature/login", "origin/feature/login")]
        );
    }

    #[test]
    fn an_upstream_with_another_name_stays_as_the_default_destination() {
        let options = push_destination_options(
            &[branch("work", Some("origin/release/1.x"))],
            &[remote("origin")],
            "work",
        );

        assert_eq!(
            values(&options),
            [
                ("origin/release/1.x", "default (origin/release/1.x)"),
                ("origin/work", "origin/work"),
            ]
        );
    }

    #[test]
    fn there_is_nowhere_to_push_without_remotes() {
        assert!(push_destination_options(&[branch("main", None)], &[], "main").is_empty());
    }

    #[test]
    fn remote_branches_never_provide_the_upstream() {
        let remote_branch = BranchItem {
            is_remote: true,
            ..branch("main", Some("origin/other"))
        };
        let options = push_destination_options(&[remote_branch], &[remote("origin")], "main");

        assert_eq!(values(&options), [("origin/main", "origin/main")]);
    }
}
