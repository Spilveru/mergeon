//! Deshacer y rehacer desde el modelo, con git real: cada tipo de operación,
//! la invalidación por un cambio externo, el `--keep` que se niega, los
//! commits publicados y los descartes recuperados.

use gpui_kit::TestAppContext;
use mergeon_backend::{
    ApplyPatchOptions, CommitOptions, MergeOptions, PullOptions, RebaseActionParams, RebaseOptions,
};
use mergeon_model::notify::NotifyType;

use super::support::{TestApp, TestRepo};
use crate::{CheckoutRequest, CheckoutTarget, LocalChangesMode};

struct Open {
    app: TestApp,
    repo: TestRepo,
    path: String,
}

impl Open {
    async fn new(repo: TestRepo, cx: &mut TestAppContext) -> Self {
        let app = TestApp::new(cx);
        let id = app.open(&repo, cx).await;
        let path = app.model.read_with(cx, |model, _| {
            model
                .repository(id.as_str())
                .expect("repositorio")
                .path
                .clone()
        });
        Self { app, repo, path }
    }

    async fn run(&self, redo: bool, cx: &mut TestAppContext) -> bool {
        let done = self
            .app
            .model
            .update(cx, |model, cx| model.run_undo(&self.path, redo, cx))
            .await;
        cx.run_until_parked();
        done
    }

    async fn undo(&self, cx: &mut TestAppContext) -> bool {
        self.run(false, cx).await
    }

    async fn redo(&self, cx: &mut TestAppContext) -> bool {
        self.run(true, cx).await
    }

    fn label(&self, redo: bool, cx: &mut TestAppContext) -> Option<String> {
        self.app
            .model
            .read_with(cx, |model, _| model.undo_action_label(&self.path, redo))
    }

    fn history(&self, redo: bool, cx: &mut TestAppContext) -> Vec<String> {
        self.app
            .model
            .read_with(cx, |model, _| model.undo_history(&self.path, redo))
    }

    fn status(&self) -> String {
        self.repo
            .git(&["status", "--porcelain"])
            .trim_end()
            .to_string()
    }

    fn warnings(&self) -> Vec<String> {
        self.app.host.messages(NotifyType::Warning)
    }
}

async fn commit(open: &Open, message: &str, options: CommitOptions, cx: &mut TestAppContext) {
    let result = open
        .app
        .model
        .update(cx, |model, cx| {
            model.commit(&open.path, message, options, false, cx)
        })
        .await;
    assert!(result.success(), "{}", result.message());
    cx.run_until_parked();
}

#[gpui_kit::test]
async fn undoing_a_commit_puts_its_changes_back_in_the_index_and_redo_recommits(
    cx: &mut TestAppContext,
) {
    let open = Open::new(TestRepo::new(), cx).await;
    let before = open.repo.head();
    open.repo.write("login.rs", "fn login() {}\n");
    open.repo.git(&["add", "login.rs"]);
    commit(&open, "Fix login\n\nbody", CommitOptions::default(), cx).await;
    let after = open.repo.head();

    assert_eq!(
        open.label(false, cx).as_deref(),
        Some("Undo commit 'Fix login'")
    );
    assert_eq!(open.label(true, cx), None);
    open.app
        .model
        .read_with(cx, |model, _| assert!(model.can_undo(false)));

    assert!(open.undo(cx).await);
    assert_eq!(open.repo.head(), before);
    assert_eq!(open.status(), "A  login.rs");
    assert_eq!(
        open.label(true, cx).as_deref(),
        Some("Redo commit 'Fix login'")
    );
    assert!(
        open.app
            .host
            .has_message(NotifyType::Success, "Undid commit 'Fix login'.")
    );
    // En el log de operaciones, como cualquier mutación.
    open.app.model.read_with(cx, |model, _| {
        assert!(
            model
                .operations_log()
                .entries
                .iter()
                .any(|entry| entry.operation.as_deref() == Some("undo")
                    && entry.outcome.as_deref() == Some("completed"))
        );
    });

    assert!(open.redo(cx).await);
    assert_eq!(open.repo.head(), after);
    assert_eq!(open.status(), "");
    assert_eq!(open.history(false, cx), ["commit 'Fix login'"]);
}

