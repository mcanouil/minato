//! The GitHub client against a mocked network boundary.
//!
//! Every test serves canned responses from a local server, so the suite never
//! reaches the real API and the failure modes that matter, throttling and
//! partial outages, can be provoked deliberately.

use std::time::Duration;

use minato::github::{Account, GitHubClient, GitHubError, RetryPolicy, Token};
use minato::model::{Provider, RepoId};
use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

/// A retry policy that exhausts immediately, so throttling tests do not sleep.
fn impatient() -> RetryPolicy {
    RetryPolicy {
        attempts: 2,
        backoff: Duration::ZERO,
    }
}

fn client_for(server: &MockServer) -> GitHubClient {
    GitHubClient::with_endpoint(Token::new("test-token"), None, server.uri())
        .expect("a client")
        .with_retry(impatient())
}

/// One repository node, with only the fields a test cares about overridden.
fn repository(name: &str) -> serde_json::Value {
    json!({
        "name": name,
        "owner": { "login": "mcanouil" },
        "isPrivate": false,
        "isArchived": false,
        "isFork": false,
        "defaultBranchRef": { "name": "main" },
        "parent": null,
        "stargazerCount": 0,
        "forkCount": 0,
        "issues": { "totalCount": 0 },
        "pullRequests": { "totalCount": 0 },
        "discussions": { "totalCount": 0 },
        "pushedAt": null,
        "primaryLanguage": null,
        "licenseInfo": null,
        "releases": { "nodes": [] }
    })
}

fn page(nodes: &[serde_json::Value], next: Option<&str>) -> serde_json::Value {
    json!({
        "data": {
            "repositoryOwner": {
                "repositories": {
                    "pageInfo": {
                        "hasNextPage": next.is_some(),
                        "endCursor": next
                    },
                    "nodes": nodes
                }
            }
        }
    })
}

#[tokio::test]
async fn reads_every_field_a_repository_reports() {
    let server = MockServer::start().await;

    let node = json!({
        "name": "Minato",
        "owner": { "login": "McAnouil" },
        "isPrivate": true,
        "isArchived": true,
        "isFork": true,
        "defaultBranchRef": { "name": "main" },
        "parent": { "name": "Upstream", "owner": { "login": "SomeOrg" } },
        "stargazerCount": 12,
        "forkCount": 3,
        "issues": { "totalCount": 4 },
        "pullRequests": { "totalCount": 5 },
        "discussions": { "totalCount": 6 },
        "pushedAt": "2026-07-19T09:10:11Z",
        "primaryLanguage": { "name": "Rust" },
        "licenseInfo": { "spdxId": "MIT" },
        "releases": {
            "nodes": [{
                "tagName": "v1.2.3",
                "publishedAt": "2026-07-01T00:00:00Z"
            }]
        }
    });

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(&[node], None)))
        .mount(&server)
        .await;

    let repositories = client_for(&server)
        .repositories(&Account::new("mcanouil"))
        .await
        .expect("a listing");

    let repository = &repositories[0];

    assert_eq!(
        repository.id.to_string(),
        "github:mcanouil/minato",
        "an identity must be case-normalised however the API cased it"
    );
    assert_eq!(repository.default_branch.as_deref(), Some("main"));
    assert!(repository.is_private);
    assert!(repository.is_archived);
    assert!(repository.is_fork);
    assert!(repository.has_upstream());
    assert_eq!(
        repository.upstream.as_ref().map(ToString::to_string),
        Some("github:someorg/upstream".to_owned())
    );
    assert_eq!(repository.metadata.stars, 12);
    assert_eq!(repository.metadata.forks, 3);
    assert_eq!(repository.metadata.open_issues, 4);
    assert_eq!(repository.metadata.open_pull_requests, 5);
    assert_eq!(repository.metadata.discussions, Some(6));
    assert_eq!(repository.metadata.language.as_deref(), Some("Rust"));
    assert_eq!(repository.metadata.licence.as_deref(), Some("MIT"));

    let release = repository
        .metadata
        .latest_release
        .as_ref()
        .expect("a release");

    assert_eq!(release.tag, "v1.2.3");
    assert_eq!(
        release.published.map(|published| published.to_string()),
        Some("2026-07-01T00:00:00Z".to_owned()),
        "the release's publish time must survive"
    );
}

