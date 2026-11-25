//! Port de `src/composables/useGit.test.js`, contra git real.
//!
//! No se porta "rejects legacy positional usage at the composable layer": las
//! opciones son un tipo y el uso posicional no compila.

use gpui_kit::TestAppContext;
use mergeon_backend::{
    BackendEvent, GitProgress, OperationEvent, OperationPhase, OperationProgress,
};
use mergeon_backend::{
    CommitOptions, DiffOptions, FetchOptions, Outcome, PullOptions, PushOptions,
};
use mergeon_model::notify::{NotifyType, ProgressToast, ToastActionKind, ToastId};

use super::support::{TestApp, TestRepo};
use crate::StructuredOutcome;

#[gpui_kit::test]
async fn clones_through_the_shared_git_client(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    let origin = repo.publish_to_origin();
    let target = repo.root().join("demo-clone");

    let result = app
        .model
        .update(cx, |model, cx| {
            model.clone_repository(
                &origin.to_string_lossy(),
                &target.to_string_lossy(),
                None,
                cx,
            )
        })
        .await;

    assert!(result.success, "{}", result.message);
    assert_eq!(result.operation.as_str(), "clone");
    assert_eq!(result.outcome, Outcome::Completed);
    assert!(result.path.is_some());
    assert!(target.join("README.md").exists());
    assert!(
        app.host
            .has_message(NotifyType::Success, "Repository cloned successfully.")
    );
    app.model.read_with(cx, |model, _| {
        assert!(!model.git_is_loading());
        assert_eq!(model.git_error(), None);
    });
}

#[gpui_kit::test]
async fn a_failed_clone_is_a_structured_result_with_its_error(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    let missing = repo.root().join("missing.git");
    let target = repo.root().join("demo-clone");

    let result = app
        .model
        .update(cx, |model, cx| {
            model.clone_repository(
                &missing.to_string_lossy(),
                &target.to_string_lossy(),
                None,
                cx,
            )
        })
        .await;

    assert!(!result.success);
    assert_eq!(result.operation.as_str(), "clone");
    assert_eq!(app.host.messages(NotifyType::Error).len(), 1);
    app.model.read_with(cx, |model, _| {
        assert_eq!(model.git_error(), Some(result.message.as_str()));
        assert!(!model.git_is_loading());
    });
}

#[gpui_kit::test]
async fn forwards_fetch_pull_and_push_with_their_options(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    let origin = repo.publish_to_origin();
    let colleague = repo.colleague(&origin);
    colleague.commit_and_push("remote.txt", "remote\n", "Remote change");
    let path = repo.path_string();

    let fetch_result = app
        .model
        .update(cx, |model, cx| {
            model.fetch(
                &path,
                FetchOptions {
                    remote: "origin".to_string(),
                    ..Default::default()
                },
                false,
                cx,
            )
        })
        .await;
    assert!(fetch_result.success(), "{}", fetch_result.message());
    assert_eq!(fetch_result.operation(), Some("fetch"));
    assert_eq!(
        fetch_result
            .result()
            .and_then(|result| result.remote.as_deref()),
        Some("origin")
    );
    assert_eq!(
        fetch_result.result().and_then(|result| result.refs_changed),
        Some(true)
    );

    let pull_result = app
        .model
        .update(cx, |model, cx| {
            model.pull(
                &path,
                PullOptions {
                    remote: Some("origin".to_string()),
                    branch: Some("main".to_string()),
                    ..Default::default()
                },
                cx,
            )
        })
        .await;
    assert!(pull_result.success(), "{}", pull_result.message());
    assert_eq!(pull_result.operation(), Some("pull"));
    assert!(repo.path.join("remote.txt").exists());

    repo.commit_file("local.txt", "local\n", "Local change");
    let push_result = app
        .model
        .update(cx, |model, cx| {
            model.push(
                &path,
                PushOptions {
                    remote: "origin".to_string(),
                    branch: Some("main".to_string()),
                    ..Default::default()
                },
                false,
                cx,
            )
        })
        .await;
    assert!(push_result.success(), "{}", push_result.message());
    assert_eq!(push_result.operation(), Some("push"));
    assert_eq!(
        super::support::git_in(&origin, &["rev-parse", "main"]).trim(),
        repo.head()
    );

    // Cada operación avisó una vez de su resultado.
    assert_eq!(app.host.toasts().len(), 3);
}