#[gpui_kit::test]
async fn an_amend_and_a_fixup_go_back_to_the_commit_they_replaced(cx: &mut TestAppContext) {
    let open = Open::new(TestRepo::new(), cx).await;
    let original = open.repo.commit_file("a.txt", "a\n", "Add a");
    open.repo.write("a.txt", "a, amended\n");
    open.repo.git(&["add", "a.txt"]);
    commit(
        &open,
        "Add a (amended)",
        CommitOptions {
            amend: true,
            ..Default::default()
        },
        cx,
    )
    .await;
    assert_eq!(
        open.label(false, cx).as_deref(),
        Some("Undo amend of 'Add a (amended)'")
    );
    assert!(open.undo(cx).await);
    assert_eq!(open.repo.head(), original);
    assert_eq!(open.status(), "M  a.txt");

    commit(
        &open,
        "",
        CommitOptions {
            fixup: Some(original.clone()),
            ..Default::default()
        },
        cx,
    )
    .await;
    assert_eq!(open.repo.log_subjects()[0], "fixup! Add a");
    assert_eq!(open.label(false, cx).as_deref(), Some("Undo fixup commit"));
    // El commit nuevo vació la pila de rehacer.
    assert_eq!(open.label(true, cx), None);
    assert!(open.undo(cx).await);
    assert_eq!(open.repo.head(), original);
}

#[gpui_kit::test]
async fn undoing_a_merge_goes_back_to_before_it(cx: &mut TestAppContext) {
    let repo = TestRepo::new();
    repo.git(&["checkout", "-b", "feature/x"]);
    repo.commit_file("feature.txt", "feature\n", "Feature work");
    repo.git(&["checkout", "main"]);
    let before = repo.commit_file("main.txt", "main\n", "Main work");
    let open = Open::new(repo, cx).await;

    let result = open
        .app
        .model
        .update(cx, |model, cx| {
            model.merge(&open.path, "feature/x", MergeOptions::default(), false, cx)
        })
        .await;
    assert!(result.success(), "{}", result.message());
    cx.run_until_parked();
    let merged = open.repo.head();
    assert_ne!(merged, before);
    assert_eq!(
        open.label(false, cx).as_deref(),
        Some("Undo merge of feature/x")
    );

    assert!(open.undo(cx).await);
    assert_eq!(open.repo.head(), before);
    assert!(!open.repo.path.join("feature.txt").exists());
    assert_eq!(open.status(), "");

    assert!(open.redo(cx).await);
    assert_eq!(open.repo.head(), merged);
    assert!(open.repo.path.join("feature.txt").exists());
}

#[gpui_kit::test]
async fn a_merge_stopped_by_conflicts_and_committed_undoes_as_one_merge(cx: &mut TestAppContext) {
    let repo = TestRepo::new();
    repo.git(&["checkout", "-b", "feature"]);
    repo.commit_file("README.md", "# Feature\n", "Feature README");
    repo.git(&["checkout", "main"]);
    let before = repo.commit_file("README.md", "# Main\n", "Main README");
    let open = Open::new(repo, cx).await;

    let result = open
        .app
        .model
        .update(cx, |model, cx| {
            model.merge(&open.path, "feature", MergeOptions::default(), false, cx)
        })
        .await;
    assert!(!result.success());
    cx.run_until_parked();
    assert_eq!(open.label(false, cx), None);

    open.repo.write("README.md", "# Both\n");
    open.repo.git(&["add", "README.md"]);
    // La app cierra el merge con "Continue".
    let result = open
        .app
        .model
        .update(cx, |model, cx| {
            model.sequence_action(&open.path, "continue", cx)
        })
        .await;
    assert!(result.success(), "{}", result.message());
    cx.run_until_parked();
    assert_eq!(
        open.label(false, cx).as_deref(),
        Some("Undo merge of feature")
    );
    assert!(open.undo(cx).await);
    assert_eq!(open.repo.head(), before);
    assert_eq!(open.status(), "");
}

