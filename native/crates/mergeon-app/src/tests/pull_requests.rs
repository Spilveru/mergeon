//! Pull requests y checks de GitHub: estado por repositorio, con un GitHub
//! guionizado y repositorios git reales.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use gpui_kit::TestAppContext;
use mergeon_backend::Store;
use mergeon_github::{CheckState, HttpRequest, HttpResponse, ReviewDecision, TransportError};
use serde_json::{Value, json};

use super::support::{TestApp, TestRepo};
use crate::pull_requests::{PullRequestCheckout, PullRequestsAvailability};
use crate::testing::{FakeGitHubTransport, TestBackend};

type Response = Result<HttpResponse, TransportError>;

fn store_with_account() -> Store {
    let store = TestBackend::memory_store();
    let account = serde_json::from_value(json!({
        "id": "7",
        "username": "ana",
        "token": "ghp_secret",
        "isPersonalAccessToken": true
    }))
    .unwrap();
    store.save_github_account(&account).unwrap();
    store.set_active_github_account_id(Some("7")).unwrap();
    store
}

async fn signed_in_app(cx: &mut TestAppContext) -> TestApp {
    let app = TestApp::with_backend(TestBackend::new().with_store(store_with_account()), cx);
    app.model
        .update(cx, |model, cx| model.load_github_accounts_from_storage(cx))
        .await;
    app
}

/// Repositorio en `feat/search` con `origin` en GitHub (`acme/web`).
fn github_repo() -> TestRepo {
    let repo = TestRepo::new();
    repo.git(&["remote", "add", "origin", "git@github.com:acme/web.git"]);
    repo.git(&["checkout", "-b", "feat/search"]);
    repo
}

fn pull(number: u64, branch: &str, owner: &str) -> Value {
    json!({
        "number": number,
        "title": format!("PR {number}"),
        "html_url": format!("https://github.com/acme/web/pull/{number}"),
        "draft": number == 2,
        "user": { "login": owner },
        "head": { "ref": branch, "sha": format!("sha-{number}"),
                  "repo": { "full_name": format!("{owner}/web"), "owner": { "login": owner } } },
        "base": { "ref": "main", "repo": { "full_name": "acme/web" } },
        "requested_reviewers": []
    })
}

fn path_of(request: &HttpRequest) -> String {
    let url = request.url.as_str();
    let path = url.split("api.github.com").nth(1).unwrap_or(url);
    path.split('?').next().unwrap_or(path).to_string()
}

/// GitHub con dos PRs abiertos: #1 es el de `feat/search` (checks 2/2 y
/// aprobado) y #2 viene de un fork (checks fallando).
fn github(request: &HttpRequest) -> Response {
    match path_of(request).as_str() {
        "/repos/acme/web/pulls" => FakeGitHubTransport::json(
            200,
            json!([pull(1, "feat/search", "acme"), pull(2, "fix", "bea")]),
        ),
        "/repos/acme/web/commits/sha-1/check-runs" => FakeGitHubTransport::json(
            200,
            json!({ "check_runs": [
                { "status": "completed", "conclusion": "success" },
                { "status": "completed", "conclusion": "success" }
            ]}),
        ),
        "/repos/acme/web/commits/sha-2/check-runs" => FakeGitHubTransport::json(
            200,
            json!({ "check_runs": [{ "status": "completed", "conclusion": "failure" }] }),
        ),
        path if path.ends_with("/status") => {
            FakeGitHubTransport::json(200, json!({ "state": "pending", "statuses": [] }))
        }
        "/repos/acme/web/pulls/1/reviews" => FakeGitHubTransport::json(
            200,
            json!([{ "user": { "login": "bea" }, "state": "APPROVED" }]),
        ),
        "/repos/acme/web/pulls/2/reviews" => FakeGitHubTransport::json(200, json!([])),
        _ => FakeGitHubTransport::json(404, json!({ "message": "Not Found" })),
    }
}

