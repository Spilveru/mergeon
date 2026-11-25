//! "Stash selected files…" del menú de un archivo y "Fixup into…" de la caja
//! de commit.

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::TestAppContext;
use mergeon_app::{AppEvent, DialogRequest};
use mergeon_view_changes::{ChangesMode, ChangesView};

use crate::support::{TestApp, TestRepo};

/// Las peticiones de diálogo que emite el modelo mientras dure el test.
fn record_dialogs(app: &TestApp, cx: &mut TestAppContext) -> Rc<RefCell<Vec<DialogRequest>>> {
    let recorded = Rc::new(RefCell::new(Vec::new()));
    let sink = recorded.clone();
    let model = app.model.clone();
    cx.update(|cx| {
        cx.subscribe(&model, move |_, event: &AppEvent, _| {
            if let AppEvent::ShowDialog(request) = event {
                sink.borrow_mut().push(request.clone());
            }
        })
        .detach();
    });
    recorded
}

fn stash_paths(requests: &[DialogRequest]) -> Vec<Vec<String>> {
    requests
        .iter()
        .filter_map(|request| match request {
            DialogRequest::Stash { paths } => Some(paths.clone()),
            _ => None,
        })
        .collect()
}

#[gpui_kit::test]
async fn stash_selected_files_takes_the_checked_files_or_the_clicked_one(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.commit_file("src/a.js", "a\n", "Add a");
    repo.commit_file("src/b.js", "b\n", "Add b");
    repo.commit_file("src/c.js", "c\n", "Add c");
    repo.write("src/a.js", "A\n");
    repo.write("src/b.js", "B\n");
    repo.write("src/c.js", "C\n");
    repo.git(&["add", "src/a.js", "src/b.js"]);
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount(cx, &repo_id, ChangesMode::List);
    app.settle(cx, &view);
    let dialogs = record_dialogs(&app, cx);

    // Un archivo marcado: todos los marcados. Unstage, separador, Blame,
    // Open in editor, Reveal in Explorer, separador, File history,
    // separador, Stash…
    app.right_click(cx, "staged:file:src/a.js");
    app.click_menu_item(cx, 8);
    // Uno sin marcar: solo él. Stage, Discard Changes y lo mismo.
    app.right_click(cx, "unstaged:file:src/c.js");
    app.click_menu_item(cx, 9);

    assert_eq!(
        stash_paths(&dialogs.borrow()),
        [
            vec!["src/a.js".to_string(), "src/b.js".to_string()],
            vec!["src/c.js".to_string()],
        ]
    );
}

fn wait_for_targets(
    app: &TestApp,
    view: &gpui_kit::Entity<ChangesView>,
    cx: &mut TestAppContext,
    count: usize,
) {
    app.wait_until(cx, "the fixup targets", |cx| {
        view.read_with(cx, |view, _| view.fixup_targets().len() == count)
    });
}

#[gpui_kit::test]
async fn fixup_into_commits_the_staged_changes_for_the_chosen_commit(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.commit_file("src/a.js", "a\n", "Add a");
    repo.commit_file("src/b.js", "b\n", "Add b");
    repo.write("src/a.js", "a fixed\n");
    repo.git(&["add", "src/a.js"]);
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount(cx, &repo_id, ChangesMode::List);
    app.settle(cx, &view);
    wait_for_targets(&app, &view, cx, 3);

    // Cabecera, "Add b", "Add a", "Initial commit".
    app.click(cx, "commit-fixup");
    assert!(app.exists(cx, "popup-menu"));
    app.click_menu_item(cx, 2);

    app.wait_until(cx, "the fixup commit", |_| {
        repo.git(&["log", "-1", "--format=%s"]) == "fixup! Add a"
    });
    app.settle(cx, &view);
    assert_eq!(repo.git(&["status", "--porcelain"]), "");
    assert!(app.host.confirms().is_empty());
    // La lista se relee con el commit nuevo.
    wait_for_targets(&app, &view, cx, 4);
}

