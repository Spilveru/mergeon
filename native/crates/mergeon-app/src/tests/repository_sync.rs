//! Port de `src/composables/useRepositorySync.test.js`.

use gpui_kit::TestAppContext;
use mergeon_backend::{FileStatus, StructuredStatus};
use mergeon_model::notify::NotifyType;
use mergeon_model::operation_types::OperationResult;

use super::support::{TestApp, TestRepo};
use crate::SyncRequest;

fn status_fixture(current: &str) -> StructuredStatus {
    StructuredStatus {
        current: Some(current.to_string()),
        is_clean: true,
        ..StructuredStatus::default()
    }
}

#[gpui_kit::test]
async fn applies_result_status_without_reading_status_again_and_updates_metadata(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    let repo_id = app.seed(&repo, cx);

    let status = StructuredStatus {
        tracking: Some("origin/main".to_string()),
        files: vec![FileStatus {
            path: "file.txt".to_string(),
            index: "M".to_string(),
            working_dir: " ".to_string(),
            from: None,
        }],
        is_clean: false,
        ..status_fixture("main")
    };

    let outcome = app
        .model
        .update(cx, |model, cx| {
            model.sync_repository_read_model(
                repo_id.as_str(),
                SyncRequest::new()
                    .status(true)
                    .changes_count(true)
                    .operation("commit")
                    .result(OperationResult {
                        operation: Some("commit".to_string()),
                        status: Some(status.clone()),
                        ..Default::default()
                    }),
                cx,
            )
        })
        .await
        .expect("sync outcome");

    assert_eq!(outcome.status.as_ref(), Some(&status));
    app.model.read_with(cx, |model, _| {
        assert_eq!(model.started_section_reads(&repo_id, "status"), 0);
        let repository = model.repository(repo_id.as_str()).unwrap();
        assert_eq!(repository.status.as_ref(), Some(&status));
        assert_eq!(repository.changes_count, 1);
        assert!(repository.sidebar_loaded.changes_count);
        assert_eq!(
            repository.last_mutation_operation.as_deref(),
            Some("commit")
        );
    });
    assert_eq!(app.recorded_mutations(), ["commit"]);
}

#[gpui_kit::test]
async fn refreshes_only_the_requested_sidebar_sections_and_reuses_branch_reads_for_remotes(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.publish_to_origin();
    repo.git(&["tag", "v1.0.0"]);
    let repo_id = app.seed(&repo, cx);

    let outcome = app
        .model
        .update(cx, |model, cx| {
            model.sync_repository_read_model(
                repo_id.as_str(),
                SyncRequest::new().branches(true).remotes(true),
                cx,
            )
        })
        .await
        .expect("sync outcome");

    assert!(outcome.branch_data.is_some());
    assert_eq!(outcome.remotes.as_ref().map(Vec::len), Some(1));
    assert_eq!(outcome.tags, None);
    app.model.read_with(cx, |model, _| {
        assert_eq!(model.started_section_reads(&repo_id, "branches"), 1);
        assert_eq!(model.started_section_reads(&repo_id, "remotes"), 1);
        assert_eq!(model.started_section_reads(&repo_id, "tags"), 0);

        let repository = model.repository(repo_id.as_str()).unwrap();
        assert!(repository.sidebar_loaded.branches);
        assert!(repository.sidebar_loaded.remotes);
        assert!(!repository.sidebar_loaded.tags);
        assert!(!repository.sidebar_loading.branches);
        assert!(!repository.sidebar_loading.remotes);

        let main = &repository.branches[0];
        assert_eq!(main.name, "main");
        assert!(main.active);
        assert_eq!(main.upstream.as_deref(), Some("origin/main"));
        assert_eq!(repository.remotes[0].name, "origin");
        assert_eq!(repository.remotes[0].branches, ["main"]);
        // Sin `operation` no se registra ninguna mutación.
        assert_eq!(repository.last_mutation_operation, None);
    });

    let ensured = app
        .model
        .update(cx, |model, cx| {
            model.ensure_repository_sections_loaded(
                repo_id.as_str(),
                SyncRequest::new().branches(true).remotes(true),
                cx,
            )
        })
        .await;

    assert_eq!(ensured, None);
    app.model.read_with(cx, |model, _| {
        assert_eq!(model.started_section_reads(&repo_id, "branches"), 1);
        assert_eq!(model.started_section_reads(&repo_id, "remotes"), 1);
    });
}

#[gpui_kit::test]
async fn ensures_only_the_sections_that_are_still_missing(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.git(&["tag", "v1.0.0"]);
    let repo_id = app.seed(&repo, cx);

    app.model
        .update(cx, |model, cx| {
            model.sync_repository_read_model(
                repo_id.as_str(),
                SyncRequest::new().branches(true),
                cx,
            )
        })
        .await;

    let ensured = app
        .model
        .update(cx, |model, cx| {
            model.ensure_repository_sections_loaded(
                repo_id.as_str(),
                SyncRequest::new().status(true).branches(true).tags(true),
                cx,
            )
        })
        .await
        .expect("missing sections were read");

    assert!(ensured.status.is_some());
    assert_eq!(ensured.branch_data, None);
    assert_eq!(ensured.tags, Some(vec!["v1.0.0".to_string()]));
    app.model.read_with(cx, |model, _| {
        assert_eq!(model.started_section_reads(&repo_id, "branches"), 1);
        let repository = model.repository(repo_id.as_str()).unwrap();
        assert_eq!(repository.tags, ["v1.0.0"]);
        assert_eq!(repository.current_branch.as_deref(), Some("main"));
    });
}