async fn refresh(app: &TestApp, repo_id: &str, cx: &mut TestAppContext) {
    app.model
        .update(cx, |model, cx| {
            model.refresh_pull_requests(repo_id, true, cx)
        })
        .await;
    cx.run_until_parked();
}

#[gpui_kit::test]
async fn lists_open_pull_requests_with_checks_and_reviews(cx: &mut TestAppContext) {
    let app = signed_in_app(cx).await;
    app.github.respond_with(github);
    let repo = github_repo();
    let repo_id = app.open(&repo, cx).await;

    refresh(&app, &repo_id, cx).await;

    app.model.read_with(cx, |model, _| {
        let state = model.active_pull_requests().expect("state");
        assert_eq!(state.availability, PullRequestsAvailability::Available);
        assert_eq!(state.error, None);
        assert_eq!(state.remote.as_ref().unwrap().name, "origin");
        assert_eq!(
            state
                .pulls
                .iter()
                .map(|item| item.pull.number)
                .collect::<Vec<_>>(),
            [1, 2]
        );
        let fork = &state.pulls[1];
        assert!(!fork.pull.same_repository);
        assert!(fork.pull.draft);
        assert_eq!(fork.checks.unwrap().state(), Some(CheckState::Failure));
        assert_eq!(fork.review.unwrap().decision, ReviewDecision::None);

        let current = model.active_branch_pull_request().expect("current PR");
        assert_eq!(current.pull.number, 1);
        let checks = current.checks.unwrap();
        assert_eq!((checks.passed, checks.total), (2, 2));
        assert_eq!(checks.state(), Some(CheckState::Success));
        assert_eq!(current.review.unwrap().decision, ReviewDecision::Approved);
    });

    // Todo con el token de la cuenta, y nunca en una URL.
    let requests = app.github.requests();
    assert!(!requests.is_empty());
    for request in &requests {
        assert_eq!(request.header("Authorization"), Some("Bearer ghp_secret"));
        assert!(!request.url.contains("ghp_secret"));
    }
}

#[gpui_kit::test]
async fn without_an_account_asks_to_sign_in_and_does_not_call_github(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    app.github.respond_with(github);
    let repo = github_repo();
    let repo_id = app.open(&repo, cx).await;

    refresh(&app, &repo_id, cx).await;

    app.model.read_with(cx, |model, _| {
        let state = model.active_pull_requests().unwrap();
        assert_eq!(state.availability, PullRequestsAvailability::SignedOut);
        assert!(state.availability.is_visible());
        assert!(model.active_branch_pull_request().is_none());
    });
    assert!(app.github.requests().is_empty());
}

#[gpui_kit::test]
async fn repositories_outside_github_hide_the_section(cx: &mut TestAppContext) {
    let app = signed_in_app(cx).await;
    app.github.respond_with(github);
    let repo = TestRepo::new();
    repo.git(&[
        "remote",
        "add",
        "origin",
        "https://github.enterprise.corp/acme/web.git",
    ]);
    let repo_id = app.open(&repo, cx).await;

    refresh(&app, &repo_id, cx).await;

    app.model.read_with(cx, |model, _| {
        let state = model.active_pull_requests().unwrap();
        assert_eq!(state.availability, PullRequestsAvailability::NotGitHub);
        assert!(!state.availability.is_visible());
    });
    assert!(app.github.requests().is_empty());
}

#[gpui_kit::test]
async fn a_repository_the_account_cannot_see_hides_the_section(cx: &mut TestAppContext) {
    let app = signed_in_app(cx).await;
    app.github
        .respond_with(|_| FakeGitHubTransport::json(404, json!({ "message": "Not Found" })));
    let repo = github_repo();
    let repo_id = app.open(&repo, cx).await;

    refresh(&app, &repo_id, cx).await;

    app.model.read_with(cx, |model, _| {
        let state = model.active_pull_requests().unwrap();
        assert_eq!(state.availability, PullRequestsAvailability::NoAccess);
        assert!(!state.availability.is_visible());
    });
}

