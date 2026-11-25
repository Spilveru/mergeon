//! Flujo completo sobre un repositorio real con un remoto bare: abrir →
//! hidratar → stage → commit → crear rama → push → fetch sin cambios.

use gpui_kit::TestAppContext;
use mergeon_backend::CommitOptions;
use mergeon_model::notify::NotifyType;

use super::support::{TestApp, TestRepo, git_in};
use crate::flows::LocalChangesMode;
use crate::history::CommitHistoryRequest;
use crate::shell::CreateBranchSubmission;
use crate::{AppEvent, DialogRequest, FetchRequest, PushRequest, StructuredOutcome, SyncRequest};

#[gpui_kit::test]
async fn open_hydrate_stage_commit_branch_push_and_fetch(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    let origin = repo.publish_to_origin();
    // El primer fetch de un clon crea `refs/remotes/origin/HEAD`.
    repo.git(&["fetch", "origin"]);
    let path = repo.path_string();

    // --- abrir e hidratar ---------------------------------------------------
    app.model
        .update(cx, |model, cx| model.handle_open_repository(&path, cx))
        .await;
    cx.run_until_parked();
    let repo_id = repo.id();
    app.model.read_with(cx, |model, _| {
        let repository = model.active_repository().expect("active repository");
        assert_eq!(repository.id, repo_id);
        assert_eq!(repository.current_branch.as_deref(), Some("main"));
        assert_eq!(repository.changes_count, 0);
        assert_eq!(repository.branches.len(), 1);
        assert_eq!(repository.remotes[0].name, "origin");
        assert_eq!(model.default_remote_name(), "origin");
        assert_eq!(model.active_status_watch_path(), Some(repo_id.as_str()));
    });
    assert_eq!(app.recorded_mutations(), ["hydrate-opened-repository"]);

    // --- stage ---------------------------------------------------------------
    repo.write("notes.txt", "notes\n");
    let staged = app
        .model
        .update(cx, |model, cx| model.stage(&path, "notes.txt", cx))
        .await;
    assert!(staged.success(), "{}", staged.message());
    app.model
        .update(cx, |model, cx| {
            model.apply_repository_operation_result(repo_id.as_str(), &staged, cx);
            let request = SyncRequest::new()
                .status(staged.status().is_none())
                .changes_count(true)
                .operation("stage")
                .result(staged.to_operation_result());
            model.sync_repository_read_model(repo_id.as_str(), request, cx)
        })
        .await;
    app.model.read_with(cx, |model, _| {
        let repository = model.repository(repo_id.as_str()).unwrap();
        assert_eq!(repository.changes_count, 1);
        assert_eq!(repository.status.as_ref().unwrap().staged, ["notes.txt"]);
        assert!(model.repositories().active_repository_can_commit());
    });

    // --- commit ---------------------------------------------------------------
    let committed = app
        .model
        .update(cx, |model, cx| {
            model.commit(&path, "Add notes", CommitOptions::default(), false, cx)
        })
        .await;
    assert!(committed.success(), "{}", committed.message());
    app.model
        .update(cx, |model, cx| {
            model.apply_repository_operation_result(repo_id.as_str(), &committed, cx);
            let request = SyncRequest::new()
                .status(committed.status().is_none())
                .changes_count(true)
                .branches(true)
                .operation("commit")
                .result(committed.to_operation_result());
            model.sync_repository_read_model(repo_id.as_str(), request, cx)
        })
        .await;
    assert_eq!(repo.log_subjects()[0], "Add notes");
    app.model.read_with(cx, |model, _| {
        let repository = model.repository(repo_id.as_str()).unwrap();
        assert_eq!(repository.changes_count, 0);
        assert!(!repository.is_dirty);
        assert_eq!(repository.tracking_status.as_ref().unwrap().ahead, 1);
        assert_eq!(
            repository.last_mutation_operation.as_deref(),
            Some("commit")
        );
    });

    let history = app
        .model
        .update(cx, |model, cx| {
            model.load_commit_history(
                &path,
                CommitHistoryRequest {
                    current_branch: Some("main".to_string()),
                    ..Default::default()
                },
                cx,
            )
        })
        .await
        .unwrap();
    assert_eq!(history.commits[0].message, "Add notes");
    assert_eq!(history.commits.len(), 2);

    // --- crear rama -------------------------------------------------------------
    app.clear_events();
    app.model
        .update(cx, |model, cx| model.handle_new_branch(cx))
        .await;
    assert!(
        app.events()
            .contains(&AppEvent::ShowDialog(DialogRequest::CreateBranch {
                base_reference: "main".to_string(),
                base_label: "main".to_string(),
                has_local_changes: false,
            }))
    );
    app.model
        .update(cx, |model, cx| {
            model.handle_create_branch_submit(
                CreateBranchSubmission {
                    branch_name: "feature".to_string(),
                    checkout_after_create: true,
                    local_changes_mode: LocalChangesMode::Keep,
                },
                cx,
            )
        })
        .await;
    assert_eq!(repo.current_branch(), "feature");
    app.model.read_with(cx, |model, _| {
        let repository = model.repository(repo_id.as_str()).unwrap();
        assert_eq!(repository.current_branch.as_deref(), Some("feature"));
        assert_eq!(repository.branches.len(), 2);
        assert_eq!(model.default_remote_branch_name(), "feature");
    });

    // --- push al remoto bare ------------------------------------------------------
    app.clear_events();
    app.model
        .update(cx, |model, cx| {
            model.handle_push_submit(
                PushRequest {
                    branch: "feature".to_string(),
                    destination: "origin/feature".to_string(),
                    push_tags: false,
                    force: false,
                },
                cx,
            )
        })
        .await;
    assert_eq!(
        git_in(&origin, &["rev-parse", "feature"]).trim(),
        repo.head()
    );
    assert_eq!(app.recorded_mutations(), ["push"]);
    app.model.read_with(cx, |model, _| {
        let repository = model.repository(repo_id.as_str()).unwrap();
        // La rama nueva aparece bajo el remoto y con su upstream.
        assert!(
            repository.remotes[0]
                .branches
                .contains(&"feature".to_string())
        );
        let feature = model.active_branch_entry().expect("feature branch");
        assert_eq!(feature.upstream.as_deref(), Some("origin/feature"));
        assert_eq!(repository.last_mutation_operation.as_deref(), Some("push"));
    });

    // --- fetch sin cambios: no marca mutación ---------------------------------------
    app.clear_events();
    app.host.clear_toasts();
    let last_mutation_at = app.model.read_with(cx, |model, _| {
        model
            .repository(repo_id.as_str())
            .unwrap()
            .last_mutation_at
            .clone()
    });
    app.model
        .update(cx, |model, cx| {
            model.handle_fetch_submit(
                FetchRequest {
                    remote: "origin".to_string(),
                    all_remotes: false,
                },
                cx,
            )
        })
        .await;
    assert!(app.recorded_mutations().is_empty());
    assert_eq!(app.host.messages(NotifyType::Info).len(), 1);
    app.model.read_with(cx, |model, _| {
        let repository = model.repository(repo_id.as_str()).unwrap();
        assert_eq!(repository.last_mutation_at, last_mutation_at);
        assert_eq!(repository.last_mutation_operation.as_deref(), Some("push"));
    });

    // --- el log de operaciones vio pasar todo -----------------------------------------
    app.wait_for(cx, "every operation to reach the log", |model| {
        ["stage", "commit", "create-branch", "push", "fetch"]
            .iter()
            .all(|operation| {
                model.operations_log().entries.iter().any(|entry| {
                    entry.operation.as_deref() == Some(operation) && entry.outcome.is_some()
                })
            })
    });
    app.model.read_with(cx, |model, _| {
        assert_eq!(model.operations_log().unseen_errors, 0);
        assert!(!model.git_is_loading());
        assert_eq!(model.busy_toolbar_operation(), None);
    });
}

