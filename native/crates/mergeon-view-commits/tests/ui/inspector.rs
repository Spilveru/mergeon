//! `CommitSummaryHeader.test.js` y `CommitInspector.test.js`, con la cabecera,
//! las cifras y el segmentado de Señal.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AppContext as _, Entity, IntoElement as _, TestAppContext};
use mergeon_backend::FileChange;
use mergeon_model::commit_view_utils::{CommitRef, CommitRow, RefType};
use mergeon_model::format::format_date_time;
use mergeon_ui::menu::MenuItem;
use mergeon_view_commits::{
    BODY_CLAMP_LINES, BODY_LINE_HEIGHT, BODY_MAX_FRACTION, CommitChangesTab, CommitCommand,
    CommitInspector, CommitSummaryHeader,
};

use crate::support::{Scene, TestApp};

fn summary_commit() -> CommitRow {
    let reference = |name: &str| CommitRef {
        name: name.to_string(),
        kind: RefType::Local,
        is_head: false,
        color: String::new(),
    };
    CommitRow {
        hash: "b46d17d".to_string(),
        full_hash: "b46d17dbf12a4565b5971f798f787e88c872502d".to_string(),
        author: "Esteban Tapia".to_string(),
        email: "tapiam.esteban@gmail.com".to_string(),
        full_date: "2026-07-26T22:09:44-04:00".to_string(),
        parents: "27959da1111 aaaa2222".to_string(),
        refs: vec![reference("main"), reference("origin/main")],
        message: "Deja de comprobar actualizaciones contra un feed que no existe".to_string(),
        message_body: "Las releases están en un repositorio privado.".to_string(),
        ..Default::default()
    }
}

#[derive(Default)]
struct Log {
    copied: RefCell<Vec<String>>,
    parents: RefCell<Vec<String>>,
    commands: RefCell<Vec<CommitCommand>>,
    menus: Cell<usize>,
    trees: Cell<usize>,
}

/// La cabecera, con el cuerpo desplegable y lo que emite anotado.
fn mount_header(
    app: &TestApp,
    commit: CommitRow,
    checkout_disabled: bool,
    cx: &mut TestAppContext,
) -> (Entity<Scene>, Rc<Log>) {
    let log = Rc::new(Log::default());
    let expanded = Rc::new(Cell::new(false));
    let scene_log = log.clone();
    let scene = app.mount(cx, move |_, _| {
        Scene::new(move |_, cx| {
            let (copied, parents, commands, menus) = (
                scene_log.clone(),
                scene_log.clone(),
                scene_log.clone(),
                scene_log.clone(),
            );
            let toggle = expanded.clone();
            let view = cx.entity();
            CommitSummaryHeader::new(&commit)
                .expanded(expanded.get())
                .checkout_disabled(checkout_disabled)
                .on_toggle(move |_, _, cx| {
                    toggle.set(!toggle.get());
                    view.update(cx, |_, cx| cx.notify());
                })
                .on_copy_sha(move |hash, _, _| copied.copied.borrow_mut().push(hash.to_string()))
                .on_select_parent(move |hash, _, _| {
                    parents.parents.borrow_mut().push(hash.to_string())
                })
                .on_command(move |command, _, _| commands.commands.borrow_mut().push(command))
                .menu(move |_, _| {
                    menus.menus.set(menus.menus.get() + 1);
                    vec![MenuItem::new("Create Branch"), MenuItem::new("Revert")]
                })
                .into_any_element()
        })
    });
    (scene, log)
}

// El cuerpo se lee de un vistazo bajo el título; solo uno largo se recorta.
#[gpui_kit::test]
async fn shows_a_short_body_whole_under_the_title(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let commit = summary_commit();
    mount_header(&app, commit.clone(), false, cx);

    assert_eq!(app.label(cx, "summary-title"), Some(commit.message.clone()));
    assert_eq!(app.label(cx, "summary-body"), Some(commit.message_body));
    assert!(!app.exists(cx, "summary-toggle"));
}

