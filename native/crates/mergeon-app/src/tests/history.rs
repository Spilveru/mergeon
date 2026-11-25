//! Tests de humo de la carga del historial (la parte de datos de
//! `CommitView.vue`). El grafo y la pertenencia se prueban en `mergeon-model`.

use gpui_kit::TestAppContext;
use mergeon_backend::CommitSearchOptions;
use mergeon_model::commit_view_utils::is_head_commit;
use mergeon_model::notify::NotifyType;

use super::support::{TestApp, TestRepo};
use crate::history::CommitHistoryRequest;

fn request() -> CommitHistoryRequest {
    CommitHistoryRequest {
        current_branch: Some("main".to_string()),
        ..Default::default()
    }
}

#[gpui_kit::test]
async fn loads_the_history_as_rows_with_graph_and_branch_membership(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.git(&["checkout", "-b", "feature"]);
    let feature = repo.commit_file("feature.txt", "feature\n", "Feature work");
    repo.git(&["checkout", "main"]);
    let head = repo.commit_file("main.txt", "main\n", "Main work");

    let history = app
        .model
        .update(cx, |model, cx| {
            model.load_commit_history(&repo.path_string(), request(), cx)
        })
        .await
        .unwrap();

    assert!(!history.has_more);
    assert!(!history.missing_requested_commit);
    assert_eq!(history.commits.len(), 3);
    assert!(history.commits.iter().all(|row| row.graph.is_some()));

    let row = |hash: &str| {
        history
            .commits
            .iter()
            .find(|row| row.full_hash == hash)
            .expect("commit row")
    };
    assert!(is_head_commit(row(&head)));
    assert_eq!(row(&head).message, "Main work");
    assert_eq!(row(&head).in_current_branch, Some(true));
    // El commit de la otra rama se pinta, pero no es de la rama actual.
    assert_eq!(row(&feature).in_current_branch, Some(false));
    assert!(
        row(&feature)
            .refs
            .iter()
            .any(|commit_ref| commit_ref.name == "feature")
    );
    assert!(!row(&head).date.is_empty());
    assert!(app.host.toasts().is_empty());
}

#[gpui_kit::test]
async fn loading_more_keeps_what_was_loaded_and_dedupes_overlaps(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "a\n", "Second");
    let path = repo.path_string();

    let first = app
        .model
        .update(cx, |model, cx| {
            model.load_commit_history(&path, request(), cx)
        })
        .await
        .unwrap();
    // La "página siguiente" pedida con una fila de menos trae un commit ya
    // conocido menos uno: el solape no duplica filas.
    let loaded = first.commits[..1].to_vec();
    let more = app
        .model
        .update(cx, |model, cx| {
            model.load_more_commit_history(&path, Some("main"), loaded, cx)
        })
        .await
        .unwrap();

    assert_eq!(more.commits.len(), 2);
    assert_eq!(more.commits, first.commits);
    assert!(!more.has_more);

    let unchanged = app
        .model
        .update(cx, |model, cx| {
            model.load_more_commit_history(&path, Some("main"), first.commits.clone(), cx)
        })
        .await
        .unwrap();
    assert_eq!(unchanged.commits, first.commits);
}

#[gpui_kit::test]
async fn warns_when_the_requested_commit_is_outside_the_loaded_history(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();

    let history = app
        .model
        .update(cx, |model, cx| {
            model.load_commit_history(
                &repo.path_string(),
                CommitHistoryRequest {
                    ensure_commit_visible: Some("0".repeat(40)),
                    ..request()
                },
                cx,
            )
        })
        .await
        .unwrap();

    assert!(history.missing_requested_commit);
    assert_eq!(history.commits.len(), 1);
    assert_eq!(
        app.host.messages(NotifyType::Warning),
        ["That commit is outside the loaded history"]
    );

    // Un commit que sí está no avisa.
    app.host.clear_toasts();
    let history = app
        .model
        .update(cx, |model, cx| {
            model.load_commit_history(
                &repo.path_string(),
                CommitHistoryRequest {
                    ensure_commit_visible: Some(repo.head()),
                    ..request()
                },
                cx,
            )
        })
        .await
        .unwrap();
    assert!(!history.missing_requested_commit);
    assert!(app.host.toasts().is_empty());
}

#[gpui_kit::test]
async fn searches_the_history_and_reports_load_failures(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "a\n", "Add the parser");
    repo.commit_file("b.txt", "b\n", "Fix the lexer");
    let path = repo.path_string();

    let found = app
        .model
        .update(cx, |model, cx| {
            model.search_commit_history(
                &path,
                CommitSearchOptions {
                    text: Some("parser".to_string()),
                    ..Default::default()
                },
                Some("main"),
                cx,
            )
        })
        .await
        .unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].message, "Add the parser");

    repo.break_repository();
    let failed = app
        .model
        .update(cx, |model, cx| {
            model.load_commit_history(&path, request(), cx)
        })
        .await;
    assert!(failed.is_err());
    assert_eq!(
        app.host.messages(NotifyType::Error),
        ["Error loading commits"]
    );
    // La búsqueda no avisa: quien la pide presenta el fallo.
    app.host.clear_toasts();
    let failed = app
        .model
        .update(cx, |model, cx| {
            model.search_commit_history(
                &path,
                CommitSearchOptions {
                    text: Some("parser".to_string()),
                    ..Default::default()
                },
                None,
                cx,
            )
        })
        .await;
    assert!(failed.is_err());
    assert!(app.host.toasts().is_empty());
}