#[gpui_kit::test]
async fn a_network_failure_keeps_what_was_known_without_toasts(cx: &mut TestAppContext) {
    let app = signed_in_app(cx).await;
    app.github.respond_with(github);
    let repo = github_repo();
    let repo_id = app.open(&repo, cx).await;
    refresh(&app, &repo_id, cx).await;
    let toasts_before = app.host.toasts().len();

    app.github
        .respond_with(|_| Err(TransportError::Network("offline".to_string())));
    refresh(&app, &repo_id, cx).await;
    refresh(&app, &repo_id, cx).await;

    app.model.read_with(cx, |model, _| {
        let state = model.active_pull_requests().unwrap();
        assert_eq!(state.availability, PullRequestsAvailability::Available);
        assert_eq!(state.pulls.len(), 2);
        assert_eq!(
            state.error.as_deref(),
            Some("Can't reach GitHub right now.")
        );
    });
    assert_eq!(app.host.toasts().len(), toasts_before);
}

#[gpui_kit::test]
async fn an_exhausted_rate_limit_stops_asking_until_it_resets(cx: &mut TestAppContext) {
    let app = signed_in_app(cx).await;
    let reset_at = mergeon_model::format::now_epoch_ms() / 1000 + 3600;
    app.github.respond_with(move |_| {
        Ok(HttpResponse {
            status: 403,
            headers: vec![
                ("x-ratelimit-limit".to_string(), "5000".to_string()),
                ("x-ratelimit-remaining".to_string(), "0".to_string()),
                ("x-ratelimit-reset".to_string(), reset_at.to_string()),
            ],
            body: br#"{"message":"API rate limit exceeded"}"#.to_vec(),
        })
    });
    let repo = github_repo();
    let repo_id = app.open(&repo, cx).await;

    refresh(&app, &repo_id, cx).await;
    let asked = app.github.requests().len();
    assert_eq!(asked, 1);

    app.model.read_with(cx, |model, _| {
        let state = model.active_pull_requests().unwrap();
        assert_eq!(state.rate_limited_until, Some(reset_at as u64));
        assert!(
            state
                .error
                .as_deref()
                .unwrap()
                .starts_with("GitHub rate limit reached. Retrying at ")
        );
    });

    // Ni forzando: hasta que se renueve el cupo no se vuelve a preguntar.
    refresh(&app, &repo_id, cx).await;
    assert_eq!(app.github.requests().len(), asked);
}

#[gpui_kit::test]
async fn a_second_refresh_reuses_the_cached_responses(cx: &mut TestAppContext) {
    let app = signed_in_app(cx).await;
    let asked_with_etag = Arc::new(AtomicUsize::new(0));
    let counter = asked_with_etag.clone();
    app.github.respond_with(move |request| {
        if request.header("If-None-Match").is_some() {
            counter.fetch_add(1, Ordering::SeqCst);
            return Ok(HttpResponse {
                status: 304,
                headers: Vec::new(),
                body: Vec::new(),
            });
        }
        let mut response = github(request)?;
        response
            .headers
            .push(("etag".to_string(), format!("\"{}\"", request.url.len())));
        Ok(response)
    });
    let repo = github_repo();
    let repo_id = app.open(&repo, cx).await;

    refresh(&app, &repo_id, cx).await;
    let first = app.github.requests().len();
    let generation = app.model.read_with(cx, |model, _| {
        model.active_pull_requests().unwrap().generation
    });
    refresh(&app, &repo_id, cx).await;

    assert_eq!(asked_with_etag.load(Ordering::SeqCst), first);
    app.model.read_with(cx, |model, _| {
        let state = model.active_pull_requests().unwrap();
        assert_eq!(state.pulls.len(), 2);
        // Nada cambió: lo que se pinta no se marca como nuevo.
        assert_eq!(state.generation, generation);
        assert!(model.active_branch_pull_request().is_some());
    });
}

