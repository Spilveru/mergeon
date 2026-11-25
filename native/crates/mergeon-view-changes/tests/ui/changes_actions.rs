//! Lo que se hace desde la vista de cambios: staging por bloque
//! (`ChangesView.test.js` › "staging one block"), menús contextuales,
//! descartes con su confirmación, ignorar, resolver un conflicto, el divisor
//! y el desplazamiento de una pestaña.

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{Entity, TestAppContext, point, px};
use mergeon_app::backend::ExternalEditorSettings;
use mergeon_model::diff::DiffLineType;
use mergeon_model::notify::NotifyType;
use mergeon_model::workspace_tabs::{FileTabDescriptor, Scroll};
use mergeon_view_changes::{ChangesMode, ChangesView};
use mergeon_view_diff::{HunkAction, PatchAction};

use crate::support::{TestApp, TestRepo};

/// `src/a.js` con dos bloques cambiados, lejos uno del otro.
fn repo_with_two_hunks() -> TestRepo {
    let repo = TestRepo::new();
    let lines: Vec<String> = (1..=30).map(|line| format!("line {line}")).collect();
    repo.commit_file("src/a.js", &(lines.join("\n") + "\n"), "Add a");
    let mut changed = lines.clone();
    changed[0] = "LINE 1".to_string();
    changed[25] = "LINE 26".to_string();
    repo.write("src/a.js", &(changed.join("\n") + "\n"));
    repo
}

fn wait_for_diff(app: &TestApp, view: &Entity<ChangesView>, cx: &mut TestAppContext, text: &str) {
    let view = view.clone();
    let text = text.to_string();
    app.wait_until(cx, &format!("a diff with {text:?}"), move |cx| {
        view.read_with(cx, |view, _| {
            !view.is_loading_diff() && view.file_diff().contains(text.as_str())
        })
    });
}

fn hunk_action(view: &Entity<ChangesView>, cx: &mut TestAppContext) -> Option<HunkAction> {
    view.read_with(cx, |view, cx| view.hunk_action(cx))
}

/// Pulsa "Stage hunk" / "Unstage hunk" del bloque `index`, como el clic en
/// su botón.
fn apply_hunk(view: &Entity<ChangesView>, cx: &mut TestAppContext, index: usize) {
    let viewer = view.read_with(cx, |view, _| view.diff_viewer().clone());
    viewer.update(cx, |viewer, cx| viewer.apply_hunk(index, cx));
}

// ---------------------------------------------------------------------------
// ChangesView: staging one block
// ---------------------------------------------------------------------------

#[gpui_kit::test]
async fn sends_the_patch_of_just_that_block(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = repo_with_two_hunks();
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount(cx, &repo_id, ChangesMode::Full);
    wait_for_diff(&app, &view, cx, "+LINE 26");
    assert_eq!(hunk_action(&view, cx), Some(HunkAction::Stage));

    apply_hunk(&view, cx, 0);
    app.wait_until(cx, "the block to be staged", |_| {
        !repo.git(&["diff", "--cached"]).is_empty()
    });
    app.settle(cx, &view);

    let staged = repo.git(&["diff", "--cached"]);
    assert!(staged.contains("+LINE 1"), "{staged}");
    assert!(!staged.contains("LINE 26"), "{staged}");
    // El otro bloque sigue sin preparar, y el archivo sale en las dos listas.
    assert!(repo.git(&["diff"]).contains("+LINE 26"));
    assert!(app.exists(cx, "unstaged:file:src/a.js"));
    assert!(app.exists(cx, "staged:file:src/a.js"));
}

#[gpui_kit::test]
async fn offers_to_unstage_when_the_file_is_already_staged(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = repo_with_two_hunks();
    repo.git(&["add", "src/a.js"]);
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount_tab(cx, &repo_id, FileTabDescriptor::index("src/a.js"));
    wait_for_diff(&app, &view, cx, "+LINE 26");
    assert_eq!(hunk_action(&view, cx), Some(HunkAction::Unstage));

    apply_hunk(&view, cx, 1);
    app.wait_until(cx, "the block to be unstaged", |_| {
        !repo.git(&["diff", "--cached"]).contains("LINE 26")
    });
    app.settle(cx, &view);

    assert!(repo.git(&["diff", "--cached"]).contains("+LINE 1"));
    assert!(repo.git(&["diff"]).contains("+LINE 26"));
}

