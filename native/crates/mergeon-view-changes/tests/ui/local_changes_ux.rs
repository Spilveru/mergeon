//! La experiencia de "Local Changes" sobre repositorios de verdad: avanzar
//! tras preparar, la selección múltiple y sus acciones en lote, el lado a la
//! vista (`Unstaged | Staged`) y sus acciones, el ancho recordado, las pistas
//! de teclado y la nota de la vista partida.

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{Entity, Modifiers, TestAppContext};
use mergeon_view_changes::{ChangesMode, ChangesSection, ChangesView, DEFAULT_LOCAL_CHANGES_WIDTH};
use mergeon_view_diff::{DiffSide, SPLIT_STAGING_NOTE};

use crate::support::{TestApp, TestRepo};

fn selected(view: &Entity<ChangesView>, cx: &mut TestAppContext) -> Option<(String, String)> {
    view.read_with(cx, |view, _| {
        view.selected_file().map(|file| {
            (
                file.path.clone(),
                format!("{}{}", file.index, file.working_dir),
            )
        })
    })
}

fn staged_names(repo: &TestRepo) -> String {
    repo.git(&["diff", "--cached", "--name-only"])
}

/// Tres archivos modificados sin preparar.
fn repo_with_three_changes() -> TestRepo {
    let repo = TestRepo::new();
    for name in ["a.js", "b.js", "c.js"] {
        repo.commit_file(name, "x\n", &format!("Add {name}"));
    }
    for name in ["a.js", "b.js", "c.js"] {
        repo.write(name, "y\n");
    }
    repo
}

/// Espacio sobre el elegido lo prepara y la selección pasa al siguiente de
/// Unstaged (no salta a Staged).
#[gpui_kit::test]
async fn space_stages_and_moves_on_to_the_next_unstaged_file(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = repo_with_three_changes();
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount(cx, &repo_id, ChangesMode::List);
    app.settle(cx, &view);

    app.click(cx, "unstaged:file:a.js");
    app.settle(cx, &view);
    app.in_window(cx, |window, cx| window.press("space", cx));
    app.wait_until(cx, "a.js to be staged", |_| staged_names(&repo) == "a.js");
    app.settle(cx, &view);

    assert_eq!(
        selected(&view, cx),
        Some(("b.js".to_string(), " M".to_string()))
    );
    // Sigue en la sección: otra vez Espacio prepara b.js.
    app.in_window(cx, |window, cx| window.press("space", cx));
    app.wait_until(cx, "b.js to be staged", |_| {
        staged_names(&repo) == "a.js\nb.js"
    });
    app.settle(cx, &view);
    assert_eq!(
        selected(&view, cx),
        Some(("c.js".to_string(), " M".to_string()))
    );
}

/// `Ctrl+A` elige la sección entera: el panel derecho resume la selección y
/// prepara todo de una vez.
#[gpui_kit::test]
async fn several_files_get_a_summary_and_batch_actions(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = repo_with_three_changes();
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount(cx, &repo_id, ChangesMode::List);
    app.settle(cx, &view);
    // Las cifras de la lista ya llegaron.
    app.wait_until(cx, "the line stats", |cx| {
        view.read_with(cx, |view, _| view.file_line_stats("c.js", false).is_some())
    });

    app.click(cx, "unstaged:file:a.js");
    app.settle(cx, &view);
    app.in_window(cx, |window, cx| window.press("ctrl-a", cx));
    assert_eq!(
        view.read_with(cx, |view, _| view.multi_selection().cloned())
            .map(|selection| (selection.section, selection.paths.len())),
        Some((ChangesSection::Unstaged, 3))
    );
    assert_eq!(
        app.label(cx, "changes-multi-summary").as_deref(),
        Some("3 files · +3 \u{2212}3")
    );
    assert!(app.exists(cx, "multi-stage"));
    assert!(app.exists(cx, "multi-discard"));
    assert!(!app.exists(cx, "toggle-diff-view"));

    app.click(cx, "multi-stage");
    app.wait_until(cx, "everything staged", |_| {
        staged_names(&repo) == "a.js\nb.js\nc.js"
    });
    app.settle(cx, &view);
    assert!(view.read_with(cx, |view, _| view.multi_selection().is_none()));
    assert!(!app.exists(cx, "changes-multi-summary"));
}

