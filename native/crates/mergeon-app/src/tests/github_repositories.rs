//! Port de `src/composables/useGitHubRepositories.test.js`.

use gpui_kit::TestAppContext;
use mergeon_github::{AuthContext, HttpRequest, HttpResponse, TransportError};
use mergeon_store::GitHubInstallation;
use serde_json::{Value, json};

use super::github_store::github_app_config;
use super::support::TestApp;
use crate::github::ListAllRepositoriesOptions;
use crate::testing::{FakeGitHubTransport, TestBackend};

type Response = Result<HttpResponse, TransportError>;

fn app(cx: &mut TestAppContext) -> TestApp {
    TestApp::with_backend(
        TestBackend::new().with_github_config(github_app_config()),
        cx,
    )
}

fn repo_updated(id: u64, updated_at: &str) -> Value {
    json!({ "id": id, "full_name": format!("owner/repo-{id}"), "updated_at": updated_at })
}

fn repo(id: u64) -> Value {
    repo_updated(id, "2026-01-01T00:00:00Z")
}

fn page(repositories: Vec<Value>) -> Response {
    FakeGitHubTransport::json(
        200,
        json!({ "total_count": repositories.len(), "repositories": repositories }),
    )
}

fn installation(id: u64) -> GitHubInstallation {
    serde_json::from_value(json!({ "id": id })).unwrap()
}

/// Instalación y página de una petición de repositorios.
fn target(request: &HttpRequest) -> (u64, u32) {
    let (_, rest) = request
        .url
        .split_once("/user/installations/")
        .expect("installation repositories request");
    let (installation_id, query) = rest.split_once("/repositories?").expect("query");
    let page = query
        .split('&')
        .find_map(|pair| pair.strip_prefix("page="))
        .expect("page");
    (installation_id.parse().unwrap(), page.parse().unwrap())
}

fn requested(app: &TestApp) -> Vec<(u64, u32)> {
    app.github.requests().iter().map(target).collect()
}

#[gpui_kit::test]
async fn paginates_each_installation_until_a_short_batch_and_tags_repos_with_their_installation(
    cx: &mut TestAppContext,
) {
    let app = app(cx);
    app.github.respond_with(|request| match target(request) {
        (1, 1) => page(vec![repo(1), repo(2)]),
        (1, _) => page(vec![repo(3)]),
        _ => page(vec![repo(4)]),
    });

    let result = app
        .model
        .update(cx, |model, cx| {
            model.list_all_repositories(
                "token-1",
                vec![installation(1), installation(2)],
                ListAllRepositoriesOptions {
                    per_page: 2,
                    ..Default::default()
                },
                cx,
            )
        })
        .await
        .unwrap()
        .expect("repositories");

    assert!(result.failures.is_empty());
    assert_eq!(result.repositories.len(), 4);
    let installation_of = |id: u64| {
        result
            .repositories
            .iter()
            .find(|item| item.repository.id == Some(id))
            .map(|item| item.installation_id)
    };
    assert_eq!(installation_of(3), Some(1));
    assert_eq!(installation_of(4), Some(2));
    assert_eq!(requested(&app), [(1, 1), (1, 2), (2, 1)]);
    app.model.read_with(cx, |model, _| {
        assert!(!model.github_repositories_loading());
        assert_eq!(model.github_repositories_error(), None);
    });
}

#[gpui_kit::test]
async fn iterates_the_active_installation_last(cx: &mut TestAppContext) {
    let app = app(cx);
    app.github.respond_with(|_| page(Vec::new()));

    app.model
        .update(cx, |model, cx| {
            model.list_all_repositories(
                "token-1",
                vec![installation(1), installation(2), installation(3)],
                ListAllRepositoriesOptions {
                    active_installation_id: Some(2),
                    ..Default::default()
                },
                cx,
            )
        })
        .await
        .unwrap();

    let order: Vec<u64> = requested(&app).into_iter().map(|(id, _)| id).collect();
    assert_eq!(order, [1, 3, 2]);
}

#[gpui_kit::test]
async fn dedupes_by_repo_id_and_sorts_by_updated_at_descending(cx: &mut TestAppContext) {
    let app = app(cx);
    app.github.respond_with(|request| match target(request).0 {
        1 => page(vec![
            repo_updated(1, "2026-02-01T00:00:00Z"),
            repo_updated(2, "2026-05-01T00:00:00Z"),
        ]),
        _ => page(vec![
            repo_updated(2, "2026-05-01T00:00:00Z"),
            repo_updated(3, "2026-03-01T00:00:00Z"),
        ]),
    });

    let result = app
        .model
        .update(cx, |model, cx| {
            model.list_all_repositories(
                "token-1",
                vec![installation(1), installation(2)],
                ListAllRepositoriesOptions::default(),
                cx,
            )
        })
        .await
        .unwrap()
        .expect("repositories");

    let ids: Vec<_> = result
        .repositories
        .iter()
        .map(|item| item.repository.id)
        .collect();
    assert_eq!(ids, [Some(2), Some(3), Some(1)]);
    assert_eq!(result.repositories[0].installation_id, 1);
}