/// Con espacios ignorados el diff que se ve no vale como parche, y pedir uno
/// limpio tampoco: los números de bloque no coincidirían.
#[gpui_kit::test]
async fn does_not_offer_blocks_while_whitespace_is_ignored(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = repo_with_two_hunks();
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount(cx, &repo_id, ChangesMode::Full);
    wait_for_diff(&app, &view, cx, "+LINE 26");
    assert_eq!(hunk_action(&view, cx), Some(HunkAction::Stage));

    app.click(cx, "toggle-ignore-whitespace");

    assert_eq!(hunk_action(&view, cx), None);
    assert!(
        app.model
            .read_with(cx, |model, _| model.diff_preferences().ignore_whitespace)
    );
    app.settle(cx, &view);
    // Y vuelve al apagarlo.
    app.click(cx, "toggle-ignore-whitespace");
    assert_eq!(hunk_action(&view, cx), Some(HunkAction::Stage));
}

/// Un archivo sin seguir no tiene diff real: se prepara entero.
#[gpui_kit::test]
async fn does_not_offer_blocks_for_an_untracked_file(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.write("src/nuevo.js", "nuevo\n");
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount(cx, &repo_id, ChangesMode::Full);
    wait_for_diff(&app, &view, cx, "+nuevo");

    assert_eq!(hunk_action(&view, cx), None);
}

// ---------------------------------------------------------------------------
// Descartar un bloque y preparar, quitar o descartar líneas sueltas
// ---------------------------------------------------------------------------

/// Pulsa el botón de `action` en la cabecera del bloque `index`.
fn apply_hunk_action(
    view: &Entity<ChangesView>,
    cx: &mut TestAppContext,
    index: usize,
    action: PatchAction,
) {
    let viewer = view.read_with(cx, |view, _| view.diff_viewer().clone());
    viewer.update(cx, |viewer, cx| viewer.apply_hunk_action(index, action, cx));
}

/// Elige en el visor las altas y bajas `lines` (`"+texto"` / `"-texto"`), como
/// un clic con mayúsculas por el medianil, y pulsa el botón de `action`.
fn apply_lines(
    view: &Entity<ChangesView>,
    cx: &mut TestAppContext,
    lines: &[&str],
    action: PatchAction,
) {
    let viewer = view.read_with(cx, |view, _| view.diff_viewer().clone());
    viewer.update(cx, |viewer, cx| {
        let rows: Vec<usize> = lines
            .iter()
            .map(|wanted| {
                let (kind, content) = match wanted.split_at(1) {
                    ("+", content) => (DiffLineType::Add, content),
                    (_, content) => (DiffLineType::Del, content),
                };
                viewer
                    .lines()
                    .position(|line| line.kind == kind && line.content == content)
                    .unwrap_or_else(|| panic!("no está {wanted}"))
            })
            .collect();
        let (first, last) = (rows[0], rows[rows.len() - 1]);
        viewer.select_lines(first..=last, cx);
        assert_eq!(viewer.selected_lines(), rows, "elegir solo {lines:?}");
        viewer.apply_lines(action, cx);
    });
}

/// `src/m.txt` con un bloque mezclado: dos bajas, tres altas y otra baja.
fn repo_with_a_mixed_hunk() -> TestRepo {
    let repo = TestRepo::new();
    repo.commit_file("src/m.txt", "uno\ndos\ntres\ncuatro\ncinco\n", "Add m");
    repo.write("src/m.txt", "uno\nDOS\nDOS bis\ntres\ncinco\nseis\n");
    repo
}

