//! Port de los casos de lógica de `src/App.test.js`.
//!
//! El original montaba `App.vue` con `useGit` y `useRepositorySync` simulados
//! y comprobaba con qué argumentos se llamaba a `syncRepositoryReadModel`.
//! Aquí corre todo de verdad sobre repositorios git reales, así que se
//! comprueba el efecto de esa relectura: qué mutación se registró
//! (`lastMutationOperation`), qué secciones se leyeron y cómo quedó el
//! repositorio.

use std::time::Duration;

use gpui_kit::TestAppContext;
use mergeon_backend::{BackendEvent, StatusChange, WatchReason};
use mergeon_model::notify::NotifyType;

use super::support::{TestApp, TestRepo};
use crate::flows::LocalChangesMode;
use crate::shell::{CreateBranchSubmission, StatusSyncReason};
use crate::testing::TestBackend;
use crate::{AppEvent, DialogRequest, FetchRequest, PullRequest, PushRequest, StashRequest};

const NOT_A_REPOSITORY: &str = "The selected directory is not a valid Git repository";

fn status_changed(app: &TestApp, repo_path: &str, reason: WatchReason, cx: &mut TestAppContext) {
    app.model.update(cx, |model, cx| {
        model.handle_backend_event(
            BackendEvent::StatusChanged(StatusChange {
                repo_path: repo_path.to_string(),
                reason,
            }),
            cx,
        );
    });
}

fn advance(cx: &mut TestAppContext, milliseconds: u64) {
    cx.executor()
        .advance_clock(Duration::from_millis(milliseconds));
    cx.run_until_parked();
}

fn dialogs(app: &TestApp) -> Vec<DialogRequest> {
    app.events()
        .into_iter()
        .filter_map(|event| match event {
            AppEvent::ShowDialog(request) => Some(request),
            _ => None,
        })
        .collect()
}

fn reads(app: &TestApp, repo_id: &str, section: &str, cx: &mut TestAppContext) -> usize {
    app.model
        .read_with(cx, |model, _| model.started_section_reads(repo_id, section))
}

#[gpui_kit::test]
async fn hydrates_repository_sections_after_opening_a_repository_from_the_dashboard(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.publish_to_origin();
    repo.write("dirty.txt", "dirty\n");

    app.model
        .update(cx, |model, cx| {
            model.handle_open_repository(&repo.path_string(), cx)
        })
        .await;
    cx.run_until_parked();

    app.model.read_with(cx, |model, _| {
        let repository = model.active_repository().expect("active repository");
        assert_eq!(repository.path, repo.id());
        assert_eq!(
            repository.last_mutation_operation.as_deref(),
            Some("hydrate-opened-repository")
        );
        // status, branches, remotes, changesCount y starredBranches.
        assert!(repository.status.is_some());
        assert_eq!(repository.changes_count, 1);
        assert_eq!(repository.current_branch.as_deref(), Some("main"));
        assert!(repository.sidebar_loaded.branches);
        assert!(repository.sidebar_loaded.remotes);
        assert!(repository.sidebar_loaded.changes_count);
        assert!(repository.sidebar_loaded.starred_branches);
        assert!(!repository.sidebar_loaded.tags);
        assert!(!repository.sidebar_loaded.stashes);
        assert_eq!(repository.remotes[0].branches, ["main"]);

        assert_eq!(model.active_status_watch_path(), Some(repo.id().as_str()));
        assert_eq!(model.visited_repositories().count(), 1);
    });
    assert_eq!(app.recorded_mutations(), ["hydrate-opened-repository"]);
    assert_eq!(
        app.host.messages(NotifyType::Success),
        ["Repository opened successfully"]
    );
}

