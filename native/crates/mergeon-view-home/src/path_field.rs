//! Campo para escribir o pegar la ruta de un repositorio local, con
//! validación (existe, es una carpeta, es un repositorio git; `~` se expande)
//! y Enter para abrir. Está en el lanzador "Open repository" junto a
//! "Open Local", y es el plan B cuando el selector de carpetas del sistema no
//! aparece (en Linux sin `xdg-desktop-portal`).

use std::path::PathBuf;
use std::rc::Rc;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, Role, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window, div, rems,
};
use mergeon_app::AppModel;
use mergeon_ui::dialog::{self, AppDialog, DialogId, DialogWidth};
use mergeon_ui::{ActivePaletteExt as _, InputEvent, InputState, MergeonStyled as _};

use crate::parts::HomeDialogs;

type OnOpen = Rc<dyn Fn(PathBuf, &mut Window, &mut App)>;
type OnCancel = Rc<dyn Fn(&mut Window, &mut App)>;

/// El campo de ruta con su botón "Open" y su mensaje de error.
pub struct RepositoryPathField {
    pub(crate) input: Entity<InputState>,
    error: Option<SharedString>,
    checking: bool,
    on_open: OnOpen,
    _subscription: Subscription,
}

impl RepositoryPathField {
    /// `on_open` recibe la ruta ya validada (la raíz del repositorio).
    pub fn new(
        on_open: impl Fn(PathBuf, &mut Window, &mut App) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let placeholder = if cfg!(windows) {
            "C:\\code\\my-repository"
        } else {
            "~/code/my-repository"
        };
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        let subscription = cx.subscribe_in(
            &input,
            window,
            |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } => this.submit(window, cx),
                InputEvent::Change if this.error.is_some() => {
                    this.error = None;
                    cx.notify();
                }
                _ => {}
            },
        );
        Self {
            input,
            error: None,
            checking: false,
            on_open: Rc::new(on_open),
            _subscription: subscription,
        }
    }

    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.input.update(cx, |state, cx| state.focus(window, cx));
    }

    /// El mensaje de error que se ve bajo el campo.
    pub fn error(&self) -> Option<&SharedString> {
        self.error.as_ref()
    }

    /// Valida la ruta en segundo plano y, si es un repositorio, la abre.
    pub fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.checking {
            return;
        }
        let text = self.input.read(cx).value().to_string();
        let check = AppModel::global(cx)
            .update(cx, |model, cx| model.check_typed_repository_path(&text, cx));
        self.checking = true;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let checked = check.await;
            this.update_in(cx, |this, window, cx| {
                this.checking = false;
                match checked {
                    Ok(path) => {
                        this.error = None;
                        (this.on_open.clone())(path, window, cx);
                    }
                    Err(error) => this.error = Some(error.message().into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

impl Render for RepositoryPathField {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        div()
            .id("repository-path-field")
            .test_support()
            .flex()
            .flex_col()
            .gap(rems(0.375))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(mergeon_ui::input(&self.input)),
                    )
                    .child(
                        mergeon_ui::button("repository-path-open")
                            .sm()
                            .loading(self.checking)
                            .label("Open")
                            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                    ),
            )
            .when_some(self.error.clone(), |this, error| {
                this.child(
                    div()
                        .id("repository-path-error")
                        .role(Role::Status)
                        .aria_label(error.clone())
                        .test_support()
                        .tw_text_xs()
                        .text_color(palette.destructive)
                        .child(error),
                )
            })
    }
}

pub(crate) const FALLBACK_TITLE: &str = "Open a local repository";

/// El campo de ruta del diálogo abierto (el lanzador o el plan B).
#[cfg(test)]
pub(crate) fn path_field_on_screen(
    window: &Window,
    cx: &mut App,
) -> Option<Entity<RepositoryPathField>> {
    HomeDialogs::open_view::<RepositoryPathField>(FALLBACK_TITLE, window, cx).or_else(|| {
        HomeDialogs::open_view::<RepositoryPathField>(crate::launcher::TITLE, window, cx)
    })
}

/// Diálogo con el campo de ruta para cuando el selector de carpetas del
/// sistema no aparece. `on_open` recibe la ruta validada (el diálogo ya
/// cerrado); `on_cancel`, si se cierra sin abrir nada.
pub fn open_repository_path_dialog(
    description: impl Into<SharedString>,
    window: &mut Window,
    cx: &mut App,
    on_open: impl Fn(PathBuf, &mut Window, &mut App) + 'static,
    on_cancel: impl Fn(&mut Window, &mut App) + 'static,
) -> DialogId {
    let dialog_id = Rc::new(std::cell::Cell::new(None::<DialogId>));
    // Se responde una sola vez: abrir, cancelar o cerrar con Esc.
    let done = Rc::new(std::cell::Cell::new(false));
    let on_cancel: OnCancel = Rc::new(on_cancel);
    let field = cx.new(|cx| {
        let dialog_id = dialog_id.clone();
        let done = done.clone();
        RepositoryPathField::new(
            move |path, window, cx| {
                if done.replace(true) {
                    return;
                }
                HomeDialogs::forget(FALLBACK_TITLE, cx);
                if let Some(id) = dialog_id.get() {
                    dialog::close_id(window, cx, id);
                }
                on_open(path, window, cx);
            },
            window,
            cx,
        )
    });
    let id = AppDialog::new()
        .title(FALLBACK_TITLE)
        .description(description)
        .width(DialogWidth::Md)
        .close_on_backdrop(false)
        .body({
            let field = field.clone();
            move |_, _| field.clone()
        })
        .footer({
            let dialog_id = dialog_id.clone();
            let done = done.clone();
            let on_cancel = on_cancel.clone();
            move |_, _| {
                let dialog_id = dialog_id.clone();
                let done = done.clone();
                let on_cancel = on_cancel.clone();
                div().flex().justify_end().child(
                    mergeon_ui::button("repository-path-cancel")
                        .ghost()
                        .sm()
                        .label("Cancel")
                        .on_click(move |_, window, cx| {
                            if done.replace(true) {
                                return;
                            }
                            HomeDialogs::forget(FALLBACK_TITLE, cx);
                            if let Some(id) = dialog_id.get() {
                                dialog::close_id(window, cx, id);
                            }
                            on_cancel(window, cx);
                        }),
                )
            }
        })
        .on_close(move |window, cx| {
            HomeDialogs::forget(FALLBACK_TITLE, cx);
            if !done.replace(true) {
                on_cancel(window, cx);
            }
        })
        .open(window, cx);
    dialog_id.set(Some(id));
    HomeDialogs::register(FALLBACK_TITLE, id, Some(field.clone().into_any()), cx);
    field.update(cx, |field, cx| field.focus(window, cx));
    id
}