#[gpui_kit::test]
async fn clamps_a_long_body_and_opens_it_on_demand(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let commit = CommitRow {
        message_body: "uno\ndos\ntres\ncuatro\ncinco".to_string(),
        ..summary_commit()
    };
    mount_header(&app, commit, false, cx);
    assert_eq!(
        app.label(cx, "summary-toggle").as_deref(),
        Some("Show more")
    );

    app.click(cx, "summary-toggle");

    assert_eq!(
        app.label(cx, "summary-toggle").as_deref(),
        Some("Show less")
    );
}

// Antes se pintaba el ISO tal cual: 2026-07-26T22:09:44-04:00.
#[gpui_kit::test]
async fn shows_a_date_a_person_can_read_keeping_the_exact_one_at_hand(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let commit = summary_commit();
    mount_header(&app, commit.clone(), false, cx);

    let shown = format_date_time(commit.full_date.as_str());
    assert_ne!(shown, commit.full_date);
    assert!(shown.contains("2026"));
    // La fecha exacta queda en el tooltip del elemento.
    assert_eq!(app.label(cx, "summary-date"), Some(commit.full_date));
}

#[gpui_kit::test]
async fn names_the_author_with_their_email(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let commit = summary_commit();
    mount_header(&app, commit.clone(), false, cx);

    assert_eq!(app.label(cx, "summary-author"), Some(commit.author));
    assert_eq!(app.label(cx, "summary-email"), Some(commit.email));
}

// Es el dato que más se copia de un commit: la cápsula y su botón copian.
#[gpui_kit::test]
async fn offers_the_sha_short_and_copyable(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let commit = summary_commit();
    let (_, log) = mount_header(&app, commit.clone(), false, cx);

    assert_eq!(app.label(cx, "summary-sha").as_deref(), Some("b46d17db"));
    app.click(cx, "summary-sha");
    app.click(cx, "summary-copy");

    assert_eq!(
        *log.copied.borrow(),
        [commit.full_hash.clone(), commit.full_hash]
    );
}

#[gpui_kit::test]
async fn lets_you_walk_back_to_any_parent(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let (_, log) = mount_header(&app, summary_commit(), false, cx);

    app.click(cx, gpui_kit::SharedString::from("parent:aaaa2222"));

    assert_eq!(*log.parents.borrow(), ["aaaa2222"]);
}

// Solo Checkout a la vista; Cherry-pick y el resto, en ⋯ (el mismo menú que
// la fila): con dos botones y las refs la cabecera se pisaba.
#[gpui_kit::test]
async fn runs_checkout_and_opens_the_commit_menu(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let (_, log) = mount_header(&app, summary_commit(), false, cx);

    assert!(!app.exists(cx, "summary-cherry-pick"));
    app.click(cx, "summary-checkout");
    assert_eq!(*log.commands.borrow(), [CommitCommand::Checkout]);

    app.click(cx, "summary-more");
    assert!(app.menu_is_open(cx));
    assert_eq!(log.menus.get(), 1);
}

#[gpui_kit::test]
async fn does_not_offer_to_check_out_the_branch_you_are_on(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let (_, log) = mount_header(&app, summary_commit(), true, cx);

    app.click(cx, "summary-checkout");

    assert!(log.commands.borrow().is_empty());
}

#[gpui_kit::test]
async fn shows_where_the_commit_sits(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    mount_header(&app, summary_commit(), false, cx);

    assert_eq!(
        app.label(cx, ("summary-ref", 0usize)).as_deref(),
        Some("main")
    );
    assert_eq!(
        app.label(cx, ("summary-ref", 1usize)).as_deref(),
        Some("origin/main")
    );
}

