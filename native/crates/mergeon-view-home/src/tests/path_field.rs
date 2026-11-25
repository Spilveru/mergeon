//! El campo de ruta del lanzador y el diálogo de plan B cuando el selector de
//! carpetas del sistema no aparece.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gpui_kit::TestAppContext;
use gpui_kit::test::TestWindowExt as _;

use super::support::{TestApp, Workspace};
use crate::{open_repository_launcher, open_repository_path_dialog};

#[gpui_kit::test]
async fn the_launcher_opens_a_typed_repository_path(cx: &mut TestAppContext) {
    let workspace = Workspace::new();
    let repository = workspace.repository("work/typed", None);
    let plain = workspace.directory("work/plain");
    let app = TestApp::new(cx);
    app.in_window(cx, |window, cx| {
        open_repository_launcher(window, cx);
    });
    assert!(app.exists(cx, "repository-path-field"));

    // Un directorio que no es repositorio: el error se ve bajo el campo y el
    // lanzador sigue abierto, sin abrir el selector del sistema.
    set_path(&app, &plain, cx);
    app.in_window(cx, |window, cx| window.press("enter", cx));
    app.settle(cx);
    assert_eq!(
        app.label(cx, "repository-path-error").as_deref(),
        Some("That folder isn't a Git repository.")
    );
    assert_eq!(app.dialog_titles(cx), ["Open repository"]);
    assert_eq!(app.host.directory_requests(), 0);

    // Pegada con comillas y espacios, con el botón.
    set_path(&app, &format!("  \"{repository}\"  "), cx);
    app.click(cx, "repository-path-open");
    app.wait_for(cx, "the typed repository to open", |cx| {
        app.model.read_with(cx, |model, _| {
            model.open_repositories().iter().any(|repo| {
                std::fs::canonicalize(&repo.path).ok() == std::fs::canonicalize(&repository).ok()
            })
        })
    });
    assert!(app.dialog_titles(cx).is_empty());
}

#[gpui_kit::test]
async fn a_missing_folder_is_reported_under_the_field(cx: &mut TestAppContext) {
    let workspace = Workspace::new();
    let app = TestApp::new(cx);
    app.in_window(cx, |window, cx| {
        open_repository_launcher(window, cx);
    });
    set_path(&app, &workspace.path("nowhere/at/all"), cx);
    app.click(cx, "repository-path-open");
    assert_eq!(
        app.label(cx, "repository-path-error").as_deref(),
        Some("That folder doesn't exist.")
    );
}

/// Escribe en el campo de ruta del diálogo de arriba.
fn set_path(app: &TestApp, text: &str, cx: &mut TestAppContext) {
    let text = text.to_string();
    app.in_window(cx, |window, cx| {
        let field =
            crate::path_field::path_field_on_screen(window, cx).expect("a path field on screen");
        field.update(cx, |field, cx| {
            field
                .input
                .update(cx, |state, cx| state.set_value(text, window, cx));
            field.focus(window, cx);
        });
    });
    app.settle(cx);
}

#[gpui_kit::test]
async fn the_fallback_dialog_answers_with_the_typed_path_or_nothing(cx: &mut TestAppContext) {
    let workspace = Workspace::new();
    let repository = workspace.repository("work/fallback", None);
    let app = TestApp::new(cx);
    let opened: Rc<RefCell<Vec<PathBuf>>> = Rc::default();
    let cancelled = Rc::new(RefCell::new(0));

    let open = |app: &TestApp, cx: &mut TestAppContext| {
        let (opened, cancelled) = (opened.clone(), cancelled.clone());
        app.in_window(cx, move |window, cx| {
            open_repository_path_dialog(
                "The system folder picker hasn't opened.",
                window,
                cx,
                move |path, _, _| opened.borrow_mut().push(path),
                move |_, _| *cancelled.borrow_mut() += 1,
            );
        });
        app.settle(cx);
    };

    open(&app, cx);
    assert_eq!(app.dialog_titles(cx), ["Open a local repository"]);
    set_path(&app, "", cx);
    app.in_window(cx, |window, cx| window.press("enter", cx));
    app.settle(cx);
    assert_eq!(
        app.label(cx, "repository-path-error").as_deref(),
        Some("Enter the path to a repository.")
    );

    set_path(&app, &repository, cx);
    app.in_window(cx, |window, cx| window.press("enter", cx));
    app.wait_for(cx, "the path to be accepted", |_| {
        !opened.borrow().is_empty()
    });
    assert_eq!(
        std::fs::canonicalize(&opened.borrow()[0]).unwrap(),
        std::fs::canonicalize(&repository).unwrap()
    );
    assert!(app.dialog_titles(cx).is_empty());
    assert_eq!(*cancelled.borrow(), 0);

    open(&app, cx);
    app.click(cx, "repository-path-cancel");
    assert_eq!(*cancelled.borrow(), 1);
    assert_eq!(opened.borrow().len(), 1);
}
