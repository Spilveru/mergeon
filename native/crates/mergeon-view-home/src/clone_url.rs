//! "Clone from URL": clonar pegando la URL de cualquier remoto (HTTPS, SSH,
//! `git@host:org/repo.git`, `file://` o una ruta local).
//!
//! La URL se valida al pulsar "Clone" (`mergeon_app::clone_url`), el nombre de
//! la carpeta sale de la URL mientras no se edite a mano, y la carpeta donde
//! clonar se elige con "Browse…" como en el selector de GitHub o se escribe
//! (el selector del sistema no siempre aparece en Linux). El clon es
//! `AppModel::clone_repository`: el mismo aviso de progreso, la misma cola, el
//! mismo reintento con credenciales HTTPS y los mismos avisos que el de
//! GitHub. Al terminar se abre en una pestaña.
//!
//! Una URL con credenciales dentro (`https://user:token@host/…`) nunca se
//! enseña tal cual fuera del campo: los errores pasan por
//! `redact_url_credentials`.

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, Role, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window, div, rems,
};
use mergeon_app::AppModel;
use mergeon_app::clone_url::{
    CloneDestinationError, is_github_url, parse_clone_url, repository_name_from_url,
    validate_folder_name,
};
use mergeon_app::local_paths::{clean_typed_path, expand_home};
use mergeon_gitcmd::credentials::redact_url_credentials;
use mergeon_ui::dialog::{self, AppDialog, DialogId, DialogWidth};
use mergeon_ui::{ActivePaletteExt as _, InputEvent, InputState, MergeonStyled as _};

use crate::parts::{HomeDialogs, card_section, field_label};
use crate::util::join_local_path;

pub(crate) const TITLE: &str = "Clone from URL";

/// El formulario del diálogo.
pub struct CloneFromUrlDialog {
    model: Entity<AppModel>,
    dialog_id: Option<DialogId>,
    pub(crate) url: Entity<InputState>,
    pub(crate) folder: Entity<InputState>,
    pub(crate) name: Entity<InputState>,
    /// El nombre se escribió a mano: la URL ya no lo cambia.
    name_edited: bool,
    /// El cambio del nombre lo hace la propia vista (no el usuario).
    syncing_name: bool,
    url_error: Option<SharedString>,
    folder_error: Option<SharedString>,
    name_error: Option<SharedString>,
    /// El fallo de git del último intento, ya sin credenciales.
    clone_error: Option<SharedString>,
    is_cloning: bool,
    _subscriptions: Vec<Subscription>,
}

