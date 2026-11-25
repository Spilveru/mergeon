//! La pestaña "Local Changes" con la disposición de Señal: la lista con sus
//! cifras `+N −N` y, debajo, la caja de commit; a la derecha el diff. Y la
//! plantilla de commit (`commit.template`).

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{Bounds, ElementId, Entity, Pixels, TestAppContext, px};
use mergeon_view_changes::{ChangesMode, ChangesView, ChangesViewEvent};

use crate::support::{TestApp, TestRepo};

fn commit_parts(view: &Entity<ChangesView>, cx: &mut TestAppContext) -> (String, String) {
    view.read_with(cx, |view, cx| {
        (
            view.commit_title(cx).to_string(),
            view.commit_body(cx).to_string(),
        )
    })
}

/// Lo que hace la app cuando el watcher ve cambiar el working tree.
async fn watcher_reread(app: &TestApp, repo_id: &str, cx: &mut TestAppContext) {
    app.model
        .update(cx, |model, cx| {
            model.sync_repository_read_model(
                repo_id,
                mergeon_app::SyncRequest::new()
                    .status(true)
                    .operation("status-watch"),
                cx,
            )
        })
        .await;
}

fn wait_for_label(app: &TestApp, cx: &mut TestAppContext, id: &str, expected: &str) {
    let id = id.to_string();
    let expected = expected.to_string();
    app.wait_until(cx, &format!("{id} to read {expected:?}"), move |cx| {
        app.label(cx, ElementId::Name(id.clone().into())).as_deref() == Some(expected.as_str())
    });
}

fn bounds(app: &TestApp, cx: &mut TestAppContext, id: &'static str) -> Bounds<Pixels> {
    app.in_window(cx, |window, _| window.find(id).bounds())
}

// ---------------------------------------------------------------------------
// Cifras de la lista
// ---------------------------------------------------------------------------

#[gpui_kit::test]
async fn the_list_shows_the_line_stats_of_each_side(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.commit_file("src/a.js", "one\ntwo\nthree\n", "Add a");
    repo.commit_file("old name.txt", "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n", "Add old");
    // Sin preparar: una línea cambiada y otra nueva.
    repo.write("src/a.js", "one\nTWO\nthree\nfour\n");
    // Renombre preparado con una línea cambiada.
    repo.git(&["mv", "old name.txt", "new name.txt"]);
    repo.write("new name.txt", "1\n2\n3\n4\n5\n6\n7\n8\n9\nten\n");
    repo.git(&["add", "new name.txt"]);
    // Sin seguimiento: un texto (sus líneas, añadidas) y un binario.
    repo.write("notas ñandú.md", "a\nb\nc\n");
    std::fs::write(repo.path.join("blob.bin"), [0u8, 1, 2, 3]).expect("write blob");
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount(cx, &repo_id, ChangesMode::List);
    app.settle(cx, &view);

    wait_for_label(&app, cx, "stats:unstaged:src/a.js", "+2 \u{2212}1");
    assert_eq!(
        app.label(cx, "stats:staged:new name.txt").as_deref(),
        Some("+1 \u{2212}1")
    );
    assert_eq!(
        app.label(cx, "stats:unstaged:notas ñandú.md").as_deref(),
        Some("+3")
    );
    // El binario sale en la lista, sin cifras.
    assert!(app.exists(cx, "unstaged:file:blob.bin"));
    assert!(!app.exists(cx, "stats:unstaged:blob.bin"));
    assert_eq!(
        view.read_with(cx, |view, _| view.file_line_stats("src/a.js", false)),
        Some((2, 1))
    );
    assert_eq!(
        view.read_with(cx, |view, _| view.file_line_stats("src/a.js", true)),
        None
    );

    // El watcher: el archivo vuelve a cambiar y el estado queda igual (` M`),
    // pero las cifras se releen.
    repo.write("src/a.js", "one\nTWO\nthree\nfour\nfive\n");
    watcher_reread(&app, &repo_id, cx).await;
    wait_for_label(&app, cx, "stats:unstaged:src/a.js", "+3 \u{2212}1");

    // Preparar mueve las cifras a su lado.
    app.click(cx, "check:unstaged:file:src/a.js");
    wait_for_label(&app, cx, "stats:staged:src/a.js", "+3 \u{2212}1");
    assert!(!app.exists(cx, "stats:unstaged:src/a.js"));
}