#[gpui_kit::test]
async fn rebase_pull_reset_cherry_pick_and_revert_can_be_undone(cx: &mut TestAppContext) {
    let repo = TestRepo::new();
    let origin = repo.publish_to_origin();
    repo.git(&["checkout", "-b", "topic"]);
    let topic = repo.commit_file("topic.txt", "topic\n", "Topic work");
    repo.git(&["checkout", "main"]);
    let main = repo.commit_file("main.txt", "main\n", "Main work");
    let open = Open::new(repo, cx).await;

    // Cherry-pick (uno) y revert.
    let result = open
        .app
        .model
        .update(cx, |model, cx| model.cherry_pick(&open.path, &topic, cx))
        .await;
    assert!(result.success(), "{}", result.message());
    cx.run_until_parked();
    assert_eq!(
        open.label(false, cx),
        Some(format!("Undo cherry-pick of {}", &topic[..7]))
    );
    assert!(open.undo(cx).await);
    assert_eq!(open.repo.head(), main);

    let result = open
        .app
        .model
        .update(cx, |model, cx| model.revert(&open.path, &main, cx))
        .await;
    assert!(result.success(), "{}", result.message());
    cx.run_until_parked();
    assert!(open.undo(cx).await);
    assert_eq!(open.repo.head(), main);

    // Reset duro: se deshace con --keep.
    let result = open
        .app
        .model
        .update(cx, |model, cx| {
            model.reset(&open.path, "HEAD~1", "hard", cx)
        })
        .await;
    assert!(result.success(), "{}", result.message());
    cx.run_until_parked();
    assert!(open.undo(cx).await);
    assert_eq!(open.repo.head(), main);

    // Rebase de topic sobre main.
    open.repo.git(&["checkout", "topic"]);
    let result = open
        .app
        .model
        .update(cx, |model, cx| {
            model.rebase(&open.path, "main", RebaseOptions::default(), cx)
        })
        .await;
    assert!(result.success(), "{}", result.message());
    cx.run_until_parked();
    assert_eq!(
        open.label(false, cx).as_deref(),
        Some("Undo rebase onto main")
    );
    assert!(open.undo(cx).await);
    assert_eq!(open.repo.head(), topic);

    // Acción de rebase (reword).
    let result = open
        .app
        .model
        .update(cx, |model, cx| {
            model.rebase_action(
                &open.path,
                "reword",
                &topic,
                RebaseActionParams {
                    new_message: Some("Topic, reworded".into()),
                },
                cx,
            )
        })
        .await;
    assert!(result.success(), "{}", result.message());
    cx.run_until_parked();
    assert_eq!(open.repo.log_subjects()[0], "Topic, reworded");
    assert!(open.undo(cx).await);
    assert_eq!(open.repo.head(), topic);

    // Pull (fast-forward) de lo que publicó otro.
    open.repo.git(&["checkout", "main"]);
    open.repo.git(&["reset", "--hard", "origin/main"]);
    let colleague = open.repo.colleague(&origin);
    colleague.commit_and_push("remote.txt", "remote\n", "Remote work");
    let before_pull = open.repo.head();
    let result = open
        .app
        .model
        .update(cx, |model, cx| {
            model.pull(&open.path, PullOptions::default(), cx)
        })
        .await;
    assert!(result.success(), "{}", result.message());
    cx.run_until_parked();
    assert_ne!(open.repo.head(), before_pull);
    assert!(open.undo(cx).await);
    assert_eq!(open.repo.head(), before_pull);
}

