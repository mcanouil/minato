//! Finding git clones on disk and reading their state.
//!
//! Everything here is read-only: it runs `git` queries that never touch a
//! working tree. Ahead and behind counts come from remote-tracking refs, so a
//! scan needs no network access, and reports whatever the last fetch left
//! behind.

pub mod remote_url;

use std::path::{Path, PathBuf};

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::config::ResolvedRoots;
use crate::git;
use crate::model::RepoId;

/// How deep below a root to look for clones.
///
/// Clones sit a few levels down under a layout such as `{owner}/{repo}`.
/// Descending without limit would walk entire source trees for nothing.
pub const DEFAULT_MAX_DEPTH: usize = 4;

/// What is checked out in a clone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Head {
    /// A branch is checked out.
    Branch(String),

    /// A commit is checked out directly, so there is no branch to update.
    Detached,

    /// The repository has no commits yet.
    Unborn,
}

/// How a branch stands against the remote-tracking ref it follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tracking {
    /// Commits the local branch has that the remote-tracking ref does not.
    pub ahead: u32,

    /// Commits the remote-tracking ref has that the local branch does not.
    pub behind: u32,
}

impl Tracking {
    /// Whether the two sides are identical.
    #[must_use]
    pub const fn is_in_sync(self) -> bool {
        self.ahead == 0 && self.behind == 0
    }
}

/// A git clone found on disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalRepo {
    /// Where the clone is.
    pub path: PathBuf,

    /// The identity its remote points at, absent when the remote is missing or
    /// not recognised.
    pub id: Option<RepoId>,

    /// The remote URL as configured, kept for reporting when it cannot be read.
    pub remote_url: Option<String>,

    /// What is checked out.
    pub head: Head,

    /// How the checked-out branch stands against its remote-tracking ref,
    /// absent when there is no branch or it tracks nothing.
    pub tracking: Option<Tracking>,

    /// Whether tracked files hold uncommitted changes.
    pub dirty: bool,

    /// The group this clone belongs to, taken from the directory it sits in
    /// beneath its root.
    ///
    /// The tree is the source of truth for grouping: a repository under
    /// `~/Projects/demo` is in the `demo` group without anyone saying so.
    /// A clone sitting directly in a root has no group.
    pub group: Option<String>,

    /// Whether the working tree holds files git is not tracking.
    ///
    /// Kept apart from [`Self::dirty`] because the two mean different things
    /// for updating: an untracked file is usually a build artefact and does not
    /// stand in the way of a fast-forward.
    pub untracked: bool,
}

impl LocalRepo {
    /// Whether this clone can be updated without discarding anything.
    ///
    /// Modified tracked files or a detached head mean an update would need a
    /// decision the user has not made, so it is reported rather than attempted.
    ///
    /// Untracked files do not block an update. A fast-forward succeeds with
    /// them present, and in the one case where it would overwrite one, git
    /// refuses on its own rather than destroying it.
    #[must_use]
    pub const fn is_updatable(&self) -> bool {
        !self.dirty && matches!(self.head, Head::Branch(_))
    }
}

/// Reads the state of the clone at `path`.
///
/// # Errors
///
/// Returns an error only when `git` itself cannot be run. A clone whose remote
/// is missing or unreadable is reported with an absent identity rather than as
/// a failure, since that is exactly the drift worth surfacing.
pub fn read(path: &Path) -> Result<LocalRepo, git::GitError> {
    read_within(path, None)
}

/// Reads a clone, noting which group it falls in relative to `root`.
///
/// # Errors
///
/// Returns an error only when `git` itself cannot be run.
pub fn read_within(path: &Path, root: Option<&Path>) -> Result<LocalRepo, git::GitError> {
    let remote_url = git::run_optional(path, &["remote", "get-url", "origin"]);

    let head = read_head(path);
    let status = git::run(path, &["status", "--porcelain"])?;

    Ok(LocalRepo {
        id: remote_url
            .as_deref()
            .and_then(|url| remote_url::parse(url).ok()),
        tracking: read_tracking(path, &head),
        dirty: has_tracked_changes(&status),
        untracked: has_untracked_files(&status),
        group: root.and_then(|root| group_of(path, root)),
        head,
        remote_url,
        path: path.to_owned(),
    })
}