#[tokio::test]
async fn follows_pagination_until_the_last_page() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .respond_with(|request: &Request| {
            let body: serde_json::Value = request.body_json().expect("a GraphQL request");
            let after = body["variables"]["after"].as_str().map(ToOwned::to_owned);

            match after.as_deref() {
                None => ResponseTemplate::new(200)
                    .set_body_json(page(&[repository("one")], Some("cursor-1"))),
                Some("cursor-1") => ResponseTemplate::new(200)
                    .set_body_json(page(&[repository("two")], Some("cursor-2"))),
                _ => ResponseTemplate::new(200).set_body_json(page(&[repository("three")], None)),
            }
        })
        .mount(&server)
        .await;

    let repositories = client_for(&server)
        .repositories(&Account::new("mcanouil"))
        .await
        .expect("a listing");

    let names: Vec<_> = repositories
        .iter()
        .map(|repo| repo.id.name.clone())
        .collect();

    assert_eq!(names, ["one", "two", "three"]);
}

#[tokio::test]
async fn stops_when_a_page_claims_more_but_gives_no_cursor() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "repositoryOwner": {
                    "repositories": {
                        "pageInfo": { "hasNextPage": true, "endCursor": null },
                        "nodes": [repository("only")]
                    }
                }
            }
        })))
        .mount(&server)
        .await;

    let repositories = client_for(&server)
        .repositories(&Account::new("mcanouil"))
        .await
        .expect("a listing rather than an endless loop");

    assert_eq!(repositories.len(), 1);
}

#[tokio::test]
async fn sends_the_token_as_a_bearer_credential() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(header("authorization", "Bearer test-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(&[], None)))
        .expect(1)
        .mount(&server)
        .await;

    client_for(&server)
        .repositories(&Account::new("mcanouil"))
        .await
        .expect("a listing");
}

#[tokio::test]
async fn reports_a_rejected_token_without_retrying() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(401))
        .expect(1)
        .mount(&server)
        .await;

    let error = client_for(&server)
        .repositories(&Account::new("mcanouil"))
        .await
        .expect_err("a rejection");

    assert!(matches!(error, GitHubError::Unauthorised { .. }));
    assert!(
        error.to_string().contains("gh auth login"),
        "the error should say how to fix it, got: {error}"
    );
}

#[tokio::test]
async fn retries_a_secondary_limit_and_succeeds_when_it_clears() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(429))
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(&[repository("one")], None)))
        .mount(&server)
        .await;

    let repositories = client_for(&server)
        .repositories(&Account::new("mcanouil"))
        .await
        .expect("the retry to succeed");

    assert_eq!(repositories.len(), 1);
}

#[tokio::test]
async fn gives_up_on_persistent_throttling_and_names_the_reset_time() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(429).insert_header("x-ratelimit-reset", "1750000000"))
        .expect(2)
        .mount(&server)
        .await;

    let error = client_for(&server)
        .repositories(&Account::new("mcanouil"))
        .await
        .expect_err("exhausted retries");

    assert!(matches!(error, GitHubError::RateLimited { .. }));
    assert!(
        error.to_string().contains("resets at"),
        "the error should say when to try again, got: {error}"
    );
}

#[tokio::test]
async fn does_not_retry_a_rate_limit_reported_in_the_body() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": null,
            "errors": [{ "message": "API rate limit exceeded", "type": "RATE_LIMITED" }]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let error = client_for(&server)
        .repositories(&Account::new("mcanouil"))
        .await
        .expect_err("throttling reported in the body");

    assert!(matches!(error, GitHubError::RateLimited { .. }));
}

