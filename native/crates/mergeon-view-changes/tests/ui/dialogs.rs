//! Diálogos de stash (`StashDiffDialog.test.js`) y de blame.

use gpui_kit::{AppContext as _, Entity, IntoElement, TestAppContext};
use mergeon_app::SyncRequest;
use mergeon_view_changes::{
    BlameDialog, StashDiffDialog, open_blame_dialog, open_stash_diff_dialog,
};
use mergeon_view_diff::BlameState;

use crate::support::{TestApp, TestRepo};

/// Un stash con dos archivos: `src/app.js` modificado y `docs/readme.md` nuevo.
fn repo_with_stash() -> TestRepo {
    let repo = TestRepo::new();
    repo.commit_file("src/app.js", "one\ntwo\nthree\n", "Add app");
    repo.write("src/app.js", "one\nTWO\nthree\nfour\n");
    repo.write("docs/readme.md", "# Readme\n\nSome text.\n");
    repo.git(&["add", "--all"]);
    repo.git(&["stash", "push", "-q", "-m", "WIP: cambios"]);
    repo
}

fn stash_hash(repo: &TestRepo) -> String {
    repo.git(&["rev-parse", "stash@{0}"])
}

fn show_stash(
    app: &TestApp,
    cx: &mut TestAppContext,
    repo: &TestRepo,
    stash_ref: String,
) -> Entity<StashDiffDialog> {
    let repo_path = repo.path_string();
    let dialog = cx
        .update_window(app.window, move |_, window, cx| {
            cx.new(|cx| StashDiffDialog::new(repo_path, stash_ref, window, cx))
        })
        .expect("la ventana de prueba se cerró");
    let shown = dialog.clone();
    app.show(cx, move |_, _| shown.clone().into_any_element());
    let waiting = dialog.clone();
    app.wait_until(cx, "the stash to load", move |cx| {
        waiting.read_with(cx, |dialog, _| !dialog.is_loading())
    });
    dialog
}

#[gpui_kit::test]
async fn loads_the_stash_files_by_sha_and_shows_the_first_diff(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = repo_with_stash();
    app.open(&repo, cx).await;

    let dialog = show_stash(&app, cx, &repo, stash_hash(&repo));

    assert!(app.exists(cx, "stash-file:src/app.js"));
    assert!(app.exists(cx, "stash-file:docs/readme.md"));
    // `git diff --name-status` ordena por ruta: el primero es docs/readme.md.
    let (selected, diff) = dialog.read_with(cx, |dialog, _| {
        (
            dialog.selected_file().map(|file| file.path.clone()),
            dialog.file_diff().to_string(),
        )
    });
    assert_eq!(selected.as_deref(), Some("docs/readme.md"));
    assert!(diff.contains("+# Readme"), "{diff}");

    app.click(cx, "stash-file:src/app.js");
    let waiting = dialog.clone();
    app.wait_until(cx, "the second diff", move |cx| {
        waiting.read_with(cx, |dialog, _| {
            !dialog.is_loading() && dialog.file_diff().contains("+TWO")
        })
    });
}

/// `git stash -u` con solo archivos sin seguimiento: antes decía "This stash
/// has no file changes"; ahora los lista (del tercer padre) como nuevos.
#[gpui_kit::test]
async fn an_untracked_only_stash_lists_its_files_as_new(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.write("scratch/idea.md", "# Idea\nmaybe\n");
    repo.git(&["stash", "push", "-q", "-u", "-m", "untracked"]);
    app.open(&repo, cx).await;

    let dialog = show_stash(&app, cx, &repo, stash_hash(&repo));

    assert!(!app.exists(cx, "stash-empty"));
    assert!(app.exists(cx, "stash-file:scratch/idea.md"));
    let waiting = dialog.clone();
    app.wait_until(cx, "the untracked diff", move |cx| {
        waiting.read_with(cx, |dialog, _| {
            !dialog.is_loading() && dialog.file_diff().contains("+# Idea")
        })
    });
    let status = dialog.read_with(cx, |dialog, _| {
        dialog.selected_file().map(|file| file.status.clone())
    });
    assert_eq!(status.as_deref(), Some("A"));
}