#[gpui_kit::test]
async fn offers_to_initialize_git_when_opening_a_non_repository_from_the_dashboard(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let directory = TestRepo::plain_directory();
    let path = directory.path_string();
    app.host.answer_confirm(true);

    app.model
        .update(cx, |model, cx| model.handle_open_repository(&path, cx))
        .await;
    cx.run_until_parked();

    let confirms = app.host.confirms();
    assert_eq!(confirms.len(), 1);
    assert_eq!(
        confirms[0].title.as_deref(),
        Some("Directory is not under Git source control")
    );
    assert_eq!(
        confirms[0].description.as_deref(),
        Some(format!("The '{path}' directory is not a Git repository.").as_str())
    );
    assert_eq!(
        confirms[0].confirm_text.as_deref(),
        Some("Initialize git repository here")
    );
    assert_eq!(confirms[0].cancel_text.as_deref(), Some("Close"));

    assert!(directory.path.join(".git").exists());
    app.model.read_with(cx, |model, _| {
        assert_eq!(
            model.active_repository().map(|repo| repo.path.clone()),
            Some(directory.id())
        );
    });
    assert_eq!(
        app.host.messages(NotifyType::Success),
        ["Git repository initialized and opened successfully"]
    );
}

#[gpui_kit::test]
async fn offers_to_initialize_git_when_opening_a_local_folder_outside_git(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let directory = TestRepo::plain_directory();
    app.host.answer_directory(Some(&directory.path));
    app.host.answer_confirm(true);

    app.model
        .update(cx, |model, cx| model.open_local_repository(cx))
        .await;
    cx.run_until_parked();

    assert_eq!(app.host.directory_requests(), 1);
    assert!(directory.path.join(".git").exists());
    app.model.read_with(cx, |model, _| {
        assert_eq!(
            model.active_repository().map(|repo| repo.path.clone()),
            Some(directory.id())
        );
    });
    assert!(app.host.has_message(
        NotifyType::Success,
        "Git repository initialized and opened successfully"
    ));
}

#[gpui_kit::test]
async fn cancelling_the_directory_picker_opens_nothing(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    app.host.answer_directory(None);

    app.model
        .update(cx, |model, cx| model.open_local_repository(cx))
        .await;

    assert_eq!(app.host.directory_requests(), 1);
    assert!(app.host.toasts().is_empty());
    app.model
        .read_with(cx, |model, _| assert!(model.active_repository().is_none()));
}

#[gpui_kit::test]
async fn does_not_show_an_error_when_the_user_declines_initializing_git(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let directory = TestRepo::plain_directory();
    app.host.answer_confirm(false);

    app.model
        .update(cx, |model, cx| {
            model.handle_open_repository(&directory.path_string(), cx)
        })
        .await;

    assert!(!directory.path.join(".git").exists());
    app.model
        .read_with(cx, |model, _| assert!(model.active_repository().is_none()));
    assert!(app.host.toasts().is_empty());
}

#[gpui_kit::test]
async fn reports_a_path_that_cannot_be_opened(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let directory = TestRepo::plain_directory();
    let missing = directory
        .path
        .join("missing")
        .to_string_lossy()
        .into_owned();

    let opened = app
        .model
        .update(cx, |model, cx| {
            model.open_repository_path(&missing, NOT_A_REPOSITORY, cx)
        })
        .await
        .unwrap();

    assert!(!opened);
    assert_eq!(app.host.messages(NotifyType::Error).len(), 1);
    assert!(app.host.confirms().is_empty());
    app.model
        .read_with(cx, |model, _| assert!(model.open_repositories().is_empty()));
}

