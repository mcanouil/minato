//! Running `git`.
//!
//! `fleet` shells out to the system `git` rather than linking a git library, so
//! that the user's SSH agent, credential helpers, and configuration apply
//! unchanged. Nothing here mutates a working tree.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

/// A `git` invocation that did not succeed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GitError {
    /// The `git` binary could not be run at all.
    #[error("cannot run `git`: {message}; check that git is installed and on PATH")]
    Unavailable {
        /// What the operating system reported.
        message: String,
    },

    /// `git` ran but reported a failure.
    #[error("`git {arguments}` failed in {}: {message}", directory.display())]
    Failed {
        /// Where it ran.
        directory: PathBuf,
        /// The arguments it was given.
        arguments: String,
        /// What it printed on standard error.
        message: String,
    },
}

/// Runs `git` in `directory` and returns its trimmed standard output.
///
/// # Errors
///
/// Returns an error when `git` cannot be run, or exits non-zero.
pub fn run<S: AsRef<OsStr>>(directory: &Path, arguments: &[S]) -> Result<String, GitError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(arguments)
        .output()
        .map_err(|error| GitError::Unavailable {
            message: error.to_string(),
        })?;

    if !output.status.success() {
        return Err(GitError::Failed {
            directory: directory.to_owned(),
            arguments: describe(arguments),
            message: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Runs `git`, treating a non-zero exit as an absent value rather than a
/// failure.
///
/// Several queries answer "there is no such thing" by exiting non-zero, which
/// is not an error worth reporting.
pub fn run_optional<S: AsRef<OsStr>>(directory: &Path, arguments: &[S]) -> Option<String> {
    run(directory, arguments)
        .ok()
        .filter(|output| !output.is_empty())
}

fn describe<S: AsRef<OsStr>>(arguments: &[S]) -> String {
    arguments
        .iter()
        .map(|argument| argument.as_ref().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whether `path` is the root of a git working tree.
///
/// A worktree or a submodule holds a `.git` file rather than a directory, so
/// both are accepted.
#[must_use]
pub fn is_repository(path: &Path) -> bool {
    path.join(".git").exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_a_failing_command_with_where_and_what_ran() {
        let directory = tempfile::tempdir().expect("a temporary directory");

        let error = run(directory.path(), &["rev-parse", "--verify", "nope"])
            .expect_err("a failure outside a repository");

        assert!(matches!(error, GitError::Failed { .. }));
        assert!(
            error.to_string().contains("rev-parse"),
            "the error should name the command, got: {error}"
        );
    }

    #[test]
    fn treats_a_failing_optional_command_as_absent() {
        let directory = tempfile::tempdir().expect("a temporary directory");

        assert_eq!(
            run_optional(directory.path(), &["rev-parse", "--verify", "nope"]),
            None
        );
    }

    #[test]
    fn recognises_a_working_tree_by_its_git_entry() {
        let directory = tempfile::tempdir().expect("a temporary directory");

        assert!(!is_repository(directory.path()));

        std::fs::create_dir(directory.path().join(".git")).expect("a .git directory");

        assert!(is_repository(directory.path()));
    }
}