#[gpui_kit::test]
async fn shows_an_empty_state_when_the_stash_has_no_file_changes(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    app.open(&repo, cx).await;
    // Un "stash" cuyo árbol es el de su base: no cambia nada.
    let tree = repo.git(&["rev-parse", "HEAD^{tree}"]);
    let empty = repo.git(&[
        "commit-tree",
        &tree,
        "-p",
        "HEAD",
        "-p",
        "HEAD",
        "-m",
        "empty",
    ]);

    let dialog = show_stash(&app, cx, &repo, empty);

    assert!(app.exists(cx, "stash-empty"));
    assert!(dialog.read_with(cx, |dialog, _| dialog.files().is_empty()));
    assert!(dialog.read_with(cx, |dialog, _| dialog.selected_file().is_none()));
}

/// El original no pedía nada mientras el diálogo no se mostraba. Aquí el
/// diálogo solo existe abierto: sin stash que mostrar, ni se abre.
#[gpui_kit::test]
async fn does_not_open_without_a_stash(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    let repo_id = app.open(&repo, cx).await;

    let opened = app.in_window(cx, |window, cx| {
        open_stash_diff_dialog(&repo_id, "", window, cx)
    });
    assert!(opened.is_none());
    let unknown = app.in_window(cx, |window, cx| {
        open_stash_diff_dialog("/no/such/repository", "abc", window, cx)
    });
    assert!(unknown.is_none());
    assert!(app.dialogs(cx).is_empty());
}

#[gpui_kit::test]
async fn the_stash_dialog_is_titled_with_its_message(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = repo_with_stash();
    let repo_id = app.open(&repo, cx).await;
    app.model
        .update(cx, |model, cx| {
            model.sync_repository_read_model(repo_id.as_str(), SyncRequest::new().stashes(true), cx)
        })
        .await;

    let hash = stash_hash(&repo);
    app.in_window(cx, |window, cx| {
        open_stash_diff_dialog(&repo_id, &hash, window, cx)
    });
    assert_eq!(app.dialogs(cx), ["On main: WIP: cambios"]);
    app.wait_until(cx, "the stash files", |cx| {
        let app_window = app.window;
        cx.update_window(app_window, |_, window, _| {
            use gpui_kit::test::TestWindowExt as _;
            window.try_find("stash-file:src/app.js").is_some()
        })
        .unwrap_or(false)
    });
}

#[gpui_kit::test]
async fn opens_the_blame_of_a_file(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.commit_file("src/app.js", "one\ntwo\n", "Add app");
    let repo_id = app.open(&repo, cx).await;

    app.in_window(cx, |window, cx| {
        open_blame_dialog(&repo_id, "src/app.js", window, cx)
    });

    assert_eq!(app.dialogs(cx), ["Blame: app.js"]);
    assert_eq!(
        app.label(cx, "blame-dialog-content").as_deref(),
        Some("src/app.js")
    );
}

#[gpui_kit::test]
async fn the_blame_dialog_fills_its_viewer(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.commit_file("src/app.js", "one\ntwo\nthree\n", "Add app");
    app.open(&repo, cx).await;

    let repo_path = repo.path_string();
    let dialog = cx
        .update_window(app.window, move |_, window, cx| {
            cx.new(|cx| BlameDialog::new(repo_path, "src/app.js", window, cx))
        })
        .expect("la ventana de prueba se cerró");
    let shown = dialog.clone();
    app.show(cx, move |_, _| shown.clone().into_any_element());

    let viewer = dialog.read_with(cx, |dialog, _| dialog.viewer().clone());
    let waiting = viewer.clone();
    app.wait_until(cx, "the blame", move |cx| {
        waiting.read_with(cx, |viewer, _| viewer.line_count() == 3)
    });
    let lines = viewer.read_with(cx, |viewer, _| match viewer.state() {
        BlameState::Lines(lines) => lines
            .iter()
            .map(|line| (line.author.to_string(), line.content.to_string()))
            .collect::<Vec<_>>(),
        other => panic!("unexpected blame state {other:?}"),
    });
    assert_eq!(
        lines,
        [
            ("Test User".to_string(), "one".to_string()),
            ("Test User".to_string(), "two".to_string()),
            ("Test User".to_string(), "three".to_string()),
        ]
    );
}