/// `Ctrl+clic` suma archivos y D descarta la selección entera, tras una sola
/// confirmación.
#[gpui_kit::test]
async fn d_discards_the_whole_selection_after_one_confirmation(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = repo_with_three_changes();
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount(cx, &repo_id, ChangesMode::List);
    app.settle(cx, &view);

    app.click(cx, "unstaged:file:a.js");
    app.settle(cx, &view);
    app.click_with(
        cx,
        "unstaged:file:c.js",
        Modifiers {
            control: true,
            ..Default::default()
        },
    );
    app.host.answer_confirm(true);
    app.in_window(cx, |window, cx| window.press("d", cx));
    app.wait_until(cx, "the discard", |_| {
        repo.read("a.js") == "x\n" && repo.read("c.js") == "x\n"
    });
    app.settle(cx, &view);

    assert_eq!(repo.read("b.js"), "y\n");
    assert_eq!(
        app.host
            .confirms()
            .last()
            .and_then(|confirm| confirm.description.clone())
            .as_deref(),
        Some("Discard local changes in 2 files?")
    );
    // La selección pasó al que quedaba en medio.
    assert_eq!(
        selected(&view, cx),
        Some(("b.js".to_string(), " M".to_string()))
    );
}

/// Un archivo a medio preparar: la cabecera dice qué lado se ve, deja
/// cambiar al otro, y con Staged a la vista ofrece "Unstage file", nunca
/// "Discard" (que descartaría lo no preparado, que no se ve).
#[gpui_kit::test]
async fn the_staged_side_offers_unstage_and_never_discards(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.commit_file("src/a.js", "uno\n", "Add a");
    repo.write("src/a.js", "staged\n");
    repo.git(&["add", "src/a.js"]);
    repo.write("src/a.js", "working\n");
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount(cx, &repo_id, ChangesMode::List);
    app.settle(cx, &view);

    app.click(cx, "staged:file:src/a.js");
    app.settle(cx, &view);
    assert_eq!(
        view.read_with(cx, |view, _| view.selected_side()),
        Some(DiffSide::Staged)
    );
    assert_eq!(app.label(cx, "diff-sides").as_deref(), Some("Staged"));
    assert!(app.exists(cx, "unstage-selected"));
    assert!(!app.exists(cx, "discard-selected"));
    assert!(!app.exists(cx, "stage-selected"));
    // La otra fila del mismo archivo lleva la marca tenue.
    assert!(app.exists(cx, "same-file:unstaged:file:src/a.js"));

    // El segmentado cambia al lado sin preparar.
    app.click(cx, "diff-side-unstaged");
    app.settle(cx, &view);
    assert_eq!(
        selected(&view, cx),
        Some(("src/a.js".to_string(), " M".to_string()))
    );
    assert!(app.exists(cx, "stage-selected"));
    assert!(app.exists(cx, "discard-selected"));
    assert!(app.exists(cx, "same-file:staged:file:src/a.js"));

    // De vuelta en Staged, "Unstage file" saca lo preparado y deja el
    // working tree como estaba.
    app.click(cx, "diff-side-staged");
    app.settle(cx, &view);
    app.click(cx, "unstage-selected");
    app.wait_until(cx, "the unstage", |_| staged_names(&repo).is_empty());
    app.settle(cx, &view);
    assert_eq!(repo.read("src/a.js"), "working\n");
}

/// "Discard" de la cabecera pregunta, como D y el menú.
#[gpui_kit::test]
async fn discard_in_the_header_asks_first(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.commit_file("a.js", "x\n", "Add a");
    repo.write("a.js", "y\n");
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount(cx, &repo_id, ChangesMode::List);
    app.settle(cx, &view);

    app.click(cx, "unstaged:file:a.js");
    app.settle(cx, &view);
    assert_eq!(
        app.label(cx, "diff-line-stats").as_deref(),
        Some("+1 \u{2212}1")
    );
    app.host.answer_confirm(false);
    app.click(cx, "discard-selected");
    app.settle(cx, &view);
    assert_eq!(repo.read("a.js"), "y\n");
    assert_eq!(
        app.host
            .confirms()
            .last()
            .and_then(|confirm| confirm.description.clone())
            .as_deref(),
        Some("Discard local changes in a.js?")
    );

    app.host.answer_confirm(true);
    app.click(cx, "discard-selected");
    app.wait_until(cx, "the discard", |_| repo.read("a.js") == "x\n");
}

/// En la vista partida no hay botones de bloque: una nota lo dice.
#[gpui_kit::test]
async fn split_view_says_where_single_blocks_are_staged(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.commit_file("a.js", "x\n", "Add a");
    repo.write("a.js", "y\n");
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount(cx, &repo_id, ChangesMode::List);
    app.settle(cx, &view);
    app.click(cx, "unstaged:file:a.js");
    app.settle(cx, &view);
    assert!(!app.exists(cx, "diff-note"));

    app.click(cx, "toggle-diff-view");
    app.settle(cx, &view);
    assert_eq!(
        app.label(cx, "diff-note").as_deref(),
        Some(SPLIT_STAGING_NOTE)
    );

    app.click(cx, "toggle-diff-view");
    app.settle(cx, &view);
    assert!(!app.exists(cx, "diff-note"));
}