#[gpui_kit::test]
async fn coalesces_identical_remote_operations(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.publish_to_origin();
    let path = repo.path_string();

    let (first, second) = app.model.update(cx, |model, cx| {
        (
            model.fetch(&path, FetchOptions::default(), false, cx),
            model.fetch(&path, FetchOptions::default(), false, cx),
        )
    });
    app.model
        .read_with(cx, |model, _| assert!(model.git_is_loading()));

    let (first, second) = (first.await, second.await);

    // Una sola llamada al backend: el mismo resultado, la misma operación en
    // la cola y un solo aviso.
    assert_eq!(first, second);
    assert!(first.operation_id().is_some());
    assert_eq!(app.host.toasts().len(), 1);
    app.model
        .read_with(cx, |model, _| assert!(!model.git_is_loading()));
}

/// Un fetch que falla y el reintento que funciona no son dos avisos que se
/// contradicen: el resultado nuevo reescribe el aviso de esa operación en ese
/// repositorio. El de otra operación sí se suma.
#[gpui_kit::test]
async fn a_new_result_replaces_the_toast_of_the_same_operation(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    let origin = repo.publish_to_origin();
    let path = repo.path_string();
    let fetch = |app: &TestApp, cx: &mut TestAppContext| {
        app.model.update(cx, |model, cx| {
            let options = FetchOptions {
                all_remotes: true,
                ..Default::default()
            };
            model.fetch(&path, options, false, cx)
        })
    };

    let hidden = origin.with_extension("moved");
    std::fs::rename(&origin, &hidden).unwrap();
    let failed = fetch(&app, cx).await;
    assert!(!failed.success());
    std::fs::rename(&hidden, &origin).unwrap();
    cx.run_until_parked();
    let retried = fetch(&app, cx).await;
    assert!(retried.succeeded_or_noop(), "{}", retried.message());

    let toasts = app.host.toasts();
    assert_eq!(toasts.len(), 2);
    assert_eq!(toasts[0].id, toasts[1].id);
    // El fallo dice por qué, no solo que falló.
    assert!(
        toasts[0]
            .full_message
            .starts_with("Fetch failed for 'origin'.\n")
            && toasts[0].full_message.len() > "Fetch failed for 'origin'.\n".len(),
        "{}",
        toasts[0].full_message
    );

    repo.write("notes.txt", "draft\n");
    app.model
        .update(cx, |model, cx| model.stage(&path, "notes.txt", cx))
        .await;
    let toasts = app.host.toasts();
    assert_ne!(toasts.last().unwrap().id, toasts[0].id);
}

#[gpui_kit::test]
async fn a_silent_fetch_shows_no_toast(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.publish_to_origin();

    let result = app
        .model
        .update(cx, |model, cx| {
            model.fetch(&repo.path_string(), FetchOptions::default(), true, cx)
        })
        .await;

    assert!(result.succeeded_or_noop());
    assert!(app.host.toasts().is_empty());
}

#[gpui_kit::test]
async fn returns_a_structured_result_for_add_remote(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();

    let result = app
        .model
        .update(cx, |model, cx| {
            model.add_remote(
                &repo.path_string(),
                "origin",
                "https://github.com/demo/repo.git",
                false,
                cx,
            )
        })
        .await;

    assert!(result.success());
    assert_eq!(result.operation(), Some("add-remote"));
    assert_eq!(result.outcome(), Some(Outcome::Completed));
    assert_eq!(
        result.result().and_then(|result| result.remote.as_deref()),
        Some("origin")
    );
    assert_eq!(
        repo.git(&["remote", "get-url", "origin"]).trim(),
        "https://github.com/demo/repo.git"
    );
}

