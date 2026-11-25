//! Port de `src/composables/useCreateBranchFlow.test.js`, contra git real.
//!
//! "notifies when the rollback pop itself fails" no se puede provocar con git
//! real (si la rama no se crea, el árbol queda como lo dejó el stash y el pop
//! siempre entra); se cubre el aviso hermano, el del pop que falla tras crear
//! la rama. "falls back to stash@{0} when the stash operation returns no ref"
//! está en los tests de `flows`.

use gpui_kit::TestAppContext;
use mergeon_model::notify::NotifyType;

use super::support::{TestApp, TestRepo};
use crate::{CreateBranchOutcome, CreateBranchRequest, FlowStatus, LocalChangesMode};

const POP_FAILED_AFTER_CREATE: &str = "The branch was created but your stashed changes could not be restored automatically. They are still in the stash list.";
const POP_FAILED_AFTER_ROLLBACK: &str = "The branch was not created and your stashed changes could not be restored automatically. They are still in the stash list.";

fn request(mode: LocalChangesMode) -> CreateBranchRequest {
    CreateBranchRequest {
        branch_name: "feature".to_string(),
        checkout_after_create: true,
        local_changes_mode: mode,
        base_reference: Some("main".to_string()),
    }
}

/// Repositorio abierto en el modelo con un cambio local sin commitear.
fn dirty_repository(app: &TestApp, cx: &mut TestAppContext) -> (TestRepo, String) {
    let repo = TestRepo::new();
    repo.write("README.md", "# Demo\n\nlocal change\n");
    let repo_id = app.seed(&repo, cx);
    (repo, repo_id)
}

async fn submit(
    app: &TestApp,
    repo_id: &str,
    request: CreateBranchRequest,
    cx: &mut TestAppContext,
) -> CreateBranchOutcome {
    app.model
        .update(cx, |model, cx| {
            model.submit_create_branch(repo_id, request, cx)
        })
        .await
}

fn stash_count(repo: &TestRepo) -> usize {
    repo.git(&["stash", "list"])
        .lines()
        .filter(|line| !line.is_empty())
        .count()
}

#[gpui_kit::test]
async fn creates_the_branch_without_touching_local_changes_in_keep_mode(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let (repo, repo_id) = dirty_repository(&app, cx);

    let outcome = submit(&app, &repo_id, request(LocalChangesMode::Keep), cx).await;

    assert_eq!(outcome.status, FlowStatus::Completed);
    assert!(!outcome.stashed_changes);
    assert_eq!(outcome.pop_result, None);
    assert_eq!(repo.current_branch(), "feature");
    assert_eq!(stash_count(&repo), 0);
    assert!(repo.git(&["status", "--porcelain"]).contains("README.md"));
    assert!(app.host.confirms().is_empty());
    // El resultado se aplicó al repositorio del modelo.
    app.model.read_with(cx, |model, _| {
        let repository = model.repository(repo_id.as_str()).unwrap();
        assert_eq!(
            repository.last_mutation_operation.as_deref(),
            Some("create-branch")
        );
    });
}

#[gpui_kit::test]
async fn stashes_creates_and_pops_using_the_captured_stash_ref(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let (repo, repo_id) = dirty_repository(&app, cx);
    // Un stash anterior: el flujo debe devolver el suyo, no este.
    repo.git(&["stash", "push", "-m", "older work"]);
    repo.write("README.md", "# Demo\n\nlocal change\n");

    let outcome = submit(&app, &repo_id, request(LocalChangesMode::Stash), cx).await;

    assert_eq!(outcome.status, FlowStatus::Completed);
    assert!(outcome.stashed_changes);
    assert!(outcome.pop_result.as_ref().is_some_and(|pop| pop.success()));
    assert_eq!(repo.current_branch(), "feature");
    assert!(repo.git(&["status", "--porcelain"]).contains("README.md"));
    let stashes = repo.git(&["stash", "list"]);
    assert_eq!(stash_count(&repo), 1);
    assert!(stashes.contains("older work"));
}

