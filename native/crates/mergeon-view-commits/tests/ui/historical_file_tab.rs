//! La pestaña de archivo histórico (`HistoricalFileTab.vue`, sin tests en el
//! original): carga del diff o del archivo, la lista lateral, el menú de
//! archivo, el reemplazo de pestaña y el reintento.

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{Entity, TestAppContext};
use mergeon_backend::ExternalEditorSettings;
use mergeon_model::notify::NotifyType;
use mergeon_model::workspace_tabs::FileTabDescriptor;
use mergeon_view_commits::{HistoricalFileTabView, file_row_id};

use crate::support::{TestApp, TestRepo};

/// Un PNG de 1×1 de verdad.
const PIXEL_PNG: [u8; 70] = [
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0x64, 0x60, 0xf8, 0x5f,
    0x0f, 0x00, 0x02, 0x87, 0x01, 0x80, 0xeb, 0x47, 0xba, 0x92, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
    0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

/// Un commit que toca dos archivos.
fn repo() -> TestRepo {
    let repo = TestRepo::empty();
    repo.commit_file("src/a.js", "a\n", "base");
    repo.write("src/a.js", "a2\n");
    repo.write("src/b.js", "b\n");
    repo.git(&["add", "--all"]);
    repo.git(&["commit", "-q", "-m", "two files"]);
    repo
}

/// Abre la pestaña de `descriptor` en el espacio de trabajo y la monta.
async fn open_tab(
    app: &TestApp,
    repo: &TestRepo,
    descriptor: FileTabDescriptor,
    cx: &mut TestAppContext,
) -> Entity<HistoricalFileTabView> {
    let repo_id = app.open(repo, cx).await;
    let path = repo.path_string();
    let tab_id = descriptor.id();
    app.model.update(cx, |model, cx| {
        model.open_workspace_file(&path, &descriptor, false, cx);
    });
    app.mount(cx, move |window, cx| {
        HistoricalFileTabView::new(repo_id, tab_id, window, cx)
    })
}

fn active_tab(app: &TestApp, repo: &TestRepo, cx: &mut TestAppContext) -> String {
    let path = repo.path_string();
    app.model
        .read_with(cx, |model, _| model.repository_workspace(&path).active_tab)
}

#[gpui_kit::test]
async fn shows_the_diff_of_the_file_and_the_files_of_its_commit(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = repo();
    let head = repo.head();
    let view = open_tab(
        &app,
        &repo,
        FileTabDescriptor::commit("src/a.js", &head),
        cx,
    )
    .await;

    app.wait_for(cx, &view, "the diff", |view, _| {
        !view.is_loading() && view.diff().contains("+a2") && view.files().len() == 2
    });
    assert!(app.exists(cx, file_row_id("src/a.js")));
    assert!(app.exists(cx, file_row_id("src/b.js")));
    assert!(app.exists(cx, "toggle-diff-view"));
}

/// Clic en otro archivo: la misma pestaña pasa a mostrarlo.
#[gpui_kit::test]
async fn a_click_in_the_list_swaps_the_file_of_the_current_tab(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = repo();
    let head = repo.head();
    let view = open_tab(
        &app,
        &repo,
        FileTabDescriptor::commit("src/a.js", &head),
        cx,
    )
    .await;
    app.wait_for(cx, &view, "the diff", |view, _| view.files().len() == 2);

    app.click(cx, file_row_id("src/b.js"));

    let b = FileTabDescriptor::commit("src/b.js", &head).id();
    assert_eq!(active_tab(&app, &repo, cx), b);
    // El panel sigue montado y ahora es el de la pestaña nueva.
    app.wait_for(cx, &view, "the swapped diff", |view, _| {
        view.tab_id() == b && view.diff().contains("+b")
    });
    let tabs = app.model.read_with(cx, |model, _| {
        model.repository_workspace(&repo.path_string()).tabs.len()
    });
    assert_eq!(tabs, 1);
}

#[gpui_kit::test]
async fn the_file_menu_opens_in_a_new_tab_and_copies_the_path(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = repo();
    let head = repo.head();
    let view = open_tab(
        &app,
        &repo,
        FileTabDescriptor::commit("src/a.js", &head),
        cx,
    )
    .await;
    app.wait_for(cx, &view, "the diff", |view, _| view.files().len() == 2);

    app.in_window(cx, |window, cx| {
        window.right_click(file_row_id("src/b.js"), cx)
    });
    // Open here · Open in new tab · Open in editor · separador · File history ·
    // separador · Copy Path
    app.click_menu_item(cx, 6);
    assert!(
        app.host
            .has_message(NotifyType::Success, "Path copied to clipboard")
    );
    let copied = cx.read_from_clipboard().and_then(|item| item.text());
    assert_eq!(copied.as_deref(), Some("src/b.js"));

    app.in_window(cx, |window, cx| {
        window.right_click(file_row_id("src/b.js"), cx)
    });
    app.click_menu_item(cx, 1);
    let tabs = app.model.read_with(cx, |model, _| {
        model.repository_workspace(&repo.path_string()).tabs.len()
    });
    assert_eq!(tabs, 2);
}

/// Con un remoto de GitHub el menú de archivo ofrece además el enlace al
/// archivo en el commit; sin él, no.
#[gpui_kit::test]
async fn the_file_menu_copies_the_github_link_only_on_github(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = repo();
    repo.git(&["remote", "add", "origin", "git@github.com:octo/demo"]);
    let head = repo.head();
    let view = open_tab(
        &app,
        &repo,
        FileTabDescriptor::commit("src/a.js", &head),
        cx,
    )
    .await;
    app.wait_for(cx, &view, "the diff", |view, _| view.files().len() == 2);

    app.in_window(cx, |window, cx| {
        window.right_click(file_row_id("src/b.js"), cx)
    });
    // Open here · Open in new tab · Open in editor · separador · File history ·
    // separador · Copy Path · Copy GitHub link
    assert_eq!(app.menu_len(cx), 8);
    app.click_menu_item(cx, 7);
    assert_eq!(
        app.clipboard(cx),
        Some(format!("https://github.com/octo/demo/blob/{head}/src/b.js"))
    );
    assert!(
        app.host
            .has_message(NotifyType::Success, "GitHub link copied to clipboard")
    );

    let app = TestApp::new(cx);
    let local = self::repo();
    let head = local.head();
    let view = open_tab(
        &app,
        &local,
        FileTabDescriptor::commit("src/a.js", &head),
        cx,
    )
    .await;
    app.wait_for(cx, &view, "the diff", |view, _| view.files().len() == 2);
    app.in_window(cx, |window, cx| {
        window.right_click(file_row_id("src/b.js"), cx)
    });
    // Sin GitHub: Open here · Open in new tab · Open in editor · separador ·
    // File history · separador · Copy Path
    assert_eq!(app.menu_len(cx), 7);
}

#[gpui_kit::test]
async fn open_in_editor_opens_the_working_tree_copy_and_warns_if_it_is_gone(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let repo = repo();
    let head = repo.head();
    let view = open_tab(
        &app,
        &repo,
        FileTabDescriptor::commit("src/a.js", &head),
        cx,
    )
    .await;
    app.wait_for(cx, &view, "the diff", |view, _| view.files().len() == 2);
    app.model
        .update(cx, |model, cx| {
            model.save_external_editor(
                ExternalEditorSettings::custom("/opt/ed", "{path}:{line}"),
                cx,
            )
        })
        .await;

    // El archivo que se ve: en la línea del diff (la 1, "a2").
    app.in_window(cx, |window, cx| {
        window.right_click(file_row_id("src/a.js"), cx)
    });
    // Open here · Open in new tab · Open in editor…
    app.click_menu_item(cx, 2);
    app.wait_until(cx, "the editor", |app, _| {
        !app.launcher.launched().is_empty()
    });
    let root = repo.path_string();
    assert_eq!(app.launcher.launched()[0].command, "/opt/ed");
    assert_eq!(
        app.launcher.launched()[0].args,
        [format!("{root}/src/a.js:1")]
    );

    // Otro archivo, sin línea; y si ya no existe en el working tree, se avisa.
    std::fs::remove_file(repo.path.join("src/b.js")).unwrap();
    app.in_window(cx, |window, cx| {
        window.right_click(file_row_id("src/b.js"), cx)
    });
    app.click_menu_item(cx, 2);
    app.wait_until(cx, "the warning", |app, _| {
        app.host.has_message(
            NotifyType::Warning,
            "src/b.js no longer exists in the working tree.",
        )
    });
    assert_eq!(app.launcher.launched().len(), 1);
}

#[gpui_kit::test]
async fn a_tree_tab_without_a_file_asks_to_pick_one(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = repo();
    let head = repo.head();
    let view = open_tab(&app, &repo, FileTabDescriptor::tree("", &head), cx).await;
    app.wait_for(cx, &view, "the tree", |view, cx| {
        !view.tree().read(cx).rows().is_empty()
    });

    assert_eq!(
        app.label(cx, "file-state").as_deref(),
        Some("Pick a file on the left to see it as it was at this commit.")
    );
    // Un clic en un archivo del árbol lo abre en la misma pestaña.
    let tree = view.read_with(cx, |view, _| view.tree().clone());
    tree.update(cx, |tree, cx| tree.activate("src", cx));
    app.wait_for(cx, &tree, "the src folder", |tree, _| {
        tree.rows().iter().any(|row| row.entry.path == "src/a.js")
    });
    tree.update(cx, |tree, cx| tree.activate("src/a.js", cx));
    assert_eq!(
        active_tab(&app, &repo, cx),
        FileTabDescriptor::tree("src/a.js", &head).id()
    );
    let viewer = view.read_with(cx, |view, _| view.file_viewer().clone());
    app.wait_for(cx, &viewer, "the file", |viewer, _| {
        viewer
            .file()
            .is_some_and(|file| file.content.contains("a2"))
    });
}

#[gpui_kit::test]
async fn a_missing_commit_offers_to_retry(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = repo();
    let gone = "0".repeat(40);
    let view = open_tab(
        &app,
        &repo,
        FileTabDescriptor::commit("src/a.js", &gone),
        cx,
    )
    .await;

    app.wait_for(cx, &view, "the error", |view, _| {
        !view.is_loading() && !view.error().is_empty()
    });
    assert!(app.exists(cx, "retry"));
    app.click(cx, "retry");
    app.wait_for(cx, &view, "the retried error", |view, _| {
        !view.is_loading() && !view.error().is_empty()
    });
}

#[gpui_kit::test]
async fn the_file_list_folds_away(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = repo();
    let head = repo.head();
    let view = open_tab(
        &app,
        &repo,
        FileTabDescriptor::commit("src/a.js", &head),
        cx,
    )
    .await;
    app.wait_for(cx, &view, "the diff", |view, _| view.files().len() == 2);
    assert!(app.exists(cx, "files-resizer"));

    view.update(cx, |view, cx| view.set_files_open(false, cx));

    assert!(!app.exists(cx, "files-resizer"));
    assert!(!app.exists(cx, file_row_id("src/a.js")));
}

/// "Whole file": el diff del commit con el archivo entero; el botón lo
/// enciende y lo apaga, y la preferencia se recuerda.
#[gpui_kit::test]
async fn whole_file_shows_the_whole_file_in_the_diff(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::empty();
    let base: String = (1..=40).map(|n| format!("line {n}\n")).collect();
    repo.commit_file("notes.txt", &base, "base");
    repo.commit_file(
        "notes.txt",
        &base.replace("line 20\n", "line twenty\n"),
        "edit",
    );
    let head = repo.head();
    let view = open_tab(
        &app,
        &repo,
        FileTabDescriptor::commit("notes.txt", &head),
        cx,
    )
    .await;

    app.wait_for(cx, &view, "the changes", |view, _| {
        !view.is_loading()
            && view.diff().contains("+line twenty")
            && !view.diff().contains(" line 1\n")
    });
    app.click(cx, "toggle-whole-file");
    app.wait_for(cx, &view, "the whole file", |view, _| {
        !view.is_loading()
            && view.diff().contains(" line 1\n")
            && view.diff().contains(" line 40\n")
    });
    assert!(
        app.model
            .read_with(cx, |model, _| model.diff_preferences().whole_file)
    );

    app.click(cx, "toggle-whole-file");
    app.wait_for(cx, &view, "only the changes again", |view, _| {
        !view.is_loading() && !view.diff().contains(" line 1\n")
    });
}

/// Una imagen cambiada en un commit: antes y después lado a lado, sin los
/// botones del diff de texto.
#[gpui_kit::test]
async fn an_image_changed_in_a_commit_shows_before_and_after(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::empty();
    let logo = repo.path.join("logo.png");
    std::fs::write(&logo, PIXEL_PNG).unwrap();
    repo.git(&["add", "logo.png"]);
    repo.git(&["commit", "-q", "-m", "add logo"]);
    let mut changed = PIXEL_PNG.to_vec();
    changed.extend_from_slice(b"trailing bytes");
    std::fs::write(&logo, &changed).unwrap();
    repo.git(&["commit", "-q", "-am", "change logo"]);
    let head = repo.head();
    let view = open_tab(
        &app,
        &repo,
        FileTabDescriptor::commit("logo.png", &head),
        cx,
    )
    .await;

    app.wait_for(cx, &view, "both versions", |view, cx| {
        view.image_viewer().read(cx).images().is_some_and(|images| {
            images
                .before
                .as_ref()
                .is_some_and(|before| before.size == 70)
                && images.after.as_ref().is_some_and(|after| after.size == 84)
        })
    });
    let before = app.label(cx, "image-before").unwrap_or_default();
    assert!(before.starts_with("1 × 1 px"), "{before}");
    assert!(app.exists(cx, "image-after"));
    assert!(!app.exists(cx, "toggle-whole-file"));
    assert!(!app.exists(cx, "toggle-diff-view"));
}

/// Un SVG cambiado en un commit: por defecto antes y después pintados;
/// "Text" enseña el diff de su XML con los botones del diff de texto.
#[gpui_kit::test]
async fn an_svg_can_be_read_as_text(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::empty();
    let svg = |fill: &str| {
        format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"120\" height=\"120\">\n  \
             <circle cx=\"60\" cy=\"60\" r=\"50\" fill=\"{fill}\"/>\n</svg>\n"
        )
    };
    repo.commit_file("img/logo.svg", &svg("red"), "add logo");
    repo.commit_file("img/logo.svg", &svg("blue"), "recolor logo");
    let head = repo.head();
    let view = open_tab(
        &app,
        &repo,
        FileTabDescriptor::commit("img/logo.svg", &head),
        cx,
    )
    .await;

    app.wait_for(cx, &view, "both versions and the diff", |view, cx| {
        !view.is_loading()
            && view.diff().contains("fill=\"blue\"")
            && view.image_viewer().read(cx).images().is_some()
    });
    let before = app.label(cx, "image-before").unwrap_or_default();
    assert!(before.starts_with("120 × 120 px"), "{before}");
    assert!(app.exists(cx, "tab-svg-text"));
    assert!(!app.exists(cx, "toggle-whole-file"));

    app.click(cx, "tab-svg-text");
    assert!(view.read_with(cx, |view, cx| view.image_viewer().read(cx).svg_as_text()));
    assert!(!app.exists(cx, "image-before"));
    assert!(app.exists(cx, "toggle-whole-file"));
    assert!(app.exists(cx, "toggle-diff-view"));
    let shown = view.read_with(cx, |view, cx| {
        view.diff_viewer().read(cx).diff().to_string()
    });
    assert!(shown.contains("+  <circle cx=\"60\" cy=\"60\" r=\"50\" fill=\"blue\"/>"));

    app.click(cx, "tab-svg-image");
    assert!(app.exists(cx, "image-before"));
    assert!(!app.exists(cx, "toggle-whole-file"));
}