#[gpui_kit::test]
async fn returns_errors_for_domain_reads_instead_of_fabricating_fallback_values(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    let head = repo.head();
    let path = repo.path_string();
    repo.break_repository();

    let changes = app
        .model
        .update(cx, |model, cx| model.has_uncommitted_changes(&path, cx))
        .await;
    let blame = app
        .model
        .update(cx, |model, cx| model.blame(&path, "README.md", cx))
        .await;
    let file = app
        .model
        .update(cx, |model, cx| model.read_file(&path, "missing.txt", cx))
        .await;
    let diff = app
        .model
        .update(cx, |model, cx| {
            model.get_commit_diff(&path, &head, "README.md", DiffOptions::default(), cx)
        })
        .await;

    assert!(changes.is_err());
    assert!(blame.is_err());
    assert!(file.is_err());
    assert!(diff.is_err());
    // Estas lecturas no avisan: quien las pide presenta el fallo.
    assert!(app.host.toasts().is_empty());
    app.model
        .read_with(cx, |model, _| assert!(model.git_error().is_some()));
}

#[gpui_kit::test]
async fn reads_that_announced_their_failure_still_do(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    let path = repo.path_string();
    repo.break_repository();

    let status = app
        .model
        .update(cx, |model, cx| model.get_status(&path, cx))
        .await;

    let error = status.unwrap_err();
    assert_eq!(
        app.host.messages(NotifyType::Error),
        ["Error loading status"]
    );
    // El texto completo del error va al detalle del aviso.
    assert_eq!(app.host.toasts()[0].full_message, error.message());
}

#[gpui_kit::test]
async fn classifies_setup_tracking_outcomes_without_noisy_errors(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.publish_to_origin();
    repo.git(&["branch", "feature"]);
    let path = repo.path_string();

    let noop_result = app
        .model
        .update(cx, |model, cx| {
            model.setup_tracking(&path, Some("main"), cx)
        })
        .await;
    let blocked_result = app
        .model
        .update(cx, |model, cx| {
            model.setup_tracking(&path, Some("feature"), cx)
        })
        .await;

    assert_eq!(noop_result.code(), "ALREADY_TRACKING");
    assert_eq!(blocked_result.code(), "REMOTE_BRANCH_NOT_FOUND");
    assert_eq!(
        app.host.messages(NotifyType::Info),
        ["main already tracks origin/main."]
    );
    assert_eq!(
        app.host.messages(NotifyType::Warning),
        ["No remote branch found for origin/feature."]
    );
    assert!(app.host.messages(NotifyType::Error).is_empty());
}

#[gpui_kit::test]
async fn creates_a_branch_from_stash_through_the_structured_operation_flow(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.write("README.md", "# Demo\n\nwork in progress\n");
    let path = repo.path_string();

    let stashed = app
        .model
        .update(cx, |model, cx| model.stash(&path, Some("wip"), cx))
        .await;
    assert!(stashed.success(), "{}", stashed.message());

    let result = app
        .model
        .update(cx, |model, cx| {
            model.stash_branch(&path, "stash@{0}", "recover-work", cx)
        })
        .await;

    assert!(result.success(), "{}", result.message());
    assert_eq!(result.operation(), Some("stash-branch"));
    assert_eq!(
        result.result().and_then(|result| result.branch.as_deref()),
        Some("recover-work")
    );
    assert_eq!(repo.current_branch(), "recover-work");
}

