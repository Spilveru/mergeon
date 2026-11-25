//! `CommitChangesTab.test.js`.

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{Entity, TestAppContext};
use mergeon_backend::FileChange;
use mergeon_ui::menu::MenuItem;
use mergeon_view_commits::{CommitChangesTab, CommitChangesTabEvent, file_row_id};
use mergeon_view_diff::{DiffViewMode, file_stats};

use crate::support::TestApp;

fn change(path: &str, status: &str, additions: u64, deletions: u64) -> FileChange {
    FileChange {
        path: path.to_string(),
        status: status.to_string(),
        additions,
        deletions,
    }
}

type Events = Rc<RefCell<Vec<CommitChangesTabEvent>>>;

fn mount_tab(
    app: &TestApp,
    files: Vec<FileChange>,
    selected: Option<&str>,
    mode: DiffViewMode,
    cx: &mut TestAppContext,
) -> (Entity<CommitChangesTab>, Events) {
    let tab = app.mount(cx, CommitChangesTab::new);
    let selected = selected.map(str::to_string);
    tab.update(cx, |tab, cx| {
        tab.set_commit_files(&files, cx);
        tab.set_selected_path(selected.as_deref(), cx);
        tab.set_diff("@@ -1 +1 @@\n-a\n+b\n", false, 0, cx);
        tab.set_diff_preferences(mode, false, cx);
    });
    let events: Events = Rc::default();
    cx.update(|cx| {
        let events = events.clone();
        cx.subscribe(&tab, move |_, event: &CommitChangesTabEvent, _| {
            events.borrow_mut().push(event.clone());
        })
        .detach();
    });
    (tab, events)
}

fn main_js() -> FileChange {
    change("src/main.js", "M", 3, 1)
}

#[gpui_kit::test]
async fn renders_files_and_emits_the_selection(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let (_, events) = mount_tab(
        &app,
        vec![main_js()],
        Some("src/main.js"),
        DiffViewMode::Unified,
        cx,
    );

    app.click(cx, file_row_id("src/main.js"));

    assert_eq!(
        *events.borrow(),
        [CommitChangesTabEvent::SelectFile(main_js())]
    );
    // Unified | Split se eligen en la pestaña del archivo.
    assert!(!app.exists(cx, "toggle-diff-view"));
}

// El backend ya calcula estas cifras por archivo; antes se tiraban.
#[gpui_kit::test]
async fn shows_how_much_each_file_changed(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let (tab, _) = mount_tab(
        &app,
        vec![main_js()],
        Some("src/main.js"),
        DiffViewMode::Unified,
        cx,
    );

    let file = tab.read_with(cx, |tab, _| tab.rows()[0].file.clone());
    let stats = file_stats(file.additions as usize, file.deletions as usize).expect("changes");
    assert_eq!(
        stats.labels(),
        (Some("+3".to_string()), Some("\u{2212}1".to_string()))
    );
}

// Las barritas de proporción acompañan a las cifras de cada archivo.
#[gpui_kit::test]
async fn draws_the_change_bars_next_to_each_files_numbers(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    mount_tab(
        &app,
        vec![main_js()],
        Some("src/main.js"),
        DiffViewMode::Unified,
        cx,
    );

    // +3 −1: cuatro líneas, un bloque por línea y uno gris.
    let bars = app.in_window(cx, |window, _| {
        window.find("change-bars").label().map(str::to_string)
    });
    assert_eq!(bars.as_deref(), Some("+++\u{2212}\u{b7}"));

    // Y caben en la fila: en una lista estrecha lo que cede es la ruta.
    let (row, bars) = app.in_window(cx, |window, _| {
        (
            window.find(file_row_id("src/main.js")).bounds(),
            window.find("change-bars").bounds(),
        )
    });
    assert!(bars.right() <= row.right(), "{bars:?} se sale de {row:?}");
}

// En el inspector la cabecera "Changed Files" sobra: las cifras ya lo dicen.
#[gpui_kit::test]
async fn can_drop_the_list_header(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let (tab, _) = mount_tab(&app, vec![main_js()], None, DiffViewMode::Unified, cx);
    tab.update(cx, |tab, cx| tab.set_external_files(true, cx));
    assert!(app.exists(cx, "file-count"));

    tab.update(cx, |tab, cx| tab.set_show_header(false, cx));

    assert!(!app.exists(cx, "file-count"));
    assert!(app.exists(cx, file_row_id("src/main.js")));
}