/// "Whole file" vuelve a la línea que se estaba leyendo, no al mismo
/// desplazamiento en píxeles (que en el otro diff es otro sitio), y al
/// quitarlo, a la más cercana que siga en los bloques.
#[gpui_kit::test]
async fn whole_file_keeps_the_line_that_was_being_read(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::empty();
    let base: String = (1..=600).map(|n| format!("line {n}\n")).collect();
    let edited: String = (1..=600)
        .map(|n| {
            if n % 40 == 0 {
                format!("line {n} edited\n")
            } else {
                format!("line {n}\n")
            }
        })
        .collect();
    repo.commit_file("long.txt", &base, "base");
    repo.commit_file("long.txt", &edited, "scattered edits");
    let head = repo.head();
    let view = open_tab(
        &app,
        &repo,
        FileTabDescriptor::commit("long.txt", &head),
        cx,
    )
    .await;
    app.wait_for(cx, &view, "the changes", |view, _| {
        !view.is_loading() && view.diff().contains("+line 320 edited")
    });

    let viewer = view.read_with(cx, |view, _| view.diff_viewer().clone());
    // Por bloques, hacia la mitad del diff.
    viewer.update(cx, |viewer, cx| {
        viewer.set_scroll_position(mergeon_view_diff::ScrollPosition::new(1_400., 0.), cx)
    });
    app.settle(cx, 30);
    app.in_window(cx, |_, _| {});
    let anchor = viewer
        .update(cx, |viewer, _| viewer.line_anchor())
        .expect("una línea arriba");
    assert!((150..500).contains(&anchor.line), "{anchor:?}");

    app.click(cx, "toggle-whole-file");
    app.wait_for(cx, &view, "the whole file", |view, _| {
        !view.is_loading() && view.diff().contains(" line 1\n")
    });
    app.in_window(cx, |_, _| {});
    let whole = viewer.update(cx, |viewer, _| viewer.line_anchor()).unwrap();
    assert_eq!(whole.line, anchor.line);
    assert!(
        (whole.offset - anchor.offset).abs() < 1.,
        "{whole:?} {anchor:?}"
    );

    // En el archivo entero, una línea entre dos bloques: la 215 (los bloques
    // llegan a la 203 y empiezan en la 237).
    let row = viewer.read_with(cx, |viewer, _| {
        viewer
            .lines()
            .position(|line| line.new_num == Some(215))
            .unwrap()
    });
    viewer.update(cx, |viewer, cx| {
        viewer.set_scroll_position(
            mergeon_view_diff::ScrollPosition::new(row as f32 * 20., 0.),
            cx,
        )
    });
    app.in_window(cx, |_, _| {});
    app.in_window(cx, |_, _| {});
    let line = viewer.update(cx, |viewer, _| viewer.line_anchor()).unwrap();
    assert!((212..=218).contains(&line.line), "{line:?}");

    app.click(cx, "toggle-whole-file");
    app.wait_for(cx, &view, "only the changes", |view, _| {
        !view.is_loading() && !view.diff().contains(" line 1\n")
    });
    app.in_window(cx, |_, _| {});
    let nearest = viewer.update(cx, |viewer, _| viewer.line_anchor()).unwrap();
    assert_eq!(nearest.line, 203);
}