#[gpui_kit::test]
async fn stages_and_commits_with_the_notifications_of_the_original(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.write("notes.txt", "notes\n");
    let path = repo.path_string();

    let staged = app
        .model
        .update(cx, |model, cx| model.stage(&path, "notes.txt", cx))
        .await;
    assert!(staged.success(), "{}", staged.message());
    assert_eq!(
        staged.status().map(|status| status.staged.clone()),
        Some(vec!["notes.txt".to_string()])
    );

    // Sin mensaje el commit se bloquea con un aviso, no con un error.
    let blocked = app
        .model
        .update(cx, |model, cx| {
            model.commit(&path, "", CommitOptions::default(), false, cx)
        })
        .await;
    assert_eq!(blocked.code(), "COMMIT_MESSAGE_REQUIRED");
    assert_eq!(app.host.messages(NotifyType::Warning).len(), 1);
    app.model.read_with(cx, |model, _| {
        assert_eq!(model.git_error(), Some(blocked.message()));
    });

    let committed = app
        .model
        .update(cx, |model, cx| {
            model.commit(&path, "Add notes", CommitOptions::default(), false, cx)
        })
        .await;
    assert!(committed.success(), "{}", committed.message());
    assert_eq!(repo.log_subjects()[0], "Add notes");
    app.model
        .read_with(cx, |model, _| assert_eq!(model.git_error(), None));

    // Las operaciones pasan por la cola: sus eventos llegan al log.
    let operation_id = committed.operation_id().unwrap().to_string();
    app.wait_for(cx, "the commit to reach the operations log", |model| {
        model.operations_log().entries.iter().any(|entry| {
            entry.operation_id == operation_id && entry.outcome.as_deref() == Some("completed")
        })
    });
}

/// Fetch, pull y push muestran un aviso de progreso mientras corren; el del
/// resultado lo reemplaza (mismo id). Arranca con la barra indeterminada (el
/// avance real lo prueba `progress_updates_rewrite_the_toast_in_place`). Un
/// fetch silencioso no muestra ninguno.
#[gpui_kit::test]
async fn long_remote_operations_show_a_progress_toast_until_their_result(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.publish_to_origin();
    let path = repo.path_string();

    app.model
        .update(cx, |model, cx| {
            model.fetch(&path, FetchOptions::default(), true, cx)
        })
        .await;
    assert!(app.host.progress_toasts().is_empty());

    let result = app
        .model
        .update(cx, |model, cx| {
            let options = FetchOptions {
                remote: "origin".to_string(),
                ..Default::default()
            };
            model.fetch(&path, options, false, cx)
        })
        .await;
    assert!(result.succeeded_or_noop(), "{}", result.message());

    let progress = app.host.progress_toasts();
    // El primero y, después, el mismo aviso reescrito (Cancel, avance).
    assert!(progress.iter().all(|toast| toast.id == progress[0].id));
    assert_eq!(progress[0].title, "Fetch origin…");
    assert_eq!(progress[0].operation, "fetch");
    assert_eq!(progress[0].fraction, None);
    assert_eq!(progress[0].cancel, None);
    app.model.read_with(cx, |model, _| {
        assert!(model.active_progress_toasts().is_empty())
    });
    // Al terminar se retira, y el resultado (si lo hay) ocupa su sitio.
    assert_eq!(app.host.dismissed_toasts(), [progress[0].id.clone()]);
    assert!(
        app.host
            .toasts()
            .iter()
            .all(|toast| toast.id == progress[0].id)
    );

    app.model
        .update(cx, |model, cx| {
            let options = PushOptions {
                remote: "origin".to_string(),
                branch: Some("main".to_string()),
                ..Default::default()
            };
            model.push(&path, options, false, cx)
        })
        .await;
    assert_eq!(
        app.host
            .progress_toasts()
            .last()
            .map(|toast| toast.title.as_str()),
        Some("Push origin/main…")
    );
}

