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
const DEFAULT_ENDPOINT: &str = "https://api.github.com/graphql";

/// How `minato` identifies itself to the API.
const USER_AGENT: &str = concat!("minato/", env!("CARGO_PKG_VERSION"));

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

/// The longest `minato` will honour a `Retry-After` before giving up instead.
///
/// A secondary limit normally clears in under a minute. A far longer wait is
/// better reported than slept through, so the user can decide.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(300);

/// A throttling response, and what it implies about waiting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Throttle {
    /// How long GitHub asked us to wait, when it said.
    retry_after: Option<Duration>,

    /// When the limit resets, when GitHub said.
    reset: Option<Timestamp>,

    /// Whether waiting a short while could plausibly clear it.
    ///
    /// A secondary limit clears in seconds, so it is worth retrying. A primary
    /// limit is an hourly quota, so retrying only wastes the user's time.
    transient: bool,
}

impl Throttle {
    /// How long to wait before the retry following `attempt`, or `None` when
    /// waiting cannot help.
    fn delay(self, policy: RetryPolicy, attempt: u32) -> Option<Duration> {
        if !self.transient {
            return None;
        }

        match self.retry_after {
            Some(requested) if requested > MAX_RETRY_AFTER => None,
            Some(requested) => Some(requested),
            None => Some(policy.delay_after(attempt)),
        }
    }
}

/// Why fetching one page did not produce a page.
///
/// Throttling is kept apart from every other failure so that the retry loop
/// decides whether to wait by matching on this, rather than by inspecting the
/// variants of a general-purpose error.
#[derive(Debug)]
enum PageFailure {
    /// The request was throttled.
    Throttled(Throttle),

    /// Something else went wrong, and waiting will not help.
    Failed(GitHubError),
}

impl From<GitHubError> for PageFailure {
    fn from(error: GitHubError) -> Self {
        Self::Failed(error)
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

    /// The same page cursor came back twice, so following it would not advance.
    #[error(
        "GitHub returned the same page cursor twice while listing repositories for `{account}`, so paging would not finish; this usually means a partial outage, so run the command again"
    )]
    StalledPagination {
        /// The account being listed.
        account: String,
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

    /// GitHub answered with a status that means nothing `minato` expects.
    #[error(
        "GitHub returned {status} while listing repositories for `{account}`; if this persists, check https://www.githubstatus.com"
    )]
    Unexpected {
        /// The account being listed.
        account: String,
        /// The status code returned.
        status: u16,
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

            // A cursor that does not advance would page forever. Failing is
            // better than hanging, since a hang reports nothing at all.
            if after.as_deref() == Some(cursor.as_str()) {
                return Err(GitHubError::StalledPagination {
                    account: account.login().to_owned(),
                });
            }

            after = Some(cursor);
        }

        Ok(repositories)
    }

    /// Fetches one page, waiting and retrying only when that could help.
    async fn page(
        &self,
        account: &Account,
        after: Option<&str>,
    ) -> Result<schema::RepositoryConnection, GitHubError> {
        let mut last_reset = None;

        for attempt in 0..self.retry.attempts {
            let throttle = match self.page_once(account, after).await {
                Ok(page) => return Ok(page),
                Err(PageFailure::Failed(error)) => return Err(error),
                Err(PageFailure::Throttled(throttle)) => throttle,
            };

            last_reset = throttle.reset;

            let is_last_attempt = attempt + 1 == self.retry.attempts;

            let Some(delay) = throttle
                .delay(self.retry, attempt)
                .filter(|_| !is_last_attempt)
            else {
                break;
            };

            tokio::time::sleep(delay).await;
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
    ) -> Result<schema::RepositoryConnection, PageFailure> {
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
        let headers = response.headers().clone();

        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(GitHubError::Unauthorised {
                token_source: self.token_source,
            }
            .into());
        }

        if let Some(throttle) = throttle_from(status, &headers) {
            return Err(PageFailure::Throttled(throttle));
        }

        // A server error is GitHub having a bad moment rather than a wrong
        // request, so it is retried like throttling. Parsing the body as JSON
        // would otherwise report "cannot understand the response", which
        // describes the symptom and hides the cause.
        if status.is_server_error() {
            return Err(PageFailure::Throttled(Throttle {
                retry_after: header_number(&headers, "retry-after").map(Duration::from_secs),
                reset: None,
                transient: true,
            }));
        }

        if !status.is_success() {
            return Err(GitHubError::Unexpected {
                account: login.to_owned(),
                status: status.as_u16(),
            }
            .into());
        }

        let body = response
            .text()
            .await
            .map_err(|source| GitHubError::Transport {
                account: login.to_owned(),
                source,
            })?;

        let parsed: Response<RepositoriesData> = serde_json::from_str(&body).map_err(|source| {
            PageFailure::Failed(GitHubError::Malformed {
                account: login.to_owned(),
                source,
            })
        })?;

        if parsed
            .errors
            .iter()
            .any(schema::ResponseError::is_rate_limited)
        {
            // A limit reported in the body is the hourly GraphQL budget, which
            // will not clear by waiting a few seconds.
            return Err(PageFailure::Throttled(Throttle {
                retry_after: None,
                reset: rate_limit_reset(&headers),
                transient: false,
            }));
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
            }
            .into());
        }

        parsed
            .data
            .and_then(|data| data.owner)
            .map(|owner| owner.repositories)
            .ok_or_else(|| {
                GitHubError::UnknownAccount {
                    account: login.to_owned(),
                }
                .into()
            })
    }
}