fn renders(view: &Entity<HistoricalFileTabView>, cx: &mut TestAppContext) -> usize {
    view.read_with(cx, |view, _| view.render_count())
}

/// El modelo avisa por cualquier cosa (un toast, el progreso de una
/// operación, otra pestaña): la pestaña solo se repinta si cambió algo de lo
/// que usa, esté a la vista o en segundo plano.
#[gpui_kit::test]
async fn unrelated_model_notifications_do_not_repaint_the_tab(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = repo();
    let head = repo.head();
    let view = open_tab(
        &app,
        &repo,
        FileTabDescriptor::commit("src/a.js", &head),
        cx,
    )
    .await;
    app.wait_for(cx, &view, "the diff", |view, _| {
        !view.is_loading() && view.files().len() == 2
    });
    app.settle(cx, 50);

    // A la vista: avisos sin nada para la pestaña.
    let before = renders(&view, cx);
    for _ in 0..3 {
        app.model.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
    }
    app.model.update(cx, |model, cx| {
        model.notify_success("Something unrelated happened", cx);
    });
    cx.run_until_parked();
    assert_eq!(renders(&view, cx), before, "an idle tab repainted");

    // Su propio scroll lo guarda en la sesión, pero no cambia lo que pinta.
    let path = repo.path_string();
    let tab_id = view.read_with(cx, |view, _| view.tab_id().to_string());
    app.model.update(cx, |model, cx| {
        model.remember_workspace_scroll(
            &path,
            &tab_id,
            mergeon_model::workspace_tabs::Scroll { top: 40., left: 0. },
            cx,
        );
    });
    cx.run_until_parked();
    assert_eq!(
        renders(&view, cx),
        before,
        "remembering the scroll repainted"
    );

    // En segundo plano: otra pestaña pasa a ser la activa (eso sí la toca)...
    app.model.update(cx, |model, cx| {
        model.open_workspace_file(
            &path,
            &FileTabDescriptor::commit("src/b.js", &head),
            false,
            cx,
        );
    });
    cx.run_until_parked();
    assert!(!view.read_with(cx, |view, cx| view.is_active(cx)));
    let background = renders(&view, cx);
    // ...y lo que pase después en el modelo ya no.
    for _ in 0..3 {
        app.model.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
    }
    app.model.update(cx, |model, cx| {
        model.notify_success("Another unrelated toast", cx);
    });
    cx.run_until_parked();
    assert_eq!(renders(&view, cx), background, "a background tab repainted");
    assert_eq!(view.read_with(cx, |view, _| view.files().len()), 2);

    // Un cambio de verdad sí la repinta: Split/Unified.
    app.model
        .update(cx, |model, cx| model.toggle_diff_view(cx).detach());
    cx.run_until_parked();
    assert!(renders(&view, cx) > background);
}
