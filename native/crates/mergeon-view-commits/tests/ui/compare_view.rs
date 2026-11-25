//! La comparación navegable de dos ramas (`CompareView`): modos, revisiones
//! fijadas, ramas sin historia común, tope de archivos y el diff del archivo
//! elegido, con repositorios git reales.

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::{Entity, TestAppContext};
use mergeon_backend::{CompareLimits, CompareMode};
use mergeon_view_commits::{CompareView, CompareViewEvent};
use mergeon_view_sidebar::ComparisonRequest;

use crate::support::{TestApp, TestRepo};

/// main y feature se separan tras `base`: feature añade `feature.txt` y
/// renombra `notes.txt`; main añade `main.txt`.
fn diverged() -> TestRepo {
    let repo = TestRepo::empty();
    repo.commit_file("notes.txt", "one\ntwo\nthree\nfour\nfive\n", "base");
    repo.git(&["checkout", "-q", "-b", "feature"]);
    repo.commit_file("feature.txt", "feature\n", "Add feature");
    repo.git(&["mv", "notes.txt", "docs notes.txt"]);
    repo.git(&["commit", "-q", "-m", "Move the notes"]);
    repo.git(&["checkout", "-q", "main"]);
    repo.commit_file("main.txt", "main\n", "Main work");
    repo
}

async fn open(
    app: &TestApp,
    repo: &TestRepo,
    base: &str,
    head: &str,
    cx: &mut TestAppContext,
) -> Entity<CompareView> {
    let repo_id = app.open(repo, cx).await;
    let request = ComparisonRequest::new(repo_id, repo.path_string(), base, head);
    let view = app.mount(cx, move |window, cx| CompareView::new(&request, window, cx));
    wait_loaded(app, &view, cx);
    view
}

fn wait_loaded(app: &TestApp, view: &Entity<CompareView>, cx: &mut TestAppContext) {
    app.wait_for(cx, view, "the comparison", |view, cx| {
        !view.is_loading()
            && (view.comparison().is_some() || view.error().is_some())
            && !view
                .changes_tab()
                .read(cx)
                .status_text()
                .is_some_and(|text| text.contains("Loading"))
    });
}

fn files(view: &Entity<CompareView>, cx: &mut TestAppContext) -> Vec<(String, String)> {
    view.read_with(cx, |view, _| {
        view.comparison()
            .map(|comparison| {
                comparison
                    .files
                    .files
                    .iter()
                    .map(|file| (file.status.clone(), file.path.clone()))
                    .collect()
            })
            .unwrap_or_default()
    })
}

fn pair(status: &str, path: &str) -> (String, String) {
    (status.to_string(), path.to_string())
}

fn shown_diff(view: &Entity<CompareView>, cx: &mut TestAppContext) -> String {
    view.read_with(cx, |view, cx| {
        view.changes_tab()
            .read(cx)
            .viewer()
            .read(cx)
            .diff()
            .to_string()
    })
}

#[gpui_kit::test]
async fn shows_the_changes_since_divergence_and_switches_to_the_direct_diff(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let repo = diverged();
    let view = open(&app, &repo, "main", "feature", cx).await;

    view.read_with(cx, |view, _| {
        assert_eq!(view.mode(), CompareMode::SinceDivergence);
        let comparison = view.comparison().unwrap();
        assert_eq!((comparison.ahead, comparison.behind), (2, 1));
    });
    assert_eq!(
        files(&view, cx),
        [pair("R", "docs notes.txt"), pair("A", "feature.txt")]
    );
    assert!(app.exists(cx, "compare-commit:".to_string() + &repo.rev("feature")));

    // El primero (por ruta) queda elegido y su diff a la vista: un renombre.
    let selected = view
        .read_with(cx, |view, _| view.selected_file().cloned())
        .unwrap();
    assert_eq!(selected.path, "docs notes.txt");
    assert_eq!(selected.old_path.as_deref(), Some("notes.txt"));
    assert!(shown_diff(&view, cx).contains("rename from notes.txt"));

    view.update(cx, |view, cx| view.select_file("feature.txt", cx));
    wait_loaded(&app, &view, cx);
    assert!(shown_diff(&view, cx).contains("+feature"));

    // Directo: main.txt aparece como borrado; el archivo elegido se conserva.
    app.click(cx, "tab-direct");
    wait_loaded(&app, &view, cx);
    assert_eq!(
        view.read_with(cx, |view, _| view.mode()),
        CompareMode::Direct
    );
    assert_eq!(
        files(&view, cx),
        [
            pair("R", "docs notes.txt"),
            pair("A", "feature.txt"),
            pair("D", "main.txt")
        ]
    );
    assert_eq!(
        view.read_with(cx, |view, _| view.selected_file().map(|f| f.path.clone())),
        Some("feature.txt".to_string())
    );

    // Cambiar de modo no vuelve a resolver las ramas ni a leer los commits.
    let reads = view.read_with(cx, |view, _| view.reads());
    app.click(cx, "tab-since");
    wait_loaded(&app, &view, cx);
    assert_eq!(files(&view, cx).len(), 2);
    assert_eq!(view.read_with(cx, |view, _| view.reads()), reads + 2);
}

