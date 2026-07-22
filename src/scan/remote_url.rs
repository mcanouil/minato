//! Turning a git remote URL back into a repository identity.
//!
//! A clone on disk knows only its remote URL. Matching it against what a
//! provider reports means recovering the identity from that URL, in every form
//! git accepts.

use crate::model::{Provider, RepoId};

/// Hosts recognised as belonging to a provider.
///
/// Only the hosted services are listed. A self-hosted instance cannot be
/// recognised from its host alone, which is why the identity type does not
/// carry one yet.
const KNOWN_HOSTS: [(&str, Provider); 2] = [
    ("github.com", Provider::GitHub),
    ("www.github.com", Provider::GitHub),
];

/// A remote URL that could not be turned into an identity.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseRemoteError {
    /// The URL did not name a host and a path.
    #[error("remote `{url}` is not a URL Minato can read")]
    Unreadable {
        /// The remote as configured.
        url: String,
    },

    /// The host is not one Minato knows.
    #[error("remote `{url}` points at `{host}`, which is not a provider Minato supports")]
    UnknownHost {
        /// The remote as configured.
        url: String,
        /// The host it named.
        host: String,
    },

    /// The path did not hold an owner and a repository name.
    #[error("remote `{url}` does not name an owner and a repository")]
    UnreadablePath {
        /// The remote as configured.
        url: String,
    },
}

/// Recovers the identity a remote URL refers to.
///
/// Handles the forms git accepts: `scp`-like SSH (`git@host:owner/repo.git`),
/// SSH URLs, HTTPS with or without credentials, and `git://`, each with or
/// without a `.git` suffix.
///
/// # Errors
///
/// Returns an error naming the URL when it cannot be read, when its host is
/// not a supported provider, or when its path does not name a repository.
pub fn parse(url: &str) -> Result<RepoId, ParseRemoteError> {
    let trimmed = url.trim();

    let (host, path) =
        split_host_and_path(trimmed).ok_or_else(|| ParseRemoteError::Unreadable {
            url: url.to_owned(),
        })?;

    let provider = KNOWN_HOSTS
        .iter()
        .find(|(known, _)| known.eq_ignore_ascii_case(host))
        .map(|(_, provider)| *provider)
        .ok_or_else(|| ParseRemoteError::UnknownHost {
            url: url.to_owned(),
            host: host.to_owned(),
        })?;

    let (owner, name) =
        split_owner_and_name(path).ok_or_else(|| ParseRemoteError::UnreadablePath {
            url: url.to_owned(),
        })?;

    Ok(RepoId::new(provider, owner, name))
}

/// Splits a remote into its host and the path after it.
fn split_host_and_path(url: &str) -> Option<(&str, &str)> {
    // ssh://git@host/path, https://host/path, git://host/path
    if let Some((_, rest)) = url.split_once("://") {
        let (authority, path) = rest.split_once('/')?;

        return Some((strip_credentials(authority), path));
    }

    // The scp-like form, git@host:owner/repo, which has no scheme.
    let (authority, path) = url.split_once(':')?;

    Some((strip_credentials(authority), path))
}

/// Removes any `user@` prefix, and any `:port` suffix, from an authority.
fn strip_credentials(authority: &str) -> &str {
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);

    host.split_once(':').map_or(host, |(host, _)| host)
}

/// Splits a repository path into its owner and name.
///
/// The last two segments win, so a nested path such as a GitLab subgroup still
/// yields the repository and its immediate parent rather than failing.
fn split_owner_and_name(path: &str) -> Option<(&str, &str)> {
    let cleaned = path.trim_matches('/');
    let cleaned = cleaned.strip_suffix(".git").unwrap_or(cleaned);
    let cleaned = cleaned.trim_end_matches('/');

    let mut segments = cleaned.rsplit('/');
    let name = segments.next().filter(|name| !name.is_empty())?;
    let owner = segments.next().filter(|owner| !owner.is_empty())?;

    Some((owner, name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expect_minato(url: &str) {
        let id = parse(url).unwrap_or_else(|error| panic!("`{url}` should parse, got: {error}"));

        assert_eq!(
            id.to_string(),
            "github:mcanouil/minato",
            "`{url}` produced the wrong identity"
        );
    }

    #[test]
    fn reads_the_scp_like_ssh_form() {
        expect_minato("git@github.com:mcanouil/minato.git");
        expect_minato("git@github.com:mcanouil/minato");
    }

    #[test]
    fn reads_ssh_urls() {
        expect_minato("ssh://git@github.com/mcanouil/minato.git");
        expect_minato("ssh://git@github.com:22/mcanouil/minato.git");
    }

    #[test]
    fn reads_https_urls() {
        expect_minato("https://github.com/mcanouil/minato.git");
        expect_minato("https://github.com/mcanouil/minato");
        expect_minato("http://github.com/mcanouil/minato");
    }

    #[test]
    fn reads_urls_carrying_credentials() {
        expect_minato("https://token@github.com/mcanouil/minato.git");
        expect_minato("https://user:password@github.com/mcanouil/minato.git");
    }

    #[test]
    fn reads_the_git_protocol() {
        expect_minato("git://github.com/mcanouil/minato.git");
    }

    #[test]
    fn ignores_a_trailing_slash_and_surrounding_whitespace() {
        expect_minato("https://github.com/mcanouil/minato/");
        expect_minato("  git@github.com:mcanouil/minato.git  ");
    }

    #[test]
    fn normalises_case_the_way_an_identity_does() {
        let id = parse("git@GitHub.com:McAnouil/Minato.git").unwrap();

        assert_eq!(
            id.to_string(),
            "github:mcanouil/minato",
            "a remote URL and an API response must agree on one identity"
        );
    }

    #[test]
    fn keeps_a_repository_name_that_ends_in_git() {
        let id = parse("https://github.com/mcanouil/not.git.git").unwrap();

        assert_eq!(id.name, "not.git");
    }

    #[test]
    fn rejects_a_host_that_is_not_a_supported_provider() {
        let error = parse("git@gitlab.com:mcanouil/minato.git").unwrap_err();

        assert!(matches!(error, ParseRemoteError::UnknownHost { .. }));
        assert!(
            error.to_string().contains("gitlab.com"),
            "the error should name the host, got: {error}"
        );
    }

    #[test]
    fn rejects_a_path_without_an_owner() {
        assert!(matches!(
            parse("https://github.com/minato.git"),
            Err(ParseRemoteError::UnreadablePath { .. })
        ));
    }

    #[test]
    fn rejects_something_that_is_not_a_remote_at_all() {
        assert!(matches!(
            parse("not-a-url"),
            Err(ParseRemoteError::Unreadable { .. })
        ));
    }

    #[test]
    fn takes_the_last_two_segments_of_a_nested_path() {
        let id = parse("https://github.com/group/subgroup/minato.git").unwrap();

        assert_eq!(id.owner, "subgroup");
        assert_eq!(id.name, "minato");
    }
}