#[tokio::test]
async fn reports_an_unknown_account_with_where_to_correct_it() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "data": { "repositoryOwner": null } })),
        )
        .mount(&server)
        .await;

    let error = client_for(&server)
        .repositories(&Account::new("nope"))
        .await
        .expect_err("an unknown account");

    assert!(matches!(error, GitHubError::UnknownAccount { .. }));
    assert!(
        error.to_string().contains("providers.github"),
        "the error should point at the configuration, got: {error}"
    );
}

#[tokio::test]
async fn surfaces_api_errors_rather_than_treating_them_as_an_empty_listing() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": null,
            "errors": [
                { "message": "Something went wrong", "type": "INTERNAL" },
                { "message": "And another thing", "type": null }
            ]
        })))
        .mount(&server)
        .await;

    let error = client_for(&server)
        .repositories(&Account::new("mcanouil"))
        .await
        .expect_err("the reported errors");

    assert!(matches!(error, GitHubError::Api { .. }));
    assert!(error.to_string().contains("Something went wrong"));
    assert!(
        error.to_string().contains("And another thing"),
        "every reported message should survive, got: {error}"
    );
}

#[tokio::test]
async fn reports_an_unreadable_body_against_the_account_being_listed() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html>502 Bad Gateway</html>"))
        .mount(&server)
        .await;

    let error = client_for(&server)
        .repositories(&Account::new("mcanouil"))
        .await
        .expect_err("a malformed body");

    assert!(matches!(error, GitHubError::Malformed { .. }));
    assert!(
        error.to_string().contains("mcanouil"),
        "the error should name the account, got: {error}"
    );
}

#[tokio::test]
async fn treats_a_forbidden_response_with_quota_left_as_a_real_refusal() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(403)
                .insert_header("x-ratelimit-remaining", "4999")
                .set_body_string("no"),
        )
        .expect(1)
        .mount(&server)
        .await;

    let error = client_for(&server)
        .repositories(&Account::new("mcanouil"))
        .await
        .expect_err("a refusal");

    assert!(
        !matches!(error, GitHubError::RateLimited { .. }),
        "a forbidden response with quota left must not be retried as throttling, got: {error}"
    );
}

#[tokio::test]
async fn reports_an_absent_discussion_count_as_unknown_rather_than_zero() {
    let server = MockServer::start().await;

    let mut node = repository("one");
    node["discussions"] = json!(null);

    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(&[node], None)))
        .mount(&server)
        .await;

    let repositories = client_for(&server)
        .repositories(&Account::new("mcanouil"))
        .await
        .expect("a listing");

    assert_eq!(
        repositories[0].metadata.discussions, None,
        "an unsupported field must stay distinguishable from a count of zero"
    );
}

/// Checks the query against the real API.
///
/// Mocked tests cannot tell whether the GraphQL document is valid, only whether
/// the client handles a response shape. This is ignored by default so the suite
/// stays offline and deterministic; run it with
/// `cargo test --test github_client -- --ignored` after `gh auth login`.
#[tokio::test]
#[ignore = "requires network access and a GitHub token"]
async fn the_query_is_accepted_by_the_real_api() {
    let (token, source) = minato::github::auth::resolve_token_from_system()
        .expect("a token from gh or the environment");

    let repositories = GitHubClient::new(token, Some(source))
        .expect("a client")
        .repositories(&Account::new("mcanouil"))
        .await
        .expect("the real API to accept the query");

    assert!(
        !repositories.is_empty(),
        "the account should report at least one repository"
    );
}

#[tokio::test]
async fn fails_rather_than_paging_forever_when_a_cursor_repeats() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "repositoryOwner": { "repositories": {
                "pageInfo": { "hasNextPage": true, "endCursor": "stuck" },
                "nodes": []
            }}}
        })))
        .mount(&server)
        .await;

    let error = tokio::time::timeout(
        Duration::from_secs(10),
        client_for(&server).repositories(&Account::new("mcanouil")),
    )
    .await
    .expect("an error rather than a hang")
    .expect_err("stalled pagination");

    assert!(matches!(error, GitHubError::StalledPagination { .. }));
}