/// La columna empieza en 400px y recuerda el ancho que se le dé.
#[gpui_kit::test]
async fn the_column_starts_at_400px_and_remembers_its_width(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.write("a.txt", "a\n");
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount(cx, &repo_id, ChangesMode::List);
    app.settle(cx, &view);
    assert_eq!(
        view.read_with(cx, |view, cx| view.sidebar_width(cx)),
        DEFAULT_LOCAL_CHANGES_WIDTH
    );

    app.model
        .update(cx, |model, cx| {
            model.save_local_changes_width(Some(520.), cx)
        })
        .await;
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |view, cx| view.sidebar_width(cx)), 520.);
    // Y lo guarda en los ajustes, junto al resto de `commitView`.
    let saved = app
        .model
        .read_with(cx, |model, _| model.local_changes_prefs());
    assert_eq!(saved.width, Some(520.));
    let list = app.in_window(cx, |window, _| window.find("changes-list").bounds());
    assert_eq!(list.size.width, gpui_kit::px(520.));
}

/// Las pistas de teclado se ven mientras no hay nada preparado y se cierran
/// para siempre con su `×`.
#[gpui_kit::test]
async fn the_shortcut_hints_show_until_something_is_staged_or_they_are_closed(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.write("a.txt", "a\n");
    repo.write("b.txt", "b\n");
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount(cx, &repo_id, ChangesMode::List);
    app.settle(cx, &view);
    assert!(app.exists(cx, "changes-shortcut-hints"));

    app.click(cx, "check:unstaged:file:a.txt");
    app.wait_until(cx, "a.txt to be staged", |_| staged_names(&repo) == "a.txt");
    app.settle(cx, &view);
    assert!(!app.exists(cx, "changes-shortcut-hints"));

    app.click(cx, "check:staged:file:a.txt");
    app.wait_until(cx, "a.txt to be unstaged", |_| {
        staged_names(&repo).is_empty()
    });
    app.settle(cx, &view);
    assert!(app.exists(cx, "changes-shortcut-hints"));
    app.click(cx, "changes-shortcut-hints-close");
    app.settle(cx, &view);
    assert!(!app.exists(cx, "changes-shortcut-hints"));
    assert!(
        app.model
            .read_with(cx, |model, _| model.local_changes_prefs().hints_dismissed)
    );
}

/// El conmutador Árbol / Lista de la lista se guarda en los ajustes.
#[gpui_kit::test]
async fn the_flat_list_is_a_saved_preference(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.write("src/lib/a.txt", "a\n");
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount(cx, &repo_id, ChangesMode::List);
    app.settle(cx, &view);
    assert!(app.exists(cx, "unstaged:folder:src/lib"));

    app.click(cx, "changes-layout-flat");
    app.settle(cx, &view);
    assert!(
        app.model
            .read_with(cx, |model, _| model.local_changes_prefs().flat_list)
    );
    assert!(!app.exists(cx, "unstaged:folder:src/lib"));
    assert!(app.exists(cx, "unstaged:file:src/lib/a.txt"));
}

fn renders(view: &Entity<ChangesView>, cx: &mut TestAppContext) -> usize {
    view.read_with(cx, |view, _| view.render_count())
}

/// Un ajuste que la vista no enseña (el ancho del historial) no la repinta;
/// uno de "Local Changes" sí.
#[gpui_kit::test]
async fn only_its_own_preferences_repaint_the_view(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.write("src/lib/a.txt", "a\n");
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount(cx, &repo_id, ChangesMode::List);
    app.settle(cx, &view);
    // Hasta que no quede nada en vuelo.
    let mut last = renders(&view, cx);
    let mut still = 0;
    while still < 10 {
        std::thread::sleep(std::time::Duration::from_millis(20));
        cx.run_until_parked();
        let now = renders(&view, cx);
        still = if now == last { still + 1 } else { 0 };
        last = now;
    }

    let before = renders(&view, cx);
    app.model
        .update(cx, |model, cx| model.persist_history_width(480., cx))
        .await;
    cx.run_until_parked();
    assert_eq!(renders(&view, cx), before);

    app.model
        .update(cx, |model, cx| model.set_changes_list_flat(true, cx))
        .await;
    cx.run_until_parked();
    assert!(renders(&view, cx) > before);
}
