//! Pull requests, revisiones y checks con respuestas grabadas: paginación,
//! `ETag`/304, cupo agotado y peticiones sin cuenta.

mod common;

use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use common::{FakeTransport, json_response, oauth_client};
use mergeon_github::{
    CheckState, GitHubErrorCode, GitHubRepositoryRef, HttpResponse, RateLimit, ResponseCache,
    ReviewDecision, TransportError,
};
use serde_json::{Value, json};

type Reply = Result<HttpResponse, TransportError>;

fn repo() -> GitHubRepositoryRef {
    GitHubRepositoryRef {
        owner: "acme".to_string(),
        repo: "web".to_string(),
    }
}

fn response(status: u16, payload: Value, headers: &[(&str, &str)]) -> Reply {
    let mut all = vec![("content-type".to_string(), "application/json".to_string())];
    all.extend(
        headers
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string())),
    );
    Ok(HttpResponse {
        status,
        headers: all,
        body: if status == 304 {
            Vec::new()
        } else {
            payload.to_string().into_bytes()
        },
    })
}

fn pull(number: u64) -> Value {
    json!({
        "number": number,
        "title": format!("PR {number}"),
        "html_url": format!("https://github.com/acme/web/pull/{number}"),
        "draft": false,
        "user": { "login": "ana", "avatar_url": "https://avatars/ana" },
        "head": { "ref": format!("feat/{number}"), "sha": format!("sha{number}"),
                  "repo": { "full_name": "acme/web", "owner": { "login": "acme" } } },
        "base": { "ref": "main", "repo": { "full_name": "acme/web" } },
        "updated_at": "2026-11-10T10:00:00Z",
        "requested_reviewers": []
    })
}

fn pulls(numbers: std::ops::RangeInclusive<u64>) -> Value {
    Value::Array(numbers.map(pull).collect())
}

#[test]
fn lists_open_pull_requests_following_the_link_header_up_to_the_limit() {
    let transport = FakeTransport::new();
    let page2 = "https://api.github.com/repositories/1/pulls?per_page=2&page=2";
    let page3 = "https://api.github.com/repositories/1/pulls?per_page=2&page=3";
    transport.respond_once(response(
        200,
        pulls(1..=2),
        &[(
            "link",
            &format!("<{page2}>; rel=\"next\", <{page3}>; rel=\"last\""),
        )],
    ));
    transport.respond_once(response(
        200,
        pulls(3..=4),
        &[("link", &format!("<{page3}>; rel=\"next\""))],
    ));
    transport.respond_once(response(200, pulls(5..=6), &[]));

    let cache = ResponseCache::new();
    let client = oauth_client(&transport);
    let list = client
        .list_open_pull_requests_paged("tok-123", &repo(), &cache, 2, 5)
        .unwrap();

    assert_eq!(
        list.iter().map(|pull| pull.number).collect::<Vec<_>>(),
        [1, 2, 3, 4, 5]
    );
    assert_eq!(list[0].head_ref, "feat/1");
    assert!(list[0].same_repository);

    let calls = transport.calls();
    assert_eq!(calls.len(), 3);
    assert_eq!(
        calls[0].url,
        "https://api.github.com/repos/acme/web/pulls?state=open&sort=updated&direction=desc&per_page=2"
    );
    assert_eq!(calls[1].url, page2);
    assert_eq!(calls[2].url, page3);
    assert_eq!(calls[0].header("Authorization"), Some("Bearer tok-123"));
    assert_eq!(
        calls[0].header("Accept"),
        Some("application/vnd.github+json")
    );
    assert_eq!(calls[0].header("If-None-Match"), None);
}

#[test]
fn stops_at_the_limit_without_asking_for_more_pages() {
    let transport = FakeTransport::new();
    transport.respond_once(response(
        200,
        pulls(1..=3),
        &[("link", "<https://api.github.com/next>; rel=\"next\"")],
    ));

    let list = oauth_client(&transport)
        .list_open_pull_requests_paged("tok", &repo(), &ResponseCache::new(), 3, 3)
        .unwrap();
    assert_eq!(list.len(), 3);
    assert_eq!(transport.call_count(), 1);
}

#[test]
fn asks_again_with_the_etag_and_reuses_the_cached_list_on_304() {
    let transport = FakeTransport::new();
    transport.respond_once(response(200, pulls(1..=2), &[("etag", "W/\"abc\"")]));
    transport.respond_once(response(304, Value::Null, &[("etag", "W/\"abc\"")]));

    let cache = ResponseCache::new();
    let client = oauth_client(&transport);
    let first = client
        .list_open_pull_requests("tok", &repo(), &cache)
        .unwrap();
    let second = client
        .list_open_pull_requests("tok", &repo(), &cache)
        .unwrap();

    assert_eq!(first, second);
    assert_eq!(second.len(), 2);
    let calls = transport.calls();
    assert_eq!(calls[0].header("If-None-Match"), None);
    assert_eq!(calls[1].header("If-None-Match"), Some("W/\"abc\""));
    assert_eq!(cache.not_modified_count(), 1);
    assert_eq!(cache.len(), 1);
}

