//! Provider-agnostic types identifying and describing repositories.
//!
//! Types describing a local clone on disk are defined alongside the scanner
//! that produces them, so that their shape is driven by real use rather than
//! guesswork.

use std::fmt;
use std::str::FromStr;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

/// How a repository identity is written, quoted in every parse failure.
const REPO_ID_FORM: &str =
    "write it as `provider:owner/name`, for example `github:mcanouil/minato`";

/// Characters allowed in an owner or a repository name, besides letters and
/// digits.
const EXTRA_NAME_CHARACTERS: [char; 3] = ['-', '_', '.'];

/// A repository hosting provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum Provider {
    /// GitHub, at github.com.
    GitHub,
}

impl Provider {
    /// Every provider `minato` can talk to.
    pub const ALL: &'static [Self] = &[Self::GitHub];

    /// The lowercase identifier used in configuration and on the command line.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::GitHub => "github",
        }
    }

    /// The host this provider is reached at.
    #[must_use]
    pub const fn host(self) -> &'static str {
        match self {
            Self::GitHub => "github.com",
        }
    }

    /// The supported identifiers, for use in error messages.
    fn supported() -> String {
        Self::ALL
            .iter()
            .map(|provider| provider.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

impl fmt::Display for Provider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl From<Provider> for String {
    fn from(provider: Provider) -> Self {
        provider.as_str().to_owned()
    }
}

/// A provider name that `minato` does not recognise.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "unknown provider `{name}`; supported providers: {}",
    Provider::supported()
)]
pub struct UnknownProviderError {
    name: String,
}

impl FromStr for Provider {
    type Err = UnknownProviderError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .iter()
            .copied()
            .find(|provider| provider.as_str().eq_ignore_ascii_case(text))
            .ok_or_else(|| UnknownProviderError {
                name: text.to_owned(),
            })
    }
}

impl TryFrom<String> for Provider {
    type Error = UnknownProviderError;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        text.parse()
    }
}

/// The fully qualified identity of a repository, written `provider:owner/name`.
///
/// The provider is part of the identity because the same `owner/name` pair can
/// exist on more than one provider and refer to unrelated repositories.
///
/// The owner and the name are held in lowercase. GitHub resolves them without
/// regard to case, so a remote URL reading `McAnouil/Minato` and an API response
/// reading `mcanouil/minato` name the same repository, and an identity used to
/// match one against the other has to agree. The casing a provider reports is
/// presentation, and belongs with the metadata rather than the identity.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct RepoId {
    /// The provider hosting the repository.
    pub provider: Provider,
    /// The user or organisation owning the repository, lowercased.
    pub owner: String,
    /// The repository name without its owner, lowercased.
    pub name: String,
}

impl RepoId {
    /// Builds an identity from its parts, lowercasing the owner and the name.
    ///
    /// Prefer parsing when the input comes from a user, since parsing also
    /// rejects characters that cannot appear in an owner or a name.
    #[must_use]
    pub fn new(provider: Provider, owner: &str, name: &str) -> Self {
        Self {
            provider,
            owner: owner.to_lowercase(),
            name: name.to_lowercase(),
        }
    }
}

impl RepoId {
    /// The URL a clone of this repository is made from.
    #[must_use]
    pub fn clone_url(&self, protocol: CloneProtocol) -> String {
        let host = self.provider.host();

        match protocol {
            CloneProtocol::Ssh => format!("git@{host}:{}/{}.git", self.owner, self.name),
            CloneProtocol::Https => format!("https://{host}/{}/{}.git", self.owner, self.name),
        }
    }
}

/// How a clone is made.
///
/// This mirrors the configured protocol, kept here so that building a URL does
/// not depend on the configuration types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloneProtocol {
    /// Over SSH, using the existing agent.
    Ssh,
    /// Over HTTPS, using the existing credential helper.
    Https,
}

impl fmt::Display for RepoId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}/{}", self.provider, self.owner, self.name)
    }
}

impl From<RepoId> for String {
    fn from(id: RepoId) -> Self {
        id.to_string()
    }
}

/// A repository identity that could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseRepoIdError {
    /// The `provider:` prefix was absent.
    #[error("repository `{input}` has no provider prefix; {REPO_ID_FORM}")]
    MissingProvider {
        /// The text that failed to parse.
        input: String,
    },

    /// The `owner/name` separator was absent.
    #[error("repository `{input}` has no owner; {REPO_ID_FORM}")]
    MissingOwner {
        /// The text that failed to parse.
        input: String,
    },

    /// More than one `/` appeared after the provider.
    #[error("repository `{input}` has more than one `/` after the provider; {REPO_ID_FORM}")]
    TooManySeparators {
        /// The text that failed to parse.
        input: String,
    },

    /// The owner or the name was empty.
    #[error("repository `{input}` has an empty {part}; {REPO_ID_FORM}")]
    EmptyPart {
        /// The text that failed to parse.
        input: String,
        /// Which part was empty.
        part: &'static str,
    },

    /// The owner or the name held a character that cannot appear in one.
    #[error(
        "repository `{input}` has `{character}` in its {part}, which is not allowed; an {part} may hold letters, digits, `-`, `_`, and `.`"
    )]
    InvalidCharacter {
        /// The text that failed to parse.
        input: String,
        /// Which part held the character.
        part: &'static str,
        /// The offending character.
        character: char,
    },

    /// The provider prefix was present but not recognised.
    #[error(transparent)]
    UnknownProvider(#[from] UnknownProviderError),
}

