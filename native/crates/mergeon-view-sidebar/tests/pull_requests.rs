//! La sección "Pull requests" en una ventana sin pantalla, con un GitHub
//! guionizado y repositorios git reales.

mod support;

use gpui_kit::TestAppContext;
use gpui_kit::test::TestWindowExt as _;
use mergeon_app::testing::{FakeGitHubTransport, TestBackend};
use mergeon_backend::Store;
use mergeon_github::{HttpRequest, HttpResponse, TransportError};
use mergeon_view_sidebar::PullRequestMenuEntry;
use serde_json::{Value, json};
use support::{Harness, TestRepo, git_in};

type Response = Result<HttpResponse, TransportError>;

fn store(signed_in: bool) -> Store {
    let store = TestBackend::memory_store();
    store
        .update_settings_json(&json!({ "sidebarSections": support::default_sections() }))
        .expect("seed settings");
    if signed_in {
        let account = serde_json::from_value(json!({
            "id": "7",
            "username": "ana",
            "token": "ghp_test",
            "isPersonalAccessToken": true
        }))
        .unwrap();
        store.save_github_account(&account).unwrap();
        store.set_active_github_account_id(Some("7")).unwrap();
    }
    store
}

fn pull(number: u64, branch: &str, owner: &str) -> Value {
    json!({
        "number": number,
        "title": format!("Pull request number {number}"),
        "html_url": format!("https://github.com/acme/demo/pull/{number}"),
        "draft": false,
        "user": { "login": owner },
        "head": { "ref": branch, "sha": format!("sha-{number}"),
                  "repo": { "full_name": format!("{owner}/demo"), "owner": { "login": owner } } },
        "base": { "ref": "main", "repo": { "full_name": "acme/demo" } },
        "requested_reviewers": []
    })
}

/// #1 es la rama `feature` del propio repositorio (checks en verde) y #2
/// viene de un fork.
fn github(request: &HttpRequest) -> Response {
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
            json!([pull(1, "feature", "acme"), pull(2, "fix", "bea")]),
        ),
        "/repos/acme/demo/commits/sha-1/check-runs" => FakeGitHubTransport::json(
            200,
            json!({ "check_runs": [{ "status": "completed", "conclusion": "success" }] }),
        ),
        path if path.contains("/commits/") => {
            FakeGitHubTransport::json(200, json!({ "check_runs": [], "statuses": [] }))
        }
        path if path.ends_with("/reviews") => FakeGitHubTransport::json(200, json!([])),
        _ => FakeGitHubTransport::json(404, json!({ "message": "Not Found" })),
    }
}

/// `origin` publicado en un bare local que GitHub conoce como `acme/demo`
/// (la URL de push), con la rama `feature` y `refs/pull/2/head`.
fn github_repo() -> (TestRepo, String) {
    let repo = TestRepo::new();
    let origin = repo.publish_to_origin();
    repo.git(&[
        "remote",
        "set-url",
        "--push",
        "origin",
        "https://github.com/acme/demo.git",
    ]);
    repo.git(&["checkout", "-b", "fork-work"]);
    let fork_head = repo.commit_file("fork.txt", "fork\n", "Fork work");
    git_in(
        &repo.path,
        &[
            "push",
            &origin.to_string_lossy(),
            "HEAD:refs/pull/2/head",
            "HEAD:refs/heads/feature",
        ],
    );
    repo.git(&["checkout", "main"]);
    repo.git(&["branch", "-D", "fork-work"]);
    (repo, fork_head)
}

async fn open(
    repo: &TestRepo,
    signed_in: bool,
    cx: &mut TestAppContext,
) -> (
    Harness,
    std::sync::Arc<mergeon_app::testing::RecordingLauncher>,
) {
    let backend = TestBackend::new().with_store(store(signed_in));
    backend.github.respond_with(github);
    let launcher = backend.launcher.clone();
    let mut harness = Harness::mount_with(backend, cx);
    harness
        .model
        .update(cx, |model, cx| model.load_github_accounts_from_storage(cx))
        .await;
    harness.repo_id = repo.id();
    harness.open_repository(repo, cx).await;
    (harness, launcher)
}

#[gpui_kit::test]
async fn lists_the_open_pull_requests_and_opens_one_in_the_browser(cx: &mut TestAppContext) {
    let (repo, _) = github_repo();
    let (harness, launcher) = open(&repo, true, cx).await;
    harness.wait(cx, "the pull requests", |model| {
        model
            .active_pull_requests()
            .is_some_and(|state| state.pulls.len() == 2)
    });

    assert!(harness.exists(cx, "section-header-pull-requests"));
    assert!(harness.exists(cx, "pull-request:1"));
    assert!(harness.exists(cx, "pull-request:2"));
    harness.wait_ui(cx, "the checks of #1", |window, _| {
        window.try_find("pull-request-checks:1").is_some()
    });

    harness.click(cx, "pull-request:1");
    harness.wait_app(cx, "the browser to open", |_| {
        launcher
            .launched_args()
            .iter()
            .any(|arg| arg == "https://github.com/acme/demo/pull/1")
    });

    // Plegada, la sección no pinta sus filas.
    harness.click(cx, "section-header-pull-requests");
    assert!(!harness.exists(cx, "pull-request:1"));
    harness.model.read_with(cx, |model, _| {
        assert_eq!(
            model.sidebar_sections().extra.get("pullRequests"),
            Some(&json!(false))
        );
    });
}