#[gpui_kit::test]
async fn switching_branches_looks_for_the_pull_request_of_the_new_branch(cx: &mut TestAppContext) {
    let app = signed_in_app(cx).await;
    app.github.respond_with(github);
    let repo = github_repo();
    let repo_id = app.open(&repo, cx).await;
    app.model
        .update(cx, |model, cx| model.watch_pull_requests(cx));
    app.wait_for(cx, "first refresh", |model| {
        model.active_branch_pull_request().is_some()
    });
    let refreshes = app
        .model
        .read_with(cx, |model, _| model.pull_request_refresh_count());

    repo.git(&["checkout", "-b", "other"]);
    app.model
        .update(cx, |model, cx| {
            model.sync_repository_read_model(
                repo_id.as_str(),
                crate::SyncRequest::new().status(true).branches(true),
                cx,
            )
        })
        .await;
    app.wait_for(cx, "refresh after checkout", |model| {
        model.pull_request_refresh_count() > refreshes
            && model
                .active_pull_requests()
                .unwrap()
                .current_branch
                .as_deref()
                == Some("other")
    });

    app.model.read_with(cx, |model, _| {
        assert!(model.active_branch_pull_request().is_none());
        assert_eq!(model.active_pull_requests().unwrap().pulls.len(), 2);
    });
}

#[gpui_kit::test]
async fn fetches_the_branch_of_a_pull_request_from_a_fork(cx: &mut TestAppContext) {
    let app = signed_in_app(cx).await;
    let repo = TestRepo::new();
    let origin = repo.publish_to_origin();
    // Se lee como `acme/web` en GitHub; se trae del remoto local.
    repo.git(&[
        "remote",
        "set-url",
        "--push",
        "origin",
        "https://github.com/acme/web.git",
    ]);
    let colleague = repo.colleague(&origin);
    let fork_head = colleague.commit_and_push("fork.txt", "fork\n", "Fork work");
    super::support::git_in(
        &colleague.path,
        &["push", "origin", "HEAD:refs/pull/2/head"],
    );
    super::support::git_in(
        &colleague.path,
        &["push", "origin", "HEAD~1:main", "--force"],
    );

    let seen = Arc::new(Mutex::new(Vec::new()));
    let recorder = seen.clone();
    app.github.respond_with(move |request| {
        recorder.lock().unwrap().push(request.url.clone());
        github(request)
    });
    let repo_id = app.open(&repo, cx).await;
    refresh(&app, &repo_id, cx).await;

    let checkout = app
        .model
        .update(cx, |model, cx| model.fetch_pull_request(&repo_id, 2, cx))
        .await;
    cx.run_until_parked();

    assert_eq!(
        checkout,
        Some(PullRequestCheckout::LocalBranch("pr-2".to_string()))
    );
    assert_eq!(repo.git(&["rev-parse", "pr-2"]).trim(), fork_head);
    // Traer no cambia de rama.
    assert_eq!(repo.current_branch(), "main");
    app.model.read_with(cx, |model, _| {
        let repository = model.active_repository().unwrap();
        assert!(
            repository
                .branches
                .iter()
                .any(|branch| branch.name == "pr-2")
        );
    });
    assert!(!seen.lock().unwrap().is_empty());

    // En `pr-2` el PR de la rama es el del fork.
    repo.git(&["checkout", "pr-2"]);
    app.model
        .update(cx, |model, cx| {
            model.sync_repository_read_model(
                repo_id.as_str(),
                crate::SyncRequest::new().status(true).branches(true),
                cx,
            )
        })
        .await;
    refresh(&app, &repo_id, cx).await;
    app.model.read_with(cx, |model, _| {
        let current = model.active_branch_pull_request().expect("fork PR");
        assert_eq!(current.pull.number, 2);
    });

    // Un PR que no se conoce no hace nada.
    let unknown = app
        .model
        .update(cx, |model, cx| model.fetch_pull_request(&repo_id, 99, cx))
        .await;
    assert_eq!(unknown, None);
}