/// The group a clone falls in: the first directory beneath its root.
///
/// A clone sitting directly in a root has no group, and neither has one whose
/// path does not lie beneath the root at all.
fn group_of(path: &Path, root: &Path) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?;
    let mut segments = relative.components();
    let first = segments.next()?;

    // A clone directly in the root is ungrouped: the last segment is the
    // repository itself, not a group.
    segments.next()?;

    Some(first.as_os_str().to_string_lossy().into_owned())
}

/// Whether `git status --porcelain` reported a change to a tracked file.
fn has_tracked_changes(status: &str) -> bool {
    status
        .lines()
        .any(|line| !line.trim().is_empty() && !line.starts_with("??"))
}

/// Whether `git status --porcelain` reported a file git is not tracking.
fn has_untracked_files(status: &str) -> bool {
    status.lines().any(|line| line.starts_with("??"))
}

/// Reads what is checked out.
fn read_head(path: &Path) -> Head {
    // A repository with no commits has a symbolic HEAD pointing nowhere, so the
    // branch name is readable even though no commit is.
    if git::run_optional(path, &["rev-parse", "--verify", "HEAD"]).is_none() {
        return Head::Unborn;
    }

    git::run_optional(path, &["symbolic-ref", "--quiet", "--short", "HEAD"])
        .map_or(Head::Detached, Head::Branch)
}

/// Reads how the checked-out branch stands against its upstream.
fn read_tracking(path: &Path, head: &Head) -> Option<Tracking> {
    let Head::Branch(branch) = head else {
        return None;
    };

    // `git` resolves `@{upstream}` itself and exits non-zero when the branch
    // tracks nothing, so a single `rev-list` replaces a `rev-parse` to name the
    // upstream ref followed by a `rev-list` against it, saving one process spawn
    // per clone in the parallel scan.
    let counts = git::run_optional(
        path,
        &[
            "rev-list",
            "--left-right",
            "--count",
            &format!("{branch}...{branch}@{{upstream}}"),
        ],
    )?;

    parse_counts(&counts)
}

/// Reads the two numbers `git rev-list --left-right --count` prints.
fn parse_counts(output: &str) -> Option<Tracking> {
    let mut counts = output.split_whitespace();
    let ahead = counts.next()?.parse().ok()?;
    let behind = counts.next()?.parse().ok()?;

    Some(Tracking { ahead, behind })
}

/// A root that could not be scanned.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("cannot scan `{}`: {message}", root.display())]
pub struct ScanError {
    /// The root that could not be read.
    pub root: PathBuf,
    /// What the operating system reported.
    pub message: String,
}

/// What a scan found, including the roots it could not read.
///
/// A root that cannot be read is reported rather than silently skipped, since a
/// mistyped root would otherwise look like an empty directory.
#[derive(Debug, Default)]
pub struct Scan {
    /// The clones found, in a stable order.
    pub repositories: Vec<LocalRepo>,

    /// Roots that could not be read.
    pub failures: Vec<ScanError>,

    /// Symlinked directories that were not followed, in a stable order.
    ///
    /// The walk does not follow symlinks, so a link cannot send it round in
    /// circles. Reporting the links it skipped means a projects tree kept
    /// behind one is explained rather than silently missing.
    pub skipped_symlinks: Vec<PathBuf>,

    /// Bare repositories that were recognised but not treated as clones, in a
    /// stable order.
    ///
    /// A bare repository has no working tree, so it has nothing to be dirty or
    /// ahead of. `minato` compares working clones, so a bare mirror is reported
    /// rather than silently missing, without being mistaken for one.
    pub skipped_bare: Vec<PathBuf>,
}

/// Finds every clone under `roots` and reads its state.
///
/// Reading is parallel, since each clone costs several `git` invocations and
/// they are independent.
#[must_use]
pub fn scan(roots: &ResolvedRoots, max_depth: usize) -> Scan {
    let mut directories: Vec<(PathBuf, PathBuf)> = Vec::new();
    let mut failures = Vec::new();
    let mut skipped_symlinks = Vec::new();
    let mut skipped_bare = Vec::new();

    for root in roots.iter() {
        let mut found = Vec::new();

        if let Err(failure) = collect(
            root,
            max_depth,
            &mut found,
            &mut skipped_symlinks,
            &mut skipped_bare,
        ) {
            failures.push(failure);
        }

        directories.extend(found.into_iter().map(|path| (path, root.clone())));
    }

    directories.sort();
    directories.dedup_by(|left, right| left.0 == right.0);

    skipped_symlinks.sort();
    skipped_symlinks.dedup();

    skipped_bare.sort();
    skipped_bare.dedup();

    let mut repositories: Vec<_> = directories
        .par_iter()
        .filter_map(|(directory, root)| read_within(directory, Some(root)).ok())
        .collect();

    repositories.sort_by(|left, right| left.path.cmp(&right.path));

    Scan {
        repositories,
        failures,
        skipped_symlinks,
        skipped_bare,
    }
}