/// Con miles de archivos las cifras se leen en segundo plano una vez por
/// cambio, y la lista no las recalcula al pintar.
#[gpui_kit::test]
async fn a_list_of_thousands_of_files_gets_its_stats(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    for folder in 0..20 {
        for file in 0..100 {
            repo.write(&format!("dir{folder:02}/file{file:03}.txt"), "line\n");
        }
    }
    repo.git(&["add", "--all"]);
    repo.git(&["commit", "-q", "-m", "Many files"]);
    for folder in 0..20 {
        for file in 0..100 {
            repo.write(&format!("dir{folder:02}/file{file:03}.txt"), "line\nmore\n");
        }
    }
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount(cx, &repo_id, ChangesMode::List);
    app.settle(cx, &view);

    wait_for_label(&app, cx, "stats:unstaged:dir00/file000.txt", "+1");
    assert_eq!(
        view.read_with(cx, |view, _| view
            .file_line_stats("dir19/file099.txt", false)),
        Some((1, 0))
    );
}

// ---------------------------------------------------------------------------
// Disposición
// ---------------------------------------------------------------------------

#[gpui_kit::test]
async fn the_commit_box_goes_under_the_list_and_the_diff_on_the_right(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.commit_file("src/a.js", "a\n", "Add a");
    repo.write("src/a.js", "A\n");
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount(cx, &repo_id, ChangesMode::List);
    app.settle(cx, &view);

    let list = bounds(&app, cx, "changes-list");
    let commit = bounds(&app, cx, "commit-panel");
    let resizer = bounds(&app, cx, "changes-resizer");
    let main = bounds(&app, cx, "changes-main");

    // La caja de commit, en la columna de la lista, debajo de ella y hasta abajo.
    assert_eq!(commit.left(), list.left());
    assert_eq!(commit.size.width, list.size.width);
    assert!(commit.top() >= list.bottom(), "{commit:?} / {list:?}");
    assert_eq!(commit.bottom(), main.bottom());
    // El diff, a la derecha del divisor y a toda la altura.
    assert!(list.right() <= resizer.left());
    assert!(main.left() >= resizer.right());
    assert_eq!(main.top(), list.top());
    assert!(main.size.width > px(500.));

    // Sin archivo elegido, el diff deja el aviso; con uno, su diff.
    assert!(app.exists(cx, "changes-no-selection"));
    app.click(cx, "unstaged:file:src/a.js");
    app.wait_until(cx, "the diff", |cx| {
        view.read_with(cx, |view, _| view.file_diff().contains("+A"))
    });
    assert!(!app.exists(cx, "changes-no-selection"));
    let toggle = bounds(&app, cx, "toggle-diff-view");
    assert!(toggle.left() >= resizer.right());
}

/// Las secciones miden lo que su contenido: los preparados van justo debajo
/// de los sin preparar, y con cientos de archivos sin preparar los dos
/// preparados se siguen viendo.
#[gpui_kit::test]
async fn the_sections_fit_their_content(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.write("a.txt", "a\n");
    repo.write("b.txt", "b\n");
    repo.write("c.txt", "c\n");
    repo.git(&["add", "b.txt", "c.txt"]);
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount(cx, &repo_id, ChangesMode::List);
    app.settle(cx, &view);

    let list = bounds(&app, cx, "changes-list");
    let unstaged = bounds(&app, cx, "unstaged-section");
    let staged = bounds(&app, cx, "staged-section");
    assert_eq!(staged.top(), unstaged.bottom());
    // Lo que sobra queda debajo.
    assert!(
        staged.bottom() < list.bottom() - px(200.),
        "{staged:?} / {list:?}"
    );
    let row = bounds(&app, cx, "unstaged:file:a.txt");
    assert!(row.bottom() <= unstaged.bottom());

    for index in 0..300 {
        repo.write(&format!("many/file{index:03}.txt"), "x\n");
    }
    watcher_reread(&app, &repo_id, cx).await;
    app.settle(cx, &view);
    let list = bounds(&app, cx, "changes-list");
    let staged = bounds(&app, cx, "staged-section");
    assert!(staged.bottom() <= list.bottom());
    for path in ["b.txt", "c.txt"] {
        let row = bounds(
            &app,
            cx,
            if path == "b.txt" {
                "staged:file:b.txt"
            } else {
                "staged:file:c.txt"
            },
        );
        assert!(
            row.bottom() <= staged.bottom(),
            "{path}: {row:?} / {staged:?}"
        );
    }
}