#[gpui_kit::test]
async fn discarding_a_hunk_asks_first_and_keeps_the_other_hunks(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = repo_with_two_hunks();
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount(cx, &repo_id, ChangesMode::Full);
    wait_for_diff(&app, &view, cx, "+LINE 26");

    // Cancelar no toca nada.
    app.host.answer_confirm(false);
    apply_hunk_action(&view, cx, 1, PatchAction::Discard);
    app.settle(cx, &view);
    assert!(repo.read("src/a.js").contains("LINE 26"));
    let confirms = app.host.confirms();
    assert_eq!(confirms.len(), 1);
    assert_eq!(confirms[0].title.as_deref(), Some("Discard hunk"));
    assert_eq!(
        confirms[0].description.as_deref(),
        Some("Discard this hunk in src/a.js?")
    );
    assert_eq!(confirms[0].confirm_text.as_deref(), Some("Discard"));
    assert_eq!(confirms[0].variant.as_deref(), Some("destructive"));

    app.host.answer_confirm(true);
    apply_hunk_action(&view, cx, 1, PatchAction::Discard);
    app.wait_until(cx, "the hunk to be discarded", |_| {
        !repo.read("src/a.js").contains("LINE 26")
    });
    app.settle(cx, &view);

    let content = repo.read("src/a.js");
    assert!(content.contains("LINE 1\n"), "{content}");
    assert!(content.contains("line 26\n"), "{content}");
    // Solo el working tree: no se preparó nada.
    assert_eq!(repo.git(&["diff", "--cached"]), "");
    assert!(app.exists(cx, "unstaged:file:src/a.js"));
}

#[gpui_kit::test]
async fn staged_hunks_are_not_discarded(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = repo_with_two_hunks();
    repo.git(&["add", "src/a.js"]);
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount_tab(cx, &repo_id, FileTabDescriptor::index("src/a.js"));
    wait_for_diff(&app, &view, cx, "+LINE 26");

    apply_hunk_action(&view, cx, 0, PatchAction::Discard);
    app.settle(cx, &view);

    assert!(app.host.confirms().is_empty());
    assert!(repo.read("src/a.js").contains("LINE 1"));
}

#[gpui_kit::test]
async fn stages_two_lines_of_a_mixed_hunk(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = repo_with_a_mixed_hunk();
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount(cx, &repo_id, ChangesMode::Full);
    wait_for_diff(&app, &view, cx, "+seis");

    apply_lines(&view, cx, &["-dos", "+DOS"], PatchAction::Stage);
    app.wait_until(cx, "the lines to be staged", |_| {
        !repo.git(&["diff", "--cached"]).is_empty()
    });
    app.settle(cx, &view);

    assert_eq!(
        repo.git(&["show", ":src/m.txt"]),
        "uno\nDOS\ntres\ncuatro\ncinco"
    );
    assert_eq!(
        repo.read("src/m.txt"),
        "uno\nDOS\nDOS bis\ntres\ncinco\nseis\n"
    );
    assert!(app.exists(cx, "staged:file:src/m.txt"));
}

#[gpui_kit::test]
async fn unstages_lines_from_the_index_tab(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = repo_with_a_mixed_hunk();
    repo.git(&["add", "src/m.txt"]);
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount_tab(cx, &repo_id, FileTabDescriptor::index("src/m.txt"));
    wait_for_diff(&app, &view, cx, "+seis");

    apply_lines(&view, cx, &["+seis"], PatchAction::Unstage);
    app.wait_until(cx, "the line to be unstaged", |_| {
        !repo.git(&["diff", "--cached"]).contains("+seis")
    });
    app.settle(cx, &view);

    assert_eq!(
        repo.git(&["show", ":src/m.txt"]),
        "uno\nDOS\nDOS bis\ntres\ncinco"
    );
    assert!(repo.git(&["diff"]).contains("+seis"));
}

#[gpui_kit::test]
async fn discarding_lines_asks_first(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = repo_with_a_mixed_hunk();
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount(cx, &repo_id, ChangesMode::Full);
    wait_for_diff(&app, &view, cx, "+seis");

    app.host.answer_confirm(true);
    apply_lines(&view, cx, &["+DOS bis", "-cuatro"], PatchAction::Discard);
    app.wait_until(cx, "the lines to be discarded", |_| {
        !repo.read("src/m.txt").contains("DOS bis")
    });
    app.settle(cx, &view);

    let confirms = app.host.confirms();
    assert_eq!(confirms[0].title.as_deref(), Some("Discard lines"));
    assert_eq!(
        confirms[0].description.as_deref(),
        Some("Discard 2 selected lines in src/m.txt?")
    );
    assert_eq!(
        repo.read("src/m.txt"),
        "uno\nDOS\ntres\ncuatro\ncinco\nseis\n"
    );
    assert_eq!(repo.git(&["diff", "--cached"]), "");
}