impl CloneFromUrlDialog {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let model = AppModel::global(cx);
        let url = cx.new(|cx| {
            InputState::new(window, cx).placeholder("https://github.com/owner/repository.git")
        });
        let folder_placeholder = if cfg!(windows) { "C:\\code" } else { "~/code" };
        let default_folder = model.read(cx).default_clone_folder().unwrap_or_default();
        let folder = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(folder_placeholder)
                .default_value(default_folder)
        });
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("repository"));

        let subscriptions = vec![
            cx.subscribe_in(
                &url,
                window,
                |this, _, event: &InputEvent, window, cx| match event {
                    InputEvent::PressEnter { .. } => this.submit(window, cx),
                    InputEvent::Change => this.url_changed(window, cx),
                    _ => {}
                },
            ),
            cx.subscribe_in(
                &folder,
                window,
                |this, _, event: &InputEvent, window, cx| match event {
                    InputEvent::PressEnter { .. } => this.submit(window, cx),
                    InputEvent::Change if this.folder_error.is_some() => {
                        this.folder_error = None;
                        cx.notify();
                    }
                    _ => {}
                },
            ),
            cx.subscribe_in(
                &name,
                window,
                |this, state, event: &InputEvent, window, cx| {
                    match event {
                        InputEvent::PressEnter { .. } => this.submit(window, cx),
                        InputEvent::Change if !this.syncing_name => {
                            // Vaciarlo vuelve a seguir a la URL.
                            this.name_edited = !state.read(cx).value().trim().is_empty();
                            this.name_error = None;
                            cx.notify();
                        }
                        _ => {}
                    }
                },
            ),
        ];

        Self {
            model,
            dialog_id: None,
            url,
            folder,
            name,
            name_edited: false,
            syncing_name: false,
            url_error: None,
            folder_error: None,
            name_error: None,
            clone_error: None,
            is_cloning: false,
            _subscriptions: subscriptions,
        }
    }

    /// Si hay un clon en curso.
    pub fn is_cloning(&self) -> bool {
        self.is_cloning
    }

    /// Los errores que se ven: URL, carpeta, nombre y el del clon.
    pub fn errors(&self) -> [Option<&SharedString>; 4] {
        [
            self.url_error.as_ref(),
            self.folder_error.as_ref(),
            self.name_error.as_ref(),
            self.clone_error.as_ref(),
        ]
    }

    /// El destino completo según lo escrito (carpeta + nombre), o `None` si
    /// falta alguno de los dos.
    pub fn clone_path(&self, cx: &App) -> Option<String> {
        let folder = clean_typed_path(&self.folder.read(cx).value()).to_string();
        let name = self.name.read(cx).value().trim().to_string();
        if folder.is_empty() || name.is_empty() {
            return None;
        }
        let folder = expand_home(&folder).to_string_lossy().into_owned();
        Some(join_local_path(&folder, &name, cfg!(windows)))
    }

    fn url_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.url_error = None;
        self.clone_error = None;
        if !self.name_edited {
            let derived = repository_name_from_url(&self.url.read(cx).value()).unwrap_or_default();
            if self.name.read(cx).value() != derived.as_str() {
                self.syncing_name = true;
                self.name
                    .update(cx, |state, cx| state.set_value(derived, window, cx));
                self.syncing_name = false;
                self.name_error = None;
            }
        }
        cx.notify();
    }

    /// "Browse…": la carpeta donde clonar, con el selector del sistema.
    fn browse(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let pick = self.model.update(cx, |model, cx| model.pick_directory(cx));
        cx.spawn_in(window, async move |this, cx| {
            let Some(path) = pick.await else {
                return;
            };
            this.update_in(cx, |this, window, cx| {
                let path = path.to_string_lossy().into_owned();
                this.folder
                    .update(cx, |state, cx| state.set_value(path, window, cx));
                this.folder_error = None;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Valida lo escrito y, si todo cuadra, comprueba el destino en disco y
    /// clona. Clonado, abre el repositorio y cierra el diálogo; si falla, el
    /// diálogo sigue abierto con el error.
    pub fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_cloning {
            return;
        }
        let windows = cfg!(windows);
        let url = parse_clone_url(&self.url.read(cx).value(), windows);
        let name = validate_folder_name(&self.name.read(cx).value(), windows).map(str::to_string);
        let folder = clean_typed_path(&self.folder.read(cx).value()).to_string();

        self.url_error = url.as_ref().err().map(|error| error.message().into());
        self.name_error = name.as_ref().err().map(|error| error.message().into());
        self.folder_error = folder
            .is_empty()
            .then(|| CloneDestinationError::NoFolder.message().into());
        self.clone_error = None;
        let (Ok(url), Ok(name), false) = (url, name, folder.is_empty()) else {
            cx.notify();
            return;
        };

        let folder = expand_home(&folder).to_string_lossy().into_owned();
        let target = join_local_path(&folder, &name, windows);
        self.is_cloning = true;
        cx.notify();

        let model = self.model.clone();
        let check = model.update(cx, |model, cx| model.check_clone_destination(&target, cx));
        cx.spawn_in(window, async move |this, cx| {
            if let Err(error) = check.await {
                this.update(cx, |this, cx| {
                    this.is_cloning = false;
                    let message = Some(error.message().into());
                    if error.is_about_the_name() {
                        this.name_error = message;
                    } else {
                        this.folder_error = message;
                    }
                    cx.notify();
                })
                .ok();
                return;
            }

            // El token de la cuenta de GitHub solo viaja a github.com
            // (`auth_config`): con otra URL no se usa.
            let cloning = model.update(cx, |model, cx| {
                let token = model.git_auth_token();
                model.clone_repository(&url, &target, token, cx)
            });
            let result = cloning.await;
            let cloned = result
                .path
                .clone()
                .filter(|path| result.success && !path.is_empty());
            // Se abre aunque el diálogo ya se haya cerrado.
            if let Some(path) = &cloned {
                model
                    .update(cx, |model, cx| {
                        model.handle_repository_cloned_from(path, is_github_url(&url), cx)
                    })
                    .detach();
            }
            this.update_in(cx, |this, window, cx| {
                this.is_cloning = false;
                if cloned.is_some() {
                    this.close(window, cx);
                } else if !result.message.is_empty() {
                    this.clone_error = Some(redact_url_credentials(&result.message).into());
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.dialog_id.take() {
            dialog::close_id(window, cx, id);
        }
        HomeDialogs::forget(TITLE, cx);
    }

    fn render_footer(&mut self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let is_cloning = self.is_cloning;
        div()
            .flex()
            .flex_1()
            .justify_end()
            .gap_3()
            .child(
                mergeon_ui::button("clone-url-cancel")
                    .ghost()
                    .label("Cancel")
                    .on_click(cx.listener(|this, _, window, cx| this.close(window, cx))),
            )
            .child(
                mergeon_ui::button("clone-url-clone")
                    .loading(is_cloning)
                    .disabled(is_cloning)
                    .label(if is_cloning { "Cloning..." } else { "Clone" })
                    .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
            )
            .into_any_element()
    }
}

/// El mensaje de error bajo un campo.
fn field_error(
    id: &'static str,
    error: Option<SharedString>,
    cx: &App,
) -> Option<impl IntoElement> {
    let palette = cx.palette();
    error.map(|error| {
        div()
            .id(id)
            .role(Role::Status)
            .aria_label(error.clone())
            .test_support()
            .tw_text_xs()
            .text_color(palette.destructive)
            .child(error)
    })
}

impl Render for CloneFromUrlDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let is_cloning = self.is_cloning;
        let destination = self.clone_path(cx);

        div()
            .id("clone-url-dialog")
            .test_support()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                card_section(cx)
                    .flex()
                    .flex_col()
                    .gap(rems(0.375))
                    .child(field_label("Repository URL", cx))
                    .child(mergeon_ui::input(&self.url).disabled(is_cloning))
                    .children(field_error("clone-url-error", self.url_error.clone(), cx))
                    .child(
                        div()
                            .tw_text_xs()
                            .text_color(palette.muted_foreground)
                            .child("HTTPS, SSH (git@host:owner/repo.git) or a local path."),
                    ),
            )
            .child(
                card_section(cx)
                    .flex()
                    .flex_col()
                    .gap(rems(0.375))
                    .child(field_label("Clone into", cx))
                    .child(
                        div()
                            .flex()
                            .gap_3()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(mergeon_ui::input(&self.folder).disabled(is_cloning)),
                            )
                            .child(
                                mergeon_ui::button("clone-url-browse")
                                    .outline()
                                    .disabled(is_cloning)
                                    .label("Browse…")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.browse(window, cx);
                                    })),
                            ),
                    )
                    .children(field_error(
                        "clone-url-folder-error",
                        self.folder_error.clone(),
                        cx,
                    ))
                    .child(field_label("Folder name", cx).mt_1())
                    .child(mergeon_ui::input(&self.name).disabled(is_cloning))
                    .children(field_error(
                        "clone-url-name-error",
                        self.name_error.clone(),
                        cx,
                    ))
                    .when_some(destination, |this, destination| {
                        this.child(
                            div()
                                .id("clone-url-destination")
                                .aria_label(SharedString::from(destination.clone()))
                                .test_support()
                                .tw_text_xs()
                                .text_color(palette.muted_foreground)
                                .child(SharedString::from(format!("Clones into {destination}"))),
                        )
                    }),
            )
            .children(field_error(
                "clone-url-clone-error",
                self.clone_error.clone(),
                cx,
            ))
    }
}

/// Abre "Clone from URL". Si ya está abierto devuelve el que hay.
pub fn open_clone_from_url_dialog(window: &mut Window, cx: &mut App) -> Entity<CloneFromUrlDialog> {
    if let Some(view) = HomeDialogs::open_view::<CloneFromUrlDialog>(TITLE, window, cx) {
        return view;
    }

    let view = cx.new(|cx| CloneFromUrlDialog::new(window, cx));
    let id = AppDialog::new()
        .title(TITLE)
        .description("Clone any Git repository and open it in a new tab.")
        .width(DialogWidth::Xl)
        .close_on_backdrop(false)
        .body({
            let view = view.clone();
            move |_, _| view.clone()
        })
        .footer({
            let view = view.downgrade();
            move |_, cx| {
                view.update(cx, |view, cx| view.render_footer(cx))
                    .unwrap_or_else(|_| div().into_any_element())
            }
        })
        .on_close(|_, cx| HomeDialogs::forget(TITLE, cx))
        .open(window, cx);

    view.update(cx, |view, cx| {
        view.dialog_id = Some(id);
        view.url.update(cx, |state, cx| state.focus(window, cx));
    });
    HomeDialogs::register(TITLE, id, Some(view.clone().into_any()), cx);
    view
}
