//! Selección múltiple del historial: Ctrl/⌘+clic, Shift+clic, Shift+flechas
//! y Esc; el resumen "N commits selected"; cherry-pick y revert en lote con
//! git real (incluido un conflicto a mitad) y "Compare these two".

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{Entity, Modifiers, SharedString, TestAppContext};

use mergeon_view_commits::{
    BatchCommand, CommitsView, MultiStats, WORKING_CHANGES_ROW_ID, batch_menu,
};

use crate::support::{TestApp, TestRepo};

async fn open_view(app: &TestApp, repo: &TestRepo, cx: &mut TestAppContext) -> Entity<CommitsView> {
    let repo_id = app.open(repo, cx).await;
    let view = app.mount(cx, move |window, cx| CommitsView::new(repo_id, window, cx));
    wait_loaded(app, &view, cx);
    view
}

fn wait_loaded(app: &TestApp, view: &Entity<CommitsView>, cx: &mut TestAppContext) {
    app.wait_for(cx, view, "the history to load", |view, _| {
        !view.commits().is_empty()
            && view.selected_commit().is_some()
            && !view.is_loading_history()
            && !view.is_loading_details()
    });
}

fn hash_of(view: &Entity<CommitsView>, message: &str, cx: &mut TestAppContext) -> String {
    view.read_with(cx, |view, _| {
        view.commits()
            .iter()
            .find(|commit| commit.message == message)
            .map(|commit| commit.full_hash.clone())
            .unwrap_or_else(|| panic!("no commit `{message}` in the history"))
    })
}

fn row(view: &Entity<CommitsView>, message: &str, cx: &mut TestAppContext) -> SharedString {
    SharedString::from(format!("commit:{}", hash_of(view, message, cx)))
}

/// Los asuntos seleccionados, en el orden de la lista.
fn selected(view: &Entity<CommitsView>, cx: &mut TestAppContext) -> Vec<String> {
    view.read_with(cx, |view, _| {
        view.multi_selected_hashes()
            .iter()
            .filter_map(|hash| {
                view.commits()
                    .iter()
                    .find(|commit| &commit.full_hash == hash)
            })
            .map(|commit| commit.message.clone())
            .collect()
    })
}

fn ctrl() -> Modifiers {
    Modifiers::secondary_key()
}

fn shift() -> Modifiers {
    Modifiers::shift()
}

/// `c4` (punta) → `c3` → `c2` → `c1` → `base` en `main`.
fn linear_repo() -> TestRepo {
    let repo = TestRepo::new();
    for name in ["c1", "c2", "c3", "c4"] {
        repo.commit_file(&format!("{name}.txt"), &format!("{name}\n"), name);
    }
    repo
}

/// Posición de una entrada del menú en lote (cabecera y separadores cuentan).
fn batch_index(count: usize, command: BatchCommand) -> usize {
    let mut index = 1;
    for (group_index, group) in batch_menu(count).into_iter().enumerate() {
        if group_index > 0 {
            index += 1;
        }
        for entry in group {
            if entry.command == command {
                return index;
            }
            index += 1;
        }
    }
    panic!("{command:?} is not in the batch menu");
}

#[gpui_kit::test]
async fn ctrl_click_adds_and_removes_commits_and_shows_the_summary(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = linear_repo();
    let view = open_view(&app, &repo, cx).await;
    assert!(!app.exists(cx, "multi-selection"));

    let c2 = row(&view, "c2", cx);
    app.click_with(cx, c2.clone(), ctrl());
    assert_eq!(selected(&view, cx), ["c4", "c2"]);
    assert_eq!(
        app.label(cx, "multi-selection-title").as_deref(),
        Some("2 commits selected")
    );
    // Con un hueco no hay totales del rango.
    assert_eq!(
        view.read_with(cx, |view, _| view.multi_stats()),
        MultiStats::NotContiguous
    );
    assert!(app.exists(cx, "multi-stats-note"));
    assert!(
        app.exists(cx, "multi-compare"),
        "two commits can be compared"
    );

    let c1 = row(&view, "c1", cx);
    app.click_with(cx, c1, ctrl());
    assert_eq!(selected(&view, cx), ["c4", "c2", "c1"]);
    assert!(!app.exists(cx, "multi-compare"));

    // Quitar uno; quitar otro deja uno solo: vuelve el inspector.
    app.click_with(cx, c2, ctrl());
    assert_eq!(selected(&view, cx), ["c4", "c1"]);
    let c4 = row(&view, "c4", cx);
    app.click_with(cx, c4, ctrl());
    assert!(!view.read_with(cx, |view, _| view.has_multi_selection()));
    assert!(!app.exists(cx, "multi-selection"));
    assert_eq!(
        view.read_with(cx, |view, _| view
            .selected_commit()
            .map(|c| c.message.clone())),
        Some("c1".to_string())
    );
}

