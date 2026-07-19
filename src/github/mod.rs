//! Talking to GitHub.
//!
//! The client is concrete rather than hidden behind a trait. A trait designed
//! against one implementation encodes that implementation's assumptions, so the
//! abstraction waits until a second provider makes its real shape visible.

pub mod auth;
mod schema;

use std::time::Duration;

use jiff::Timestamp;
use serde::Serialize;

use crate::model::RemoteRepo;

pub use auth::{Token, TokenSource};

use schema::{RepositoriesData, Response};

/// Where the GraphQL API lives.
pub const DEFAULT_ENDPOINT: &str = "https://api.github.com/graphql";

/// How `fleet` identifies itself to the API.
const USER_AGENT: &str = concat!("fleet/", env!("CARGO_PKG_VERSION"));

/// An account whose repositories are enumerated.
///
/// GitHub resolves users and organisations through the same field, so the two
/// do not need to be distinguished when querying.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account(String);

impl Account {
    /// Names an account by its login.
    #[must_use]
    pub fn new(login: impl Into<String>) -> Self {
        Self(login.into())
    }

    /// The login as written.
    #[must_use]
    pub fn login(&self) -> &str {
        &self.0
    }
}

/// How often, and how patiently, a throttled request is retried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    /// How many attempts to make in total, including the first.
    pub attempts: u32,

    /// How long to wait before the first retry; each retry doubles it.
    pub backoff: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            attempts: 4,
            backoff: Duration::from_secs(1),
        }
    }
}

impl RetryPolicy {
    /// How long to wait before the retry following `attempt`, counting from
    /// zero.
    fn delay_after(self, attempt: u32) -> Duration {
        self.backoff * 2_u32.saturating_pow(attempt)
    }
}

/// Anything that can go wrong talking to GitHub.
#[derive(Debug, thiserror::Error)]
pub enum GitHubError {
    /// The token was rejected.
    #[error(
        "GitHub rejected the token{}; check it has not expired and carries the `repo` scope, or run `gh auth login` again",
        source_note(*token_source)
    )]
    Unauthorised {
        /// Where the rejected token came from, when known.
        token_source: Option<TokenSource>,
    },

    /// The rate limit was hit and did not clear within the retry budget.
    #[error(
        "GitHub rate limit reached while listing repositories for `{account}`{}; wait and run the command again, or use a token with a higher limit",
        reset_note(*reset)
    )]
    RateLimited {
        /// The account being listed when the limit was hit.
        account: String,
        /// When the limit resets, when GitHub says.
        reset: Option<Timestamp>,
    },

    /// No such user or organisation exists, or it is not visible.
    #[error(
        "GitHub has no visible user or organisation called `{account}`; check the spelling in `providers.github`, and that the token can see it if it is private"
    )]
    UnknownAccount {
        /// The account that could not be resolved.
        account: String,
    },

    /// The API reported errors.
    #[error("GitHub reported an error while listing repositories for `{account}`: {messages}")]
    Api {
        /// The account being listed.
        account: String,
        /// The messages GitHub returned, joined.
        messages: String,
    },

    /// The response could not be understood.
    #[error(
        "cannot understand GitHub's response while listing repositories for `{account}`: {source}"
    )]
    Malformed {
        /// The account being listed.
        account: String,
        /// What went wrong reading the body.
        #[source]
        source: serde_json::Error,
    },

    /// The request never completed.
    #[error("cannot reach GitHub while listing repositories for `{account}`: {source}")]
    Transport {
        /// The account being listed.
        account: String,
        /// The underlying failure.
        #[source]
        source: reqwest::Error,
    },

    /// The client could not be constructed.
    #[error("cannot build an HTTP client for GitHub: {source}")]
    Client {
        /// The underlying failure.
        #[source]
        source: reqwest::Error,
    },
}

fn source_note(source: Option<TokenSource>) -> String {
    source.map_or_else(String::new, |source| format!(" from {source}"))
}

fn reset_note(reset: Option<Timestamp>) -> String {
    reset.map_or_else(String::new, |reset| format!(", which resets at {reset}"))
}

/// The body of a GraphQL request.
#[derive(Debug, Serialize)]
struct Request<'a> {
    query: &'a str,
    variables: serde_json::Value,
}

/// A client for GitHub's GraphQL API.
#[derive(Debug, Clone)]
pub struct GitHubClient {
    http: reqwest::Client,
    endpoint: String,
    token: Token,
    token_source: Option<TokenSource>,
    retry: RetryPolicy,
}

impl GitHubClient {
    /// Builds a client against the public API.
    ///
    /// # Errors
    ///
    /// Returns an error when the underlying HTTP client cannot be built.
    pub fn new(token: Token, token_source: Option<TokenSource>) -> Result<Self, GitHubError> {
        Self::with_endpoint(token, token_source, DEFAULT_ENDPOINT)
    }

    /// Builds a client against a specific endpoint.
    ///
    /// # Errors
    ///
    /// Returns an error when the underlying HTTP client cannot be built.
    pub fn with_endpoint(
        token: Token,
        token_source: Option<TokenSource>,
        endpoint: impl Into<String>,
    ) -> Result<Self, GitHubError> {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .build()
            .map_err(|source| GitHubError::Client { source })?;

        Ok(Self {
            http,
            endpoint: endpoint.into(),
            token,
            token_source,
            retry: RetryPolicy::default(),
        })
    }