#[gpui_kit::test]
async fn restores_open_repository_tabs_from_the_persisted_workspace_session_without_rewriting_recents(
    cx: &mut TestAppContext,
) {
    let first = TestRepo::new();
    let second = TestRepo::new();
    let gone = TestRepo::plain_directory();
    let store = TestBackend::memory_store();
    store
        .set_workspace_session(&mergeon_store::WorkspaceSession {
            open_repositories: vec![first.id(), gone.id(), second.id()],
            active_repository_path: Some(second.id()),
            ..Default::default()
        })
        .unwrap();
    let app = TestApp::with_backend(TestBackend::new().with_store(store), cx);

    app.model.update(cx, |model, cx| model.start(cx)).await;
    cx.run_until_parked();

    app.model.read_with(cx, |model, _| {
        // Lo que ya no es un repositorio se queda fuera sin avisar.
        let paths: Vec<_> = model
            .open_repositories()
            .iter()
            .map(|repo| repo.path.clone())
            .collect();
        assert_eq!(paths, [first.id(), second.id()]);
        assert_eq!(
            model.active_repository().map(|repo| repo.path.clone()),
            Some(second.id())
        );
        assert_eq!(model.active_status_watch_path(), Some(second.id().as_str()));
        assert!(model.active_repository().unwrap().sidebar_loaded.branches);
        // El otro repositorio se hidrata cuando se visite.
        assert!(
            model
                .repository(first.id().as_str())
                .unwrap()
                .status
                .is_none()
        );
    });
    assert!(app.backend.storage().get_recent_repositories().is_empty());
    assert!(app.host.toasts().is_empty());
    assert_eq!(app.recorded_mutations(), ["hydrate-opened-repository"]);
    assert_eq!(
        app.backend
            .storage()
            .get_workspace_session()
            .open_repositories,
        [first.id(), second.id()]
    );
}

#[gpui_kit::test]
async fn hydrates_repository_sections_when_switching_between_already_open_repositories(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let first = TestRepo::new();
    let second = TestRepo::new();
    app.seed(&first, cx);
    let second_id = app.seed(&second, cx);
    app.model.update(cx, |model, _| {
        model.repositories.active_repository_index = Some(0);
    });

    app.model
        .update(cx, |model, cx| model.select_repository_tab(1, cx));
    cx.run_until_parked();

    assert_eq!(
        app.events()
            .iter()
            .filter(|event| matches!(event, AppEvent::MutationRecorded { .. }))
            .cloned()
            .collect::<Vec<_>>(),
        [AppEvent::MutationRecorded {
            repo_id: second_id.clone(),
            operation: Some("hydrate-opened-repository".to_string()),
        }]
    );
    app.model.read_with(cx, |model, _| {
        assert_eq!(model.active_status_watch_path(), Some(second_id.as_str()));
        assert!(model.repository(0usize).unwrap().status.is_none());
    });

    // Volver al inicio deja de vigilar.
    app.model.update(cx, |model, cx| model.show_home_tab(cx));
    cx.run_until_parked();
    app.model.read_with(cx, |model, _| {
        assert_eq!(model.active_status_watch_path(), None)
    });
}

#[gpui_kit::test]
async fn watches_the_active_repository_and_syncs_only_status_data_when_local_changes_events_arrive(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    let repo_id = app.open(&repo, cx).await;
    app.clear_events();
    let (status_reads, branch_reads) = (
        reads(&app, &repo_id, "status", cx),
        reads(&app, &repo_id, "branches", cx),
    );

    repo.write("notes.txt", "notes\n");
    status_changed(&app, &repo_id, WatchReason::FsWatchWorkingTree, cx);

    // Nada se relee hasta que pasan 250 ms sin más avisos.
    advance(cx, 249);
    assert!(app.recorded_mutations().is_empty());
    advance(cx, 1);

    assert_eq!(app.recorded_mutations(), ["status-watch"]);
    assert_eq!(reads(&app, &repo_id, "status", cx), status_reads + 1);
    assert_eq!(reads(&app, &repo_id, "branches", cx), branch_reads);
    app.model.read_with(cx, |model, _| {
        assert_eq!(model.active_repository().unwrap().changes_count, 1);
    });
}

#[gpui_kit::test]
async fn refreshes_branches_and_records_an_external_head_change_when_head_metadata_changes(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    let repo_id = app.open(&repo, cx).await;
    app.clear_events();
    let branch_reads = reads(&app, &repo_id, "branches", cx);

    repo.git(&["checkout", "-b", "from-terminal"]);
    status_changed(&app, &repo_id, WatchReason::FsWatchHead, cx);
    advance(cx, 250);

    assert_eq!(app.recorded_mutations(), ["external-head-change"]);
    assert_eq!(reads(&app, &repo_id, "branches", cx), branch_reads + 1);
    app.model.read_with(cx, |model, _| {
        let repository = model.active_repository().unwrap();
        assert_eq!(repository.current_branch.as_deref(), Some("from-terminal"));
        assert_eq!(repository.branches.len(), 2);
    });
}