#[tokio::test]
async fn does_not_retry_an_exhausted_hourly_quota() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(403)
                .insert_header("x-ratelimit-remaining", "0")
                .insert_header("x-ratelimit-reset", "1750000000"),
        )
        .expect(1)
        .mount(&server)
        .await;

    let error = client_for(&server)
        .repositories(&Account::new("mcanouil"))
        .await
        .expect_err("an immediate failure");

    assert!(matches!(error, GitHubError::RateLimited { .. }));
    assert!(
        error.to_string().contains("resets at"),
        "the error should say when the quota returns, got: {error}"
    );
}

#[tokio::test]
async fn reports_a_fork_whose_parent_was_deleted_as_a_fork_without_an_upstream() {
    let server = MockServer::start().await;

    let mut node = repository("orphaned");
    node["isFork"] = json!(true);
    node["parent"] = json!(null);

    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(&[node], None)))
        .mount(&server)
        .await;

    let repositories = client_for(&server)
        .repositories(&Account::new("mcanouil"))
        .await
        .expect("a listing");

    assert!(repositories[0].is_fork, "it is still a fork");
    assert!(
        !repositories[0].has_upstream(),
        "but there is no parent left to compare against"
    );
}

#[tokio::test]
async fn retries_a_server_error_rather_than_calling_it_unreadable() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(502).set_body_string("<html>Bad Gateway</html>"))
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(&[repository("one")], None)))
        .mount(&server)
        .await;

    let repositories = client_for(&server)
        .repositories(&Account::new("mcanouil"))
        .await
        .expect("the retry to succeed");

    assert_eq!(repositories.len(), 1);
}

#[tokio::test]
async fn a_persistent_server_error_is_reported_as_throttling_not_as_bad_json() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(503))
        .expect(2)
        .mount(&server)
        .await;

    let error = client_for(&server)
        .repositories(&Account::new("mcanouil"))
        .await
        .expect_err("exhausted retries");

    assert!(
        matches!(error, GitHubError::Unexpected { status: 503, .. }),
        "a spent server-error retry reports the server error, not an unreadable body or a rate limit, got: {error}"
    );
}

#[tokio::test]
async fn an_unexpected_status_names_the_status_rather_than_the_body() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(418).set_body_string("not json"))
        .expect(1)
        .mount(&server)
        .await;

    let error = client_for(&server)
        .repositories(&Account::new("mcanouil"))
        .await
        .expect_err("an unexpected status");

    assert!(matches!(error, GitHubError::Unexpected { status: 418, .. }));
    assert!(
        error.to_string().contains("418"),
        "the error should name the status, got: {error}"
    );
}

#[tokio::test]
async fn a_fork_behind_its_parent_is_reported_as_behind_not_ahead() {
    let server = MockServer::start().await;

    let mut fork = repository("pandoc");
    fork["isFork"] = json!(true);
    fork["parent"] = json!({
        "name": "pandoc",
        "owner": { "login": "jgm" },
        "defaultBranchRef": { "name": "main" }
    });

    Mock::given(method("POST"))
        .respond_with(move |request: &Request| {
            let body: serde_json::Value = request.body_json().expect("a GraphQL request");
            let query = body["query"].as_str().unwrap_or_default();

            // The second request is the batched fork comparison.
            if query.contains("compare") {
                // GitHub reports the parent as 429 ahead of the fork.
                return ResponseTemplate::new(200).set_body_json(json!({
                    "data": {
                        "f0": { "defaultBranchRef": { "compare": { "aheadBy": 429, "behindBy": 0 } } }
                    }
                }));
            }

            ResponseTemplate::new(200).set_body_json(page(&[fork.clone()], None))
        })
        .mount(&server)
        .await;

    let repositories = client_for(&server)
        .repositories(&Account::new("mcanouil"))
        .await
        .expect("a listing");

    let upstream = repositories[0]
        .metadata
        .upstream
        .expect("a comparison against the parent");

    assert_eq!(
        upstream.behind, 429,
        "the parent being ahead means the fork is behind"
    );
    assert_eq!(upstream.ahead, 0);
    assert!(upstream.is_behind());
    assert!(
        upstream.can_fast_forward(),
        "a fork with no commits of its own can simply catch up"
    );
}