#[gpui_kit::test]
async fn shift_click_selects_a_range_and_counts_its_changes(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = linear_repo();
    let view = open_view(&app, &repo, cx).await;

    // Clic simple en c3 (ancla) y Shift+clic en c1.
    let c3 = row(&view, "c3", cx);
    app.click(cx, c3);
    let c1 = row(&view, "c1", cx);
    app.click_with(cx, c1, shift());
    assert_eq!(selected(&view, cx), ["c3", "c2", "c1"]);

    app.wait_for(cx, &view, "the range totals", |view, _| {
        matches!(view.multi_stats(), MultiStats::Ready(_))
    });
    let MultiStats::Ready(stats) = view.read_with(cx, |view, _| view.multi_stats()) else {
        unreachable!()
    };
    assert_eq!((stats.files, stats.insertions, stats.deletions), (3, 3, 0));
    assert_eq!(
        app.label(cx, "multi-stat-files").as_deref(),
        Some("3 files")
    );

    // Un clic simple vuelve a la selección simple.
    let c4 = row(&view, "c4", cx);
    app.click(cx, c4);
    assert!(!view.read_with(cx, |view, _| view.has_multi_selection()));
}

#[gpui_kit::test]
async fn shift_arrows_extend_and_escape_collapses(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = linear_repo();
    let view = open_view(&app, &repo, cx).await;

    app.in_window(cx, |window, cx| window.press("shift-down", cx));
    app.in_window(cx, |window, cx| window.press("shift-down", cx));
    assert_eq!(selected(&view, cx), ["c4", "c3", "c2"]);
    app.in_window(cx, |window, cx| window.press("shift-up", cx));
    assert_eq!(selected(&view, cx), ["c4", "c3"]);

    app.in_window(cx, |window, cx| window.press("escape", cx));
    assert!(!view.read_with(cx, |view, _| view.has_multi_selection()));
    assert_eq!(
        view.read_with(cx, |view, _| view
            .selected_commit()
            .map(|c| c.message.clone())),
        Some("c3".to_string()),
        "Esc keeps the commit the keyboard moved to"
    );
    // Las flechas solas siguen moviendo la selección simple.
    app.in_window(cx, |window, cx| window.press("down", cx));
    app.wait_for(cx, &view, "the next commit", |view, _| {
        view.selected_commit()
            .is_some_and(|commit| commit.message == "c2")
    });
    assert!(!view.read_with(cx, |view, _| view.has_multi_selection()));
}

#[gpui_kit::test]
async fn the_uncommitted_changes_row_is_not_selectable(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = linear_repo();
    repo.write("c1.txt", "changed\n");
    let view = open_view(&app, &repo, cx).await;
    app.wait_until(cx, "the uncommitted changes row", |app, cx| {
        app.exists(cx, WORKING_CHANGES_ROW_ID)
    });

    app.click_with(cx, WORKING_CHANGES_ROW_ID, ctrl());
    app.click_with(cx, WORKING_CHANGES_ROW_ID, shift());
    assert!(!view.read_with(cx, |view, _| view.has_multi_selection()));
}

#[gpui_kit::test]
async fn cherry_picks_the_selected_commits_oldest_first(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.git(&["checkout", "-q", "-b", "feature"]);
    for name in ["f1", "f2", "f3"] {
        repo.commit_file(&format!("{name}.txt"), "x\n", name);
    }
    repo.git(&["checkout", "-q", "main"]);
    repo.commit_file("main.txt", "m\n", "main moved on");
    let view = open_view(&app, &repo, cx).await;

    // Con dos ramas a la vez la lista puede intercalarlas: se eligen a mano.
    let f3 = row(&view, "f3", cx);
    app.click(cx, f3);
    let (f2, f1) = (row(&view, "f2", cx), row(&view, "f1", cx));
    app.click_with(cx, f1.clone(), ctrl());
    app.click_with(cx, f2, ctrl());
    assert_eq!(selected(&view, cx), ["f3", "f2", "f1"]);

    app.host.answer_confirm(true);
    app.in_window(cx, |window, cx| window.right_click(f1, cx));
    assert!(app.menu_is_open(cx));
    app.click_menu_item(cx, batch_index(3, BatchCommand::CherryPick));

    app.wait_until(cx, "the cherry-picks", |_, _| {
        repo.git(&["log", "-3", "--format=%s", "main"]) == "f3\nf2\nf1"
    });
    let confirms = app.host.confirms();
    assert_eq!(confirms[0].title.as_deref(), Some("Cherry-pick commits"));
    app.wait_for(cx, &view, "the reloaded history", |view, _| {
        !view.has_multi_selection()
            && view
                .commits()
                .iter()
                .filter(|commit| commit.message == "f1")
                .count()
                == 2
    });
}

