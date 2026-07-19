//! The GitHub client against a mocked network boundary.
//!
//! Every test serves canned responses from a local server, so the suite never
//! reaches the real API and the failure modes that matter, throttling and
//! partial outages, can be provoked deliberately.

use std::time::Duration;

use fleet::github::{Account, GitHubClient, GitHubError, RetryPolicy, Token};
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
        "name": "Fleet",
        "owner": { "login": "McAnouil" },
        "isPrivate": true,
        "isArchived": true,
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
                "publishedAt": "2026-07-01T00:00:00Z",
                "releaseAssets": { "nodes": [{ "downloadCount": 7 }, { "downloadCount": 8 }] }
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
        "github:mcanouil/fleet",
        "an identity must be case-normalised however the API cased it"
    );
    assert_eq!(repository.default_branch.as_deref(), Some("main"));
    assert!(repository.is_private);
    assert!(repository.is_archived);
    assert!(repository.is_fork());
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
        release.downloads, 15,
        "downloads must be summed across every asset"
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
async fn retries_a_throttled_request_and_succeeds_when_the_limit_clears() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(403)
                .insert_header("x-ratelimit-remaining", "0")
                .insert_header("x-ratelimit-reset", "1750000000"),
        )
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
async fn treats_a_graphql_rate_limit_error_as_throttling() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": null,
            "errors": [{ "message": "API rate limit exceeded", "type": "RATE_LIMITED" }]
        })))
        .expect(2)
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
    let (token, source) = fleet::github::auth::resolve_token_from_system()
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