/// Collects the clones under `directory`, without descending into them.
fn collect(
    directory: &Path,
    remaining_depth: usize,
    found: &mut Vec<PathBuf>,
    skipped_symlinks: &mut Vec<PathBuf>,
    skipped_bare: &mut Vec<PathBuf>,
) -> Result<(), ScanError> {
    if git::is_repository(directory) {
        found.push(directory.to_owned());

        // A clone's own contents are not searched: nested repositories are
        // submodules or vendored copies, not separate checkouts to manage.
        return Ok(());
    }

    if git::is_bare_repository(directory) {
        // A bare repository has no working tree to compare, so it is reported
        // rather than treated as a clone, and its object store is not walked
        // into.
        skipped_bare.push(directory.to_owned());
        return Ok(());
    }

    if remaining_depth == 0 {
        return Ok(());
    }

    let entries = std::fs::read_dir(directory).map_err(|error| ScanError {
        root: directory.to_owned(),
        message: error.to_string(),
    })?;

    for entry in entries.flatten() {
        let path = entry.path();

        // `file_type` does not follow symlinks, so a link pointing back up the
        // tree cannot send the walk round in circles.
        let Ok(kind) = entry.file_type() else {
            continue;
        };

        if is_hidden(&path) {
            continue;
        }

        if kind.is_symlink() {
            // The link is not followed, but a link to a directory is reported,
            // so a projects tree kept behind one is explained rather than
            // silently absent. `metadata` follows the link, so a broken one
            // resolves to an error and is left out.
            if path.metadata().is_ok_and(|meta| meta.is_dir()) {
                skipped_symlinks.push(path);
            }
            continue;
        }

        if kind.is_dir() {
            // A directory that cannot be read is skipped rather than failing
            // the whole scan; only an unreadable root is worth reporting.
            let _ = collect(
                &path,
                remaining_depth - 1,
                found,
                skipped_symlinks,
                skipped_bare,
            );
        }
    }

    Ok(())
}

/// Whether a directory should be skipped during the walk.
fn is_hidden(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with('.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_both_counts_in_the_order_git_prints_them() {
        assert_eq!(
            parse_counts("3\t5"),
            Some(Tracking {
                ahead: 3,
                behind: 5
            })
        );
    }

    #[test]
    fn ignores_output_that_is_not_two_numbers() {
        assert_eq!(parse_counts(""), None);
        assert_eq!(parse_counts("3"), None);
        assert_eq!(parse_counts("three\tfive"), None);
    }

    #[test]
    fn treats_matching_counts_as_in_sync() {
        assert!(
            Tracking {
                ahead: 0,
                behind: 0
            }
            .is_in_sync()
        );
        assert!(
            !Tracking {
                ahead: 0,
                behind: 1
            }
            .is_in_sync()
        );
    }

    #[test]
    fn separates_a_modified_tracked_file_from_an_untracked_one() {
        assert!(has_tracked_changes(" M src/main.rs"));
        assert!(!has_untracked_files(" M src/main.rs"));

        assert!(!has_tracked_changes("?? build.log"));
        assert!(has_untracked_files("?? build.log"));

        let both = " M src/main.rs\n?? build.log";
        assert!(has_tracked_changes(both));
        assert!(has_untracked_files(both));

        assert!(!has_tracked_changes(""));
        assert!(!has_untracked_files(""));
    }

    #[test]
    fn treats_a_staged_addition_as_a_tracked_change() {
        assert!(has_tracked_changes("A  new.rs"));
    }

    #[test]
    fn skips_hidden_directories() {
        assert!(is_hidden(Path::new("/tmp/.cargo")));
        assert!(!is_hidden(Path::new("/tmp/projects")));
    }
}