#[gpui_kit::test]
async fn returns_none_when_the_request_is_cancelled_mid_flight(cx: &mut TestAppContext) {
    let app = app(cx);
    app.github.respond_with(|_| page(vec![repo(1)]));

    let request = app.model.update(cx, |model, cx| {
        model.list_all_repositories(
            "token-1",
            vec![installation(1), installation(2)],
            ListAllRepositoriesOptions::default(),
            cx,
        )
    });
    app.model
        .read_with(cx, |model, _| assert!(model.github_repositories_loading()));

    // Se deja correr solo hasta que la primera instalación responde.
    while app.github.requests().is_empty() {
        assert!(cx.executor().tick(), "the request never started");
    }
    app.model
        .update(cx, |model, cx| model.cancel_repository_requests(cx));

    assert_eq!(request.await, Ok(None));
    app.model
        .read_with(cx, |model, _| assert!(!model.github_repositories_loading()));
    // The second installation is never requested once the load is stale.
    assert_eq!(app.github.requests().len(), 1);
}

#[gpui_kit::test]
async fn keeps_going_when_one_installation_fails_and_reports_the_failure(cx: &mut TestAppContext) {
    let app = app(cx);
    app.github.respond_with(|request| match target(request).0 {
        1 => FakeGitHubTransport::json(403, json!({ "message": "installation suspended" })),
        _ => page(vec![repo(1)]),
    });
    let broken: GitHubInstallation =
        serde_json::from_value(json!({ "id": 1, "accountLogin": "broken-org" })).unwrap();

    let result = app
        .model
        .update(cx, |model, cx| {
            model.list_all_repositories(
                "token-1",
                vec![broken, installation(2)],
                ListAllRepositoriesOptions::default(),
                cx,
            )
        })
        .await
        .unwrap()
        .expect("repositories");

    assert_eq!(result.repositories.len(), 1);
    assert_eq!(result.repositories[0].repository.id, Some(1));
    assert_eq!(result.failures.len(), 1);
    assert_eq!(result.failures[0].installation_id, 1);
    assert_eq!(result.failures[0].account_login, "broken-org");
    assert_eq!(
        result.failures[0].message,
        "Failed to fetch repositories: installation suspended"
    );
    app.model.read_with(cx, |model, _| {
        assert_eq!(model.github_repositories_error(), None)
    });
}

#[gpui_kit::test]
async fn fails_when_every_installation_fails(cx: &mut TestAppContext) {
    let app = app(cx);
    app.github
        .respond_with(|_| FakeGitHubTransport::json(500, json!({ "message": "boom" })));

    let error = app
        .model
        .update(cx, |model, cx| {
            model.list_all_repositories(
                "token-1",
                vec![installation(1), installation(2)],
                ListAllRepositoriesOptions::default(),
                cx,
            )
        })
        .await
        .unwrap_err();

    assert_eq!(error.message, "Failed to fetch repositories: boom");
    app.model.read_with(cx, |model, _| {
        assert!(!model.github_repositories_loading());
        assert_eq!(
            model.github_repositories_error(),
            Some("Failed to fetch repositories: boom")
        );
    });
}

#[gpui_kit::test]
async fn lists_and_searches_a_single_page(cx: &mut TestAppContext) {
    let app = app(cx);
    app.github.respond_with(|_| {
        page(vec![
            json!({ "id": 1, "full_name": "owner/alpha", "description": "First" }),
            json!({ "id": 2, "full_name": "owner/beta", "description": "Second" }),
        ])
    });
    let context = || AuthContext::new("token-1", Some(1));

    let listed = app
        .model
        .update(cx, |model, cx| {
            model.list_repositories(context(), 1, 100, cx)
        })
        .await
        .unwrap()
        .expect("repositories");
    assert_eq!(listed.len(), 2);

    let found = app
        .model
        .update(cx, |model, cx| {
            model.search_repositories(context(), "beta", 1, 30, cx)
        })
        .await
        .unwrap()
        .expect("search result");
    assert_eq!(found.repositories.len(), 1);
    assert_eq!(
        found.repositories[0].full_name.as_deref(),
        Some("owner/beta")
    );
}

#[gpui_kit::test]
async fn only_the_latest_request_counts(cx: &mut TestAppContext) {
    let app = app(cx);
    app.github.respond_with(|_| page(vec![repo(1)]));
    let context = || AuthContext::new("token-1", Some(1));

    let (first, second) = app.model.update(cx, |model, cx| {
        (
            model.list_repositories(context(), 1, 100, cx),
            model.list_repositories(context(), 1, 100, cx),
        )
    });

    // La primera quedó obsoleta al pedir la segunda.
    assert_eq!(first.await, Ok(None));
    assert_eq!(second.await.unwrap().map(|repos| repos.len()), Some(1));
    app.model
        .read_with(cx, |model, _| assert!(!model.github_repositories_loading()));
}