#[gpui_kit::test]
async fn stays_pinned_to_its_revisions_until_refresh(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = diverged();
    let view = open(&app, &repo, "main", "feature", cx).await;
    let pinned = view.read_with(cx, |view, _| {
        view.comparison().unwrap().revisions.head.clone()
    });

    repo.git(&["checkout", "-q", "feature"]);
    repo.commit_file("later.txt", "later\n", "Later");
    repo.git(&["checkout", "-q", "main"]);

    // Otro modo y vuelta: sigue sobre el SHA fijado.
    view.update(cx, |view, cx| view.set_mode(CompareMode::Direct, cx));
    wait_loaded(&app, &view, cx);
    view.update(cx, |view, cx| {
        view.set_mode(CompareMode::SinceDivergence, cx)
    });
    wait_loaded(&app, &view, cx);
    assert!(!files(&view, cx).contains(&pair("A", "later.txt")));

    app.click(cx, "compare-refresh");
    wait_loaded(&app, &view, cx);
    let refreshed = view.read_with(cx, |view, _| {
        view.comparison().unwrap().revisions.head.clone()
    });
    assert_ne!(refreshed, pinned);
    assert!(files(&view, cx).contains(&pair("A", "later.txt")));

    // Intercambiar A y B usa los mismos SHA, al revés.
    app.click(cx, "compare-swap");
    wait_loaded(&app, &view, cx);
    view.read_with(cx, |view, _| {
        assert_eq!((view.base_label(), view.head_label()), ("feature", "main"));
        assert_eq!(view.comparison().unwrap().revisions.base, refreshed);
    });
    assert_eq!(files(&view, cx), [pair("A", "main.txt")]);
}

#[gpui_kit::test]
async fn branches_without_common_history_offer_the_direct_diff(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = diverged();
    repo.git(&["checkout", "-q", "--orphan", "pages"]);
    repo.git(&["rm", "-rf", "-q", "."]);
    repo.commit_file("index.html", "<h1>hi</h1>\n", "Pages");
    repo.git(&["checkout", "-q", "main"]);

    let view = open(&app, &repo, "main", "pages", cx).await;
    assert!(app.exists(cx, "compare-no-common-history"));
    assert!(files(&view, cx).is_empty());

    app.click(cx, "compare-show-direct");
    wait_loaded(&app, &view, cx);
    assert!(!app.exists(cx, "compare-no-common-history"));
    assert_eq!(
        files(&view, cx),
        [
            pair("A", "index.html"),
            pair("D", "main.txt"),
            pair("D", "notes.txt")
        ]
    );
}

#[gpui_kit::test]
async fn an_unknown_branch_shows_the_error_in_place(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = diverged();
    let view = open(&app, &repo, "main", "gone", cx).await;

    assert!(app.exists(cx, "compare-error"));
    assert_eq!(
        view.read_with(cx, |view, _| view.error().map(str::to_string)),
        Some("Failed to compare revisions: Unknown branch or revision 'gone'.".to_string())
    );
}

#[gpui_kit::test]
async fn says_how_many_files_it_did_not_list(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = diverged();
    repo.git(&["checkout", "-q", "feature"]);
    for index in 0..6 {
        repo.write(&format!("many/{index}.txt"), "x\n");
    }
    repo.git(&["add", "--all"]);
    repo.git(&["commit", "-q", "-m", "Many"]);
    repo.git(&["checkout", "-q", "main"]);

    let view = open(&app, &repo, "main", "feature", cx).await;
    assert!(!app.exists(cx, "files-note"));
    view.update(cx, |view, cx| {
        view.set_limits(CompareLimits {
            commits: 1,
            files: 3,
        });
        view.refresh(cx);
    });
    wait_loaded(&app, &view, cx);

    assert_eq!(files(&view, cx).len(), 3);
    assert!(app.exists(cx, "files-note"));
    assert!(app.exists(cx, "compare-commits-more"));
    view.read_with(cx, |view, _| {
        let comparison = view.comparison().unwrap();
        assert_eq!(comparison.files.total, 8);
        assert_eq!(comparison.files.omitted(), 5);
        assert_eq!(comparison.ahead_omitted(), 2);
    });
}

#[gpui_kit::test]
async fn double_clicking_a_commit_asks_to_show_it(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = diverged();
    let view = open(&app, &repo, "main", "feature", cx).await;
    let events = Rc::new(RefCell::new(Vec::new()));
    let sink = events.clone();
    cx.update(|cx| {
        cx.subscribe(&view, move |_, event: &CompareViewEvent, _| {
            sink.borrow_mut().push(event.clone())
        })
        .detach();
    });

    let feature = repo.rev("feature");
    app.in_window(cx, |window, cx| {
        use gpui_kit::test::TestWindowExt as _;
        window.double_click(format!("compare-commit:{feature}"), cx);
    });
    assert_eq!(
        events.borrow().as_slice(),
        [CompareViewEvent::ShowCommit { full_hash: feature }]
    );
}