#[gpui_kit::test]
async fn rolls_back_the_stash_with_the_captured_ref_when_branch_creation_fails(
    cx: &mut TestAppContext,
) {
    let app = TestApp::new(cx);
    let (repo, repo_id) = dirty_repository(&app, cx);
    // La rama ya existe: crearla falla.
    repo.git(&["branch", "feature"]);

    let outcome = submit(&app, &repo_id, request(LocalChangesMode::Stash), cx).await;

    assert_eq!(outcome.status, FlowStatus::Failed);
    assert!(outcome.stashed_changes);
    assert!(
        outcome
            .branch_result
            .as_ref()
            .is_some_and(|result| !result.success())
    );
    assert!(outcome.pop_result.as_ref().is_some_and(|pop| pop.success()));
    assert_eq!(repo.current_branch(), "main");
    assert_eq!(stash_count(&repo), 0);
    assert!(repo.git(&["status", "--porcelain"]).contains("README.md"));
    assert!(
        !app.host
            .has_message(NotifyType::Error, POP_FAILED_AFTER_ROLLBACK)
    );
}

#[gpui_kit::test]
async fn warns_when_the_stash_cannot_be_restored_on_the_new_branch(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = TestRepo::new();
    // La rama nace de un commit donde README es distinto: el cambio local
    // guardado en el stash choca al devolverlo.
    let base = repo.head();
    repo.commit_file("README.md", "# Demo\n\ncommitted line\n", "Change README");
    repo.write("README.md", "# Demo\n\ncommitted line\nlocal change\n");
    let repo_id = app.seed(&repo, cx);

    let outcome = submit(
        &app,
        &repo_id,
        CreateBranchRequest {
            base_reference: Some(base),
            ..request(LocalChangesMode::Stash)
        },
        cx,
    )
    .await;

    assert_eq!(outcome.status, FlowStatus::Completed);
    assert!(outcome.stashed_changes);
    assert!(
        outcome
            .pop_result
            .as_ref()
            .is_some_and(|pop| !pop.success())
    );
    assert_eq!(repo.current_branch(), "feature");
    assert_eq!(stash_count(&repo), 1);
    assert!(
        app.host
            .has_message(NotifyType::Warning, POP_FAILED_AFTER_CREATE)
    );
}

#[gpui_kit::test]
async fn aborts_without_discarding_when_the_confirm_dialog_is_cancelled(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let (repo, repo_id) = dirty_repository(&app, cx);
    app.host.answer_confirm(false);

    let outcome = submit(&app, &repo_id, request(LocalChangesMode::Discard), cx).await;

    assert_eq!(outcome.status, FlowStatus::Aborted);
    assert_eq!(outcome.branch_result, None);
    assert_eq!(repo.current_branch(), "main");
    assert!(repo.git(&["status", "--porcelain"]).contains("README.md"));

    let confirms = app.host.confirms();
    assert_eq!(confirms.len(), 1);
    assert_eq!(confirms[0].title.as_deref(), Some("Discard local changes"));
    assert_eq!(
        confirms[0].description.as_deref(),
        Some("Discard 1 local change before creating 'feature'?")
    );
    assert_eq!(confirms[0].confirm_text.as_deref(), Some("Discard"));
    assert_eq!(confirms[0].variant.as_deref(), Some("destructive"));
}

#[gpui_kit::test]
async fn discards_local_changes_once_confirmed(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let (repo, repo_id) = dirty_repository(&app, cx);
    repo.write("extra.txt", "untracked\n");
    app.host.answer_confirm(true);

    let outcome = submit(&app, &repo_id, request(LocalChangesMode::Discard), cx).await;

    assert_eq!(outcome.status, FlowStatus::Completed);
    assert_eq!(repo.current_branch(), "feature");
    assert_eq!(repo.git(&["status", "--porcelain"]).trim(), "");
    assert_eq!(
        app.host.confirms()[0].description.as_deref(),
        Some("Discard 2 local changes before creating 'feature'?")
    );
}

#[gpui_kit::test]
async fn aborts_with_a_notification_when_inspecting_local_changes_fails(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let (repo, repo_id) = dirty_repository(&app, cx);
    repo.break_repository();

    let outcome = submit(&app, &repo_id, request(LocalChangesMode::Keep), cx).await;

    assert_eq!(outcome.status, FlowStatus::Aborted);
    assert_eq!(outcome.branch_result, None);
    let errors = app.host.messages(NotifyType::Error);
    assert_eq!(errors.len(), 1);
    assert!(errors[0].starts_with("Failed to check uncommitted changes"));
}

#[gpui_kit::test]
async fn aborts_when_the_repository_is_not_open(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);

    let outcome = submit(&app, "missing", request(LocalChangesMode::Keep), cx).await;

    assert_eq!(outcome.status, FlowStatus::Aborted);
    assert!(app.host.toasts().is_empty());
}