// El archivo cambió desde que se leyó el diff: `git apply --check` lo para y
// el aviso dice qué hacer, sin tocar nada.
#[gpui_kit::test]
async fn a_stale_selection_explains_why_it_did_not_apply(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = repo_with_a_mixed_hunk();
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount(cx, &repo_id, ChangesMode::Full);
    wait_for_diff(&app, &view, cx, "+seis");
    app.settle(cx, &view);

    // Ya se preparó por otro lado: el parche no encaja con el índice.
    repo.git(&["add", "src/m.txt"]);
    apply_lines(&view, cx, &["-dos", "+DOS"], PatchAction::Stage);
    let expected = "The selected changes no longer apply to src/m.txt";
    app.wait_until(cx, "the error", |_| {
        app.host
            .messages(NotifyType::Error)
            .iter()
            .any(|message| message.contains(expected))
    });
    app.settle(cx, &view);
    assert_eq!(
        repo.git(&["show", ":src/m.txt"]),
        "uno\nDOS\nDOS bis\ntres\ncinco\nseis"
    );
}

// ---------------------------------------------------------------------------
// Menú contextual de un archivo
// ---------------------------------------------------------------------------

#[gpui_kit::test]
async fn the_context_menu_of_a_row_copies_its_path(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.commit_file("src/a.js", "a\n", "Add a");
    repo.write("src/a.js", "A\n");
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount(cx, &repo_id, ChangesMode::List);
    app.settle(cx, &view);

    app.right_click(cx, "unstaged:file:src/a.js");
    assert!(app.exists(cx, "popup-menu"));
    // Stage, Discard Changes, separador, Blame, Open in editor, Reveal in
    // Explorer, separador, File history, separador, Stash selected files…,
    // separador, Copy Path.
    app.click_menu_item(cx, 11);

    assert!(!app.exists(cx, "popup-menu"));
    assert_eq!(
        cx.read_from_clipboard().and_then(|item| item.text()),
        Some("src/a.js".to_string())
    );
    assert!(
        app.host
            .has_message(NotifyType::Success, "Path copied to clipboard")
    );
}

#[gpui_kit::test]
async fn open_in_editor_from_the_menu_opens_the_working_tree_file_at_the_visible_line(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.commit_file("ñandú b.txt", "x\n", "Add other");
    let lines: Vec<String> = (1..=30).map(|line| format!("line {line}")).collect();
    repo.commit_file("src/a.js", &(lines.join("\n") + "\n"), "Add a");
    let mut changed = lines.clone();
    changed[0] = "LINE 1".to_string();
    changed[25] = "LINE 26".to_string();
    repo.write("src/a.js", &(changed.join("\n") + "\n"));
    repo.write("ñandú b.txt", "y\n");
    let repo_id = app.open(&repo, cx).await;
    app.model
        .update(cx, |model, cx| {
            model.save_external_editor(
                ExternalEditorSettings::custom("/opt/ed", "--goto {path}:{line}:{column}"),
                cx,
            )
        })
        .await;
    let view = app.mount(cx, &repo_id, ChangesMode::Full);
    app.settle(cx, &view);
    app.click(cx, "unstaged:file:src/a.js");
    wait_for_diff(&app, &view, cx, "+LINE 26");
    let repo_path = app.repo_path(cx, &repo_id);

    // El archivo seleccionado: en la primera línea del diff que se ve (la
    // cabecera del primer bloque lleva a la línea 1).
    app.right_click(cx, "unstaged:file:src/a.js");
    // Stage, Discard Changes, separador, Blame, Open in editor…
    app.click_menu_item(cx, 4);
    app.wait_until(cx, "the editor", |_| !app.launcher.launched().is_empty());
    let launched = app.launcher.launched();
    assert_eq!(launched[0].command, "/opt/ed");
    assert_eq!(
        launched[0].args,
        ["--goto".to_string(), format!("{repo_path}/src/a.js:1:1")]
    );

    // Otro archivo (no seleccionado): sin línea, y la ruta con espacios y
    // no ASCII en un solo argumento.
    app.right_click(cx, "unstaged:file:ñandú b.txt");
    app.click_menu_item(cx, 4);
    app.wait_until(cx, "the second file", |_| {
        app.launcher.launched().len() == 2
    });
    assert_eq!(
        app.launcher.launched()[1].args,
        ["--goto".to_string(), format!("{repo_path}/ñandú b.txt:1:1")]
    );
    assert!(app.host.toasts().is_empty());
}