#[gpui_kit::test]
async fn a_narrow_window_stacks_the_columns(cx: &mut TestAppContext) {
    let app = TestApp::with_size(880., 800., cx);
    let repo = TestRepo::new();
    repo.write("notes.txt", "notes\n");
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount(cx, &repo_id, ChangesMode::List);
    app.settle(cx, &view);

    let list = bounds(&app, cx, "changes-list");
    let commit = bounds(&app, cx, "commit-panel");
    let main = bounds(&app, cx, "changes-main");
    assert!(!app.exists(cx, "changes-resizer"));
    // La lista, la caja de commit y el diff, uno encima de otro a todo el ancho.
    assert!(list.bottom() <= commit.top());
    assert!(commit.bottom() <= main.top());
    assert_eq!(list.size.width, px(880.));
    assert_eq!(main.size.width, px(880.));
    assert!(main.size.height >= px(160.));
    // La lista no queda aplastada por la caja de commit.
    assert!(list.size.height >= px(180.), "{list:?}");
}

// ---------------------------------------------------------------------------
// Plantilla de commit
// ---------------------------------------------------------------------------

const TEMPLATE: &str = "feat: \n\n# Why this change?\nRefs: #\n";

/// Un repositorio con `commit.template` (relativo al repositorio) y algo
/// preparado.
fn repo_with_template() -> TestRepo {
    let repo = TestRepo::new();
    std::fs::write(repo.path.join(".git/commit template.txt"), TEMPLATE).expect("template");
    repo.git(&["config", "commit.template", ".git/commit template.txt"]);
    repo.write("src/a.js", "a\n");
    repo.git(&["add", "src/a.js"]);
    repo
}

#[gpui_kit::test]
async fn a_commit_template_fills_an_empty_message_without_its_comments(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = repo_with_template();
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount(cx, &repo_id, ChangesMode::List);
    app.settle(cx, &view);

    app.wait_until(cx, "the template", |cx| {
        commit_parts(&view, cx) == ("feat: ".to_string(), "Refs: #".to_string())
    });

    // Se completa y se hace commit: el mensaje es el escrito, sin comentarios.
    app.click(cx, "commit-title");
    app.in_window(cx, |window, cx| {
        window.press("end", cx);
        window.input("add a", cx);
    });
    app.click(cx, "commit");
    app.wait_until(cx, "the commit", |_| {
        app.events
            .borrow()
            .iter()
            .any(|event| matches!(event, ChangesViewEvent::CommitCreated { .. }))
    });
    app.settle(cx, &view);
    assert_eq!(
        repo.git(&["log", "-1", "--format=%B"]),
        "feat: add a\n\nRefs: #"
    );
    // Como git, el siguiente commit vuelve a partir de la plantilla.
    assert_eq!(
        commit_parts(&view, cx),
        ("feat: ".to_string(), "Refs: #".to_string())
    );
}

#[gpui_kit::test]
async fn a_commit_template_never_replaces_a_draft(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = repo_with_template();
    let repo_id = app.open(&repo, cx).await;
    let path = app.repo_path(cx, &repo_id);
    app.model.update(cx, |model, cx| {
        model.set_commit_draft(&path, "My own subject\n\nMy own body", cx);
    });

    let view = app.mount(cx, &repo_id, ChangesMode::List);
    app.settle(cx, &view);
    // Lo que se lea de la plantilla ya llegó (`settle` espera al fondo).
    app.in_window(cx, |_, _| {});

    assert_eq!(
        commit_parts(&view, cx),
        ("My own subject".to_string(), "My own body".to_string())
    );
}

