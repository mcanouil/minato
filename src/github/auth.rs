//! Finding a GitHub token without ever storing one.
//!
//! `minato` never writes a token to its configuration. It reads one from the
//! environment, or borrows the one the `gh` CLI already holds, so that a token
//! lives wherever the user already chose to keep it.

use std::fmt;
use std::process::Command;

/// Environment variables consulted for a token, in order.
///
/// The `minato`-specific variable comes first so that a token scoped to this
/// tool can override a broader one already exported for something else.
const TOKEN_VARIABLES: [&str; 2] = ["MINATO_GITHUB_TOKEN", "GITHUB_TOKEN"];

/// A GitHub token, which never appears in debug output or error messages.
#[derive(Clone, PartialEq, Eq)]
pub struct Token(String);

impl Token {
    /// Wraps a token value.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The token value, for putting in an `Authorization` header.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

/// Redacted, so that a token cannot reach a log or a panic message by
/// accident.
impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Token(redacted)")
    }
}

/// Where a token was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenSource {
    /// An environment variable, named by the contained string.
    Environment(&'static str),

    /// The `gh` CLI's own stored credentials.
    GhCli,
}

impl fmt::Display for TokenSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Environment(name) => write!(f, "the {name} environment variable"),
            Self::GhCli => f.write_str("the gh CLI"),
        }
    }
}

/// No token could be found anywhere `minato` looks.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "no GitHub token found. Run `gh auth login` to sign in with the gh CLI, or set {} to a personal access token with the `repo` scope",
    TOKEN_VARIABLES[0]
)]
pub struct NoTokenError;

/// Finds a token, preferring the environment over the `gh` CLI.
///
/// Both lookups are supplied by the caller so that this stays a pure function
/// of its inputs, testable without touching the process environment or running
/// a subprocess.
///
/// # Errors
///
/// Returns [`NoTokenError`] when neither source yields a non-empty token.
pub fn resolve_token(
    variable: impl Fn(&str) -> Option<String>,
    gh_token: impl FnOnce() -> Option<String>,
) -> Result<(Token, TokenSource), NoTokenError> {
    for name in TOKEN_VARIABLES {
        if let Some(value) = variable(name).filter(|value| !value.trim().is_empty()) {
            return Ok((Token::new(value.trim()), TokenSource::Environment(name)));
        }
    }

    gh_token()
        .filter(|value| !value.trim().is_empty())
        .map(|value| (Token::new(value.trim()), TokenSource::GhCli))
        .ok_or(NoTokenError)
}

/// Finds a token from the real environment and the real `gh` CLI.
///
/// # Errors
///
/// Returns [`NoTokenError`] when neither source yields a token.
pub fn resolve_token_from_system() -> Result<(Token, TokenSource), NoTokenError> {
    resolve_token(
        |name| std::env::var(name).ok(),
        || {
            let output = Command::new("gh").args(["auth", "token"]).output().ok()?;

            output
                .status
                .success()
                .then(|| String::from_utf8(output.stdout).ok())
                .flatten()
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_variables(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn prefers_the_minato_variable_over_the_general_one() {
        let (token, source) = resolve_token(
            |name| Some(format!("token-for-{name}")),
            || Some("gh-token".to_owned()),
        )
        .unwrap();

        assert_eq!(token.expose(), "token-for-MINATO_GITHUB_TOKEN");
        assert_eq!(source, TokenSource::Environment("MINATO_GITHUB_TOKEN"));
    }

    #[test]
    fn falls_back_to_the_general_variable() {
        let (token, source) = resolve_token(
            |name| (name == "GITHUB_TOKEN").then(|| "general".to_owned()),
            || Some("gh-token".to_owned()),
        )
        .unwrap();

        assert_eq!(token.expose(), "general");
        assert_eq!(source, TokenSource::Environment("GITHUB_TOKEN"));
    }

    #[test]
    fn falls_back_to_the_gh_cli_when_no_variable_is_set() {
        let (token, source) = resolve_token(no_variables, || Some("gh-token".to_owned())).unwrap();

        assert_eq!(token.expose(), "gh-token");
        assert_eq!(source, TokenSource::GhCli);
    }

    #[test]
    fn ignores_a_variable_that_is_set_but_empty() {
        let (token, source) = resolve_token(
            |name| (name == "MINATO_GITHUB_TOKEN").then(|| "   ".to_owned()),
            || Some("gh-token".to_owned()),
        )
        .unwrap();

        assert_eq!(
            source,
            TokenSource::GhCli,
            "an exported but empty variable should not mask the gh CLI"
        );
        assert_eq!(token.expose(), "gh-token");
    }

    #[test]
    fn trims_the_trailing_newline_the_gh_cli_prints() {
        let (token, _) = resolve_token(no_variables, || Some("gh-token\n".to_owned())).unwrap();

        assert_eq!(token.expose(), "gh-token");
    }

    #[test]
    fn reports_a_missing_token_with_both_ways_to_provide_one() {
        let error = resolve_token(no_variables, || None).unwrap_err();

        assert!(error.to_string().contains("gh auth login"));
        assert!(error.to_string().contains("MINATO_GITHUB_TOKEN"));
    }

    #[test]
    fn keeps_the_token_out_of_debug_output() {
        let token = Token::new("ghp_verysecretvalue");

        assert_eq!(format!("{token:?}"), "Token(redacted)");
        assert!(!format!("{token:?}").contains("verysecret"));
    }
}