#[gpui_kit::test]
async fn open_in_editor_warns_when_the_file_is_no_longer_there(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.commit_file("src/a.js", "a\n", "Add a");
    repo.write("src/a.js", "A\n");
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount(cx, &repo_id, ChangesMode::List);
    app.settle(cx, &view);

    app.right_click(cx, "unstaged:file:src/a.js");
    std::fs::remove_file(repo.path.join("src/a.js")).unwrap();
    app.click_menu_item(cx, 4);
    app.wait_until(cx, "the warning", |_| {
        app.host.has_message(
            NotifyType::Warning,
            "src/a.js no longer exists in the working tree.",
        )
    });
    assert!(app.launcher.launched().is_empty());
}

#[gpui_kit::test]
async fn the_context_menu_stages_and_opens_the_blame(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.commit_file("src/a.js", "a\n", "Add a");
    repo.write("src/a.js", "A\n");
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount(cx, &repo_id, ChangesMode::List);
    app.settle(cx, &view);

    app.right_click(cx, "unstaged:file:src/a.js");
    app.click_menu_item(cx, 0);
    app.wait_until(cx, "the file to be staged", |cx| {
        app.exists(cx, "staged:file:src/a.js")
    });
    assert_eq!(repo.git(&["diff", "--cached", "--name-only"]), "src/a.js");

    // En lo preparado: Unstage, separador, Blame…
    app.right_click(cx, "staged:file:src/a.js");
    app.click_menu_item(cx, 2);
    assert_eq!(app.dialogs(cx), ["Blame: a.js"]);
}

#[gpui_kit::test]
async fn discarding_a_file_from_the_menu_asks_first(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.commit_file("src/a.js", "a\n", "Add a");
    repo.write("src/a.js", "A\n");
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount(cx, &repo_id, ChangesMode::List);
    app.settle(cx, &view);

    // Primero se cancela: nada cambia.
    app.host.answer_confirm(false);
    app.right_click(cx, "unstaged:file:src/a.js");
    app.click_menu_item(cx, 1);
    app.settle(cx, &view);
    assert_eq!(repo.read("src/a.js"), "A\n");
    let confirms = app.host.confirms();
    assert_eq!(confirms.len(), 1);
    assert_eq!(confirms[0].title.as_deref(), Some("Discard changes"));
    assert_eq!(
        confirms[0].description.as_deref(),
        Some("Discard local changes in src/a.js?")
    );
    assert_eq!(confirms[0].confirm_text.as_deref(), Some("Discard"));
    assert_eq!(confirms[0].variant.as_deref(), Some("destructive"));

    app.host.answer_confirm(true);
    app.right_click(cx, "unstaged:file:src/a.js");
    app.click_menu_item(cx, 1);
    app.wait_until(cx, "the discard", |_| repo.read("src/a.js") == "a\n");
    app.settle(cx, &view);
    assert!(app.exists(cx, "changes-empty"));
}