/// El progreso que publica la cola reescribe el aviso de su operación: barra
/// con el avance de la fase, texto con la fase y la velocidad, y "Cancel" en
/// cuanto la operación arranca. Lo que no cambia lo que se ve no se repinta,
/// y nada de esto avisa a las vistas del modelo.
#[gpui_kit::test]
async fn progress_updates_rewrite_the_toast_in_place(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    let path = app.open(&repo, cx).await;
    let toast_id = ToastId::Named(format!("operation:fetch:{path}"));

    app.model.update(cx, |model, cx| {
        model.begin_progress(
            ProgressToast {
                id: toast_id.clone(),
                operation: "fetch".to_string(),
                title: "Fetch origin…".to_string(),
                description: None,
                fraction: None,
                cancel: None,
            },
            &path,
            cx,
        );
    });
    let running = BackendEvent::Operation(OperationEvent {
        operation_id: "fetch-7".to_string(),
        repo_path: path.clone(),
        operation: "fetch".to_string(),
        phase: OperationPhase::Running,
        position: 0,
        queue_length: 0,
        active_operation_id: Some("fetch-7".to_string()),
        operations: Vec::new(),
        error: None,
    });
    let update = |phase: &str, processed: u64, throughput: Option<&str>| {
        BackendEvent::OperationProgress(OperationProgress {
            operation_id: "fetch-7".to_string(),
            repo_path: path.clone(),
            operation: "fetch".to_string(),
            progress: GitProgress {
                phase: phase.to_string(),
                percent: Some(processed as u8 / 10),
                processed,
                total: Some(1000),
                throughput: throughput.map(str::to_string),
                ..Default::default()
            },
        })
    };

    // El evento de la cola sí avisa (alimenta el log); el progreso, no.
    app.model
        .update(cx, |model, cx| model.handle_backend_event(running, cx));
    let notifications = app.count_notifications(cx);
    app.model.update(cx, |model, cx| {
        model.handle_backend_event(update("Receiving objects", 450, Some("2.00 MiB/s")), cx);
        // Mismo porcentaje y texto: no se repinta.
        model.handle_backend_event(update("Receiving objects", 450, Some("2.00 MiB/s")), cx);
        // Otra operación: no es la de este aviso.
        let mut other = update("Receiving objects", 900, None);
        if let BackendEvent::OperationProgress(progress) = &mut other {
            progress.operation_id = "push-1".to_string();
            progress.operation = "push".to_string();
        }
        model.handle_backend_event(other, cx);
    });

    let shown = app.host.progress_toasts();
    assert_eq!(shown.len(), 3, "{shown:?}");
    assert!(shown.iter().all(|toast| toast.id == toast_id));
    // Al arrancar: "Cancel", todavía sin avance.
    assert_eq!(shown[1].fraction, None);
    let cancel = shown[1].cancel.clone().expect("cancel action");
    assert_eq!(cancel.label, "Cancel");
    assert!((shown[2].fraction.unwrap() - 0.45).abs() < 0.001);
    assert_eq!(
        shown[2].description.as_deref(),
        Some("Receiving objects 450/1,000 · 2.00 MiB/s")
    );
    assert_eq!(notifications.get(), 0);

    // "Cancel" pide cancelar la operación (que aquí ya no existe).
    let ToastActionKind::Custom(key) = cancel.kind else {
        panic!("custom action");
    };
    app.model.update(cx, |model, cx| {
        assert!(model.run_toast_action(&key, cx));
    });
    app.wait_for(cx, "the cancel answer", |_| {
        !app.host.messages(NotifyType::Warning).is_empty()
    });

    // Terminada la operación, las actualizaciones tardías no pintan nada.
    app.model.update(cx, |model, cx| {
        model.end_progress(&toast_id);
        model.handle_backend_event(update("Resolving deltas", 1000, None), cx);
    });
    assert_eq!(app.host.progress_toasts().len(), 3);
}

