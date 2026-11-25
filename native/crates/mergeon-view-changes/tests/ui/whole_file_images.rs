//! "Whole file" e imágenes antes/después en el diff de Local Changes, con un
//! repositorio git real.

use gpui_kit::TestAppContext;
use mergeon_view_changes::ChangesMode;

use crate::support::{TestApp, TestRepo};

/// Un PNG de 1×1 de verdad.
const PIXEL_PNG: [u8; 70] = [
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0x64, 0x60, 0xf8, 0x5f,
    0x0f, 0x00, 0x02, 0x87, 0x01, 0x80, 0xeb, 0x47, 0xba, 0x92, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
    0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

// Con "Whole file" el diff trae el archivo entero en un bloque: sin botones de
// bloque (la nota lo explica) y la preferencia se recuerda.
#[gpui_kit::test]
async fn whole_file_brings_the_whole_file_without_block_buttons(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    let base: String = (1..=40).map(|n| format!("line {n}\n")).collect();
    repo.commit_file("notes.txt", &base, "notes");
    repo.write("notes.txt", &base.replace("line 20\n", "line twenty\n"));
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount(cx, &repo_id, ChangesMode::List);
    app.settle(cx, &view);

    app.click(cx, "unstaged:file:notes.txt");
    app.wait_until(cx, "the diff of the changes", |cx| {
        view.read_with(cx, |view, _| {
            view.file_diff().contains("+line twenty") && !view.file_diff().contains(" line 1\n")
        })
    });
    app.settle(cx, &view);
    assert!(app.exists(cx, ("hunk-action", 0usize)));

    app.click(cx, "toggle-whole-file");
    app.wait_until(cx, "the whole file", |cx| {
        view.read_with(cx, |view, _| {
            view.file_diff().contains(" line 1\n") && view.file_diff().contains(" line 40\n")
        })
    });
    app.settle(cx, &view);
    assert!(!app.exists(cx, ("hunk-action", 0usize)));
    assert!(app.exists(cx, "diff-note"));
    assert!(
        app.model
            .read_with(cx, |model, _| model.diff_preferences().whole_file)
    );
}

// Una imagen cambiada sin preparar: el índice frente al disco, lado a lado.
#[gpui_kit::test]
async fn a_changed_image_shows_the_index_and_the_disk(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    std::fs::write(repo.path.join("logo.png"), PIXEL_PNG).expect("write logo");
    repo.git(&["add", "logo.png"]);
    repo.git(&["commit", "-q", "-m", "logo"]);
    let mut changed = PIXEL_PNG.to_vec();
    changed.extend_from_slice(b"trailing bytes");
    std::fs::write(repo.path.join("logo.png"), &changed).expect("write logo");
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount(cx, &repo_id, ChangesMode::List);
    app.settle(cx, &view);

    app.click(cx, "unstaged:file:logo.png");
    app.wait_until(cx, "both versions", |cx| {
        app.exists(cx, "image-before") && app.exists(cx, "image-after")
    });
    let before = app.label(cx, "image-before").unwrap_or_default();
    assert!(before.starts_with("1 × 1 px"), "{before}");
    assert!(!app.exists(cx, "diff-empty"));
}

// "Whole file" no salta: vuelve a la línea que se leía en lugar de quedarse en
// el mismo desplazamiento en píxeles, que en el otro diff es otro sitio.
#[gpui_kit::test]
async fn whole_file_keeps_the_line_in_view(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    let base: String = (1..=600).map(|n| format!("line {n}\n")).collect();
    repo.commit_file("long.txt", &base, "long");
    let edited: String = (1..=600)
        .map(|n| {
            if n % 40 == 0 {
                format!("line {n} edited\n")
            } else {
                format!("line {n}\n")
            }
        })
        .collect();
    repo.write("long.txt", &edited);
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount(cx, &repo_id, ChangesMode::List);
    app.settle(cx, &view);

    app.click(cx, "unstaged:file:long.txt");
    app.wait_until(cx, "the changes", |cx| {
        view.read_with(cx, |view, _| view.file_diff().contains("+line 320 edited"))
    });
    app.settle(cx, &view);
    let viewer = view.read_with(cx, |view, _| view.diff_viewer().clone());
    viewer.update(cx, |viewer, cx| {
        viewer.set_scroll_position(mergeon_view_diff::ScrollPosition::new(1_400., 0.), cx)
    });
    app.in_window(cx, |_, _| {});
    let anchor = viewer
        .update(cx, |viewer, _| viewer.line_anchor())
        .expect("una línea arriba");
    assert!((150..500).contains(&anchor.line), "{anchor:?}");

    app.click(cx, "toggle-whole-file");
    app.wait_until(cx, "the whole file", |cx| {
        view.read_with(cx, |view, _| view.file_diff().contains(" line 1\n"))
    });
    app.settle(cx, &view);
    let whole = viewer.update(cx, |viewer, _| viewer.line_anchor()).unwrap();
    assert_eq!(whole.line, anchor.line);
    assert!(
        (whole.offset - anchor.offset).abs() < 1.,
        "{whole:?} {anchor:?}"
    );
}

// Un SVG sin preparar se ve pintado y, con "Text", como el diff de su XML,
// con las acciones de bloque de cualquier archivo de texto.
#[gpui_kit::test]
async fn a_changed_svg_can_be_read_as_text(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    let svg = |fill: &str| {
        format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"40\" height=\"40\">\n  \
             <rect width=\"40\" height=\"40\" fill=\"{fill}\"/>\n</svg>\n"
        )
    };
    repo.commit_file("logo.svg", &svg("red"), "logo");
    repo.write("logo.svg", &svg("blue"));
    let repo_id = app.open(&repo, cx).await;
    let view = app.mount(cx, &repo_id, ChangesMode::List);
    app.settle(cx, &view);

    app.click(cx, "unstaged:file:logo.svg");
    app.wait_until(cx, "both versions", |cx| {
        app.exists(cx, "image-before") && app.exists(cx, "image-after")
    });
    let before = app.label(cx, "image-before").unwrap_or_default();
    assert!(before.starts_with("40 × 40 px"), "{before}");
    assert!(!app.exists(cx, "toggle-whole-file"));

    app.click(cx, "tab-svg-text");
    app.settle(cx, &view);
    assert!(!app.exists(cx, "image-before"));
    assert!(app.exists(cx, "toggle-whole-file"));
    assert!(app.exists(cx, ("hunk-action", 0usize)));
    assert!(view.read_with(cx, |view, _| view.file_diff().contains("fill=\"blue\"")));
}
