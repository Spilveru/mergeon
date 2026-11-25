//! "Clone from URL": desde el lanzador, validación, nombre de carpeta
//! derivado de la URL, clon de extremo a extremo (un repositorio local hace de
//! remoto) y errores sin credenciales a la vista.

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{Entity, TestAppContext};
use mergeon_app::model_types::notify::NotifyType;
use mergeon_app::{AppEvent, DialogRequest};

use super::support::{TestApp, Workspace};
use crate::{CloneFromUrlDialog, open_clone_from_url_dialog, open_repository_launcher};

fn open(app: &TestApp, cx: &mut TestAppContext) -> Entity<CloneFromUrlDialog> {
    let dialog = app.in_window(cx, open_clone_from_url_dialog);
    app.settle(cx);
    dialog
}

/// Escribe en un campo del diálogo como lo haría el usuario: lo enfoca, lo
/// vacía y teclea `text` (un texto vacío se teclea y se borra, para que el
/// campo avise del cambio como al borrarlo a mano).
fn type_into(
    app: &TestApp,
    dialog: &Entity<CloneFromUrlDialog>,
    field: fn(&CloneFromUrlDialog) -> &Entity<mergeon_ui::InputState>,
    text: &str,
    cx: &mut TestAppContext,
) {
    let input = dialog.read_with(cx, |dialog, _| field(dialog).clone());
    app.in_window(cx, |window, cx| {
        input.update(cx, |state, cx| {
            state.set_value("", window, cx);
            state.focus(window, cx);
        });
    });
    app.settle(cx);
    let text = text.to_string();
    app.in_window(cx, |window, cx| {
        if text.is_empty() {
            window.input("x", cx);
            window.press("backspace", cx);
        } else {
            window.input(&text, cx);
        }
    });
    app.settle(cx);
    assert_eq!(value(dialog, field, cx), text);
}

fn value(
    dialog: &Entity<CloneFromUrlDialog>,
    field: fn(&CloneFromUrlDialog) -> &Entity<mergeon_ui::InputState>,
    cx: &mut TestAppContext,
) -> String {
    dialog.read_with(cx, |dialog, cx| field(dialog).read(cx).value().to_string())
}

fn url(dialog: &CloneFromUrlDialog) -> &Entity<mergeon_ui::InputState> {
    &dialog.url
}
fn folder(dialog: &CloneFromUrlDialog) -> &Entity<mergeon_ui::InputState> {
    &dialog.folder
}
fn name(dialog: &CloneFromUrlDialog) -> &Entity<mergeon_ui::InputState> {
    &dialog.name
}

#[gpui_kit::test]
async fn the_launcher_offers_cloning_from_a_url(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    app.in_window(cx, |window, cx| {
        open_repository_launcher(window, cx);
    });
    app.click(cx, "launcher-url");

    assert!(
        app.events()
            .contains(&AppEvent::ShowDialog(DialogRequest::CloneFromUrl))
    );
    // El lanzador se cierra; el diálogo lo abre quien atiende la petición.
    assert!(app.dialog_titles(cx).is_empty());
    app.in_window(cx, |window, cx| {
        assert!(crate::handle_dialog_request(
            &DialogRequest::CloneFromUrl,
            window,
            cx
        ));
    });
    assert_eq!(app.dialog_titles(cx), ["Clone from URL"]);
    // Pedirlo otra vez no apila otro.
    open(&app, cx);
    assert_eq!(app.dialog_titles(cx), ["Clone from URL"]);
}

#[gpui_kit::test]
async fn the_folder_name_follows_the_url_until_it_is_edited(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let dialog = open(&app, cx);

    type_into(
        &app,
        &dialog,
        url,
        "git@github.com:owner/first-repo.git",
        cx,
    );
    assert_eq!(value(&dialog, name, cx), "first-repo");
    type_into(&app, &dialog, url, "https://host.example/team/second", cx);
    assert_eq!(value(&dialog, name, cx), "second");

    type_into(&app, &dialog, name, "my-copy", cx);
    type_into(
        &app,
        &dialog,
        url,
        "https://host.example/team/third.git",
        cx,
    );
    assert_eq!(value(&dialog, name, cx), "my-copy");

    // Vaciado, vuelve a seguir a la URL.
    type_into(&app, &dialog, name, "", cx);
    type_into(
        &app,
        &dialog,
        url,
        "ssh://git@host.example/team/fourth.git",
        cx,
    );
    assert_eq!(value(&dialog, name, cx), "fourth");

    type_into(&app, &dialog, folder, "/srv/code/", cx);
    if !cfg!(windows) {
        assert_eq!(
            app.label(cx, "clone-url-destination").as_deref(),
            Some("/srv/code/fourth")
        );
    }
}