/// Un pull que se detiene con conflictos lo dice con el número de archivos,
/// los nombra y ofrece "Resolve" (Local Changes) y "Abort merge" (la
/// operación de abortar de siempre, con su confirmación).
#[gpui_kit::test]
async fn a_conflicted_pull_offers_to_resolve_or_abort_the_merge(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.commit_file("shared.txt", "base\n", "Base");
    let origin = repo.publish_to_origin();
    let colleague = repo.colleague(&origin);
    colleague.commit_and_push("shared.txt", "theirs\n", "Their change");
    repo.commit_file("shared.txt", "mine\n", "My change");
    app.seed(&repo, cx);
    let path = repo.path_string();

    let result = app
        .model
        .update(cx, |model, cx| {
            let options = PullOptions {
                remote: Some("origin".to_string()),
                branch: Some("main".to_string()),
                ..Default::default()
            };
            model.pull(&path, options, cx)
        })
        .await;
    assert!(!result.success());
    assert!(repo.path.join(".git/MERGE_HEAD").exists());

    let toast = app
        .host
        .toasts()
        .pop()
        .expect("the pull announced its result");
    assert_eq!(toast.kind, NotifyType::Error);
    assert_eq!(toast.title, "Pull stopped: 1 conflict");
    assert_eq!(toast.description.as_deref(), Some("shared.txt"));
    let action = toast.action.expect("Resolve");
    let cancel = toast.cancel.expect("Abort merge");
    assert_eq!(action.label, "Resolve");
    assert_eq!(cancel.label, "Abort merge");
    let key = |action: mergeon_model::notify::ToastAction| match action.kind {
        mergeon_model::notify::ToastActionKind::Custom(key) => key,
        other => panic!("expected a custom action, got {other:?}"),
    };
    let (resolve, abort) = (key(action), key(cancel));

    assert!(
        app.model
            .update(cx, |model, cx| model.run_toast_action(&resolve, cx))
    );
    app.model.read_with(cx, |model, _| {
        assert_eq!(
            model.active_view(),
            mergeon_model::workspace_tabs::WorkspaceSection::Changes
        );
    });

    app.host.answer_confirm(true);
    assert!(
        app.model
            .update(cx, |model, cx| model.run_toast_action(&abort, cx))
    );
    app.wait_for(cx, "the aborted merge", |_| {
        !repo.path.join(".git/MERGE_HEAD").exists()
    });
    assert_eq!(
        app.host.confirms()[0].title.as_deref(),
        Some("Abort operation")
    );
    assert_eq!(repo.git(&["status", "--porcelain"]).trim(), "");
}

/// Resolver el último conflicto no avisa: el banner de la operación ya dice
/// "All conflicts resolved. Ready to continue merge." y el aviso taparía sus
/// botones Continue / Abort. El "Resolved …" anterior se retira.
#[gpui_kit::test]
async fn resolving_the_last_conflict_leaves_the_operation_banner_uncovered(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "base\n", "Base a");
    repo.commit_file("b.txt", "base\n", "Base b");
    repo.git(&["checkout", "-b", "feature"]);
    repo.commit_file("a.txt", "feature\n", "Feature a");
    repo.commit_file("b.txt", "feature\n", "Feature b");
    repo.git(&["checkout", "main"]);
    repo.commit_file("a.txt", "main\n", "Main a");
    repo.commit_file("b.txt", "main\n", "Main b");
    // `git merge` sale con error al quedar en conflicto: es lo que se busca.
    let _ = mergeon_gitcmd::Git::new(&repo.path).raw(["merge", "feature"]);
    app.open(&repo, cx).await;
    let path = repo.path_string();
    let toast = ToastId::Named(format!("operation:resolve-conflict:{path}"));

    let first = app
        .model
        .update(cx, |model, cx| {
            model.resolve_conflict(&path, "a.txt", "resolved\n", cx)
        })
        .await;
    assert!(first.success(), "{}", first.message());
    assert_eq!(app.host.messages(NotifyType::Success), ["Resolved a.txt."]);
    assert!(!app.host.dismissed_toasts().contains(&toast));

    let last = app
        .model
        .update(cx, |model, cx| {
            model.resolve_conflict(&path, "b.txt", "resolved\n", cx)
        })
        .await;
    assert!(last.success(), "{}", last.message());
    assert!(repo.path.join(".git/MERGE_HEAD").exists());
    // Nada nuevo, y el aviso del primero se retira.
    assert_eq!(app.host.messages(NotifyType::Success), ["Resolved a.txt."]);
    assert!(app.host.dismissed_toasts().contains(&toast));
    // Es justo cuando el banner pasa a "Ready to continue".
    let status = last.to_operation_result().status.expect("status");
    assert!(status.conflict_state.ready_to_continue);
}