#[gpui_kit::test]
async fn shows_the_diff_in_the_mode_that_was_chosen(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let (tab, _) = mount_tab(
        &app,
        vec![main_js()],
        Some("src/main.js"),
        DiffViewMode::Split,
        cx,
    );
    app.in_window(cx, |_, _| {});

    let viewer = tab.read_with(cx, |tab, _| tab.viewer().clone());
    assert_eq!(
        viewer.read_with(cx, |viewer, _| viewer.view_mode()),
        DiffViewMode::Split
    );
    assert_eq!(
        tab.read_with(cx, |tab, _| tab.diff_view_mode()),
        DiffViewMode::Split
    );
}

#[gpui_kit::test]
async fn says_nothing_about_a_file_with_no_counted_changes(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let logo = change("assets/logo.png", "M", 0, 0);
    let (tab, _) = mount_tab(&app, vec![logo], None, DiffViewMode::Unified, cx);

    assert!(file_stats(0, 0).is_none());
    assert_eq!(
        tab.read_with(cx, |tab, _| tab.status_text()).as_deref(),
        Some("Select a file to view changes")
    );
}

#[gpui_kit::test]
async fn asks_for_a_new_tab_on_double_click_and_offers_a_menu_on_right_click(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let (tab, events) = mount_tab(
        &app,
        vec![main_js()],
        Some("src/main.js"),
        DiffViewMode::Unified,
        cx,
    );
    let menus: Rc<RefCell<Vec<String>>> = Rc::default();
    tab.update(cx, |tab, _| {
        let menus = menus.clone();
        tab.set_file_context_menu(move |file, _, _| {
            menus.borrow_mut().push(file.path.clone());
            vec![MenuItem::new("Open here")]
        });
    });

    app.in_window(cx, |window, cx| {
        window.double_click(file_row_id("src/main.js"), cx)
    });
    assert!(
        events
            .borrow()
            .contains(&CommitChangesTabEvent::OpenFile(main_js()))
    );

    app.in_window(cx, |window, cx| {
        window.right_click(file_row_id("src/main.js"), cx)
    });
    assert_eq!(*menus.borrow(), ["src/main.js"]);
    assert!(app.menu_is_open(cx));
}

#[gpui_kit::test]
async fn lists_files_flat_with_the_folder_in_grey_next_to_the_name(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let (tab, _) = mount_tab(
        &app,
        vec![
            change("src/main.js", "M", 0, 0),
            change("README.md", "A", 0, 0),
        ],
        None,
        DiffViewMode::Unified,
        cx,
    );

    let rows: Vec<(String, String)> = tab.read_with(cx, |tab, _| {
        tab.rows()
            .iter()
            .map(|row| (row.name.clone(), row.dir.clone()))
            .collect()
    });
    assert_eq!(
        rows,
        [
            ("README.md".to_string(), String::new()),
            ("main.js".to_string(), "src".to_string())
        ]
    );
    assert!(app.exists(cx, file_row_id("src/main.js")));
    assert!(app.exists(cx, file_row_id("README.md")));
    assert!(!app.exists(cx, gpui_kit::SharedString::from("folder:src")));
}

// Antes la lista armaba todas sus filas en cada render: con un commit de miles
// de archivos, desplazar, pasar el ratón o elegir uno costaba cientos de ms.
#[gpui_kit::test]
async fn builds_only_the_visible_rows_of_a_huge_commit(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let files: Vec<FileChange> = (0..5_000)
        .map(|ix| change(&format!("src/m{:02}/f{ix:04}.txt", ix / 100), "A", 1, 0))
        .collect();
    let first = files[0].path.clone();
    let last = files[4_999].path.clone();
    let (tab, events) = mount_tab(&app, files, None, DiffViewMode::Unified, cx);

    assert_eq!(tab.read_with(cx, |tab, _| tab.rows().len()), 5_000);
    let (shows_first, shows_last) = app.in_window(cx, |window, _| {
        (
            window.try_find(file_row_id(&first)).is_some(),
            window.try_find(file_row_id(&last)).is_some(),
        )
    });
    assert!(shows_first, "la primera fila está a la vista");
    assert!(!shows_last, "la última no se construye hasta que se vea");

    // Las filas virtuales siguen respondiendo al clic.
    app.click(cx, file_row_id(&first));
    assert!(matches!(
        events.borrow().as_slice(),
        [CommitChangesTabEvent::SelectFile(file)] if file.path == first
    ));
}

/// El mismo archivo de 600 líneas con un cambio cada 40: entero o por
/// bloques con 3 líneas de contexto.
fn whole_and_hunks() -> (String, String) {
    let changed = |line: u32| line.is_multiple_of(40) && line < 600;
    let mut whole = String::from("@@ -1,600 +1,600 @@\n");
    for line in 1..=600 {
        if changed(line) {
            whole.push_str(&format!("-old {line}\n+new {line}\n"));
        } else {
            whole.push_str(&format!(" line {line}\n"));
        }
    }
    let mut hunks = String::new();
    for change in (1..=600).filter(|line| changed(*line)) {
        hunks.push_str(&format!("@@ -{0},7 +{0},7 @@\n", change - 3));
        for line in change - 3..=change + 3 {
            if line == change {
                hunks.push_str(&format!("-old {line}\n+new {line}\n"));
            } else {
                hunks.push_str(&format!(" line {line}\n"));
            }
        }
    }
    (whole, hunks)
}

