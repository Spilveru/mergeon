//! Lanzador "Open repository" (`src/components/layout/RepositoryLauncher.vue`):
//! abrir una carpeta local (con el selector del sistema o escribiendo la
//! ruta), clonar desde GitHub o clonar pegando una URL.

use gpui_kit::base::Button as BaseButton;
use gpui_kit::component::Icon;
use gpui_kit::{
    App, AppContext as _, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    Styled as _, Window, div, px, relative, rems,
};
use mergeon_app::AppModel;

use crate::path_field::RepositoryPathField;
use mergeon_ui::dialog::{self, AppDialog, DialogId};
use mergeon_ui::{ActivePaletteExt as _, Lucide, MergeonStyled as _, Radius};

use crate::parts::{HomeDialogs, github_icon, lucide, plain_button};

pub(crate) const TITLE: &str = "Open repository";

/// Cierra el lanzador y abre `path` (ya validada por el campo de ruta).
fn open_typed_path(path: std::path::PathBuf, window: &mut Window, cx: &mut App) {
    if let Some((id, _)) = HomeDialogs::open_dialog(TITLE, window, cx) {
        dialog::close_id(window, cx, id);
    }
    HomeDialogs::forget(TITLE, cx);
    let path = path.to_string_lossy().into_owned();
    AppModel::global(cx).update(cx, |model, cx| {
        model
            .open_repository_path(
                &path,
                "The selected directory is not a valid Git repository",
                cx,
            )
            .detach();
    });
}

/// Qué hacer con el lanzador ya cerrado.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Choice {
    Local,
    GitHub,
    Url,
}

/// `choose(action)`: cierra el lanzador y arranca el flujo elegido.
fn choose(choice: Choice, window: &mut Window, cx: &mut App) {
    if let Some((id, _)) = HomeDialogs::open_dialog(TITLE, window, cx) {
        dialog::close_id(window, cx, id);
    }
    HomeDialogs::forget(TITLE, cx);

    AppModel::global(cx).update(cx, |model, cx| match choice {
        Choice::Local => model.open_local_repository(cx).detach(),
        Choice::GitHub => model.open_github_picker(cx).detach(),
        Choice::Url => model.show_clone_from_url(cx),
    });
}

fn option(
    id: &'static str,
    icon: Icon,
    title: &'static str,
    hint: &'static str,
    choice: Choice,
    window: &mut Window,
    cx: &mut App,
) -> BaseButton {
    let palette = *cx.palette();
    plain_button(id, Radius::Md.px(cx), window, cx)
        .w_full()
        .justify_start()
        .gap(px(14.))
        .p(px(18.))
        .border_1()
        .border_color(palette.border)
        .text_color(palette.foreground)
        .bg(palette.card)
        .hover(|style| style.bg(palette.accent).border_color(palette.primary))
        .on_click(move |_, window, cx| choose(choice, window, cx))
        .child(icon)
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .gap(px(4.))
                .text_left()
                .child(
                    div()
                        .tw_text_sm()
                        .font_weight(FontWeight::BOLD)
                        .child(title),
                )
                .child(
                    // `<small>`: `font-size: smaller` sobre los 14px del diálogo.
                    div()
                        .text_size(rems(0.875 / 1.2))
                        .line_height(relative(20. / 14.))
                        .text_color(palette.muted_foreground)
                        .child(hint),
                ),
        )
        .child(lucide(Lucide::ArrowRight, px(16.)))
}

/// Abre el lanzador. Elegir una opción lo cierra y llama a
/// `AppModel::open_local_repository`, `AppModel::open_github_picker` o
/// `AppModel::show_clone_from_url`.
pub fn open_repository_launcher(window: &mut Window, cx: &mut App) -> DialogId {
    if let Some((id, _)) = HomeDialogs::open_dialog(TITLE, window, cx) {
        return id;
    }

    let path_field = cx.new(|cx| RepositoryPathField::new(open_typed_path, window, cx));
    let field_view = path_field.clone();
    let id = AppDialog::new()
        .title(TITLE)
        .description("What would you like to do?")
        .body(move |window, cx| {
            let palette = *cx.palette();
            // `.launcher-options`
            div()
                .flex()
                .flex_col()
                .gap(px(10.))
                .child(option(
                    "launcher-local",
                    lucide(Lucide::FolderOpen, px(22.)),
                    "Open Local",
                    "Choose a folder on this computer…",
                    Choice::Local,
                    window,
                    cx,
                ))
                .child(option(
                    "launcher-github",
                    github_icon(px(22.)),
                    "Clone from GitHub",
                    "Choose a repository from your accounts",
                    Choice::GitHub,
                    window,
                    cx,
                ))
                .child(option(
                    "launcher-url",
                    lucide(Lucide::Link, px(22.)),
                    "Clone from URL",
                    "Paste an HTTPS or SSH URL, or a local path",
                    Choice::Url,
                    window,
                    cx,
                ))
                .child(
                    // Escribir o pegar la ruta: también sirve cuando el
                    // selector del sistema no aparece.
                    div()
                        .mt(px(6.))
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(
                            div()
                                .tw_text_xs()
                                .text_color(palette.muted_foreground)
                                .child("Or type or paste the path to a repository"),
                        )
                        .child(path_field.clone()),
                )
                .into_any_element()
        })
        .on_close(|_, cx| HomeDialogs::forget(TITLE, cx))
        .open(window, cx);
    HomeDialogs::register(TITLE, id, Some(field_view.into_any()), cx);
    id
}