#[gpui_kit::test]
async fn holds_up_with_a_commit_that_has_no_body_email_or_refs(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let commit = CommitRow {
        message_body: String::new(),
        email: String::new(),
        refs: Vec::new(),
        parents: String::new(),
        ..summary_commit()
    };
    mount_header(&app, commit.clone(), false, cx);

    assert_eq!(app.label(cx, "summary-title"), Some(commit.message));
    assert!(!app.exists(cx, "summary-body"));
    assert!(!app.exists(cx, "summary-email"));
    assert!(!app.exists(cx, gpui_kit::SharedString::from("parent:27959da1111")));
    assert!(!app.exists(cx, ("summary-ref", 0usize)));
}

// --- CommitInspector -----------------------------------------------------------

fn inspector_commit() -> CommitRow {
    CommitRow {
        hash: "abc1234".to_string(),
        full_hash: "abc1234567890".to_string(),
        author: "Test User".to_string(),
        email: "test@example.com".to_string(),
        full_date: "2026-03-08T14:30:00-04:00".to_string(),
        parents: "fedcba0987654321".to_string(),
        refs: vec![CommitRef {
            name: "main".to_string(),
            ..Default::default()
        }],
        message: "Selected commit".to_string(),
        message_body: "Body".to_string(),
        ..Default::default()
    }
}

fn inspector_files() -> Vec<FileChange> {
    let change = |path: &str, additions, deletions| FileChange {
        path: path.to_string(),
        status: "M".to_string(),
        additions,
        deletions,
    };
    vec![change("src/a.rs", 10, 4), change("src/b.rs", 3, 0)]
}

fn mount_inspector(app: &TestApp, cx: &mut TestAppContext) -> Rc<Log> {
    mount_inspector_with(app, inspector_commit(), inspector_files(), cx)
}

fn mount_inspector_with(
    app: &TestApp,
    commit: CommitRow,
    files: Vec<FileChange>,
    cx: &mut TestAppContext,
) -> Rc<Log> {
    let log = Rc::new(Log::default());
    let scene_log = log.clone();
    let expanded = Rc::new(Cell::new(false));
    app.mount(cx, move |window, cx| {
        let changes = cx.new(|cx| {
            let mut tab = CommitChangesTab::new(window, cx);
            tab.set_commit_files(&files, cx);
            tab
        });
        Scene::new(move |_, cx| {
            let (copied, parents, trees) =
                (scene_log.clone(), scene_log.clone(), scene_log.clone());
            let toggle = expanded.clone();
            let view = cx.entity();
            CommitInspector::new(&commit, &changes)
                .expanded(expanded.get())
                .on_toggle_details(move |_, _, cx| {
                    toggle.set(!toggle.get());
                    view.update(cx, |_, cx| cx.notify());
                })
                .on_open_tree(move |_, _, _| trees.trees.set(trees.trees.get() + 1))
                .on_copy_sha(move |hash, _, _| copied.copied.borrow_mut().push(hash.to_string()))
                .on_select_parent(move |hash, _, _| {
                    parents.parents.borrow_mut().push(hash.to_string())
                })
                .into_any_element()
        })
    });
    log
}

// El porqué y el qué a la vez: antes eran dos pestañas y había que alternar.
#[gpui_kit::test]
async fn shows_the_commit_message_next_to_its_changes(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    mount_inspector(&app, cx);

    assert_eq!(
        app.label(cx, "summary-title").as_deref(),
        Some("Selected commit")
    );
    assert!(app.exists(cx, "changes-tab"));
}

// Cuánto cambió el commit, de un vistazo: archivos, líneas añadidas y quitadas.
#[gpui_kit::test]
async fn sums_up_files_and_lines_in_big_numbers(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    mount_inspector(&app, cx);

    assert_eq!(app.label(cx, "stat-files").as_deref(), Some("2 files"));
    assert_eq!(
        app.label(cx, "stat-additions").as_deref(),
        Some("+13 lines")
    );
    assert_eq!(
        app.label(cx, "stat-deletions").as_deref(),
        Some("\u{2212}4 lines")
    );
}