/// Checks that `value` can be an owner or a repository name.
fn check_part(input: &str, part: &'static str, value: &str) -> Result<(), ParseRepoIdError> {
    if value.is_empty() {
        return Err(ParseRepoIdError::EmptyPart {
            input: input.to_owned(),
            part,
        });
    }

    if let Some(character) = value.chars().find(|character| {
        !character.is_ascii_alphanumeric() && !EXTRA_NAME_CHARACTERS.contains(character)
    }) {
        return Err(ParseRepoIdError::InvalidCharacter {
            input: input.to_owned(),
            part,
            character,
        });
    }

    Ok(())
}

impl FromStr for RepoId {
    type Err = ParseRepoIdError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let (provider, path) =
            text.split_once(':')
                .ok_or_else(|| ParseRepoIdError::MissingProvider {
                    input: text.to_owned(),
                })?;

        let (owner, name) = path
            .split_once('/')
            .ok_or_else(|| ParseRepoIdError::MissingOwner {
                input: text.to_owned(),
            })?;

        if name.contains('/') {
            return Err(ParseRepoIdError::TooManySeparators {
                input: text.to_owned(),
            });
        }

        check_part(text, "owner", owner)?;
        check_part(text, "name", name)?;

        Ok(Self::new(provider.trim().parse::<Provider>()?, owner, name))
    }
}

impl TryFrom<String> for RepoId {
    type Error = ParseRepoIdError;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        text.parse()
    }
}

/// A repository as a provider reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteRepo {
    /// Which repository this is.
    pub id: RepoId,

    /// The default branch, absent when the repository has no commits.
    pub default_branch: Option<String>,

    /// Whether the repository is private.
    pub is_private: bool,

    /// Whether the repository is archived, and so never expected to change.
    pub is_archived: bool,

    /// Whether the repository is a fork.
    ///
    /// This is reported separately from [`Self::upstream`] because a fork whose
    /// parent has been deleted is still a fork, but has no parent to name.
    pub is_fork: bool,

    /// The repository this one was forked from, when it is a fork whose parent
    /// is still visible.
    pub upstream: Option<RepoId>,

    /// Everything reported about the repository that is not its identity.
    pub metadata: Metadata,
}

impl RemoteRepo {
    /// Whether this repository can be compared against an upstream.
    ///
    /// A fork whose parent was deleted cannot, so being a fork is not enough.
    #[must_use]
    pub const fn has_upstream(&self) -> bool {
        self.upstream.is_some()
    }
}

/// What a provider reports about a repository beyond its identity.
///
/// Fields a provider does not support are absent rather than zero, so that
/// "this provider has no discussions" stays distinguishable from "this
/// repository has no discussions".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Metadata {
    /// How many users have starred the repository.
    pub stars: u32,

    /// How many forks exist.
    pub forks: u32,

    /// How many issues are open.
    pub open_issues: u32,

    /// How many pull requests are open.
    pub open_pull_requests: u32,

    /// How many discussions exist, when the provider supports discussions.
    pub discussions: Option<u32>,

    /// The most recent release, when there is one.
    pub latest_release: Option<Release>,

    /// When the repository was last pushed to.
    pub last_pushed: Option<Timestamp>,

    /// The language the provider considers primary.
    pub language: Option<String>,

    /// The SPDX identifier of the licence, when one is detected.
    pub licence: Option<String>,

    /// How this fork stands against its parent, absent when it is not a fork,
    /// the parent is gone, or the provider could not compare them.
    ///
    /// Absent means unknown rather than in step, so a fork whose parent was
    /// deleted is never reported as up to date with something that is not
    /// there.
    pub upstream: Option<UpstreamStanding>,
}

/// How a fork stands against the repository it was forked from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpstreamStanding {
    /// Commits the fork has that the parent does not.
    pub ahead: u32,

    /// Commits the parent has that the fork does not.
    pub behind: u32,
}

impl UpstreamStanding {
    /// Whether the fork trails its parent.
    #[must_use]
    pub const fn is_behind(self) -> bool {
        self.behind > 0
    }

    /// Whether the fork can be brought level by fast-forwarding.
    ///
    /// A fork holding its own commits cannot: catching up would mean a merge
    /// or a rebase, which is a decision rather than an update.
    #[must_use]
    pub const fn can_fast_forward(self) -> bool {
        self.behind > 0 && self.ahead == 0
    }
}