#[tokio::test]
async fn a_fork_that_cannot_be_compared_is_unknown_rather_than_level() {
    let server = MockServer::start().await;

    let mut fork = repository("orphaned");
    fork["isFork"] = json!(true);
    fork["parent"] = json!(null);

    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(&[fork], None)))
        .mount(&server)
        .await;

    let repositories = client_for(&server)
        .repositories(&Account::new("mcanouil"))
        .await
        .expect("a listing");

    assert_eq!(
        repositories[0].metadata.upstream, None,
        "a fork with no visible parent must not look up to date with it"
    );
}

#[tokio::test]
async fn merge_upstream_fast_forwards_a_fork() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/repos/mcanouil/forked/merge-upstream"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(
                json!({ "merge_type": "fast-forward", "message": "Fast-forwarded" }),
            ),
        )
        .expect(1)
        .mount(&server)
        .await;

    client_for(&server)
        .merge_upstream(&RepoId::new(Provider::GitHub, "mcanouil", "forked"), "main")
        .await
        .expect("the sync to succeed");
}

#[tokio::test]
async fn merge_upstream_reports_a_diverged_fork_rather_than_merging() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/repos/mcanouil/forked/merge-upstream"))
        .respond_with(ResponseTemplate::new(409))
        .mount(&server)
        .await;

    let error = client_for(&server)
        .merge_upstream(&RepoId::new(Provider::GitHub, "mcanouil", "forked"), "main")
        .await
        .expect_err("a diverged fork cannot be fast-forwarded");

    assert!(matches!(error, GitHubError::SyncFailed { .. }));
    assert!(
        error.to_string().contains("diverged"),
        "the error should explain why, got: {error}"
    );
}

/// A server that accepts each connection, reads the request, then drops the
/// socket without replying, the way GitHub cancels a response it has begun.
///
/// It counts the connections it accepted, so a test can tell whether the client
/// tried again. The thread is detached and left blocked on `accept`; the
/// process ends it, and a bound socket costs nothing.
fn dropping_listener() -> (String, std::sync::Arc<std::sync::atomic::AtomicU32>) {
    use std::io::Read as _;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a listener");
    let address = listener.local_addr().expect("an address");
    let connections = Arc::new(AtomicU32::new(0));

    let seen = Arc::clone(&connections);
    std::thread::spawn(move || {
        while let Ok((mut socket, _)) = listener.accept() {
            seen.fetch_add(1, Ordering::SeqCst);
            let _ = socket.read(&mut [0_u8; 1024]);
        }
    });

    (format!("http://{address}"), connections)
}

#[tokio::test]
async fn retries_a_connection_reset_mid_response() {
    use std::sync::atomic::Ordering;

    let (endpoint, connections) = dropping_listener();

    let client = GitHubClient::with_endpoint(Token::new("test-token"), None, endpoint)
        .expect("a client")
        .with_retry(impatient());

    let error = tokio::time::timeout(
        Duration::from_secs(10),
        client.repositories(&Account::new("mcanouil")),
    )
    .await
    .expect("an error rather than a hang")
    .expect_err("a dropped connection should fail once retries are spent");

    // A reset after the connection was made is retried, then, once the retries
    // are spent, reported as an unreachable host rather than as a rate limit,
    // which would send the user to the wrong remedy.
    assert!(
        matches!(error, GitHubError::Unreachable { .. }),
        "a spent reset retry should report an unreachable host, got: {error}"
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        connections.load(Ordering::SeqCst),
        2,
        "the impatient policy should make one retry, so two connections in all"
    );
}