#[gpui_kit::test]
async fn without_an_account_offers_to_sign_in(cx: &mut TestAppContext) {
    let (repo, _) = github_repo();
    let (harness, _) = open(&repo, false, cx).await;
    harness.wait_ui(cx, "the sign-in prompt", |window, _| {
        window.try_find("pull-requests-sign-in").is_some()
    });
    assert!(!harness.exists(cx, "pull-request:1"));

    harness.click(cx, "pull-requests-sign-in");
    harness.model.read_with(cx, |model, _| {
        assert!(model.show_preferences());
        assert_eq!(model.settings_section(), "accounts");
    });
}

#[gpui_kit::test]
async fn repositories_outside_github_have_no_section(cx: &mut TestAppContext) {
    let repo = TestRepo::new();
    repo.add_bare_remote("origin");
    let (harness, _) = open(&repo, true, cx).await;
    harness.wait(cx, "the remote check", |model| {
        model.active_pull_requests().is_some_and(|state| {
            state.availability == mergeon_app::PullRequestsAvailability::NotGitHub
        })
    });

    assert!(!harness.exists(cx, "section-header-pull-requests"));
    assert!(harness.exists(cx, "section-header-branches"));
}

#[gpui_kit::test]
async fn checks_out_the_branch_of_a_pull_request_from_a_fork(cx: &mut TestAppContext) {
    let (repo, fork_head) = github_repo();
    let (harness, _) = open(&repo, true, cx).await;
    harness.wait(cx, "the pull requests", |model| {
        model
            .active_pull_requests()
            .is_some_and(|state| state.pulls.len() == 2)
    });

    let section = harness
        .sidebar
        .read_with(cx, |sidebar, _| sidebar.pull_requests_section().clone());
    let menu = section.read_with(cx, |section, cx| section.menu(2, cx));
    let index = menu
        .iter()
        .position(|entry| {
            matches!(entry, PullRequestMenuEntry::Item { label, .. } if *label == "Checkout PR branch")
        })
        .expect("checkout entry");
    // Un fork no se puede comparar como rama remota.
    assert!(!menu.iter().any(|entry| matches!(
        entry,
        PullRequestMenuEntry::Item { label, .. } if *label == "Compare with current branch"
    )));

    harness.right_click(cx, "pull-request:2");
    harness.in_window(cx, move |window, cx| {
        window.within("popup-menu").click(index, cx);
    });

    harness.wait_repo(cx, "the checkout of pr-2", |repository| {
        repository.current_branch.as_deref() == Some("pr-2")
    });
    assert_eq!(repo.current_branch(), "pr-2");
    assert_eq!(repo.head(), fork_head);
}

#[gpui_kit::test]
async fn checks_out_a_pull_request_branch_of_the_same_repository(cx: &mut TestAppContext) {
    let (repo, fork_head) = github_repo();
    let (harness, _) = open(&repo, true, cx).await;
    harness.wait(cx, "the pull requests", |model| {
        model
            .active_pull_requests()
            .is_some_and(|state| state.pulls.len() == 2)
    });
    // El clon todavía no conoce `origin/feature`: hay que traerla.
    assert!(
        repo.git(&["branch", "-r", "--list", "origin/feature"])
            .trim()
            .is_empty()
    );

    let section = harness
        .sidebar
        .read_with(cx, |sidebar, _| sidebar.pull_requests_section().clone());
    harness.in_window(cx, |window, cx| {
        section.update(cx, |section, cx| {
            section.run_command(
                mergeon_view_sidebar::PullRequestCommand::Checkout(1),
                window,
                cx,
            );
        });
    });

    // Como un doble clic en `origin/feature`: "Track Remote Branch".
    harness.wait_ui(cx, "the tracking dialog", |window, cx| {
        mergeon_ui::dialog::open_count(window, cx) == 1
    });
    assert_eq!(harness.dialog_titles(cx), ["Track Remote Branch"]);
    harness.click(cx, "checkout-submit");

    harness.wait_repo(cx, "the checkout of feature", |repository| {
        repository.current_branch.as_deref() == Some("feature")
    });
    assert_eq!(repo.head(), fork_head);
    assert_eq!(
        repo.git(&["rev-parse", "--abbrev-ref", "feature@{upstream}"])
            .trim(),
        "origin/feature"
    );
}

/// "Compare with current branch" de un PR del propio repositorio trae su
/// rama y abre la comparación navegable (como la de una rama remota).
#[gpui_kit::test]
async fn compares_a_pull_request_with_the_current_branch(cx: &mut TestAppContext) {
    let (repo, _) = github_repo();
    let (harness, _) = open(&repo, true, cx).await;
    harness.wait(cx, "the pull requests", |model| {
        model
            .active_pull_requests()
            .is_some_and(|state| state.pulls.len() == 2)
    });

    let section = harness
        .sidebar
        .read_with(cx, |sidebar, _| sidebar.pull_requests_section().clone());
    harness.in_window(cx, |window, cx| {
        section.update(cx, |section, cx| {
            section.run_command(
                mergeon_view_sidebar::PullRequestCommand::CompareWithCurrent(1),
                window,
                cx,
            );
        });
    });

    let comparisons = |harness: &support::Harness| {
        harness
            .events()
            .into_iter()
            .filter_map(|event| match event {
                mergeon_view_sidebar::SidebarEvent::OpenComparison(request) => Some(request),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    harness.wait_app(cx, "the comparison", |_| !comparisons(&harness).is_empty());
    let requests = comparisons(&harness);
    assert_eq!(
        (requests[0].base.as_str(), requests[0].head.as_str()),
        ("main", "origin/feature")
    );
    // Solo lee: ni checkout ni diálogos.
    assert_eq!(repo.current_branch(), "main");
    assert!(harness.dialog_titles(cx).is_empty());
}
