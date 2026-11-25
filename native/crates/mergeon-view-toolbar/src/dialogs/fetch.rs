//! Diálogo Fetch (`src/components/repository/FetchDialog.vue`).

use gpui_kit::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Subscription, Window, div,
};
use mergeon_app::{AppModel, FetchRequest};
use mergeon_ui::Lucide;
use mergeon_ui::dialog::{self, AppDialog, DialogId, DialogWidth};

use super::{action_field, action_layout, is_open};

const TITLE: &str = "Fetch";

/// Formulario del diálogo: qué remoto traer, o todos.
pub(crate) struct FetchDialog {
    model: Entity<AppModel>,
    selected_remote: SharedString,
    all_remotes: bool,
    dialog_id: Option<DialogId>,
    /// Los remotos del repositorio activo: del modelo solo enseña eso, así
    /// que solo se repinta cuando cambian (no con cada aviso del modelo).
    shown_remotes: Vec<SharedString>,
    _observer: Subscription,
}

impl FetchDialog {
    fn new(cx: &mut Context<Self>) -> Self {
        let model = AppModel::global(cx);
        // `defaultRemote || remotes[0]?.name || 'origin'` ya lo resuelve el modelo.
        let selected_remote = model.read(cx).default_remote_name().into();
        let observer = cx.observe(&model, |this: &mut Self, model, cx| {
            let remotes = remote_options(model.read(cx));
            if remotes != this.shown_remotes {
                this.shown_remotes = remotes;
                cx.notify();
            }
        });
        let shown_remotes = remote_options(model.read(cx));
        Self {
            model,
            selected_remote,
            all_remotes: true,
            dialog_id: None,
            shown_remotes,
            _observer: observer,
        }
    }

    /// `canSubmit`.
    fn can_submit(&self) -> bool {
        self.all_remotes || !self.selected_remote.is_empty()
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
        let request = FetchRequest {
            remote: self.selected_remote.to_string(),
            all_remotes: self.all_remotes,
        };
        // `handleFetchSubmit`: primero se cierra, luego corre el fetch.
        self.close(window, cx);
        self.model
            .update(cx, |model, cx| model.handle_fetch_submit(request, cx))
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
                mergeon_ui::button("fetch-cancel")
                    .ghost()
                    .sm()
                    .label("Cancel")
                    .on_click(move |_, window, cx| {
                        cancel.update(cx, |form, cx| form.close(window, cx));
                    }),
            )
            .child(
                mergeon_ui::button("fetch-submit")
                    .sm()
                    .label("Fetch")
                    .disabled(!can_submit)
                    .on_click(move |_, window, cx| {
                        submit.update(cx, |form, cx| form.submit(window, cx));
                    }),
            )
    }
}

impl Render for FetchDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Lo leído al avisar el modelo: pintar no lo lee (cada aviso suyo
        // repintaría la ventana).
        let remotes = self.shown_remotes.clone();
        let no_remotes = remotes.is_empty();

        action_layout(
            Lucide::ArrowDownToLine,
            [
                action_field(
                    "Remote:",
                    mergeon_ui::select("fetch-remote")
                        .items(remotes)
                        .value(self.selected_remote.clone())
                        .disabled(self.all_remotes || no_remotes)
                        .on_change(cx.listener(|this, value: &SharedString, _, cx| {
                            this.selected_remote = value.clone();
                            cx.notify();
                        })),
                    cx,
                )
                .into_any_element(),
                mergeon_ui::checkbox("fetch-all-remotes")
                    .label("Fetch all remotes")
                    .checked(self.all_remotes)
                    .on_change(cx.listener(|this, checked: &bool, _, cx| {
                        this.all_remotes = *checked;
                        cx.notify();
                    }))
                    .into_any_element(),
            ],
            cx,
        )
    }
}

/// Los remotos del repositorio activo, por nombre.
fn remote_options(model: &AppModel) -> Vec<SharedString> {
    model
        .repositories()
        .active_repository_remotes()
        .iter()
        .map(|remote| SharedString::from(remote.name.clone()))
        .collect()
}

/// Abre el diálogo Fetch del repositorio activo. Al enviarlo llama a
/// `AppModel::handle_fetch_submit`. Devuelve `None` si ya estaba abierto.
pub fn open_fetch_dialog(window: &mut Window, cx: &mut App) -> Option<DialogId> {
    if is_open(TITLE, window, cx) {
        return None;
    }

    let form = cx.new(FetchDialog::new);
    let id = AppDialog::new()
        .title(TITLE)
        .description("Fetch latest changes from remote repository")
        .width(DialogWidth::Lg)
        .body({
            let form = form.clone();
            move |_, _| form.clone()
        })
        .footer({
            let form = form.clone();
            move |_, cx| FetchDialog::footer(&form, cx)
        })
        .open(window, cx);
    form.update(cx, |form, _| form.dialog_id = Some(id));
    Some(id)
}