#[test]
fn a_304_without_a_cached_response_is_an_invalid_response() {
    let transport = FakeTransport::new();
    transport.respond_once(response(304, Value::Null, &[]));

    let error = oauth_client(&transport)
        .list_open_pull_requests("tok", &repo(), &ResponseCache::new())
        .unwrap_err();
    assert_eq!(error.code, GitHubErrorCode::InvalidResponse);
}

#[test]
fn an_exhausted_rate_limit_is_reported_with_its_reset_time() {
    let transport = FakeTransport::new();
    transport.respond_once(response(
        403,
        json!({ "message": "API rate limit exceeded for user." }),
        &[
            ("x-ratelimit-limit", "5000"),
            ("x-ratelimit-remaining", "0"),
            ("x-ratelimit-reset", "1767225600"),
        ],
    ));

    let cache = ResponseCache::new();
    let error = oauth_client(&transport)
        .list_open_pull_requests("tok", &repo(), &cache)
        .unwrap_err();

    assert_eq!(error.code, GitHubErrorCode::RateLimited);
    assert_eq!(error.status, 403);
    assert_eq!(
        cache.rate_limit(),
        Some(RateLimit {
            limit: 5000,
            remaining: 0,
            reset_at: 1_767_225_600,
        })
    );
}

#[test]
fn a_secondary_rate_limit_waits_for_retry_after() {
    let transport = FakeTransport::new();
    transport.respond_once(response(
        429,
        json!({ "message": "You have exceeded a secondary rate limit." }),
        &[("retry-after", "90")],
    ));

    let cache = ResponseCache::new();
    let client = oauth_client(&transport).with_clock(|| UNIX_EPOCH + Duration::from_secs(1_000));
    let error = client
        .list_open_pull_requests("tok", &repo(), &cache)
        .unwrap_err();

    assert_eq!(error.code, GitHubErrorCode::RateLimited);
    let limit = cache.rate_limit().unwrap();
    assert!(limit.is_exhausted());
    assert_eq!(limit.reset_at, 1_090);
}

#[test]
fn remembers_the_remaining_quota_of_successful_responses() {
    let transport = FakeTransport::new();
    transport.respond_once(response(
        200,
        pulls(1..=1),
        &[
            ("x-ratelimit-limit", "5000"),
            ("x-ratelimit-remaining", "4321"),
            ("x-ratelimit-reset", "1767225600"),
        ],
    ));

    let cache = ResponseCache::new();
    oauth_client(&transport)
        .list_open_pull_requests("tok", &repo(), &cache)
        .unwrap();
    let limit = cache.rate_limit().unwrap();
    assert_eq!(limit.remaining, 4321);
    assert!(!limit.is_exhausted());
}

#[test]
fn a_forbidden_response_with_quota_left_is_a_plain_error() {
    let transport = FakeTransport::new();
    transport.respond_once(response(
        403,
        json!({ "message": "Resource not accessible by integration" }),
        &[("x-ratelimit-remaining", "4999")],
    ));

    let error = oauth_client(&transport)
        .list_open_pull_requests("tok", &repo(), &ResponseCache::new())
        .unwrap_err();
    assert_eq!(error.code, GitHubErrorCode::GithubError);
    assert_eq!(error.status, 403);
    assert_eq!(error.message, "Resource not accessible by integration");
}

#[test]
fn without_an_account_no_authorization_header_is_sent() {
    let transport = FakeTransport::new();
    transport.respond_once(response(
        404,
        json!({ "message": "Not Found" }),
        &[("x-ratelimit-remaining", "59")],
    ));

    let error = oauth_client(&transport)
        .list_open_pull_requests("", &repo(), &ResponseCache::new())
        .unwrap_err();
    assert_eq!(error.status, 404);
    assert_eq!(transport.calls()[0].header("Authorization"), None);
}

#[test]
fn an_expired_token_asks_to_reconnect() {
    let transport = FakeTransport::new();
    transport.respond_once(json_response_with_status_401());

    let error = oauth_client(&transport)
        .list_open_pull_requests("expired", &repo(), &ResponseCache::new())
        .unwrap_err();
    assert_eq!(error.code, GitHubErrorCode::AuthRequired);
}

fn json_response_with_status_401() -> Reply {
    common::json_response_with_status(json!({ "message": "Bad credentials" }), 401)
}