/// La plantilla no se queda la de la primera lectura: al volver a "Local
/// Changes" o al vaciar la caja se mira la vigente. Un borrador nunca se pisa.
#[gpui_kit::test]
async fn the_commit_template_follows_changes_to_it(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = repo_with_template();
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount(cx, &repo_id, ChangesMode::List);
    app.wait_until(cx, "the template", |cx| {
        commit_parts(&view, cx).0 == "feat: "
    });
    let reopen = |cx: &mut TestAppContext| {
        view.update(cx, |view, cx| {
            view.set_active(false, cx);
            view.set_active(true, cx);
        });
    };

    // Otro texto en el mismo archivo: con la caja sin tocar, se cambia.
    std::fs::write(
        repo.path.join(".git/commit template.txt"),
        "fix: \n\nWhy:\n# comment\n",
    )
    .expect("template");
    reopen(cx);
    app.wait_until(cx, "the edited template", |cx| {
        commit_parts(&view, cx) == ("fix: ".to_string(), "Why:".to_string())
    });

    // Con algo escrito, quitar la plantilla no toca el borrador.
    app.click(cx, "commit-title");
    app.in_window(cx, |window, cx| {
        window.press("end", cx);
        window.input("typo", cx);
    });
    repo.git(&["config", "--unset", "commit.template"]);
    reopen(cx);
    app.settle(cx, &view);
    app.in_window(cx, |_, _| {});
    assert_eq!(
        commit_parts(&view, cx),
        ("fix: typo".to_string(), "Why:".to_string())
    );

    // Una plantilla nueva llega en cuanto la caja se queda vacía.
    repo.write("docs template.txt", "docs: \n");
    repo.git(&["config", "commit.template", "docs template.txt"]);
    for field in ["commit-title", "commit-body"] {
        app.click(cx, field);
        app.in_window(cx, |window, cx| {
            window.press("ctrl-a", cx);
            window.press("backspace", cx);
        });
    }
    app.wait_until(cx, "the new template", |cx| {
        commit_parts(&view, cx) == ("docs: ".to_string(), String::new())
    });
}

/// Con la plantilla sin tocar, "Amend last commit" parte del mensaje de HEAD
/// como con la caja vacía.
#[gpui_kit::test]
async fn an_untouched_template_gives_way_to_the_amended_message(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = repo_with_template();
    repo.git(&["commit", "-q", "-m", "Add a", "-m", "With a body."]);
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount(cx, &repo_id, ChangesMode::List);
    app.wait_until(cx, "the template and the amend checkbox", |cx| {
        commit_parts(&view, cx).0 == "feat: " && app.exists(cx, "amend-last-commit")
    });

    app.click(cx, "amend-last-commit");

    assert_eq!(
        commit_parts(&view, cx),
        ("Add a".to_string(), "With a body.".to_string())
    );
}

/// "Amend last commit" refleja el nuevo HEAD tras hacer un commit y tras
/// deshacerlo, y deshacer un commit devuelve su mensaje a la caja vacía.
#[gpui_kit::test]
async fn amend_follows_head_and_undoing_a_commit_restores_its_message(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "a\n", "First commit");
    repo.write("b.txt", "b\n");
    repo.git(&["add", "b.txt"]);
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount(cx, &repo_id, ChangesMode::List);
    app.settle(cx, &view);

    // Hacer un nuevo commit desde la vista.
    app.click(cx, "commit-title");
    app.in_window(cx, |window, cx| window.input("Add notes", cx));
    app.click(cx, "commit-body");
    app.in_window(cx, |window, cx| window.input("Notes body text", cx));
    app.click(cx, "commit");
    app.wait_until(cx, "the commit", |_| {
        app.events
            .borrow()
            .iter()
            .any(|event| matches!(event, ChangesViewEvent::CommitCreated { .. }))
    });
    app.settle(cx, &view);
    assert_eq!(commit_parts(&view, cx), (String::new(), String::new()));

    // Marcar "Amend last commit": debe proponer "Add notes", no "First commit".
    app.wait_until(cx, "last commit refreshed", |cx| {
        view.read_with(cx, |view, _| {
            view.amendable_message().as_deref() == Some("Add notes\n\nNotes body text")
        })
    });
    app.click(cx, "amend-last-commit");
    assert_eq!(
        commit_parts(&view, cx),
        ("Add notes".to_string(), "Notes body text".to_string())
    );

    // Desmarcar Amend y vaciar la caja; deshacer el commit debe devolver su mensaje.
    app.click(cx, "amend-last-commit");
    for field in ["commit-title", "commit-body"] {
        app.click(cx, field);
        app.in_window(cx, |window, cx| {
            window.press("ctrl-a", cx);
            window.press("backspace", cx);
        });
    }
    assert_eq!(commit_parts(&view, cx), (String::new(), String::new()));

    let undone = app.model.update(cx, |model, cx| model.undo(cx)).await;
    assert!(undone);
    app.wait_until(cx, "restored commit message", |cx| {
        commit_parts(&view, cx) == ("Add notes".to_string(), "Notes body text".to_string())
    });
    // Y tras deshacer, HEAD volvió a "First commit": Amend también se actualiza.
    app.wait_until(cx, "last commit after undo", |cx| {
        view.read_with(cx, |view, _| {
            view.amendable_message().as_deref() == Some("First commit")
        })
    });
}