#[gpui_kit::test]
async fn checkout_and_branch_operations_can_be_undone(cx: &mut TestAppContext) {
    let repo = TestRepo::new();
    repo.git(&["branch", "feature"]);
    let open = Open::new(repo, cx).await;

    // Checkout por el flujo de la app.
    let outcome = open
        .app
        .model
        .update(cx, |model, cx| {
            model.run_checkout(
                open.path.as_str(),
                CheckoutRequest {
                    label: "feature".into(),
                    target: CheckoutTarget::Reference("feature".into()),
                    local_changes_mode: LocalChangesMode::Keep,
                },
                cx,
            )
        })
        .await;
    assert_eq!(outcome.status, crate::FlowStatus::Completed);
    cx.run_until_parked();
    assert_eq!(
        open.label(false, cx).as_deref(),
        Some("Undo checkout of 'feature'")
    );
    assert!(open.undo(cx).await);
    assert_eq!(open.repo.current_branch(), "main");
    assert!(open.redo(cx).await);
    assert_eq!(open.repo.current_branch(), "feature");
    assert!(open.undo(cx).await);

    // Crear (con checkout), renombrar y borrar.
    let result = open
        .app
        .model
        .update(cx, |model, cx| {
            model.create_branch(&open.path, "foo", None, true, cx)
        })
        .await;
    assert!(result.success(), "{}", result.message());
    cx.run_until_parked();
    assert!(open.undo(cx).await);
    assert_eq!(open.repo.current_branch(), "main");
    assert!(
        open.repo
            .git(&["branch", "--list", "foo"])
            .trim()
            .is_empty()
    );
    assert!(open.redo(cx).await);
    assert_eq!(open.repo.current_branch(), "foo");
    open.repo.git(&["checkout", "main"]);

    let result = open
        .app
        .model
        .update(cx, |model, cx| {
            model.rename_branch(&open.path, "foo", "bar", cx)
        })
        .await;
    assert!(result.success(), "{}", result.message());
    cx.run_until_parked();
    assert!(open.undo(cx).await);
    assert!(
        !open
            .repo
            .git(&["branch", "--list", "foo"])
            .trim()
            .is_empty()
    );

    let sha = open.repo.git(&["rev-parse", "foo"]).trim().to_string();
    let result = open
        .app
        .model
        .update(cx, |model, cx| model.delete_branch(&open.path, "foo", cx))
        .await;
    assert!(result.success(), "{}", result.message());
    cx.run_until_parked();
    assert_eq!(
        open.label(false, cx).as_deref(),
        Some("Undo delete branch 'foo'")
    );
    assert!(open.undo(cx).await);
    assert_eq!(open.repo.git(&["rev-parse", "foo"]).trim(), sha);
}

#[gpui_kit::test]
async fn tags_come_back_even_when_annotated(cx: &mut TestAppContext) {
    let open = Open::new(TestRepo::new(), cx).await;
    let result = open
        .app
        .model
        .update(cx, |model, cx| {
            model.create_tag(&open.path, "v1.0", None, Some("First release"), cx)
        })
        .await;
    assert!(result.success(), "{}", result.message());
    cx.run_until_parked();
    let object = open
        .repo
        .git(&["rev-parse", "refs/tags/v1.0"])
        .trim()
        .to_string();
    assert!(open.undo(cx).await);
    assert!(open.repo.git(&["tag", "-l"]).trim().is_empty());
    assert!(open.redo(cx).await);

    let result = open
        .app
        .model
        .update(cx, |model, cx| model.delete_tag(&open.path, "v1.0", cx))
        .await;
    assert!(result.success(), "{}", result.message());
    cx.run_until_parked();
    assert_eq!(
        open.label(false, cx).as_deref(),
        Some("Undo delete tag 'v1.0'")
    );
    assert!(open.undo(cx).await);
    assert_eq!(
        open.repo.git(&["rev-parse", "refs/tags/v1.0"]).trim(),
        object
    );
    assert_eq!(open.repo.git(&["cat-file", "-t", "v1.0"]).trim(), "tag");
}