#[test]
fn network_failures_do_not_leak_the_token() {
    let transport = FakeTransport::new();
    transport.respond(Err(TransportError::Network("dns failure".to_string())));

    let error = oauth_client(&transport)
        .list_open_pull_requests("secret-token", &repo(), &ResponseCache::new())
        .unwrap_err();
    assert_eq!(error.code, GitHubErrorCode::GithubUnavailable);
    assert!(!format!("{error:?}").contains("secret-token"));
    assert!(!format!("{:?}", transport.calls()[0]).contains("secret-token"));
}

#[test]
fn finds_the_pull_request_of_a_branch_by_its_head() {
    let transport = FakeTransport::new();
    transport.respond_once(response(200, pulls(7..=7), &[]));
    transport.respond_once(response(200, json!([]), &[]));

    let cache = ResponseCache::new();
    let client = oauth_client(&transport);
    let found = client
        .find_pull_request_for_branch("tok", &repo(), "acme", "feat/7 x", &cache)
        .unwrap();
    assert_eq!(found.map(|pull| pull.number), Some(7));
    assert_eq!(
        transport.calls()[0].url,
        "https://api.github.com/repos/acme/web/pulls?state=open&head=acme%3Afeat%2F7+x&per_page=1"
    );

    let missing = client
        .find_pull_request_for_branch("tok", &repo(), "acme", "other", &cache)
        .unwrap();
    assert_eq!(missing, None);

    // Sin rama (HEAD desanclado) no se pregunta.
    assert_eq!(
        client
            .find_pull_request_for_branch("tok", &repo(), "acme", "", &cache)
            .unwrap(),
        None
    );
    assert_eq!(transport.call_count(), 2);
}

#[test]
fn summarizes_the_reviews_of_a_pull_request() {
    let transport = FakeTransport::new();
    transport.respond_once(json_response(json!([
        { "user": { "login": "bea" }, "state": "CHANGES_REQUESTED" },
        { "user": { "login": "bea" }, "state": "APPROVED" },
        { "user": { "login": "carl" }, "state": "COMMENTED" }
    ])));

    let summary = oauth_client(&transport)
        .pull_request_reviews("tok", &repo(), 12, 1, &ResponseCache::new())
        .unwrap();
    assert_eq!(summary.decision, ReviewDecision::Approved);
    assert_eq!(summary.approvals, 1);
    assert_eq!(
        transport.calls()[0].url,
        "https://api.github.com/repos/acme/web/pulls/12/reviews?per_page=100"
    );
}

#[test]
fn combines_check_runs_and_commit_statuses() {
    let transport = Arc::new(FakeTransport::default());
    transport.respond_once(json_response(json!({ "total_count": 2, "check_runs": [
        { "status": "completed", "conclusion": "success" },
        { "status": "completed", "conclusion": "success" }
    ]})));
    transport.respond_once(json_response(json!({ "state": "pending", "statuses": [
        { "state": "pending", "context": "ci/legacy" }
    ]})));

    let summary = oauth_client(&transport)
        .commit_checks("tok", &repo(), "abc123", &ResponseCache::new())
        .unwrap();
    assert_eq!(summary.state(), Some(CheckState::Pending));
    assert_eq!((summary.passed, summary.total), (2, 3));

    let urls: Vec<String> = transport.calls().into_iter().map(|call| call.url).collect();
    assert_eq!(
        urls,
        [
            "https://api.github.com/repos/acme/web/commits/abc123/check-runs?per_page=100",
            "https://api.github.com/repos/acme/web/commits/abc123/status?per_page=100",
        ]
    );
}

#[test]
fn checks_without_permission_count_what_can_be_read() {
    let transport = FakeTransport::new();
    transport.respond_once(response(
        403,
        json!({ "message": "Resource not accessible by integration" }),
        &[("x-ratelimit-remaining", "4000")],
    ));
    transport.respond_once(json_response(json!({ "state": "success", "statuses": [
        { "state": "success" }
    ]})));

    let summary = oauth_client(&transport)
        .commit_checks("tok", &repo(), "abc123", &ResponseCache::new())
        .unwrap();
    assert_eq!(summary.state(), Some(CheckState::Success));
    assert_eq!(summary.total, 1);
}

#[test]
fn checks_stop_when_the_rate_limit_runs_out() {
    let transport = FakeTransport::new();
    transport.respond_once(response(
        403,
        json!({ "message": "API rate limit exceeded" }),
        &[("x-ratelimit-remaining", "0"), ("x-ratelimit-reset", "10")],
    ));

    let error = oauth_client(&transport)
        .commit_checks("tok", &repo(), "abc123", &ResponseCache::new())
        .unwrap_err();
    assert_eq!(error.code, GitHubErrorCode::RateLimited);
    assert_eq!(transport.call_count(), 1);
}