// Unified | Split ya no está aquí: el diff se abre en su pestaña, que tiene
// su propio Split.
#[gpui_kit::test]
async fn has_no_diff_mode_toggle_next_to_the_numbers(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    mount_inspector(&app, cx);

    assert!(app.exists(cx, "open-repo-tree"));
    assert!(!app.exists(cx, "tab-unified"));
    assert!(!app.exists(cx, "tab-split"));
}

// El árbol ya no vive dentro del inspector: se abre como pestaña propia.
#[gpui_kit::test]
async fn offers_to_browse_the_repository_tree_in_its_own_tab(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let log = mount_inspector(&app, cx);

    app.click(cx, "open-repo-tree");

    assert_eq!(log.trees.get(), 1);
}

#[gpui_kit::test]
async fn passes_the_copy_and_parent_actions_up(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let log = mount_inspector(&app, cx);

    app.click(cx, "summary-sha");
    assert_eq!(*log.copied.borrow(), ["abc1234567890"]);

    app.in_window(cx, |window, cx| {
        window.click(gpui_kit::SharedString::from("parent:fedcba0987654321"), cx)
    });
    assert_eq!(*log.parents.borrow(), ["fedcba0987654321"]);
}

// --- Mensajes largos -------------------------------------------------------------

/// Alto de una fila de "Changed Files" y de la cabecera de la lista.
const FILE_ROW: f32 = 26.;
const FILES_HEADER: f32 = 34.;
/// Medio píxel de margen al comparar altos.
const SLACK: f32 = 0.5;

/// Un commit como los de este repo: asunto, ~20 líneas con viñetas y el
/// trailer de coautor.
fn long_message_commit() -> CommitRow {
    let mut body = String::from("Pestañas: arreglos, desborde, atajos y cierre seguro.\n\n");
    for index in 1..=20 {
        body.push_str(&format!(
            "- Punto {index}: un cambio con su explicación breve\n"
        ));
    }
    body.push_str("\nCo-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>\n");
    CommitRow {
        message_body: body,
        ..inspector_commit()
    }
}

fn many_files(count: usize) -> Vec<FileChange> {
    (0..count)
        .map(|index| FileChange {
            path: format!("src/file_{index:02}.rs"),
            status: "M".to_string(),
            additions: 1,
            deletions: 1,
        })
        .collect()
}

fn height(bounds: gpui_kit::Bounds<gpui_kit::Pixels>) -> f32 {
    f32::from(bounds.size.height)
}

/// Filas de archivos que caben en la zona de la lista.
fn file_rows(app: &TestApp, cx: &mut TestAppContext) -> f32 {
    (height(app.bounds(cx, "commit-files")) - FILES_HEADER) / FILE_ROW
}

fn clamped_height() -> f32 {
    BODY_LINE_HEIGHT * BODY_CLAMP_LINES as f32
}

// Antes `line_clamp` no contaba los saltos de línea: un cuerpo de 20 líneas se
// veía casi entero sobre "Show more" y la lista quedaba con cuatro o cinco
// filas.
#[gpui_kit::test]
async fn keeps_a_long_message_to_three_lines_so_the_files_get_the_room(cx: &mut TestAppContext) {
    let app = TestApp::with_size(1440., 900., cx);
    mount_inspector_with(&app, long_message_commit(), many_files(40), cx);

    let body = height(app.bounds(cx, "summary-body"));
    assert!(
        body <= clamped_height() + SLACK,
        "the collapsed body is {body}px tall, more than three lines"
    );
    assert_eq!(
        app.label(cx, "summary-toggle").as_deref(),
        Some("Show more")
    );
    // El trailer no se ve plegado.
    assert!(!app.exists(cx, "summary-trailers"));
    let rows = file_rows(&app, cx);
    assert!(rows >= 18., "the file list only has room for {rows} rows");
}