#[gpui_kit::test]
async fn discard_all_counts_the_unstaged_files_and_keeps_the_staged_ones(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.commit_file("src/a.js", "a\n", "Add a");
    repo.commit_file("src/b.js", "b\n", "Add b");
    repo.write("src/a.js", "A\n");
    repo.write("src/b.js", "B\n");
    repo.write("staged.txt", "staged\n");
    repo.git(&["add", "staged.txt"]);
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount(cx, &repo_id, ChangesMode::List);
    app.settle(cx, &view);

    app.host.answer_confirm(true);
    app.click(cx, "discard-all");
    app.wait_until(cx, "the discard", |_| {
        repo.read("src/a.js") == "a\n" && repo.read("src/b.js") == "b\n"
    });
    app.settle(cx, &view);

    let confirms = app.host.confirms();
    assert_eq!(
        confirms[0].description.as_deref(),
        Some("This will discard all unstaged changes (2 files).")
    );
    assert_eq!(confirms[0].confirm_text.as_deref(), Some("Discard all"));
    assert_eq!(repo.git(&["diff", "--cached", "--name-only"]), "staged.txt");
}

#[gpui_kit::test]
async fn ignoring_an_untracked_file_adds_it_to_gitignore(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.write("notes.txt", "notes\n");
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount(cx, &repo_id, ChangesMode::List);
    app.settle(cx, &view);

    app.right_click(cx, "unstaged:file:notes.txt");
    // Stage, Discard Changes, separador, Ignore…
    app.click_menu_item(cx, 3);
    app.wait_until(cx, "the ignore", |cx| {
        !app.exists(cx, "unstaged:file:notes.txt")
    });

    assert!(repo.read(".gitignore").contains("notes.txt"));
    assert!(app.exists(cx, "unstaged:file:.gitignore"));
}

// ---------------------------------------------------------------------------
// Conflictos
// ---------------------------------------------------------------------------

#[gpui_kit::test]
async fn resolving_a_conflict_stages_the_file(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new().with_merge_conflict();
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount_tab(cx, &repo_id, FileTabDescriptor::working("notes.txt"));
    app.wait_until(cx, "the conflict", |cx| {
        view.read_with(cx, |view, _| view.file_content().contains("<<<<<<<"))
    });

    // El resolvedor analiza las tres versiones en un hilo de fondo.
    app.wait_until(cx, "the analysis", |cx| {
        app.in_window(cx, |_, _| {});
        view.read_with(cx, |view, cx| {
            !view.conflict_resolver().read(cx).is_loading()
        })
    });
    app.click(cx, "take-incoming");
    app.click(cx, "mark-resolved");
    app.wait_until(cx, "the resolution", |cx| {
        app.model.read_with(cx, |model, _| {
            model
                .repository(repo_id.as_str())
                .is_some_and(|repository| repository.conflict_state.pending_count == 0)
        })
    });
    app.settle(cx, &view);

    assert_eq!(repo.read("notes.txt"), "from feature\n");
    assert_eq!(repo.git(&["diff", "--cached", "--name-only"]), "notes.txt");
    assert!(
        repo.git(&["diff", "--name-only", "--diff-filter=U"])
            .is_empty()
    );
    // La pestaña era del lado del working tree, que ya no tiene cambios.
    assert_eq!(
        app.label(cx, "file-review-state").as_deref(),
        Some("No changes to display for this file in Local.")
    );
}

// ---------------------------------------------------------------------------
// Divisor y desplazamiento
// ---------------------------------------------------------------------------

#[gpui_kit::test]
async fn the_divider_resizes_the_list(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.write("notes.txt", "notes\n");
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount(cx, &repo_id, ChangesMode::List);
    app.settle(cx, &view);
    let width = |cx: &mut TestAppContext| view.read_with(cx, |view, cx| view.sidebar_width(cx));
    // La columna de la lista y la caja de commit empieza en 400: lo justo
    // para la lista y una caja de commit compacta, y el resto para el diff.
    assert_eq!(width(cx), 400.0);

    let start = app.in_window(cx, |window, _| {
        window.find("changes-resizer").bounds().center()
    });
    app.in_window(cx, |window, cx| {
        window.drag(start, point(px(450.), start.y), cx);
    });
    assert_eq!(width(cx), 450.0);
    // Se guarda al soltar.
    app.wait_until(cx, "the saved width", |cx| {
        app.model
            .read_with(cx, |model, _| model.local_changes_prefs().width)
            == Some(450.0)
    });

    // Más allá del máximo se queda en él.
    let start = app.in_window(cx, |window, _| {
        window.find("changes-resizer").bounds().center()
    });
    app.in_window(cx, |window, cx| {
        window.drag(start, point(px(1150.), start.y), cx);
    });
    assert_eq!(width(cx), 840.0);

    // El doble clic vuelve al ancho inicial (y olvida el guardado).
    app.double_click(cx, "changes-resizer");
    assert_eq!(width(cx), 400.0);
    assert_eq!(
        app.model
            .read_with(cx, |model, _| model.local_changes_prefs().width),
        None
    );
}

