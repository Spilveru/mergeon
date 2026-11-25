//! El indicador del PR de la rama actual en la barra de estado.

use gpui_kit::TestAppContext;
use gpui_kit::test::TestWindowExt as _;
use mergeon_app::testing::{FakeGitHubTransport, TestBackend};
use mergeon_github::{HttpRequest, HttpResponse, TransportError};
use serde_json::json;

use crate::support::{TestApp, TestRepo};

fn signed_in_backend() -> TestBackend {
    let store = TestBackend::memory_store();
    let account = serde_json::from_value(json!({
        "id": "7",
        "username": "ana",
        "token": "ghp_test",
        "isPersonalAccessToken": true
    }))
    .unwrap();
    store.save_github_account(&account).unwrap();
    store.set_active_github_account_id(Some("7")).unwrap();
    TestBackend::new().with_store(store)
}

/// El PR #123 es el de `feature`; sus checks: 2 en verde y 1 pendiente.
fn github(request: &HttpRequest) -> Result<HttpResponse, TransportError> {
    let path = request
        .url
        .split("api.github.com")
        .nth(1)
        .unwrap_or_default()
        .split('?')
        .next()
        .unwrap_or_default();
    match path {
        "/repos/acme/demo/pulls" => FakeGitHubTransport::json(
            200,
            json!([{
                "number": 123,
                "title": "Round VAT per line",
                "html_url": "https://github.com/acme/demo/pull/123",
                "user": { "login": "ana" },
                "head": { "ref": "feature", "sha": "abc",
                          "repo": { "full_name": "acme/demo", "owner": { "login": "acme" } } },
                "base": { "ref": "main", "repo": { "full_name": "acme/demo" } }
            }]),
        ),
        "/repos/acme/demo/commits/abc/check-runs" => FakeGitHubTransport::json(
            200,
            json!({ "check_runs": [
                { "status": "completed", "conclusion": "success" },
                { "status": "completed", "conclusion": "success" },
                { "status": "in_progress" }
            ]}),
        ),
        "/repos/acme/demo/commits/abc/status" => {
            FakeGitHubTransport::json(200, json!({ "statuses": [] }))
        }
        "/repos/acme/demo/pulls/123/reviews" => FakeGitHubTransport::json(200, json!([])),
        _ => FakeGitHubTransport::json(404, json!({ "message": "Not Found" })),
    }
}

fn github_repo() -> TestRepo {
    let repo = TestRepo::new();
    repo.git(&[
        "remote",
        "add",
        "origin",
        "https://github.com/acme/demo.git",
    ]);
    repo.git(&["checkout", "-q", "-b", "feature"]);
    repo
}

#[gpui_kit::test]
async fn shows_the_pull_request_of_the_current_branch_and_opens_it(cx: &mut TestAppContext) {
    let backend = signed_in_backend();
    backend.github.respond_with(github);
    let app = TestApp::with_backend(backend, 1200., cx);
    app.model
        .update(cx, |model, cx| model.load_github_accounts_from_storage(cx))
        .await;
    let repo = github_repo();
    app.open(&repo, cx).await;

    let indicator = app.shell.read_with(cx, |shell, cx| {
        shell.status_bar.read(cx).pull_request_indicator().clone()
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while indicator
        .read_with(cx, |indicator, cx| indicator.label(cx))
        .is_none()
    {
        assert!(
            std::time::Instant::now() < deadline,
            "no pull request shown"
        );
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(
        indicator.read_with(cx, |indicator, cx| indicator.label(cx)),
        Some("#123 · 2/3 checks".to_string())
    );
    assert!(app.in_window(cx, |window, _| {
        window.try_find("status-pull-request").is_some()
    }));

    app.click(cx, "status-pull-request");
    cx.run_until_parked();
    assert!(
        app.launcher
            .launched_args()
            .iter()
            .any(|arg| arg == "https://github.com/acme/demo/pull/123")
    );
}

#[gpui_kit::test]
async fn without_a_pull_request_the_status_bar_shows_nothing_more(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let repo = github_repo();
    app.open(&repo, cx).await;
    cx.run_until_parked();

    assert!(app.in_window(cx, |window, _| window.try_find("status-bar").is_some()));
    assert!(!app.in_window(cx, |window, _| {
        window.try_find("status-pull-request").is_some()
    }));
    assert!(
        app.model
            .read_with(cx, |model, _| model.is_watching_pull_requests())
    );
}