// "Whole file" en el detalle de un commit (y en Compare): el diff que llega
// después vuelve a la línea que se leía.
#[gpui_kit::test]
async fn whole_file_keeps_the_line_that_was_being_read(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let (whole, hunks) = whole_and_hunks();
    let (tab, events) = mount_tab(
        &app,
        vec![change("long.txt", "M", 14, 14)],
        Some("long.txt"),
        DiffViewMode::Unified,
        cx,
    );
    tab.update(cx, |tab, cx| tab.set_diff(hunks.clone(), false, 0, cx));
    app.in_window(cx, |_, _| {});
    let viewer = tab.read_with(cx, |tab, _| tab.viewer().clone());
    viewer.update(cx, |viewer, cx| {
        viewer.set_scroll_position(mergeon_view_diff::ScrollPosition::new(1_210., 0.), cx)
    });
    app.in_window(cx, |_, _| {});
    let anchor = viewer
        .update(cx, |viewer, _| viewer.line_anchor())
        .expect("una línea arriba");

    app.click(cx, "toggle-whole-file");
    assert_eq!(
        events.borrow().last(),
        Some(&CommitChangesTabEvent::ToggleWholeFile)
    );
    // Quien aloja la lista vuelve a pedir el diff: llega mientras se carga.
    tab.update(cx, |tab, cx| {
        tab.set_loading_diff(true, cx);
        tab.set_whole_file(true, cx);
    });
    app.in_window(cx, |_, _| {});
    tab.update(cx, |tab, cx| {
        tab.set_diff(whole.clone(), false, 0, cx);
        tab.set_loading_diff(false, cx);
    });
    app.in_window(cx, |_, _| {});
    app.in_window(cx, |_, _| {});
    let again = viewer.update(cx, |viewer, _| viewer.line_anchor()).unwrap();
    assert_eq!(again.line, anchor.line);
    assert!(viewer.read_with(cx, |viewer, _| viewer.scroll_position().top) > 2_000.);

    // Otro archivo empieza arriba, sin heredar la línea.
    app.click(cx, "toggle-whole-file");
    tab.update(cx, |tab, cx| {
        tab.set_selected_path(Some("other.txt"), cx);
        tab.set_diff(hunks.clone(), false, 0, cx);
    });
    app.in_window(cx, |_, _| {});
    assert_eq!(
        viewer.read_with(cx, |viewer, _| viewer.scroll_position().top),
        0.
    );
}

// "Ignore spaces" conserva también la línea que se estaba leyendo al recargar
// el diff (igual que "Whole file").
#[gpui_kit::test]
async fn ignore_whitespace_keeps_the_line_that_was_being_read(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let (whole, hunks) = whole_and_hunks();
    let (tab, events) = mount_tab(
        &app,
        vec![change("long.txt", "M", 14, 14)],
        Some("long.txt"),
        DiffViewMode::Unified,
        cx,
    );
    tab.update(cx, |tab, cx| tab.set_diff(hunks.clone(), false, 0, cx));
    app.in_window(cx, |_, _| {});
    let viewer = tab.read_with(cx, |tab, _| tab.viewer().clone());
    viewer.update(cx, |viewer, cx| {
        viewer.set_scroll_position(mergeon_view_diff::ScrollPosition::new(1_210., 0.), cx)
    });
    app.in_window(cx, |_, _| {});
    let anchor = viewer
        .update(cx, |viewer, _| viewer.line_anchor())
        .expect("una línea arriba");

    app.click(cx, "toggle-ignore-whitespace");
    assert_eq!(
        events.borrow().last(),
        Some(&CommitChangesTabEvent::ToggleIgnoreWhitespace)
    );
    tab.update(cx, |tab, cx| {
        tab.set_loading_diff(true, cx);
        tab.set_diff_preferences(DiffViewMode::Unified, true, cx);
    });
    app.in_window(cx, |_, _| {});
    tab.update(cx, |tab, cx| {
        tab.set_diff(whole.clone(), false, 0, cx);
        tab.set_loading_diff(false, cx);
    });
    app.in_window(cx, |_, _| {});
    app.in_window(cx, |_, _| {});
    let again = viewer.update(cx, |viewer, _| viewer.line_anchor()).unwrap();
    assert_eq!(again.line, anchor.line);
}