#[gpui_kit::test]
async fn a_file_tab_shares_the_saved_list_width_and_resets_it_on_double_click(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.write("notes.txt", "notes\n");
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount_tab(cx, &repo_id, FileTabDescriptor::working("notes.txt"));
    app.settle(cx, &view);
    let width = |cx: &mut TestAppContext| view.read_with(cx, |view, cx| view.sidebar_width(cx));
    let shared = |cx: &mut TestAppContext| {
        app.model
            .read_with(cx, |model, _| model.file_list_width().width)
    };
    assert_eq!(width(cx), 240.0);

    let start = app.in_window(cx, |window, _| {
        window.find("changes-resizer").bounds().center()
    });
    app.in_window(cx, |window, cx| {
        window.drag(start, point(start.x + px(100.), start.y), cx);
    });
    // El ancho es hasta el puntero, que estaba en el centro del divisor.
    let expected = f64::from(f32::from(start.x)) + 100.0;
    assert_eq!(width(cx), expected);
    assert_eq!(shared(cx), expected);

    app.double_click(cx, "changes-resizer");
    assert_eq!(width(cx), 240.0);
}

#[gpui_kit::test]
async fn hiding_the_file_list_leaves_the_diff_alone(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.write("notes.txt", "notes\n");
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount_tab(cx, &repo_id, FileTabDescriptor::working("notes.txt"));
    wait_for_diff(&app, &view, cx, "+notes");
    assert!(app.exists(cx, "changes-list"));

    view.update(cx, |view, cx| view.set_files_open(false, cx));

    assert!(!app.exists(cx, "changes-list"));
    assert!(!app.exists(cx, "changes-resizer"));
    assert!(app.exists(cx, "toggle-diff-view"));
}

/// Una pestaña vuelve a donde estaba, y lo que se desplaza queda recordado en
/// el espacio de trabajo.
#[gpui_kit::test]
async fn a_file_tab_restores_and_remembers_its_scroll(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    let long: String = (1..=400).map(|line| format!("line {line}\n")).collect();
    repo.write("long.txt", &long);
    let repo_id = app.open(&repo, cx).await;
    let path = app.repo_path(cx, &repo_id);
    let descriptor = FileTabDescriptor::working("long.txt");
    app.model.update(cx, |model, cx| {
        model.open_workspace_file(&path, &descriptor, false, cx);
        model.remember_workspace_scroll(
            &path,
            &descriptor.id(),
            Scroll {
                top: 300.0,
                left: 0.0,
            },
            cx,
        );
    });
    let view = app.mount(
        cx,
        &repo_id,
        ChangesMode::FileTab {
            tab_id: descriptor.id(),
        },
    );
    wait_for_diff(&app, &view, cx, "+line 400");
    app.settle(cx, &view);

    let viewer = view.read_with(cx, |view, _| view.diff_viewer().clone());
    let top =
        |cx: &mut TestAppContext| viewer.read_with(cx, |viewer, _| viewer.scroll_position().top);
    assert_eq!(top(cx), 300.0);

    app.in_window(cx, |window, cx| {
        window.scroll(
            "changes-main",
            gpui_kit::ScrollDelta::Pixels(point(px(0.), px(-200.))),
            cx,
        );
    });
    app.in_window(cx, |_, _| {});
    let remembered = app.model.read_with(cx, |model, _| {
        model
            .repository_workspace(&path)
            .tabs
            .iter()
            .find(|tab| tab.id == descriptor.id())
            .map(|tab| tab.scroll.top)
    });
    assert_eq!(remembered, Some(f64::from(top(cx))));
    assert_eq!(remembered, Some(500.0));
}