#[gpui_kit::test]
async fn fixing_up_a_pushed_commit_asks_first(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new().with_origin();
    repo.commit_file("src/a.js", "a\n", "Add a");
    repo.write("src/a.js", "a fixed\n");
    repo.git(&["add", "src/a.js"]);
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount(cx, &repo_id, ChangesMode::List);
    app.settle(cx, &view);
    wait_for_targets(&app, &view, cx, 2);
    let pushed = view.read_with(cx, |view, _| {
        let targets = view.fixup_targets();
        assert!(!targets[0].published, "Add a is local");
        assert!(targets[1].published, "the initial commit is on origin");
        targets[1].clone()
    });

    app.host.answer_confirm(false);
    view.update(cx, |view, cx| view.commit_fixup(&pushed, cx));
    app.settle(cx, &view);
    let confirms = app.host.confirms();
    assert_eq!(confirms.len(), 1);
    assert_eq!(
        confirms[0].title.as_deref(),
        Some("Fix up a pushed commit?")
    );
    assert!(
        confirms[0]
            .description
            .as_deref()
            .unwrap()
            .contains("is already on origin/main")
    );
    assert_eq!(repo.git(&["log", "-1", "--format=%s"]), "Add a");

    app.host.answer_confirm(true);
    view.update(cx, |view, cx| view.commit_fixup(&pushed, cx));
    app.wait_until(cx, "the fixup commit", |_| {
        repo.git(&["log", "-1", "--format=%s"]) == "fixup! Initial commit"
    });
}

#[gpui_kit::test]
async fn fixup_into_needs_staged_changes_and_no_amend(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.commit_file("src/a.js", "a\n", "Add a");
    repo.write("src/a.js", "a fixed\n");
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount(cx, &repo_id, ChangesMode::List);
    app.settle(cx, &view);
    wait_for_targets(&app, &view, cx, 2);

    // Sin nada preparado el botón no abre el menú.
    app.click(cx, "commit-fixup");
    assert!(!app.exists(cx, "popup-menu"));

    app.click(cx, "check:unstaged:file:src/a.js");
    app.wait_until(cx, "the staged file", |cx| {
        app.exists(cx, "staged:file:src/a.js")
    });
    app.settle(cx, &view);
    app.click(cx, "amend-last-commit");
    app.click(cx, "commit-fixup");
    assert!(!app.exists(cx, "popup-menu"));

    app.click(cx, "amend-last-commit");
    app.click(cx, "commit-fixup");
    assert!(app.exists(cx, "popup-menu"));
}

#[gpui_kit::test]
async fn fixup_into_is_off_while_a_merge_is_in_progress(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.commit_file("src/a.js", "base\n", "Add a");
    repo.git(&["checkout", "-q", "-b", "side"]);
    repo.commit_file("src/a.js", "side\n", "Side a");
    repo.git(&["checkout", "-q", "main"]);
    repo.commit_file("src/a.js", "main\n", "Main a");
    assert!(
        repo.try_git(&["merge", "side"]).is_err(),
        "the merge should conflict"
    );
    // Conflicto resuelto y preparado: solo falta el commit del merge.
    repo.write("src/a.js", "both\n");
    repo.git(&["add", "src/a.js"]);

    let repo_id = app.open(&repo, cx).await;
    let view = app.mount(cx, &repo_id, ChangesMode::List);
    app.settle(cx, &view);
    app.wait_until(cx, "the fixup targets", |cx| {
        view.read_with(cx, |view, _| !view.fixup_targets().is_empty())
    });
    app.wait_until(cx, "the staged file", |cx| {
        app.exists(cx, "staged:file:src/a.js")
    });

    app.click(cx, "commit-fixup");
    assert!(!app.exists(cx, "popup-menu"));

    // Ni por la paleta (que llama directo a `commit_fixup`).
    let target = view.read_with(cx, |view, _| view.fixup_targets()[0].clone());
    view.update(cx, |view, cx| view.commit_fixup(&target, cx));
    app.settle(cx, &view);
    assert_eq!(repo.git(&["log", "-1", "--format=%s"]), "Main a");
    assert!(
        repo.try_git(&["rev-parse", "-q", "--verify", "MERGE_HEAD"])
            .is_ok()
    );
}
