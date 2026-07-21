//! Scanning real git repositories.
//!
//! These build actual repositories with the real `git` binary rather than
//! mocking it, because the whole purpose of this module is to agree with what
//! git reports.

use std::path::{Path, PathBuf};
use std::process::Command;

use minato::config::ResolvedRoots;
use minato::scan::{self, Head};

/// Wraps temporary directory paths, already absolute, as resolved scan roots.
fn roots(paths: Vec<PathBuf>) -> ResolvedRoots {
    ResolvedRoots::from_resolved(paths)
}

/// Runs `git` in `directory`, panicking with its output when it fails.
fn git(directory: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(arguments)
        .output()
        .expect("git to be installed");

    assert!(
        output.status.success(),
        "git {arguments:?} failed in {}: {}",
        directory.display(),
        String::from_utf8_lossy(&output.stderr)
    );

    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// Creates a repository with one commit and a deterministic identity.
fn init_repository(path: &Path) {
    std::fs::create_dir_all(path).expect("the directory");

    git(path, &["init", "--initial-branch=main", "--quiet"]);
    git(path, &["config", "user.name", "Test"]);
    git(path, &["config", "user.email", "test@example.com"]);
    git(path, &["config", "commit.gpgsign", "false"]);
}

fn commit(path: &Path, message: &str) {
    std::fs::write(path.join("file.txt"), message).expect("a file");
    git(path, &["add", "."]);
    git(path, &["commit", "--quiet", "--message", message]);
}

/// A repository, plus a clone of it that tracks it.
struct Pair {
    _root: tempfile::TempDir,
    origin: PathBuf,
    clone: PathBuf,
}

fn clone_pair() -> Pair {
    let root = tempfile::tempdir().expect("a temporary directory");
    let origin = root.path().join("origin");
    let clone = root.path().join("clone");

    init_repository(&origin);
    commit(&origin, "first");

    let status = Command::new("git")
        .args(["clone", "--quiet"])
        .arg(&origin)
        .arg(&clone)
        .status()
        .expect("git to be installed");
    assert!(status.success(), "the clone should succeed");

    git(&clone, &["config", "user.name", "Test"]);
    git(&clone, &["config", "user.email", "test@example.com"]);
    git(&clone, &["config", "commit.gpgsign", "false"]);

    Pair {
        _root: root,
        origin,
        clone,
    }
}

#[test]
fn reports_a_fresh_clone_as_in_sync_on_its_branch() {
    let pair = clone_pair();

    let repository = scan::read(&pair.clone).expect("a readable clone");

    assert_eq!(repository.head, Head::Branch("main".to_owned()));
    assert!(!repository.dirty);
    assert!(
        repository.tracking.expect("tracking").is_in_sync(),
        "a fresh clone has nothing to reconcile"
    );
    assert!(repository.is_updatable());
}

#[test]
fn counts_commits_the_remote_has_and_the_clone_does_not() {
    let pair = clone_pair();

    commit(&pair.origin, "second");
    commit(&pair.origin, "third");
    git(&pair.clone, &["fetch", "--quiet"]);

    let tracking = scan::read(&pair.clone)
        .expect("a readable clone")
        .tracking
        .expect("tracking");

    assert_eq!(tracking.behind, 2);
    assert_eq!(tracking.ahead, 0);
}

#[test]
fn counts_commits_the_clone_has_and_the_remote_does_not() {
    let pair = clone_pair();

    commit(&pair.clone, "local work");

    let tracking = scan::read(&pair.clone)
        .expect("a readable clone")
        .tracking
        .expect("tracking");

    assert_eq!(tracking.ahead, 1);
    assert_eq!(tracking.behind, 0);
}

#[test]
fn counts_both_sides_when_history_has_diverged() {
    let pair = clone_pair();

    commit(&pair.origin, "their work");
    commit(&pair.clone, "our work");
    git(&pair.clone, &["fetch", "--quiet"]);

    let tracking = scan::read(&pair.clone)
        .expect("a readable clone")
        .tracking
        .expect("tracking");

    assert_eq!(tracking.ahead, 1);
    assert_eq!(tracking.behind, 1);
    assert!(!tracking.is_in_sync());
}

#[test]
fn reads_counts_without_needing_the_network() {
    let pair = clone_pair();

    commit(&pair.origin, "second");
    git(&pair.clone, &["fetch", "--quiet"]);

    // Destroying the origin makes it unreachable while leaving the clone's
    // upstream configuration and remote-tracking refs intact, which is the
    // situation an offline scan is in.
    std::fs::remove_dir_all(&pair.origin).expect("the origin to be removed");

    let repository = scan::read(&pair.clone).expect("a readable clone");

    assert_eq!(
        repository.tracking.expect("tracking").behind,
        1,
        "counts come from refs on disk, not from contacting the remote"
    );
}

#[test]
fn reports_an_uncommitted_change_as_dirty_and_not_updatable() {
    let pair = clone_pair();

    std::fs::write(pair.clone.join("file.txt"), "edited").expect("a file");

    let repository = scan::read(&pair.clone).expect("a readable clone");

    assert!(repository.dirty);
    assert!(
        !repository.is_updatable(),
        "an update would have to decide what to do with the change"
    );
}

#[test]
fn does_not_let_an_untracked_file_block_an_update() {
    let pair = clone_pair();

    std::fs::write(pair.clone.join("build.log"), "artefact").expect("a file");

    let repository = scan::read(&pair.clone).expect("a readable clone");

    assert!(repository.untracked);
    assert!(!repository.dirty, "nothing tracked was modified");
    assert!(
        repository.is_updatable(),
        "a stray artefact must not stop a repository being updated forever"
    );
}

#[test]
fn a_fast_forward_really_does_succeed_alongside_an_untracked_file() {
    let pair = clone_pair();

    commit(&pair.origin, "second");
    git(&pair.clone, &["fetch", "--quiet"]);
    std::fs::write(pair.clone.join("build.log"), "artefact").expect("a file");

    assert!(
        scan::read(&pair.clone)
            .expect("a readable clone")
            .is_updatable(),
        "the scanner should consider this updatable"
    );

    // And git agrees, which is what makes that judgement correct.
    git(&pair.clone, &["merge", "--ff-only", "@{upstream}"]);
}

#[test]
fn reports_a_detached_head_as_not_updatable() {
    let pair = clone_pair();

    let head = git(&pair.clone, &["rev-parse", "HEAD"]);
    git(&pair.clone, &["checkout", "--quiet", &head]);

    let repository = scan::read(&pair.clone).expect("a readable clone");

    assert_eq!(repository.head, Head::Detached);
    assert_eq!(repository.tracking, None, "a detached head tracks nothing");
    assert!(!repository.is_updatable());
}

#[test]
fn reports_a_repository_with_no_commits_rather_than_failing() {
    let root = tempfile::tempdir().expect("a temporary directory");
    let path = root.path().join("empty");
    init_repository(&path);

    let repository = scan::read(&path).expect("a readable repository");

    assert_eq!(repository.head, Head::Unborn);
    assert_eq!(repository.tracking, None);
}

#[test]
fn reports_a_branch_that_tracks_nothing() {
    let pair = clone_pair();

    git(&pair.clone, &["checkout", "--quiet", "-b", "untracked"]);

    let repository = scan::read(&pair.clone).expect("a readable clone");

    assert_eq!(repository.head, Head::Branch("untracked".to_owned()));
    assert_eq!(repository.tracking, None);
}

#[test]
fn recovers_the_identity_from_a_recognised_remote() {
    let pair = clone_pair();

    git(
        &pair.clone,
        &[
            "remote",
            "set-url",
            "origin",
            "git@github.com:McAnouil/Minato.git",
        ],
    );

    let repository = scan::read(&pair.clone).expect("a readable clone");

    assert_eq!(
        repository.id.expect("an identity").to_string(),
        "github:mcanouil/minato"
    );
}

#[test]
fn keeps_an_unrecognised_remote_for_reporting_rather_than_discarding_it() {
    let pair = clone_pair();

    git(
        &pair.clone,
        &[
            "remote",
            "set-url",
            "origin",
            "git@example.com:some/thing.git",
        ],
    );

    let repository = scan::read(&pair.clone).expect("a readable clone");

    assert_eq!(
        repository.id, None,
        "an unsupported host yields no identity to match against"
    );
    assert_eq!(
        repository.remote_url.as_deref(),
        Some("git@example.com:some/thing.git"),
        "the remote is kept so the report can say what it found"
    );
}

#[test]
fn reports_a_repository_with_no_remote_at_all() {
    let root = tempfile::tempdir().expect("a temporary directory");
    let path = root.path().join("local-only");
    init_repository(&path);
    commit(&path, "only here");

    let repository = scan::read(&path).expect("a readable repository");

    assert_eq!(repository.id, None);
    assert_eq!(repository.remote_url, None);
}

#[test]
fn finds_clones_nested_under_a_layout() {
    let root = tempfile::tempdir().expect("a temporary directory");

    for path in ["github/mcanouil/one", "github/some-org/two"] {
        let full = root.path().join(path);
        init_repository(&full);
        commit(&full, "first");
    }

    std::fs::create_dir_all(root.path().join("not-a-repository")).expect("a directory");

    let found = scan::scan(
        &roots(vec![root.path().to_owned()]),
        scan::DEFAULT_MAX_DEPTH,
    );

    assert_eq!(found.repositories.len(), 2);
    assert!(found.failures.is_empty());
}

#[test]
fn does_not_descend_into_a_clone_it_has_already_found() {
    let root = tempfile::tempdir().expect("a temporary directory");
    let outer = root.path().join("outer");
    init_repository(&outer);
    commit(&outer, "first");

    let inner = outer.join("vendor/inner");
    init_repository(&inner);
    commit(&inner, "first");

    let found = scan::scan(
        &roots(vec![root.path().to_owned()]),
        scan::DEFAULT_MAX_DEPTH,
    );

    assert_eq!(
        found.repositories.len(),
        1,
        "a repository inside a clone is a submodule or a vendored copy"
    );
    assert_eq!(found.repositories[0].path, outer);
}

#[test]
fn stops_at_the_depth_limit() {
    let root = tempfile::tempdir().expect("a temporary directory");
    let deep = root.path().join("a/b/c/d/e/buried");
    init_repository(&deep);
    commit(&deep, "first");

    assert!(
        scan::scan(&roots(vec![root.path().to_owned()]), 2)
            .repositories
            .is_empty(),
        "a shallow scan should not walk the whole tree"
    );
    assert_eq!(
        scan::scan(&roots(vec![root.path().to_owned()]), 8)
            .repositories
            .len(),
        1
    );
}

#[test]
fn reports_a_root_that_does_not_exist_rather_than_finding_nothing() {
    let found = scan::scan(&roots(vec![PathBuf::from("/nonexistent/root")]), 2);

    assert!(found.repositories.is_empty());
    assert_eq!(
        found.failures.len(),
        1,
        "a mistyped root must not look empty"
    );
    assert!(
        found.failures[0].to_string().contains("/nonexistent/root"),
        "the failure should name the root, got: {}",
        found.failures[0]
    );
}

#[test]
fn returns_clones_in_a_stable_order() {
    let root = tempfile::tempdir().expect("a temporary directory");

    for path in ["c", "a", "b"] {
        let full = root.path().join(path);
        init_repository(&full);
        commit(&full, "first");
    }

    let first = scan::scan(&roots(vec![root.path().to_owned()]), 2);
    let second = scan::scan(&roots(vec![root.path().to_owned()]), 2);

    let paths: Vec<_> = first.repositories.iter().map(|repo| &repo.path).collect();
    let again: Vec<_> = second.repositories.iter().map(|repo| &repo.path).collect();

    assert_eq!(paths, again, "parallel reading must not reorder the output");
    assert!(paths.windows(2).all(|pair| pair[0] <= pair[1]));
}

#[test]
fn scans_the_same_root_once_even_when_it_is_listed_twice() {
    let root = tempfile::tempdir().expect("a temporary directory");
    let path = root.path().join("one");
    init_repository(&path);
    commit(&path, "first");

    let found = scan::scan(
        &roots(vec![root.path().to_owned(), root.path().to_owned()]),
        scan::DEFAULT_MAX_DEPTH,
    );

    assert_eq!(found.repositories.len(), 1);
}

#[cfg(unix)]
#[test]
fn reports_a_symlinked_directory_rather_than_following_it() {
    // The clone lives outside the scanned root, reachable only through a
    // symlink, so following the link is the only way it could be found.
    let target = tempfile::tempdir().expect("a target directory");
    let clone = target.path().join("project");
    init_repository(&clone);
    commit(&clone, "first");

    let root = tempfile::tempdir().expect("a temporary directory");
    let link = root.path().join("projects");
    std::os::unix::fs::symlink(target.path(), &link).expect("a symlink");

    let found = scan::scan(
        &roots(vec![root.path().to_owned()]),
        scan::DEFAULT_MAX_DEPTH,
    );

    assert!(
        found.repositories.is_empty(),
        "a symlink is not followed, so the clone behind it is not found"
    );
    assert_eq!(
        found.skipped_symlinks,
        vec![link],
        "the skipped link is reported so the clone behind it is not silently missing"
    );
}
