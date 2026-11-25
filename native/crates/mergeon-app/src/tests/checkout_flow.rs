//! `useCheckoutFlow` no tenía tests propios en el original; estos cubren sus
//! tres modos contra git real.

use gpui_kit::TestAppContext;
use mergeon_model::notify::NotifyType;

use super::support::{TestApp, TestRepo};
use crate::{CheckoutOutcome, CheckoutRequest, CheckoutTarget, FlowStatus, LocalChangesMode};

/// Repositorio con una rama `feature` donde README es distinto, y un cambio
/// local en README que impide cambiar de rama sin más.
fn repository_with_blocking_change(app: &TestApp, cx: &mut TestAppContext) -> (TestRepo, String) {
    let repo = TestRepo::new();
    repo.git(&["checkout", "-b", "feature"]);
    repo.commit_file("README.md", "# Feature\n", "Feature README");
    repo.git(&["checkout", "main"]);
    repo.write("README.md", "# Demo\n\nlocal change\n");
    let repo_id = app.seed(&repo, cx);
    (repo, repo_id)
}

async fn checkout(
    app: &TestApp,
    repo_id: &str,
    target: CheckoutTarget,
    mode: LocalChangesMode,
    cx: &mut TestAppContext,
) -> CheckoutOutcome {
    app.model
        .update(cx, |model, cx| {
            model.run_checkout(
                repo_id,
                CheckoutRequest {
                    label: "feature".to_string(),
                    target,
                    local_changes_mode: mode,
                },
                cx,
            )
        })
        .await
}

fn feature() -> CheckoutTarget {
    CheckoutTarget::Reference("feature".to_string())
}

#[gpui_kit::test]
async fn a_plain_checkout_blocked_by_local_changes_fails_and_keeps_them(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let (repo, repo_id) = repository_with_blocking_change(&app, cx);

    let outcome = checkout(&app, &repo_id, feature(), LocalChangesMode::Keep, cx).await;

    assert_eq!(outcome.status, FlowStatus::Failed);
    assert!(!outcome.stashed_changes);
    assert_eq!(repo.current_branch(), "main");
    assert!(repo.git(&["status", "--porcelain"]).contains("README.md"));
}

#[gpui_kit::test]
async fn discards_local_changes_before_switching(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let (repo, repo_id) = repository_with_blocking_change(&app, cx);

    let outcome = checkout(&app, &repo_id, feature(), LocalChangesMode::Discard, cx).await;

    assert_eq!(outcome.status, FlowStatus::Completed);
    assert_eq!(repo.current_branch(), "feature");
    assert_eq!(repo.git(&["status", "--porcelain"]).trim(), "");
    app.model.read_with(cx, |model, _| {
        let repository = model.repository(repo_id.as_str()).unwrap();
        assert_eq!(
            repository.last_mutation_operation.as_deref(),
            Some("checkout")
        );
    });
}

#[gpui_kit::test]
async fn stashes_switches_and_warns_when_the_stash_no_longer_applies(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let (repo, repo_id) = repository_with_blocking_change(&app, cx);

    let outcome = checkout(&app, &repo_id, feature(), LocalChangesMode::Stash, cx).await;

    // El checkout entra; el cambio guardado choca con el README de la rama y
    // se queda en la lista de stashes.
    assert_eq!(outcome.status, FlowStatus::Completed);
    assert!(outcome.stashed_changes);
    assert_eq!(repo.current_branch(), "feature");
    assert!(
        repo.git(&["stash", "list"])
            .contains("WIP: switching to feature")
    );
    assert!(app.host.has_message(
        NotifyType::Warning,
        "Your stashed changes could not be restored automatically. They are still in the stash list."
    ));
}

#[gpui_kit::test]
async fn stashes_and_restores_changes_that_do_not_conflict(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.git(&["branch", "feature"]);
    repo.write("README.md", "# Demo\n\nlocal change\n");
    let repo_id = app.seed(&repo, cx);

    let outcome = checkout(&app, &repo_id, feature(), LocalChangesMode::Stash, cx).await;

    assert_eq!(outcome.status, FlowStatus::Completed);
    assert!(outcome.stashed_changes);
    assert_eq!(repo.current_branch(), "feature");
    assert_eq!(repo.git(&["stash", "list"]).trim(), "");
    assert!(repo.git(&["status", "--porcelain"]).contains("README.md"));
    assert!(app.host.messages(NotifyType::Warning).is_empty());
}

#[gpui_kit::test]
async fn returns_the_stash_when_the_checkout_fails(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    repo.write("README.md", "# Demo\n\nlocal change\n");
    let repo_id = app.seed(&repo, cx);

    let outcome = checkout(
        &app,
        &repo_id,
        CheckoutTarget::Reference("does-not-exist".to_string()),
        LocalChangesMode::Stash,
        cx,
    )
    .await;

    assert_eq!(outcome.status, FlowStatus::Failed);
    assert!(outcome.stashed_changes);
    assert_eq!(repo.current_branch(), "main");
    assert_eq!(repo.git(&["stash", "list"]).trim(), "");
    assert!(repo.git(&["status", "--porcelain"]).contains("README.md"));
}

#[gpui_kit::test]
async fn checks_out_remote_branches_and_tags(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    let origin = repo.publish_to_origin();
    repo.git(&["tag", "v1.0.0"]);
    let colleague = repo.colleague(&origin);
    super::support::git_in(&colleague.path, &["checkout", "-b", "shared"]);
    super::support::git_in(&colleague.path, &["push", "origin", "shared"]);
    repo.git(&["fetch", "origin"]);
    let repo_id = app.seed(&repo, cx);

    let remote = checkout(
        &app,
        &repo_id,
        CheckoutTarget::RemoteBranch("origin/shared".to_string()),
        LocalChangesMode::Keep,
        cx,
    )
    .await;
    assert_eq!(remote.status, FlowStatus::Completed);
    assert_eq!(repo.current_branch(), "shared");

    let tag = checkout(
        &app,
        &repo_id,
        CheckoutTarget::Tag("v1.0.0".to_string()),
        LocalChangesMode::Keep,
        cx,
    )
    .await;
    assert_eq!(tag.status, FlowStatus::Completed);
    assert_eq!(repo.current_branch(), "HEAD");
}

#[gpui_kit::test]
async fn aborts_when_the_repository_is_not_open(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);

    let outcome = checkout(&app, "missing", feature(), LocalChangesMode::Keep, cx).await;

    assert_eq!(outcome.status, FlowStatus::Aborted);
}