    /// Replaces the retry policy, mainly so that tests need not wait.
    #[must_use]
    pub const fn with_retry(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    /// Lists every repository owned by `account`, following pagination.
    ///
    /// # Errors
    ///
    /// Returns an error when the token is rejected, the account cannot be
    /// resolved, the rate limit does not clear within the retry budget, or the
    /// response cannot be understood. Every variant names the account.
    pub async fn repositories(&self, account: &Account) -> Result<Vec<RemoteRepo>, GitHubError> {
        let mut repositories = Vec::new();
        let mut after: Option<String> = None;

        loop {
            let page = self.page(account, after.as_deref()).await?;

            repositories.extend(page.nodes.into_iter().map(RemoteRepo::from));

            if !page.page_info.has_next_page {
                break;
            }

            let Some(cursor) = page.page_info.end_cursor else {
                break;
            };

            after = Some(cursor);
        }

        Ok(repositories)
    }

    /// Fetches one page, retrying while the rate limit says to.
    async fn page(
        &self,
        account: &Account,
        after: Option<&str>,
    ) -> Result<schema::RepositoryConnection, GitHubError> {
        let mut last_reset = None;

        for attempt in 0..self.retry.attempts {
            match self.page_once(account, after).await {
                Err(GitHubError::RateLimited { reset, .. }) => {
                    last_reset = reset;

                    if attempt + 1 < self.retry.attempts {
                        tokio::time::sleep(self.retry.delay_after(attempt)).await;
                    }
                }
                outcome => return outcome,
            }
        }

        Err(GitHubError::RateLimited {
            account: account.login().to_owned(),
            reset: last_reset,
        })
    }

    /// Fetches one page, without retrying.
    async fn page_once(
        &self,
        account: &Account,
        after: Option<&str>,
    ) -> Result<schema::RepositoryConnection, GitHubError> {
        let login = account.login();

        let response = self
            .http
            .post(&self.endpoint)
            .bearer_auth(self.token.expose())
            .json(&Request {
                query: schema::REPOSITORIES_QUERY,
                variables: schema::variables(login, after),
            })
            .send()
            .await
            .map_err(|source| GitHubError::Transport {
                account: login.to_owned(),
                source,
            })?;

        let status = response.status();
        let reset = rate_limit_reset(response.headers());

        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(GitHubError::Unauthorised {
                token_source: self.token_source,
            });
        }

        if is_rate_limited(status, response.headers()) {
            return Err(GitHubError::RateLimited {
                account: login.to_owned(),
                reset,
            });
        }

        let body = response
            .text()
            .await
            .map_err(|source| GitHubError::Transport {
                account: login.to_owned(),
                source,
            })?;

        let parsed: Response<RepositoriesData> =
            serde_json::from_str(&body).map_err(|source| GitHubError::Malformed {
                account: login.to_owned(),
                source,
            })?;

        if parsed
            .errors
            .iter()
            .any(schema::ResponseError::is_rate_limited)
        {
            return Err(GitHubError::RateLimited {
                account: login.to_owned(),
                reset,
            });
        }

        if !parsed.errors.is_empty() {
            return Err(GitHubError::Api {
                account: login.to_owned(),
                messages: parsed
                    .errors
                    .iter()
                    .map(|error| error.message.as_str())
                    .collect::<Vec<_>>()
                    .join("; "),
            });
        }

        parsed
            .data
            .and_then(|data| data.owner)
            .map(|owner| owner.repositories)
            .ok_or_else(|| GitHubError::UnknownAccount {
                account: login.to_owned(),
            })
    }
}

/// Whether a response says the request was throttled.
///
/// GitHub signals a primary limit with 403 and an exhausted remaining count,
/// and a secondary limit with 429.
fn is_rate_limited(status: reqwest::StatusCode, headers: &reqwest::header::HeaderMap) -> bool {
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return true;
    }

    if status != reqwest::StatusCode::FORBIDDEN {
        return false;
    }

    headers
        .get("x-ratelimit-remaining")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .is_some_and(|remaining| remaining == 0)
        || headers.contains_key("retry-after")
}

/// When GitHub says the limit resets, if it says.
fn rate_limit_reset(headers: &reqwest::header::HeaderMap) -> Option<Timestamp> {
    let seconds = headers
        .get("x-ratelimit-reset")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<i64>().ok())?;

    Timestamp::from_second(seconds).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doubles_the_delay_between_attempts() {
        let policy = RetryPolicy {
            attempts: 4,
            backoff: Duration::from_secs(1),
        };

        assert_eq!(policy.delay_after(0), Duration::from_secs(1));
        assert_eq!(policy.delay_after(1), Duration::from_secs(2));
        assert_eq!(policy.delay_after(2), Duration::from_secs(4));
    }

    #[test]
    fn treats_too_many_requests_as_throttling() {
        let headers = reqwest::header::HeaderMap::new();

        assert!(is_rate_limited(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            &headers
        ));
    }

    #[test]
    fn treats_a_forbidden_response_with_no_quota_left_as_throttling() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("x-ratelimit-remaining", "0".parse().unwrap());

        assert!(is_rate_limited(reqwest::StatusCode::FORBIDDEN, &headers));
    }

    #[test]
    fn treats_a_forbidden_response_with_quota_left_as_a_real_refusal() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("x-ratelimit-remaining", "42".parse().unwrap());

        assert!(
            !is_rate_limited(reqwest::StatusCode::FORBIDDEN, &headers),
            "a forbidden response with quota left is a permissions problem, not throttling"
        );
    }

    #[test]
    fn reads_the_reset_time_when_present() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("x-ratelimit-reset", "1750000000".parse().unwrap());

        assert_eq!(
            rate_limit_reset(&headers),
            Timestamp::from_second(1_750_000_000).ok()
        );
    }

    #[test]
    fn tolerates_a_missing_or_unparsable_reset_time() {
        let mut headers = reqwest::header::HeaderMap::new();

        assert_eq!(rate_limit_reset(&headers), None);

        headers.insert("x-ratelimit-reset", "soon".parse().unwrap());

        assert_eq!(rate_limit_reset(&headers), None);
    }
}
