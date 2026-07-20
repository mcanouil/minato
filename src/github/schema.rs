//! The GraphQL query, and the shapes it returns.
//!
//! One query per account fetches identity, fork parent, and every surfaced
//! metadata field together. Asking for all of it at once keeps the number of
//! requests low, which is also what keeps the rate limit comfortable.

use std::fmt::Write as _;

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
        isFork
        defaultBranchRef { name }
        parent { name owner { login } defaultBranchRef { name } }
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
#[serde(rename_all = "camelCase")]
pub struct RepositoriesData {
    /// The account, absent when no such user or organisation exists.
    #[serde(rename = "repositoryOwner")]
    pub owner: Option<RepositoryOwner>,
}

/// The account the query was made against.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepositoryOwner {
    /// One page of repositories.
    pub repositories: RepositoryConnection,
}

/// One page of repositories, with the cursor needed to fetch the next.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepositoryConnection {
    /// Where this page sits in the sequence.
    pub page_info: PageInfo,

    /// The repositories on this page.
    pub nodes: Vec<RepositoryNode>,
}

/// Whether more pages follow, and where to resume from.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageInfo {
    /// Whether another page exists.
    pub has_next_page: bool,

    /// The cursor to pass as `after` for the next page.
    pub end_cursor: Option<String>,
}

/// A single repository as the query returns it.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepositoryNode {
    name: String,
    owner: Owner,
    is_private: bool,
    is_archived: bool,
    is_fork: bool,
    default_branch_ref: Option<Ref>,
    parent: Option<Parent>,
    stargazer_count: u32,
    fork_count: u32,
    issues: TotalCount,
    pull_requests: TotalCount,
    discussions: Option<TotalCount>,
    pushed_at: Option<Timestamp>,
    primary_language: Option<Named>,
    license_info: Option<License>,
    releases: ReleaseConnection,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Owner {
    login: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Ref {
    name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Parent {
    name: String,
    owner: Owner,
    default_branch_ref: Option<Ref>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TotalCount {
    total_count: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Named {
    name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct License {
    spdx_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReleaseConnection {
    nodes: Vec<ReleaseNode>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReleaseNode {
    tag_name: String,
    published_at: Option<Timestamp>,
    release_assets: AssetConnection,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AssetConnection {
    nodes: Vec<AssetNode>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AssetNode {
    download_count: u64,
}

/// A fork and the parent ref it should be compared against.
///
/// Both are needed to ask GitHub for a comparison, and the parent's default
/// branch is only known once the first query has answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForkComparison {
    /// The fork.
    pub id: RepoId,
    /// The parent, written as GitHub wants it: `owner:branch`.
    pub head_ref: String,
}

impl RepositoryNode {
    /// What would be needed to compare this fork against its parent, when it
    /// is a fork whose parent is still visible and has a default branch.
    pub fn fork_comparison(&self) -> Option<ForkComparison> {
        let parent = self.parent.as_ref()?;
        let branch = parent.default_branch_ref.as_ref()?;

        Some(ForkComparison {
            id: RepoId::new(Provider::GitHub, &self.owner.login, &self.name),
            head_ref: format!("{}:{}", parent.owner.login, branch.name),
        })
    }
}

/// The query comparing forks against their parents, built with one alias per
/// fork so that a page of them costs one request.
#[must_use]
pub fn comparison_query(forks: &[ForkComparison]) -> String {
    let mut query = String::from("query {\n");

    for (index, fork) in forks.iter().enumerate() {
        let _ = writeln!(
            query,
            "  f{index}: repository(owner: \"{}\", name: \"{}\") {{ defaultBranchRef {{ compare(headRef: \"{}\") {{ aheadBy behindBy }} }} }}",
            fork.id.owner, fork.id.name, fork.head_ref
        );
    }

    query.push_str("}\n");

    query
}

/// One comparison result, keyed by the alias it was requested under.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComparisonNode {
    pub default_branch_ref: Option<CompareRef>,
}

/// The comparison itself, absent when GitHub could not resolve the ref.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompareRef {
    pub compare: Option<Compared>,
}

/// How a fork stands against its parent.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Compared {
    /// Commits the fork has that the parent does not.
    pub ahead_by: u32,
    /// Commits the parent has that the fork does not.
    pub behind_by: u32,
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
            is_fork: node.is_fork,
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
                upstream: None,
                licence: node.license_info.and_then(|license| license.spdx_id),
            },
        }
    }
}