/// The most recent release of a repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Release {
    /// The tag the release points at.
    pub tag: String,

    /// When the release was published, absent for a draft.
    pub published: Option<Timestamp>,

    /// Downloads counted across every asset attached to the release.
    pub downloads: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_fully_qualified_identity() {
        let id: RepoId = "github:mcanouil/minato".parse().unwrap();

        assert_eq!(id.provider, Provider::GitHub);
        assert_eq!(id.owner, "mcanouil");
        assert_eq!(id.name, "minato");
    }

    #[test]
    fn round_trips_through_its_text_form() {
        let text = "github:mcanouil/minato";

        assert_eq!(text.parse::<RepoId>().unwrap().to_string(), text);
    }

    #[test]
    fn treats_identities_differing_only_in_case_as_the_same_repository() {
        let from_api: RepoId = "github:mcanouil/minato".parse().unwrap();
        let from_remote_url: RepoId = "github:McAnouil/Minato".parse().unwrap();

        assert_eq!(
            from_api, from_remote_url,
            "a remote URL and an API response naming one repository must produce one identity"
        );
        assert_eq!(from_remote_url.to_string(), "github:mcanouil/minato");
    }

    #[test]
    fn accepts_a_provider_in_any_case() {
        assert_eq!("GitHub".parse::<Provider>().unwrap(), Provider::GitHub);
        assert_eq!("github".parse::<Provider>().unwrap(), Provider::GitHub);
    }

    #[test]
    fn parses_a_provider_the_same_way_through_serde() {
        for text in ["\"github\"", "\"GitHub\""] {
            let provider: Provider = serde_json::from_str(text).unwrap();

            assert_eq!(provider, Provider::GitHub);
        }
    }

    #[test]
    fn rejects_an_identity_without_a_provider() {
        let error = "mcanouil/minato".parse::<RepoId>().unwrap_err();

        assert!(matches!(error, ParseRepoIdError::MissingProvider { .. }));
        assert!(
            error.to_string().contains("github:mcanouil/minato"),
            "the error should show the expected form, got: {error}"
        );
    }

    #[test]
    fn rejects_an_identity_without_an_owner() {
        let error = "github:minato".parse::<RepoId>().unwrap_err();

        assert!(matches!(error, ParseRepoIdError::MissingOwner { .. }));
    }

    #[test]
    fn rejects_empty_owner_or_name() {
        assert!(matches!(
            "github:/minato".parse::<RepoId>().unwrap_err(),
            ParseRepoIdError::EmptyPart { part: "owner", .. }
        ));
        assert!(matches!(
            "github:mcanouil/".parse::<RepoId>().unwrap_err(),
            ParseRepoIdError::EmptyPart { part: "name", .. }
        ));
    }

    #[test]
    fn reports_an_extra_separator_as_such_rather_than_as_an_empty_name() {
        let error = "github:mcanouil/minato/extra"
            .parse::<RepoId>()
            .unwrap_err();

        assert!(matches!(error, ParseRepoIdError::TooManySeparators { .. }));
        assert!(
            error.to_string().contains("more than one `/`"),
            "the error should name the real problem, got: {error}"
        );
    }

    #[test]
    fn rejects_whitespace_around_an_owner_or_a_name() {
        for text in ["github: mcanouil/minato", "github:mcanouil/minato "] {
            assert!(
                matches!(
                    text.parse::<RepoId>(),
                    Err(ParseRepoIdError::InvalidCharacter { .. })
                ),
                "`{text}` should be rejected rather than silently accepted"
            );
        }
    }

    #[test]
    fn rejects_a_stray_colon_in_an_owner() {
        assert!(matches!(
            "github::mcanouil/minato".parse::<RepoId>().unwrap_err(),
            ParseRepoIdError::InvalidCharacter { part: "owner", .. }
        ));
    }

    #[test]
    fn accepts_the_punctuation_that_appears_in_real_repository_names() {
        let id: RepoId = "github:my-org/some_repo.rs".parse().unwrap();

        assert_eq!(id.owner, "my-org");
        assert_eq!(id.name, "some_repo.rs");
    }

    #[test]
    fn builds_clone_urls_in_both_protocols() {
        let id = RepoId::new(Provider::GitHub, "mcanouil", "minato");

        assert_eq!(
            id.clone_url(CloneProtocol::Ssh),
            "git@github.com:mcanouil/minato.git"
        );
        assert_eq!(
            id.clone_url(CloneProtocol::Https),
            "https://github.com/mcanouil/minato.git"
        );
    }

    #[test]
    fn a_clone_url_round_trips_back_to_the_same_identity() {
        let id = RepoId::new(Provider::GitHub, "mcanouil", "minato");

        for protocol in [CloneProtocol::Ssh, CloneProtocol::Https] {
            let url = id.clone_url(protocol);
            let parsed = crate::scan::remote_url::parse(&url)
                .unwrap_or_else(|error| panic!("`{url}` should parse back: {error}"));

            assert_eq!(
                parsed, id,
                "a URL minato builds must be one minato can read back"
            );
        }
    }

    #[test]
    fn rejects_an_unknown_provider_and_lists_the_supported_ones() {
        let error = "gitlab:mcanouil/minato".parse::<RepoId>().unwrap_err();

        assert!(matches!(error, ParseRepoIdError::UnknownProvider(_)));
        assert!(
            error.to_string().contains("github"),
            "the error should list supported providers, got: {error}"
        );
    }
}