#[gpui_kit::test]
async fn ignores_watcher_events_for_other_repositories_and_coalesces_bursts_while_a_sync_is_running(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    let repo_id = app.open(&repo, cx).await;
    app.clear_events();

    status_changed(&app, "/tmp/other", WatchReason::FsWatchWorkingTree, cx);
    advance(cx, 300);
    assert!(app.recorded_mutations().is_empty());

    // Una ráfaga de avisos es una sola relectura.
    status_changed(&app, &repo_id, WatchReason::FsWatchWorkingTree, cx);
    status_changed(&app, &repo_id, WatchReason::FsWatchWorkingTree, cx);
    advance(cx, 250);
    assert_eq!(app.recorded_mutations(), ["status-watch"]);

    // Con una relectura en curso, lo que llegue se funde y espera a que
    // termine: aquí llegan un cambio de árbol y otro de HEAD.
    let running = app.model.update(cx, |model, cx| {
        model.sync_active_repository_status(StatusSyncReason::PollWorkingTree, cx)
    });
    app.model.update(cx, |model, cx| {
        model
            .sync_active_repository_status(StatusSyncReason::FsWatchWorkingTree, cx)
            .detach();
        model
            .sync_active_repository_status(StatusSyncReason::PollHead, cx)
            .detach();
    });
    running.await;
    assert_eq!(app.recorded_mutations(), ["status-watch", "status-watch"]);

    advance(cx, 250);
    assert_eq!(
        app.recorded_mutations(),
        ["status-watch", "status-watch", "external-head-change"]
    );
}

#[gpui_kit::test]
async fn uses_directed_sync_after_a_successful_push(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    let origin = repo.publish_to_origin();
    repo.commit_file("local.txt", "local\n", "Local change");
    let repo_id = app.open(&repo, cx).await;
    app.clear_events();
    let (tag_reads, remote_reads) = (
        reads(&app, &repo_id, "tags", cx),
        reads(&app, &repo_id, "remotes", cx),
    );
    app.model.read_with(cx, |model, _| {
        assert_eq!(
            model
                .active_repository()
                .unwrap()
                .tracking_status
                .as_ref()
                .unwrap()
                .ahead,
            1
        );
    });

    app.model
        .update(cx, |model, cx| {
            model.handle_push_submit(
                PushRequest {
                    branch: "main".to_string(),
                    destination: "origin/main".to_string(),
                    push_tags: false,
                    force: false,
                },
                cx,
            )
        })
        .await;

    assert_eq!(
        super::support::git_in(&origin, &["rev-parse", "main"]).trim(),
        repo.head()
    );
    // status, branches, remotes y changesCount; no tags.
    assert_eq!(app.recorded_mutations(), ["push"]);
    assert_eq!(reads(&app, &repo_id, "remotes", cx), remote_reads + 1);
    assert_eq!(reads(&app, &repo_id, "tags", cx), tag_reads);
    app.model.read_with(cx, |model, _| {
        let repository = model.active_repository().unwrap();
        assert_eq!(repository.tracking_status.as_ref().unwrap().ahead, 0);
        assert_eq!(model.busy_toolbar_operation(), None);
    });
}

#[gpui_kit::test]
async fn uses_directed_sync_after_a_successful_pull(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    let origin = repo.publish_to_origin();
    let colleague = repo.colleague(&origin);
    let remote_head = colleague.commit_and_push("remote.txt", "remote\n", "Remote change");
    let repo_id = app.open(&repo, cx).await;
    app.clear_events();
    let tag_reads = reads(&app, &repo_id, "tags", cx);

    app.model
        .update(cx, |model, cx| {
            model.handle_pull_submit(
                PullRequest {
                    remote: "origin".to_string(),
                    branch: Some("main".to_string()),
                    rebase: false,
                    autostash: false,
                },
                cx,
            )
        })
        .await;

    assert_eq!(repo.head(), remote_head);
    // status, branches, remotes y tags.
    assert_eq!(app.recorded_mutations(), ["pull"]);
    assert_eq!(reads(&app, &repo_id, "tags", cx), tag_reads + 1);
    app.model.read_with(cx, |model, _| {
        assert!(model.active_repository().unwrap().sidebar_loaded.tags);
    });
}