/// Un cherry-pick que git rechaza (pisaría cambios locales) no trae estado:
/// el modelo tiene que releerlo para que la fila de cambios sin commitear y
/// los contadores no desaparezcan hasta reiniciar (QA 0.12.0).
#[gpui_kit::test]
async fn a_refused_cherry_pick_keeps_the_local_changes_visible(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    let path = repo.path_string();
    repo.git(&["checkout", "-q", "-b", "side"]);
    let picked = repo.commit_file("README.md", "# Side\n", "Side change");
    repo.git(&["checkout", "-q", "main"]);
    repo.write("README.md", "# Local edit\n");

    app.model
        .update(cx, |model, cx| model.handle_open_repository(&path, cx))
        .await;
    cx.run_until_parked();
    let repo_id = repo.id();
    app.wait_for(cx, "the local change", |model| {
        model
            .repository(repo_id.as_str())
            .is_some_and(|repository| repository.changes_count == 1)
    });

    let result = app
        .model
        .update(cx, |model, cx| model.cherry_pick(&path, &picked, cx))
        .await;
    assert!(!result.success(), "git should refuse the cherry-pick");
    app.model.update(cx, |model, cx| {
        model.apply_repository_operation_result(repo_id.as_str(), &result, cx)
    });

    app.wait_for(cx, "the status read again after the failure", |model| {
        model
            .repository(repo_id.as_str())
            .is_some_and(|repository| repository.status.is_some() && repository.changes_count == 1)
    });
    assert_eq!(repo.git(&["status", "--porcelain"]).trim(), "M README.md");
}