/// Classifies a response as throttled, and says whether waiting could help.
///
/// GitHub signals a primary limit with 403 and an exhausted remaining count,
/// and a secondary limit with 429 or a `Retry-After`. The distinction matters:
/// a secondary limit clears in seconds, whereas a primary one is an hourly
/// quota that no amount of short backoff will outlast.
fn throttle_from(
    status: reqwest::StatusCode,
    headers: &reqwest::header::HeaderMap,
) -> Option<Throttle> {
    let retry_after = header_number(headers, "retry-after").map(Duration::from_secs);
    let reset = rate_limit_reset(headers);

    let quota_exhausted = header_number(headers, "x-ratelimit-remaining") == Some(0);

    let transient = match status {
        reqwest::StatusCode::TOO_MANY_REQUESTS => true,
        reqwest::StatusCode::FORBIDDEN if retry_after.is_some() => true,
        reqwest::StatusCode::FORBIDDEN if quota_exhausted => false,
        _ => return None,
    };

    Some(Throttle {
        retry_after,
        reset,
        transient,
    })
}

/// Reads a header as a non-negative number.
fn header_number(headers: &reqwest::header::HeaderMap, name: &str) -> Option<u64> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
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
    fn treats_too_many_requests_as_throttling_worth_waiting_out() {
        let headers = reqwest::header::HeaderMap::new();

        let throttle =
            throttle_from(reqwest::StatusCode::TOO_MANY_REQUESTS, &headers).expect("throttling");

        assert!(
            throttle.transient,
            "a secondary limit clears in seconds, so it is worth retrying"
        );
    }

    #[test]
    fn treats_an_exhausted_quota_as_not_worth_waiting_out() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("x-ratelimit-remaining", "0".parse().unwrap());

        let throttle = throttle_from(reqwest::StatusCode::FORBIDDEN, &headers).expect("throttling");

        assert!(
            !throttle.transient,
            "an hourly quota will not clear within a few seconds of backoff"
        );
        assert_eq!(
            throttle.delay(RetryPolicy::default(), 0),
            None,
            "there is no point sleeping before failing"
        );
    }

    #[test]
    fn treats_a_forbidden_response_with_quota_left_as_a_real_refusal() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("x-ratelimit-remaining", "42".parse().unwrap());

        assert_eq!(
            throttle_from(reqwest::StatusCode::FORBIDDEN, &headers),
            None,
            "a forbidden response with quota left is a permissions problem, not throttling"
        );
    }

    #[test]
    fn honours_the_wait_github_asks_for() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("retry-after", "42".parse().unwrap());

        let throttle =
            throttle_from(reqwest::StatusCode::TOO_MANY_REQUESTS, &headers).expect("throttling");

        assert_eq!(
            throttle.delay(RetryPolicy::default(), 0),
            Some(Duration::from_secs(42)),
            "GitHub knows better than a guessed backoff"
        );
    }

    #[test]
    fn refuses_to_sleep_through_an_unreasonably_long_wait() {
        let throttle = Throttle {
            retry_after: Some(Duration::from_secs(3600)),
            reset: None,
            transient: true,
        };

        assert_eq!(
            throttle.delay(RetryPolicy::default(), 0),
            None,
            "an hour-long wait should be reported rather than slept through"
        );
    }

    #[test]
    fn falls_back_to_doubling_backoff_when_github_says_nothing() {
        let throttle = Throttle {
            retry_after: None,
            reset: None,
            transient: true,
        };
        let policy = RetryPolicy {
            attempts: 4,
            backoff: Duration::from_secs(1),
        };

        assert_eq!(throttle.delay(policy, 0), Some(Duration::from_secs(1)));
        assert_eq!(throttle.delay(policy, 2), Some(Duration::from_secs(4)));
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
