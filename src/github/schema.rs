//! The GraphQL query, and the shapes it returns.
//!
//! One query per account fetches identity, fork parent, and every surfaced
//! metadata field together. Asking for all of it at once keeps the number of
//! requests low, which is also what keeps the rate limit comfortable.

use jiff::Timestamp;
use serde::Deserialize;

use crate::model::{Metadata, Provider, Release, RemoteRepo, RepoId};

/// How many repositories to ask for per page.
///
/// One hundred is the maximum GitHub allows for a connection.
pub const PAGE_SIZE: u32 = 100;

/// How many release assets to count downloads across.
const ASSET_PAGE_SIZE: u32 = 100;

/// The query used to enumerate an account's repositories.
///
/// `repositoryOwner` resolves both users and organisations, so the caller does
/// not have to know which kind of account it is looking at.
pub const REPOSITORIES_QUERY: &str = r"
query($login: String!, $after: String, $pageSize: Int!, $assetPageSize: Int!) {
  repositoryOwner(login: $login) {
    repositories(first: $pageSize, after: $after, ownerAffiliations: [OWNER], orderBy: {field: NAME, direction: ASC}) {
      pageInfo { hasNextPage endCursor }
      nodes {
        name
        owner { login }
        isPrivate
        isArchived
        defaultBranchRef { name }
        parent { name owner { login } }
        stargazerCount
        forkCount
        issues(states: OPEN) { totalCount }
        pullRequests(states: OPEN) { totalCount }
        discussions { totalCount }
        pushedAt
        primaryLanguage { name }
        licenseInfo { spdxId }
        releases(first: 1, orderBy: {field: CREATED_AT, direction: DESC}) {
          nodes {
            tagName
            publishedAt
            releaseAssets(first: $assetPageSize) { nodes { downloadCount } }
          }
        }
      }
    }
  }
}
";

/// Builds the variables for one page of the query.
pub fn variables(login: &str, after: Option<&str>) -> serde_json::Value {
    serde_json::json!({
        "login": login,
        "after": after,
        "pageSize": PAGE_SIZE,
        "assetPageSize": ASSET_PAGE_SIZE,
    })
}

/// The envelope every GraphQL response arrives in.
#[derive(Debug, Deserialize)]
pub struct Response<T> {
    /// The requested data, absent when the query failed outright.
    pub data: Option<T>,

    /// Errors reported by the API, which can accompany partial data.
    #[serde(default)]
    pub errors: Vec<ResponseError>,
}

/// One error reported inside a GraphQL response.
#[derive(Debug, Deserialize)]
pub struct ResponseError {
    /// A human-readable description of what went wrong.
    pub message: String,

    /// The machine-readable category, when the API supplies one.
    #[serde(rename = "type")]
    pub kind: Option<String>,
}

impl ResponseError {
    /// Whether this error means the request was rate limited.
    pub fn is_rate_limited(&self) -> bool {
        self.kind.as_deref() == Some("RATE_LIMITED")
    }
}

/// The `data` payload of the repositories query.
#[derive(Debug, Deserialize)]
pub struct RepositoriesData {
    /// The account, absent when no such user or organisation exists.
    #[serde(rename = "repositoryOwner")]
    pub owner: Option<RepositoryOwner>,
}

/// The account the query was made against.
#[derive(Debug, Deserialize)]
pub struct RepositoryOwner {
    /// One page of repositories.
    pub repositories: RepositoryConnection,
}

/// One page of repositories, with the cursor needed to fetch the next.
#[derive(Debug, Deserialize)]
pub struct RepositoryConnection {
    /// Where this page sits in the sequence.
    #[serde(rename = "pageInfo")]
    pub page_info: PageInfo,

    /// The repositories on this page.
    pub nodes: Vec<RepositoryNode>,
}

/// Whether more pages follow, and where to resume from.
#[derive(Debug, Deserialize)]
pub struct PageInfo {
    /// Whether another page exists.
    #[serde(rename = "hasNextPage")]
    pub has_next_page: bool,

    /// The cursor to pass as `after` for the next page.
    #[serde(rename = "endCursor")]
    pub end_cursor: Option<String>,
}

/// A single repository as the query returns it.
#[derive(Debug, Deserialize)]
pub struct RepositoryNode {
    name: String,
    owner: Owner,
    #[serde(rename = "isPrivate")]
    is_private: bool,
    #[serde(rename = "isArchived")]
    is_archived: bool,
    #[serde(rename = "defaultBranchRef")]
    default_branch_ref: Option<Ref>,
    parent: Option<Parent>,
    #[serde(rename = "stargazerCount")]
    stargazer_count: u32,
    #[serde(rename = "forkCount")]
    fork_count: u32,
    issues: TotalCount,
    #[serde(rename = "pullRequests")]
    pull_requests: TotalCount,
    discussions: Option<TotalCount>,
    #[serde(rename = "pushedAt")]
    pushed_at: Option<Timestamp>,
    #[serde(rename = "primaryLanguage")]
    primary_language: Option<Named>,
    #[serde(rename = "licenseInfo")]
    license_info: Option<License>,
    releases: ReleaseConnection,
}

#[derive(Debug, Deserialize)]
struct Owner {
    login: String,
}

#[derive(Debug, Deserialize)]
struct Ref {
    name: String,
}

#[derive(Debug, Deserialize)]
struct Parent {
    name: String,
    owner: Owner,
}

#[derive(Debug, Deserialize)]
struct TotalCount {
    #[serde(rename = "totalCount")]
    total_count: u32,
}

#[derive(Debug, Deserialize)]
struct Named {
    name: String,
}

#[derive(Debug, Deserialize)]
struct License {
    #[serde(rename = "spdxId")]
    spdx_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ReleaseConnection {
    nodes: Vec<ReleaseNode>,
}

#[derive(Debug, Deserialize)]
struct ReleaseNode {
    #[serde(rename = "tagName")]
    tag_name: String,
    #[serde(rename = "publishedAt")]
    published_at: Option<Timestamp>,
    #[serde(rename = "releaseAssets")]
    release_assets: AssetConnection,
}

#[derive(Debug, Deserialize)]
struct AssetConnection {
    nodes: Vec<AssetNode>,
}

#[derive(Debug, Deserialize)]
struct AssetNode {
    #[serde(rename = "downloadCount")]
    download_count: u64,
}

impl From<RepositoryNode> for RemoteRepo {
    fn from(node: RepositoryNode) -> Self {
        let latest_release = node
            .releases
            .nodes
            .into_iter()
            .next()
            .map(|release| Release {
                tag: release.tag_name,
                published: release.published_at,
                downloads: release
                    .release_assets
                    .nodes
                    .iter()
                    .map(|asset| asset.download_count)
                    .sum(),
            });

        Self {
            id: RepoId::new(Provider::GitHub, &node.owner.login, &node.name),
            default_branch: node.default_branch_ref.map(|reference| reference.name),
            is_private: node.is_private,
            is_archived: node.is_archived,
            upstream: node
                .parent
                .map(|parent| RepoId::new(Provider::GitHub, &parent.owner.login, &parent.name)),
            metadata: Metadata {
                stars: node.stargazer_count,
                forks: node.fork_count,
                open_issues: node.issues.total_count,
                open_pull_requests: node.pull_requests.total_count,
                discussions: node.discussions.map(|count| count.total_count),
                latest_release,
                last_pushed: node.pushed_at,
                language: node.primary_language.map(|language| language.name),
                licence: node.license_info.and_then(|license| license.spdx_id),
            },
        }
    }
}