/// Blame navegable: "Blame previous revision" pasa al blame del archivo en el
/// padre del commit de la línea (con la ruta de antes de un renombre), "Back"
/// vuelve, y el clic en un bloque cierra el diálogo y lleva al commit.
#[gpui_kit::test]
async fn the_blame_dialog_walks_back_through_revisions(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.commit_file("old.txt", "one\ntwo\n", "Add old");
    repo.git(&["mv", "old.txt", "new.txt"]);
    repo.git(&["commit", "-q", "-m", "Rename"]);
    repo.commit_file("new.txt", "one\nTWO\n", "Change two");
    let repo_id = app.open(&repo, cx).await;

    app.in_window(cx, |window, cx| {
        open_blame_dialog(&repo_id, "new.txt", window, cx)
    });
    assert_eq!(app.dialogs(cx), ["Blame: new.txt"]);

    let repo_path = repo.path_string();
    let dialog = cx
        .update_window(app.window, move |_, window, cx| {
            cx.new(|cx| BlameDialog::new(repo_path, "new.txt", window, cx))
        })
        .expect("la ventana de prueba se cerró");
    let viewer = dialog.read_with(cx, |dialog, _| dialog.viewer().clone());
    let content = |viewer: &Entity<mergeon_view_diff::BlameViewer>, cx: &mut TestAppContext| {
        viewer.read_with(cx, |viewer, _| match viewer.state() {
            BlameState::Lines(lines) => lines
                .iter()
                .map(|line| (line.summary.to_string(), line.content.to_string()))
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        })
    };
    let waiting = viewer.clone();
    app.wait_until(cx, "the blame", move |cx| {
        waiting.read_with(cx, |viewer, _| viewer.line_count() == 2)
    });
    assert_eq!(content(&viewer, cx)[1], ("Change two".into(), "TWO".into()));

    viewer.update(cx, |viewer, cx| viewer.blame_previous_at(1, cx));
    let waiting = viewer.clone();
    app.wait_until(cx, "the previous revision", move |cx| {
        waiting.read_with(cx, |viewer, _| match viewer.state() {
            BlameState::Lines(lines) => lines.get(1).is_some_and(|line| &*line.content == "two"),
            _ => false,
        })
    });
    assert_eq!(content(&viewer, cx)[1], ("Add old".into(), "two".into()));
    let parent = repo.git(&["rev-parse", "HEAD~1"]);
    assert_eq!(
        dialog.read_with(cx, |dialog, _| dialog
            .revision()
            .map(|(hash, path)| (hash.to_string(), path.to_string()))),
        Some((parent, "new.txt".to_string()))
    );

    dialog.update(cx, |dialog, cx| dialog.back(cx));
    let waiting = viewer.clone();
    app.wait_until(cx, "the working tree again", move |cx| {
        waiting.read_with(cx, |viewer, _| match viewer.state() {
            BlameState::Lines(lines) => lines.get(1).is_some_and(|line| &*line.content == "TWO"),
            _ => false,
        })
    });
    assert!(dialog.read_with(cx, |dialog, _| dialog.revision().is_none()));

    // El clic en un bloque: se cierra el diálogo y se pide el commit.
    let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let _subscription = cx.update(|cx| {
        let events = events.clone();
        cx.subscribe(&app.model, move |_, event: &mergeon_app::AppEvent, _| {
            if let mergeon_app::AppEvent::CommitSelectionRequested { hash, .. } = event {
                events.borrow_mut().push(hash.clone());
            }
        })
    });
    let handle = app.window;
    app.wait_until(cx, "the blocks of the dialog", move |cx| {
        cx.update_window(handle, |_, window, cx| {
            use gpui_kit::test::TestWindowExt as _;
            window.render_frame(cx);
            window.try_find("blame-commit:1").is_some()
        })
        .unwrap_or(false)
    });
    app.in_window(cx, |window, cx| {
        use gpui_kit::test::TestWindowExt as _;
        window.click("blame-commit:1", cx);
    });
    assert!(app.dialogs(cx).is_empty());
    assert_eq!(events.borrow().as_slice(), [repo.head()]);
}