#[gpui_kit::test]
async fn stash_push_pop_and_drop_can_be_undone(cx: &mut TestAppContext) {
    let open = Open::new(TestRepo::new(), cx).await;
    open.repo.write("README.md", "# Demo\n\nwip\n");

    let result = open
        .app
        .model
        .update(cx, |model, cx| model.stash(&open.path, Some("wip"), cx))
        .await;
    assert!(result.success(), "{}", result.message());
    cx.run_until_parked();
    assert_eq!(open.status(), "");
    assert!(open.undo(cx).await);
    assert_eq!(open.status(), " M README.md");
    assert!(open.repo.git(&["stash", "list"]).trim().is_empty());
    assert!(open.redo(cx).await);
    assert_eq!(open.status(), "");
    assert_eq!(open.repo.git(&["stash", "list"]).lines().count(), 1);

    // Pop: deshacerlo devuelve el stash a la lista y limpia lo aplicado.
    let result = open
        .app
        .model
        .update(cx, |model, cx| model.pop_stash(&open.path, "stash@{0}", cx))
        .await;
    assert!(result.success(), "{}", result.message());
    cx.run_until_parked();
    assert!(open.undo(cx).await);
    assert_eq!(open.status(), "");
    assert_eq!(open.repo.git(&["stash", "list"]).lines().count(), 1);

    // Drop: vuelve con su mensaje.
    let result = open
        .app
        .model
        .update(cx, |model, cx| {
            model.drop_stash(&open.path, "stash@{0}", cx)
        })
        .await;
    assert!(result.success(), "{}", result.message());
    cx.run_until_parked();
    assert!(open.undo(cx).await);
    assert_eq!(
        open.repo.git(&["stash", "list", "--format=%gs"]).trim(),
        "On main: wip"
    );
}

#[gpui_kit::test]
async fn discarded_files_come_back_including_untracked_ones(cx: &mut TestAppContext) {
    let open = Open::new(TestRepo::new(), cx).await;
    open.repo.write("README.md", "# Demo\n\nlocal edit\n");
    open.repo.write("scratch/notes.txt", "untracked notes\n");
    let before = open.status();

    let result = open
        .app
        .model
        .update(cx, |model, cx| {
            model.discard_changes(&open.path, "scratch/notes.txt", cx)
        })
        .await;
    assert!(result.success(), "{}", result.message());
    cx.run_until_parked();
    assert!(!open.repo.path.join("scratch/notes.txt").exists());
    assert_eq!(
        open.label(false, cx).as_deref(),
        Some("Undo discard of changes in scratch/notes.txt")
    );
    assert!(open.undo(cx).await);
    assert_eq!(
        std::fs::read_to_string(open.repo.path.join("scratch/notes.txt")).unwrap(),
        "untracked notes\n"
    );

    let result = open
        .app
        .model
        .update(cx, |model, cx| model.discard_all(&open.path, None, cx))
        .await;
    assert!(result.success(), "{}", result.message());
    cx.run_until_parked();
    assert_eq!(open.status(), "");
    assert!(open.undo(cx).await);
    assert_eq!(open.status(), before);
    assert_eq!(
        std::fs::read_to_string(open.repo.path.join("README.md")).unwrap(),
        "# Demo\n\nlocal edit\n"
    );

    // Rehacer vuelve a descartar (y la copia se reutiliza).
    assert!(open.redo(cx).await);
    assert_eq!(open.status(), "");
}

#[gpui_kit::test]
async fn discarded_lines_come_back(cx: &mut TestAppContext) {
    let open = Open::new(TestRepo::new(), cx).await;
    open.repo.write("README.md", "# Demo\nadded line\n");
    let patch = open.repo.git(&["diff", "--", "README.md"]);

    let result = open
        .app
        .model
        .update(cx, |model, cx| {
            model.apply_patch(
                &open.path,
                &patch,
                ApplyPatchOptions {
                    reverse: true,
                    worktree: true,
                    file_path: Some("README.md".into()),
                },
                cx,
            )
        })
        .await;
    assert!(result.success(), "{}", result.message());
    cx.run_until_parked();
    assert_eq!(open.status(), "");
    assert_eq!(
        open.label(false, cx).as_deref(),
        Some("Undo discard of selected changes in README.md")
    );
    assert!(open.undo(cx).await);
    assert_eq!(
        std::fs::read_to_string(open.repo.path.join("README.md")).unwrap(),
        "# Demo\nadded line\n"
    );
}

#[gpui_kit::test]
async fn an_external_commit_invalidates_the_history(cx: &mut TestAppContext) {
    let open = Open::new(TestRepo::new(), cx).await;
    open.repo.write("a.txt", "a\n");
    open.repo.git(&["add", "a.txt"]);
    commit(&open, "In the app", CommitOptions::default(), cx).await;
    // Otro commit desde la terminal.
    let external = open.repo.commit_file("b.txt", "b\n", "In the terminal");

    assert!(!open.undo(cx).await);
    assert_eq!(open.repo.head(), external);
    assert_eq!(open.label(false, cx), None);
    assert!(
        open.warnings()
            .iter()
            .any(|warning| warning.contains("changed outside Mergeon")),
        "{:?}",
        open.warnings()
    );
}

