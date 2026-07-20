//! Running `git`.
//!
//! `minato` shells out to the system `git` rather than linking a git library, so
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

/// Fetches from `origin`, updating remote-tracking refs only.
///
/// This never touches the working tree or any local branch, so it is always
/// safe to run, including on a repository with uncommitted changes.
///
/// # Errors
///
/// Returns an error when `git` cannot be run or the fetch fails.
pub fn fetch(directory: &Path) -> Result<(), GitError> {
    run(directory, &["fetch", "--quiet", "--prune", "origin"]).map(|_| ())
}

/// Fast-forwards the checked-out branch to its upstream, and nothing else.
///
/// `--ff-only` is what makes this safe: git refuses rather than creating a
/// merge commit, rewriting history, or discarding a change. If the caller has
/// misjudged the situation, git declines instead of improvising.
///
/// # Errors
///
/// Returns an error when the branch cannot be fast-forwarded, which includes
/// the case where doing so would overwrite an untracked file.
pub fn fast_forward(directory: &Path) -> Result<(), GitError> {
    run(directory, &["merge", "--ff-only", "@{upstream}"]).map(|_| ())
}

/// Clones `url` into `destination`.
///
/// The parent directory is created first, since a layout puts clones several
/// levels below a root.
///
/// # Errors
///
/// Returns an error when the parent cannot be created, or the clone fails.
pub fn clone(url: &str, destination: &Path, shallow: bool) -> Result<(), GitError> {
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent).map_err(|error| GitError::Unavailable {
            message: format!("cannot create {}: {error}", parent.display()),
        })?;
    }

    let mut arguments = vec!["clone".to_owned(), "--quiet".to_owned()];

    if shallow {
        arguments.push("--depth".to_owned());
        arguments.push("1".to_owned());
    }

    arguments.push(url.to_owned());
    arguments.push(destination.display().to_string());

    // `git -C` needs an existing directory, and the destination does not exist
    // yet, so the clone runs from the parent.
    let working = destination.parent().unwrap_or(destination);

    run(working, &arguments).map(|_| ())
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