#[gpui_kit::test]
async fn reverts_the_selected_commits_newest_first_or_as_one(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = linear_repo();
    let view = open_view(&app, &repo, cx).await;
    app.host.confirm_by_default(true);

    let c3 = row(&view, "c3", cx);
    app.click(cx, c3);
    let c2 = row(&view, "c2", cx);
    app.click_with(cx, c2, shift());
    app.click(cx, "multi-revert");
    app.wait_until(cx, "the reverts", |_, _| {
        repo.git(&["log", "-2", "--format=%s"]) == "Revert \"c2\"\nRevert \"c3\""
    });
    wait_loaded(&app, &view, cx);
    app.wait_for(cx, &view, "the reloaded history", |view, _| {
        !view.is_loading_history() && view.commits().len() == 7
    });

    let c4 = row(&view, "c4", cx);
    app.click(cx, c4);
    let c1 = row(&view, "c1", cx);
    app.click_with(cx, c1.clone(), ctrl());
    app.in_window(cx, |window, cx| window.right_click(c1, cx));
    app.click_menu_item(cx, batch_index(2, BatchCommand::RevertAsOne));
    app.wait_until(cx, "the single revert commit", |_, _| {
        repo.git(&["log", "-1", "--format=%s"]) == "Revert 2 commits"
    });
    assert!(!repo.path.join("c4.txt").exists() && !repo.path.join("c1.txt").exists());
}

/// El segundo commit choca: el primero queda aplicado, la secuencia a medias
/// (con Skip) y la vista sigue usable.
#[gpui_kit::test]
async fn a_batch_cherry_pick_stops_at_the_first_conflict(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.commit_file("shared.txt", "base\n", "shared base");
    repo.git(&["checkout", "-q", "-b", "feature"]);
    repo.commit_file("one.txt", "1\n", "f1");
    repo.commit_file("shared.txt", "feature\n", "f2");
    repo.commit_file("three.txt", "3\n", "f3");
    repo.git(&["checkout", "-q", "main"]);
    repo.commit_file("shared.txt", "main\n", "main change");
    let view = open_view(&app, &repo, cx).await;
    app.host.confirm_by_default(true);

    let f3 = row(&view, "f3", cx);
    app.click(cx, f3);
    let (f2, f1) = (row(&view, "f2", cx), row(&view, "f1", cx));
    app.click_with(cx, f2, ctrl());
    app.click_with(cx, f1, ctrl());
    assert_eq!(selected(&view, cx), ["f3", "f2", "f1"]);
    app.click(cx, "multi-cherry-pick");

    app.wait_until(cx, "the conflicted sequence", |_, _| {
        repo.path.join(".git/CHERRY_PICK_HEAD").exists()
    });
    assert_eq!(repo.git(&["log", "-1", "--format=%s"]), "f1");
    assert!(repo.path.join(".git/sequencer").is_dir());
    app.wait_until(cx, "the operation state", |app, cx| {
        app.model.read_with(cx, |model, _| {
            model
                .active_repository()
                .and_then(|repo| repo.operation_state.clone())
                .is_some_and(|state| state.in_progress && state.can_skip)
        })
    });
    wait_loaded(&app, &view, cx);
    assert!(view.read_with(cx, |view, _| view.selected_commit().is_some()));
}

#[gpui_kit::test]
async fn compares_the_two_selected_commits(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = linear_repo();
    let view = open_view(&app, &repo, cx).await;

    let c2 = row(&view, "c2", cx);
    app.click_with(cx, c2, ctrl());
    app.click(cx, "multi-compare");
    app.wait_for_dialog(cx, "Compare");
    let (c2, c4) = (hash_of(&view, "c2", cx), hash_of(&view, "c4", cx));
    let compare = view
        .read_with(cx, |view, _| view.compare_view().cloned())
        .expect("the comparison is open");
    app.wait_for(cx, &compare, "the comparison", |compare, _| {
        compare.comparison().is_some()
    });
    compare.read_with(cx, |compare, _| {
        assert_eq!(compare.base_label(), c2, "the older commit is A");
        assert_eq!(compare.head_label(), c4);
    });

    // Doble clic en un commit de la comparación: se cierra y se selecciona.
    let c3 = hash_of(&view, "c3", cx);
    compare.update(cx, |compare, cx| compare.show_commit(&c3, cx));
    app.wait_until(cx, "the dialog to close", |app, cx| {
        app.dialogs(cx).is_empty()
    });
    app.wait_for(cx, &view, "the selected commit", |view, _| {
        !view.has_multi_selection()
            && view
                .selected_commit()
                .is_some_and(|commit| commit.full_hash == c3)
    });
}

#[gpui_kit::test]
async fn copies_the_selected_shas_oldest_first(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = linear_repo();
    let view = open_view(&app, &repo, cx).await;

    let c3 = row(&view, "c3", cx);
    app.click_with(cx, c3, ctrl());
    app.click(cx, "multi-copy-shas");
    assert_eq!(
        app.clipboard(cx),
        Some(format!("{}\n{}", repo.rev("HEAD~1"), repo.rev("HEAD")))
    );
}