#[gpui_kit::test]
async fn a_pull_that_moved_no_ref_rereads_the_status_without_recording_a_mutation(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.publish_to_origin();
    let repo_id = app.open(&repo, cx).await;
    app.clear_events();
    let status_reads = reads(&app, &repo_id, "status", cx);

    app.model
        .update(cx, |model, cx| {
            model.handle_pull_submit(
                PullRequest {
                    remote: "origin".to_string(),
                    branch: Some("main".to_string()),
                    rebase: false,
                    autostash: false,
                },
                cx,
            )
        })
        .await;

    assert!(app.recorded_mutations().is_empty());
    assert_eq!(reads(&app, &repo_id, "status", cx), status_reads + 1);
}

#[gpui_kit::test]
async fn ignores_duplicate_fetch_triggers_while_the_first_fetch_is_still_running(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    let origin = repo.publish_to_origin();
    let colleague = repo.colleague(&origin);
    colleague.commit_and_push("remote.txt", "remote\n", "Remote change");
    let repo_id = app.open(&repo, cx).await;
    app.clear_events();
    let fetch = || FetchRequest {
        remote: "origin".to_string(),
        all_remotes: true,
    };

    let (first, second) = app.model.update(cx, |model, cx| {
        let first = model.handle_fetch_submit(fetch(), cx);
        // La barra de herramientas deshabilita el botón mientras tanto.
        assert_eq!(model.busy_toolbar_operation(), Some("fetch"));
        (first, model.handle_fetch_submit(fetch(), cx))
    });
    first.await;
    second.await;

    // Un solo fetch: un aviso y una relectura (status, branches, remotes, tags).
    assert_eq!(app.host.toasts().len(), 1);
    assert_eq!(app.recorded_mutations(), ["fetch"]);
    app.model.read_with(cx, |model, _| {
        assert_eq!(model.busy_toolbar_operation(), None);
        let repository = model.repository(repo_id.as_str()).unwrap();
        assert!(repository.sidebar_loaded.tags);
        assert_eq!(repository.tracking_status.as_ref().unwrap().behind, 1);
    });
}

/*
 * Un fetch que no movió ningún ref no debe marcar mutación: hacerlo obligaba a
 * la vista de commits a reconstruir el historial entero, también en cada
 * auto-fetch periódico.
 */
#[gpui_kit::test]
async fn skips_the_read_model_sync_when_a_fetch_brought_no_new_refs(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.publish_to_origin();
    // El primer fetch de un clon crea `refs/remotes/origin/HEAD`: se hace
    // antes para que el del test no traiga de verdad nada nuevo.
    repo.git(&["fetch", "origin"]);
    let repo_id = app.open(&repo, cx).await;
    app.clear_events();
    let status_reads = reads(&app, &repo_id, "status", cx);

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
    assert_eq!(reads(&app, &repo_id, "status", cx), status_reads);
    app.model.read_with(cx, |model, _| {
        assert_eq!(
            model
                .repository(repo_id.as_str())
                .unwrap()
                .last_mutation_operation
                .as_deref(),
            Some("hydrate-opened-repository")
        );
    });
    // El fetch manual sí avisa de que no había nada nuevo.
    assert_eq!(app.host.messages(NotifyType::Info).len(), 1);
}