#[gpui_kit::test]
async fn deduplicates_concurrent_branch_reads_for_the_same_repository(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    let repo_id = app.seed(&repo, cx);

    let (first_sync, second_sync) = app.model.update(cx, |model, cx| {
        (
            model.sync_repository_read_model(
                repo_id.as_str(),
                SyncRequest::new().branches(true),
                cx,
            ),
            model.sync_repository_read_model(
                repo_id.as_str(),
                SyncRequest::new().branches(true),
                cx,
            ),
        )
    });

    // La lectura queda registrada al pedirla, antes de que nada corra.
    app.model.read_with(cx, |model, _| {
        assert_eq!(model.started_section_reads(&repo_id, "branches"), 1);
    });

    let first = first_sync.await.expect("first sync");
    let second = second_sync.await.expect("second sync");

    assert_eq!(first.branch_data, second.branch_data);
    app.model.read_with(cx, |model, _| {
        assert_eq!(model.started_section_reads(&repo_id, "branches"), 1);
        assert!(
            model
                .repository(repo_id.as_str())
                .unwrap()
                .sidebar_loaded
                .branches
        );
    });
}

#[gpui_kit::test]
async fn drops_a_branch_response_that_started_before_a_mutation_result(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    // La lectura de ramas dirá que la rama actual es esta; el resultado de la
    // mutación, que llega después, dice que es `main`.
    repo.git(&["checkout", "-b", "stale-branch"]);
    let repo_id = app.seed(&repo, cx);

    let stale_read = app.model.update(cx, |model, cx| {
        model.sync_repository_read_model(repo_id.as_str(), SyncRequest::new().branches(true), cx)
    });
    app.model.read_with(cx, |model, _| {
        assert_eq!(model.started_section_reads(&repo_id, "branches"), 1);
    });

    app.model
        .update(cx, |model, cx| {
            model.sync_repository_read_model(
                repo_id.as_str(),
                SyncRequest::new()
                    .operation("checkout")
                    .result(OperationResult {
                        operation: Some("checkout".to_string()),
                        status: Some(status_fixture("main")),
                        ..Default::default()
                    }),
                cx,
            )
        })
        .await
        .expect("mutation sync");

    assert_eq!(stale_read.await, None);

    app.model.read_with(cx, |model, _| {
        let repository = model.repository(repo_id.as_str()).unwrap();
        assert!(repository.branches.is_empty());
        assert!(!repository.sidebar_loaded.branches);
        assert!(!repository.sidebar_loading.branches);
        assert_eq!(repository.current_branch.as_deref(), Some("main"));
    });
}

#[gpui_kit::test]
async fn surfaces_an_error_and_keeps_previous_data_when_requested_sections_fail_to_load(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.publish_to_origin();
    repo.git(&["tag", "v1.0.0"]);
    repo.write("keep.txt", "keep\n");
    repo.git(&["stash", "push", "--include-untracked", "-m", "wip"]);
    repo.write("dirty.txt", "dirty\n");
    app.backend
        .storage()
        .add_starred_branch(&repo.id(), "main")
        .unwrap();
    let repo_id = app.seed(&repo, cx);

    let everything = || {
        SyncRequest::new()
            .status(true)
            .branches(true)
            .remotes(true)
            .tags(true)
            .stashes(true)
            .starred_branches(true)
    };
    app.model
        .update(cx, |model, cx| {
            model.sync_repository_read_model(repo_id.as_str(), everything(), cx)
        })
        .await
        .expect("first sync");
    let before = app
        .model
        .read_with(cx, |model, _| model.repository(repo_id.as_str()).cloned())
        .unwrap();
    assert_eq!(before.changes_count, 1);
    assert_eq!(before.tags, ["v1.0.0"]);
    assert_eq!(before.stashes.len(), 1);
    assert_eq!(before.starred_branches, ["main"]);
    assert!(app.host.toasts().is_empty());

    repo.break_repository();

    let outcome = app
        .model
        .update(cx, |model, cx| {
            model.sync_repository_read_model(repo_id.as_str(), everything(), cx)
        })
        .await
        .expect("a failed read still resolves");

    assert_eq!(outcome.status, None);
    assert_eq!(outcome.branch_data, None);
    assert_eq!(outcome.remotes, None);
    assert_eq!(outcome.tags, None);
    assert_eq!(outcome.stashes, None);
    // Las ramas destacadas salen de la configuración, no de git.
    assert_eq!(outcome.starred_branches, Some(vec!["main".to_string()]));

    let after = app
        .model
        .read_with(cx, |model, _| model.repository(repo_id.as_str()).cloned())
        .unwrap();
    assert_eq!(after.status, before.status);
    assert_eq!(after.branches, before.branches);
    assert_eq!(after.remotes, before.remotes);
    assert_eq!(after.tags, before.tags);
    assert_eq!(after.stashes, before.stashes);
    assert_eq!(after.starred_branches, before.starred_branches);
    assert_eq!(after.sidebar_loading, before.sidebar_loading);

    // Un aviso por sección fallida: las dos capas que en el original avisaban
    // del mismo error (useGit y useRepositorySync) cuentan como una.
    assert_eq!(
        app.host.messages(NotifyType::Error),
        [
            "Error loading status",
            "Error loading branches",
            "Error loading remotes",
            "Error loading tags",
            "Error loading stashes",
        ]
    );
    assert_eq!(app.host.toasts().len(), 5);
}

#[gpui_kit::test]
async fn ignores_repositories_that_are_not_open(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);

    let outcome = app
        .model
        .update(cx, |model, cx| {
            model.sync_repository_read_model("missing", SyncRequest::new().status(true), cx)
        })
        .await;

    assert_eq!(outcome, None);
    assert!(app.recorded_mutations().is_empty());
}