#[gpui_kit::test]
async fn keep_refuses_to_overwrite_local_changes_and_keeps_the_entry(cx: &mut TestAppContext) {
    let repo = TestRepo::new();
    repo.git(&["checkout", "-b", "feature"]);
    repo.commit_file("README.md", "# Feature\n", "Feature README");
    repo.git(&["checkout", "main"]);
    let open = Open::new(repo, cx).await;
    let result = open
        .app
        .model
        .update(cx, |model, cx| {
            model.merge(&open.path, "feature", MergeOptions::default(), false, cx)
        })
        .await;
    assert!(result.success(), "{}", result.message());
    cx.run_until_parked();
    let merged = open.repo.head();
    open.repo.write("README.md", "# Local edit\n");

    assert!(!open.undo(cx).await);
    assert_eq!(open.repo.head(), merged);
    assert_eq!(
        std::fs::read_to_string(open.repo.path.join("README.md")).unwrap(),
        "# Local edit\n"
    );
    assert!(
        open.warnings()
            .iter()
            .any(|warning| warning.contains("README.md") && warning.contains("Commit, stash")),
        "{:?}",
        open.warnings()
    );
    // La entrada sigue: tras guardar los cambios se puede deshacer.
    assert_eq!(
        open.label(false, cx).as_deref(),
        Some("Undo merge of feature")
    );
    open.repo.git(&["checkout", "--", "README.md"]);
    assert!(open.undo(cx).await);
}

#[gpui_kit::test]
async fn undoing_published_commits_asks_first(cx: &mut TestAppContext) {
    let repo = TestRepo::new();
    repo.publish_to_origin();
    let open = Open::new(repo, cx).await;
    let before = open.repo.head();
    open.repo.write("a.txt", "a\n");
    open.repo.git(&["add", "a.txt"]);
    commit(&open, "Published", CommitOptions::default(), cx).await;
    open.repo.git(&["push", "origin", "main"]);

    open.app.host.answer_confirm(false);
    assert!(!open.undo(cx).await);
    let confirms = open.app.host.confirms();
    let description = confirms
        .last()
        .and_then(|confirm| confirm.description.clone())
        .unwrap_or_default();
    assert!(description.contains("origin/main"), "{description}");
    assert!(description.contains("force push"), "{description}");
    assert_ne!(open.repo.head(), before);

    open.app.host.answer_confirm(true);
    assert!(open.undo(cx).await);
    assert_eq!(open.repo.head(), before);
}

#[gpui_kit::test]
async fn there_is_no_undo_while_a_merge_is_in_progress(cx: &mut TestAppContext) {
    let repo = TestRepo::new();
    repo.git(&["checkout", "-b", "feature"]);
    repo.commit_file("README.md", "# Feature\n", "Feature README");
    repo.git(&["checkout", "main"]);
    repo.write("x.txt", "x\n");
    repo.git(&["add", "x.txt"]);
    let open = Open::new(repo, cx).await;
    commit(&open, "Before", CommitOptions::default(), cx).await;
    open.repo
        .commit_file("README.md", "# Main\n", "Main README");
    let result = open
        .app
        .model
        .update(cx, |model, cx| {
            model.merge(&open.path, "feature", MergeOptions::default(), false, cx)
        })
        .await;
    // Lo que hacen las vistas con el resultado.
    open.app.model.update(cx, |model, cx| {
        let id = model.active_repository().unwrap().id.clone();
        model.apply_repository_operation_result(id.as_str(), &result, cx);
        model
            .sync_repository_read_model(id.as_str(), crate::SyncRequest::new().status(true), cx)
            .detach();
    });
    open.app.wait_for(cx, "el merge a medias", |model| {
        model
            .active_repository()
            .and_then(|repo| repo.operation_state.as_ref())
            .is_some_and(|state| state.in_progress)
    });

    assert!(!open.undo(cx).await);
    let info = open.app.host.messages(NotifyType::Info);
    assert!(
        info.iter().any(|message| message.contains("in progress")),
        "{info:?}"
    );
    assert!(
        info.iter()
            .all(|message| !message.contains("in progress in progress")),
        "{info:?}"
    );
    open.app
        .model
        .read_with(cx, |model, _| assert!(!model.can_undo(false)));
}