#[gpui_kit::test]
async fn applies_directed_sync_for_stash_and_new_branch_toolbar_actions(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.write("README.md", "# Demo\n\nwork in progress\n");
    let repo_id = app.open(&repo, cx).await;
    app.clear_events();

    // El botón abre el diálogo; lo que envía el diálogo hace el stash.
    app.model
        .update(cx, |model, cx| model.handle_stash(cx))
        .await;
    assert_eq!(dialogs(&app), [DialogRequest::Stash { paths: vec![] }]);
    assert!(app.recorded_mutations().is_empty());

    app.model
        .update(cx, |model, cx| {
            model.handle_stash_submit(
                StashRequest {
                    message: Some("  stash message  ".to_string()),
                    ..Default::default()
                },
                cx,
            )
        })
        .await;

    assert!(repo.git(&["stash", "list"]).contains("stash message"));
    assert!(app.recorded_mutations().contains(&"stash".to_string()));
    app.model.read_with(cx, |model, _| {
        let repository = model.repository(repo_id.as_str()).unwrap();
        // El resultado traía el estado; además se releyeron los stashes.
        assert_eq!(repository.changes_count, 0);
        assert!(repository.sidebar_loaded.stashes);
        assert_eq!(repository.stashes.len(), 1);
        assert_eq!(repository.last_mutation_operation.as_deref(), Some("stash"));
    });

    app.clear_events();
    app.model
        .update(cx, |model, cx| model.handle_new_branch(cx))
        .await;
    assert_eq!(
        dialogs(&app),
        [DialogRequest::CreateBranch {
            base_reference: "main".to_string(),
            base_label: "main".to_string(),
            has_local_changes: false,
        }]
    );

    app.model
        .update(cx, |model, cx| {
            model.handle_create_branch_submit(
                CreateBranchSubmission {
                    branch_name: "feature/new".to_string(),
                    checkout_after_create: true,
                    local_changes_mode: LocalChangesMode::Keep,
                },
                cx,
            )
        })
        .await;

    assert_eq!(repo.current_branch(), "feature/new");
    assert!(
        app.recorded_mutations()
            .contains(&"create-branch".to_string())
    );
    app.model.read_with(cx, |model, _| {
        let repository = model.repository(repo_id.as_str()).unwrap();
        assert_eq!(repository.current_branch.as_deref(), Some("feature/new"));
        assert_eq!(repository.branches.len(), 2);
        assert!(
            repository
                .branches
                .iter()
                .any(|branch| branch.name == "feature/new" && branch.active)
        );
        assert_eq!(
            repository.last_mutation_operation.as_deref(),
            Some("create-branch")
        );
    });
}

#[gpui_kit::test]
async fn stashes_only_the_selected_files_with_their_options(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.write("README.md", "# Demo\n\nwork in progress\n");
    repo.write("new notes.txt", "draft\n");
    let repo_id = app.open(&repo, cx).await;
    app.clear_events();

    app.model.update(cx, |model, cx| {
        model.handle_stash_paths(vec!["new notes.txt".to_string()], cx)
    });
    assert_eq!(
        dialogs(&app),
        [DialogRequest::Stash {
            paths: vec!["new notes.txt".to_string()]
        }]
    );

    app.model
        .update(cx, |model, cx| {
            model.handle_stash_submit(
                StashRequest {
                    paths: vec!["new notes.txt".to_string()],
                    include_untracked: true,
                    ..Default::default()
                },
                cx,
            )
        })
        .await;

    assert_eq!(
        repo.git(&[
            "stash",
            "show",
            "--include-untracked",
            "--name-only",
            "stash@{0}"
        ])
        .trim(),
        "new notes.txt"
    );
    assert_eq!(repo.git(&["status", "--porcelain"]).trim(), "M README.md");
    app.model.read_with(cx, |model, _| {
        let repository = model.repository(repo_id.as_str()).unwrap();
        assert_eq!(repository.stashes.len(), 1);
        assert_eq!(repository.changes_count, 1);
    });
}

#[gpui_kit::test]
async fn does_not_open_the_create_branch_dialog_when_local_changes_inspection_fails(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    app.seed(&repo, cx);
    repo.break_repository();

    app.model
        .update(cx, |model, cx| model.handle_new_branch(cx))
        .await;

    assert!(dialogs(&app).is_empty());
    assert_eq!(app.host.messages(NotifyType::Error).len(), 1);
}