// Desplegado crece hasta el 40 % del inspector y el resto se desplaza dentro;
// "Show less" sigue a mano y vuelve a recortarlo.
#[gpui_kit::test]
async fn grows_an_opened_message_up_to_its_share_and_scrolls_the_rest(cx: &mut TestAppContext) {
    let app = TestApp::with_size(1440., 900., cx);
    mount_inspector_with(&app, long_message_commit(), many_files(40), cx);

    app.click(cx, "summary-toggle");

    let inspector = height(app.bounds(cx, "commit-details"));
    let body = height(app.bounds(cx, "summary-body"));
    let limit = inspector * BODY_MAX_FRACTION;
    assert!(
        body <= limit + SLACK,
        "the opened body is {body}px tall, over its {limit}px"
    );
    // 22 líneas de 19px no caben en el 40 % de 900: llega al tope.
    assert!(
        body >= limit - 1.,
        "the opened body is {body}px tall, short of its {limit}px"
    );
    assert_eq!(
        app.label(cx, "summary-trailers").as_deref(),
        Some("Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>")
    );
    assert_eq!(
        app.label(cx, "summary-toggle").as_deref(),
        Some("Show less")
    );
    assert!(file_rows(&app, cx) >= 5.);

    app.click(cx, "summary-toggle");

    assert!(height(app.bounds(cx, "summary-body")) <= clamped_height() + SLACK);
    assert!(!app.exists(cx, "summary-trailers"));
}

// Con la ventana baja el que cede es el cuerpo: la cabecera, las cifras y
// cinco filas de archivos se quedan enteras.
#[gpui_kit::test]
async fn keeps_the_header_the_stats_and_five_file_rows_in_a_low_window(cx: &mut TestAppContext) {
    let app = TestApp::with_size(1440., 480., cx);
    mount_inspector_with(&app, long_message_commit(), many_files(40), cx);
    app.click(cx, "summary-toggle");

    assert!(file_rows(&app, cx) >= 5.);
    // Las cifras: 8 + 24 + 2 + 14 + 8 de alto, más el borde.
    assert!(height(app.bounds(cx, "commit-stats")) >= 56.);
    assert!(height(app.bounds(cx, "summary-sha")) >= 22. - SLACK);
    assert!(height(app.bounds(cx, "summary-title")) >= 22. - SLACK);
    // La línea del autor queda dentro de la cabecera, no recortada.
    let summary = app.bounds(cx, "commit-summary");
    let author = app.bounds(cx, "summary-author");
    assert!(author.bottom() <= summary.bottom());
    // Y el cuerpo, aunque cedió, conserva al menos una línea.
    assert!(height(app.bounds(cx, "summary-body")) >= BODY_LINE_HEIGHT - SLACK);
}

// Un solo párrafo sin saltos también se recorta si al pintarlo ocupa más de
// tres líneas.
#[gpui_kit::test]
async fn counts_the_lines_a_paragraph_takes_once_wrapped(cx: &mut TestAppContext) {
    let app = TestApp::with_size(700., 800., cx);
    let commit = CommitRow {
        message_body: "palabra ".repeat(60).trim_end().to_string(),
        ..summary_commit()
    };
    mount_header(&app, commit, false, cx);

    assert_eq!(
        app.label(cx, "summary-toggle").as_deref(),
        Some("Show more")
    );
    assert!(height(app.bounds(cx, "summary-body")) <= clamped_height() + SLACK);
}

// Tres líneas y un trailer caben: no hay botón y el trailer se ve aparte.
#[gpui_kit::test]
async fn does_not_count_the_trailers_against_the_three_lines(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let commit = CommitRow {
        message_body: "uno\ndos\ntres\n\nCo-Authored-By: Ana <ana@example.com>\nSigned-off-by: Luis <luis@example.com>\n".to_string(),
        ..summary_commit()
    };
    mount_header(&app, commit, false, cx);

    assert!(!app.exists(cx, "summary-toggle"));
    assert_eq!(
        app.label(cx, "summary-body").as_deref(),
        Some("uno\ndos\ntres")
    );
    assert_eq!(
        app.label(cx, "summary-trailers").as_deref(),
        Some("Co-Authored-By: Ana <ana@example.com>\nSigned-off-by: Luis <luis@example.com>")
    );
}