#[gpui_kit::test]
async fn external_head_sync_clears_stale_undo_stack_before_clicking_undo(cx: &mut TestAppContext) {
    let open = Open::new(TestRepo::new(), cx).await;
    open.repo.write("a.txt", "a\n");
    open.repo.git(&["add", "a.txt"]);
    commit(&open, "In app", CommitOptions::default(), cx).await;
    assert!(open.label(false, cx).is_some());

    // Crear una rama ajena no mueve HEAD: la pila se conserva.
    open.repo.git(&["branch", "topic"]);
    open.app
        .model
        .update(cx, |model, cx| {
            model.sync_active_repository_status(crate::shell::StatusSyncReason::FsWatchHead, cx)
        })
        .await;
    assert!(open.label(false, cx).is_some());

    // Un commit en terminal sí mueve HEAD: al sincronizar, ↶ se apaga ya.
    open.repo.commit_file("b.txt", "b\n", "External commit");
    open.app
        .model
        .update(cx, |model, cx| {
            model.sync_active_repository_status(crate::shell::StatusSyncReason::FsWatchHead, cx)
        })
        .await;
    assert_eq!(open.label(false, cx), None);
}

#[gpui_kit::test]
async fn undoing_a_hard_reset_that_discarded_changes_notes_that_they_did_not_return(
    cx: &mut TestAppContext,
) {
    let open = Open::new(TestRepo::new(), cx).await;
    let first = open.repo.head();
    let second = open.repo.commit_file("a.txt", "a\n", "Second");
    open.repo.write("a.txt", "local wip\n");
    // Sincronizar el estado para que el modelo sepa del cambio local.
    let repo_id = open
        .app
        .model
        .read_with(cx, |model, _| model.active_repository().unwrap().id.clone());
    open.app
        .model
        .update(cx, |model, cx| {
            model.sync_repository_read_model(
                repo_id.as_str(),
                crate::SyncRequest::new().status(true),
                cx,
            )
        })
        .await;

    let result = open
        .app
        .model
        .update(cx, |model, cx| model.reset(&open.path, &first, "hard", cx))
        .await;
    assert!(result.success(), "{}", result.message());
    cx.run_until_parked();
    assert_eq!(open.repo.head(), first);

    assert!(open.undo(cx).await);
    assert_eq!(open.repo.head(), second);
    let successes = open.app.host.messages(NotifyType::Success);
    assert!(
        successes.iter().any(|msg| msg
            .contains("Uncommitted changes discarded by the hard reset were not restored.")),
        "{successes:?}"
    );
}

#[gpui_kit::test]
async fn the_shortcut_undoes_in_the_active_repository(cx: &mut TestAppContext) {
    let open = Open::new(TestRepo::new(), cx).await;
    let before = open.repo.head();
    open.repo.write("a.txt", "a\n");
    open.repo.git(&["add", "a.txt"]);
    commit(&open, "Via shortcut", CommitOptions::default(), cx).await;

    open.app.model.update(cx, |model, cx| {
        model.handle_shortcut(mergeon_model::keyboard_shortcuts::ShortcutAction::Undo, cx);
    });
    open.app.wait_for(cx, "el deshacer", |model| {
        model.undo_action_label(&open.path, true).is_some()
    });
    assert_eq!(open.repo.head(), before);

    open.app.model.update(cx, |model, cx| {
        model.handle_shortcut(mergeon_model::keyboard_shortcuts::ShortcutAction::Redo, cx);
    });
    open.app.wait_for(cx, "el rehacer", |model| {
        model.undo_action_label(&open.path, false).is_some()
    });
    assert_ne!(open.repo.head(), before);
}