#[gpui_kit::test]
async fn reports_what_is_missing_or_wrong_before_cloning(cx: &mut TestAppContext) {
    let workspace = Workspace::new();
    let taken = workspace.repository("clones/taken", None);
    let app = TestApp::new(cx);
    let dialog = open(&app, cx);
    type_into(&app, &dialog, folder, "", cx);

    app.click(cx, "clone-url-clone");
    assert_eq!(
        app.label(cx, "clone-url-error").as_deref(),
        Some("Enter the URL of the repository to clone.")
    );
    assert_eq!(
        app.label(cx, "clone-url-folder-error").as_deref(),
        Some("Choose the folder to clone into.")
    );

    type_into(&app, &dialog, url, "github.com/owner/repo", cx);
    // Escribir borra el error de la URL.
    assert!(!app.exists(cx, "clone-url-error"));
    app.click(cx, "clone-url-clone");
    assert_eq!(
        app.label(cx, "clone-url-error").as_deref(),
        Some("Add https:// at the start of the URL.")
    );

    // Ya hay un repositorio con ese nombre en la carpeta.
    type_into(&app, &dialog, url, "https://github.com/owner/taken.git", cx);
    type_into(&app, &dialog, folder, &workspace.path("clones"), cx);
    app.click(cx, "clone-url-clone");
    app.wait_for(cx, "the destination check", |cx| {
        dialog.read_with(cx, |dialog, _| !dialog.is_cloning())
    });
    assert!(!app.exists(cx, "clone-url-error"));
    assert_eq!(
        app.label(cx, "clone-url-name-error").as_deref(),
        Some("taken already exists in that folder.")
    );
    assert!(std::path::Path::new(&taken).join("main.txt").exists());

    // Una carpeta que no existe.
    type_into(&app, &dialog, folder, &workspace.path("nowhere"), cx);
    app.click(cx, "clone-url-clone");
    app.wait_for(cx, "the destination check", |cx| {
        dialog.read_with(cx, |dialog, _| !dialog.is_cloning())
    });
    assert_eq!(
        app.label(cx, "clone-url-folder-error").as_deref(),
        Some("That folder doesn't exist.")
    );
    assert_eq!(app.dialog_titles(cx), ["Clone from URL"]);
    assert!(app.host.progress_toasts().is_empty());
}

#[gpui_kit::test]
async fn clones_a_repository_from_its_url_and_opens_it_in_a_tab(cx: &mut TestAppContext) {
    let workspace = Workspace::new();
    let remote = workspace.bare_remote("team-repo");
    let destination = workspace.directory("clones");
    let destination_dir = workspace.base.join("clones");
    let app = TestApp::new(cx);
    app.host.answer_directory(Some(&destination_dir));
    let dialog = open(&app, cx);

    // Pegada con espacios alrededor.
    type_into(&app, &dialog, url, &format!("  {remote}  "), cx);
    assert_eq!(value(&dialog, name, cx), "team-repo");
    // La carpeta, con "Browse…" (el selector del sistema).
    app.click(cx, "clone-url-browse");
    assert_eq!(app.host.directory_requests(), 1);
    assert_eq!(
        value(&dialog, folder, cx),
        destination_dir.to_string_lossy()
    );

    app.click(cx, "clone-url-clone");
    let clone_path = format!("{destination}/team-repo");
    app.wait_for(cx, "the clone to open", |cx| {
        app.model.read_with(cx, |model, _| {
            model
                .open_repositories()
                .iter()
                .any(|repo| repo.path == clone_path)
        })
    });
    assert!(destination_dir.join("team-repo").join("main.txt").exists());
    // El clon de siempre: con su aviso de progreso.
    assert!(
        app.host
            .progress_toasts()
            .iter()
            .any(|toast| toast.title.contains("Clone team-repo"))
    );
    // Clonado y abierto, el diálogo se cierra.
    assert!(app.dialog_titles(cx).is_empty());
    app.model.read_with(cx, |model, _| {
        let recent = &model.recent_repositories()[0];
        assert_eq!(recent.path, clone_path);
        // Un remoto que no es github.com.
        assert!(!recent.is_github);
    });
}

#[gpui_kit::test]
async fn a_failed_clone_keeps_the_dialog_open_without_showing_credentials(cx: &mut TestAppContext) {
    let workspace = Workspace::new();
    let destination = workspace.directory("clones");
    let app = TestApp::new(cx);
    let dialog = open(&app, cx);

    // Nadie escucha en el puerto 9 de la máquina: falla enseguida.
    type_into(
        &app,
        &dialog,
        url,
        "https://alice:s3cr3t-token@127.0.0.1:9/team/repo.git",
        cx,
    );
    assert_eq!(value(&dialog, name, cx), "repo");
    type_into(&app, &dialog, folder, &destination, cx);
    app.click(cx, "clone-url-clone");
    app.wait_for(cx, "the clone to fail", |cx| {
        dialog.read_with(cx, |dialog, _| !dialog.is_cloning())
            && !app.host.messages(NotifyType::Error).is_empty()
    });

    assert_eq!(app.dialog_titles(cx), ["Clone from URL"]);
    let shown = app.label(cx, "clone-url-clone-error").expect("the error");
    assert!(shown.starts_with("Failed to clone repository"), "{shown}");
    let mut texts = vec![shown];
    texts.extend(app.host.messages(NotifyType::Error));
    texts.extend(
        app.host
            .progress_toasts()
            .into_iter()
            .flat_map(|toast| [toast.title, toast.description.unwrap_or_default()]),
    );
    for text in texts {
        assert!(!text.contains("s3cr3t"), "{text}");
        assert!(!text.contains("alice"), "{text}");
    }
    assert!(
        app.model
            .read_with(cx, |model, _| model.open_repositories().is_empty())
    );
    // Nada a medias en el destino.
    assert!(!workspace.base.join("clones").join("repo").exists());
}
