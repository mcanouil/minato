//! Provider-agnostic types identifying repositories.
//!
//! Types describing what a provider reports about a repository, and what a
//! local clone looks like on disk, are defined alongside the code that produces
//! them, so that their shape is driven by real use rather than guesswork.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// A repository hosting provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum Provider {
    /// GitHub, at github.com.
    GitHub,
}

impl Provider {
    /// Every provider `fleet` can talk to.
    pub const ALL: &'static [Self] = &[Self::GitHub];

    /// The lowercase identifier used in configuration and on the command line.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::GitHub => "github",
        }
    }
}

impl fmt::Display for Provider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A provider name that `fleet` does not recognise.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown provider `{name}`; supported providers: {}", supported())]
pub struct UnknownProviderError {
    name: String,
}

fn supported() -> String {
    Provider::ALL
        .iter()
        .map(|provider| provider.as_str())
        .collect::<Vec<_>>()
        .join(", ")
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

/// The fully qualified identity of a repository, written `provider:owner/name`.
///
/// The provider is part of the identity because the same `owner/name` pair can
/// exist on more than one provider and refer to unrelated repositories.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct RepoId {
    /// The provider hosting the repository.
    pub provider: Provider,
    /// The user or organisation owning the repository.
    pub owner: String,
    /// The repository name, without its owner.
    pub name: String,
}

impl RepoId {
    /// Builds an identity from its parts.
    #[must_use]
    pub fn new(provider: Provider, owner: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            provider,
            owner: owner.into(),
            name: name.into(),
        }
    }

    /// The `owner/name` pair, without the provider prefix.
    #[must_use]
    pub fn path(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }
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
    #[error(
        "repository `{input}` has no provider prefix; write it as `provider:owner/name`, for example `github:mcanouil/fleet`"
    )]
    MissingProvider {
        /// The text that failed to parse.
        input: String,
    },

    /// The `owner/name` separator was absent.
    #[error(
        "repository `{input}` has no owner; write it as `provider:owner/name`, for example `github:mcanouil/fleet`"
    )]
    MissingOwner {
        /// The text that failed to parse.
        input: String,
    },

    /// The owner or the name was empty.
    #[error(
        "repository `{input}` has an empty {part}; write it as `provider:owner/name`, for example `github:mcanouil/fleet`"
    )]
    EmptyPart {
        /// The text that failed to parse.
        input: String,
        /// Which part was empty.
        part: &'static str,
    },

    /// The provider prefix was present but not recognised.
    #[error(transparent)]
    UnknownProvider(#[from] UnknownProviderError),
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

        if owner.is_empty() {
            return Err(ParseRepoIdError::EmptyPart {
                input: text.to_owned(),
                part: "owner",
            });
        }

        if name.is_empty() || name.contains('/') {
            return Err(ParseRepoIdError::EmptyPart {
                input: text.to_owned(),
                part: "name",
            });
        }

        Ok(Self::new(provider.parse::<Provider>()?, owner, name))
    }
}

impl TryFrom<String> for RepoId {
    type Error = ParseRepoIdError;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        text.parse()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_fully_qualified_identity() {
        let id: RepoId = "github:mcanouil/fleet".parse().unwrap();

        assert_eq!(id.provider, Provider::GitHub);
        assert_eq!(id.owner, "mcanouil");
        assert_eq!(id.name, "fleet");
    }

    #[test]
    fn round_trips_through_its_text_form() {
        let text = "github:mcanouil/fleet";

        assert_eq!(text.parse::<RepoId>().unwrap().to_string(), text);
    }

    #[test]
    fn accepts_a_provider_in_any_case() {
        assert_eq!("GitHub".parse::<Provider>().unwrap(), Provider::GitHub);
        assert_eq!("github".parse::<Provider>().unwrap(), Provider::GitHub);
    }

    #[test]
    fn rejects_an_identity_without_a_provider() {
        let error = "mcanouil/fleet".parse::<RepoId>().unwrap_err();

        assert!(matches!(error, ParseRepoIdError::MissingProvider { .. }));
        assert!(
            error.to_string().contains("github:mcanouil/fleet"),
            "the error should show the expected form, got: {error}"
        );
    }

    #[test]
    fn rejects_an_identity_without_an_owner() {
        let error = "github:fleet".parse::<RepoId>().unwrap_err();

        assert!(matches!(error, ParseRepoIdError::MissingOwner { .. }));
    }

    #[test]
    fn rejects_empty_owner_or_name() {
        assert!(matches!(
            "github:/fleet".parse::<RepoId>().unwrap_err(),
            ParseRepoIdError::EmptyPart { part: "owner", .. }
        ));
        assert!(matches!(
            "github:mcanouil/".parse::<RepoId>().unwrap_err(),
            ParseRepoIdError::EmptyPart { part: "name", .. }
        ));
    }

    #[test]
    fn rejects_a_name_containing_a_further_slash() {
        assert!(matches!(
            "github:mcanouil/fleet/extra".parse::<RepoId>().unwrap_err(),
            ParseRepoIdError::EmptyPart { part: "name", .. }
        ));
    }

    #[test]
    fn rejects_an_unknown_provider_and_lists_the_supported_ones() {
        let error = "gitlab:mcanouil/fleet".parse::<RepoId>().unwrap_err();

        assert!(matches!(error, ParseRepoIdError::UnknownProvider(_)));
        assert!(
            error.to_string().contains("github"),
            "the error should list supported providers, got: {error}"
        );
    }

    #[test]
    fn exposes_the_owner_and_name_without_the_provider() {
        let id = RepoId::new(Provider::GitHub, "mcanouil", "fleet");

        assert_eq!(id.path(), "mcanouil/fleet");
    }
}
