//! What the actions do, and refuse to do, against real repositories.
//!
//! These use the real `git` binary. The point of the module under test is that
//! it never destroys work, and that is not a claim a mock can support.

use std::path::{Path, PathBuf};
use std::process::Command;

use minato::actions::{self, Mode, Outcome};
use minato::compare::{self, TrackedOwners};
use minato::config::{Local, Protocol};
use minato::model::Provider;
use minato::scan;

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

    assert!(
        Command::new("git")
            .args(["clone", "--quiet"])
            .arg(&origin)
            .arg(&clone)
            .status()
            .expect("git")
            .success()
    );

    git(&clone, &["config", "user.name", "Test"]);
    git(&clone, &["config", "user.email", "test@example.com"]);
    git(&clone, &["config", "commit.gpgsign", "false"]);

    Pair {
        _root: root,
        origin,
        clone,
    }
}

/// Compares one clone against a remote that reports it, so the action layer
/// sees the states it would in a real run.
///
/// These fixtures have a filesystem path as their origin, which resolves to no
/// provider identity, so the clone is given a synthetic one and a matching
/// reported repository. Git still works against the real local origin, because
/// the actions operate on the path rather than on the identity.
fn comparisons_for(clone: &Path) -> Vec<compare::Comparison> {
    let id = minato::model::RepoId::new(Provider::GitHub, "mcanouil", "fixture");

    let mut local = scan::read(clone).expect("a readable clone");
    local.id = Some(id.clone());

    let reported = minato::model::RemoteRepo {
        id,
        default_branch: Some("main".to_owned()),
        is_private: false,
        is_archived: false,
        is_fork: false,
        upstream: None,
        metadata: minato::model::Metadata::default(),
    };

    compare::compare(
        std::slice::from_ref(&reported),
        std::slice::from_ref(&local),
        &TrackedOwners::default(),
    )
}

fn head_of(path: &Path) -> String {
    git(path, &["rev-parse", "HEAD"])
}

#[test]
fn a_clean_repository_that_is_behind_is_fast_forwarded() {
    let pair = clone_pair();

    commit(&pair.origin, "second");
    git(&pair.clone, &["fetch", "--quiet"]);

    let before = head_of(&pair.clone);
    let summary = actions::update_all(&comparisons_for(&pair.clone), Mode::Execute);

    assert_eq!(summary.counts().done, 1, "{:?}", summary.reports);
    assert_ne!(head_of(&pair.clone), before, "the branch should have moved");
    assert_eq!(
        head_of(&pair.clone),
        head_of(&pair.origin),
        "it should now match the remote"
    );
}

#[test]
fn a_dirty_repository_is_left_untouched_and_its_change_survives() {
    let pair = clone_pair();

    commit(&pair.origin, "second");
    git(&pair.clone, &["fetch", "--quiet"]);
    std::fs::write(pair.clone.join("file.txt"), "my unsaved work").expect("a file");

    let before = head_of(&pair.clone);
    let summary = actions::update_all(&comparisons_for(&pair.clone), Mode::Execute);

    assert_eq!(summary.counts().skipped, 1);
    assert_eq!(head_of(&pair.clone), before, "nothing should have moved");
    assert_eq!(
        std::fs::read_to_string(pair.clone.join("file.txt")).expect("the file"),
        "my unsaved work",
        "an uncommitted change must survive an update"
    );
}

#[test]
fn a_diverged_repository_is_left_untouched_and_keeps_its_commit() {
    let pair = clone_pair();

    commit(&pair.origin, "their work");
    commit(&pair.clone, "our work");
    git(&pair.clone, &["fetch", "--quiet"]);

    let before = head_of(&pair.clone);
    let summary = actions::update_all(&comparisons_for(&pair.clone), Mode::Execute);

    assert_eq!(summary.counts().skipped, 1);
    assert_eq!(
        head_of(&pair.clone),
        before,
        "a diverged history must never be rewritten"
    );
    assert_eq!(
        git(&pair.clone, &["log", "-1", "--pretty=%s"]),
        "our work",
        "the local commit must still be there"
    );
}

#[test]
fn a_detached_head_is_left_untouched() {
    let pair = clone_pair();

    commit(&pair.origin, "second");
    git(&pair.clone, &["fetch", "--quiet"]);

    let head = head_of(&pair.clone);
    git(&pair.clone, &["checkout", "--quiet", &head]);

    let summary = actions::update_all(&comparisons_for(&pair.clone), Mode::Execute);

    assert_eq!(summary.counts().skipped, 1);
    assert_eq!(head_of(&pair.clone), head);
}

#[test]
fn a_rehearsal_changes_nothing_but_says_what_it_would_do() {
    let pair = clone_pair();

    commit(&pair.origin, "second");
    git(&pair.clone, &["fetch", "--quiet"]);

    let before = head_of(&pair.clone);
    let summary = actions::update_all(&comparisons_for(&pair.clone), Mode::DryRun);

    assert_eq!(summary.counts().would, 1);
    assert_eq!(summary.counts().done, 0);
    assert_eq!(
        head_of(&pair.clone),
        before,
        "a rehearsal must not move anything"
    );

    let Outcome::Would { detail } = &summary.reports[0].outcome else {
        panic!("expected a rehearsal outcome, got {:?}", summary.reports[0]);
    };
    assert!(detail.contains("fast-forward"), "got: {detail}");
}

#[test]
fn an_untracked_file_does_not_prevent_an_update_and_is_not_destroyed() {
    let pair = clone_pair();

    commit(&pair.origin, "second");
    git(&pair.clone, &["fetch", "--quiet"]);
    std::fs::write(pair.clone.join("notes.txt"), "mine").expect("a file");

    let summary = actions::update_all(&comparisons_for(&pair.clone), Mode::Execute);

    assert_eq!(summary.counts().done, 1, "{:?}", summary.reports);
    assert_eq!(
        std::fs::read_to_string(pair.clone.join("notes.txt")).expect("the file"),
        "mine",
        "an untracked file must survive the update"
    );
}

#[test]
fn git_refuses_when_a_fast_forward_would_overwrite_an_untracked_file() {
    let pair = clone_pair();

    // The incoming commit adds a file the clone already has, untracked.
    std::fs::write(pair.origin.join("added.txt"), "theirs").expect("a file");
    git(&pair.origin, &["add", "."]);
    git(&pair.origin, &["commit", "--quiet", "--message", "add"]);
    git(&pair.clone, &["fetch", "--quiet"]);
    std::fs::write(pair.clone.join("added.txt"), "mine").expect("a file");

    let summary = actions::update_all(&comparisons_for(&pair.clone), Mode::Execute);

    assert_eq!(
        summary.counts().failed,
        1,
        "git should refuse rather than clobber: {:?}",
        summary.reports
    );
    assert!(summary.has_failures());
    assert_eq!(
        std::fs::read_to_string(pair.clone.join("added.txt")).expect("the file"),
        "mine",
        "the untracked file must be intact after the refusal"
    );
}

#[test]
fn fetching_never_moves_a_branch_even_when_behind() {
    let pair = clone_pair();

    commit(&pair.origin, "second");

    let before = head_of(&pair.clone);
    let summary = actions::fetch_all(&comparisons_for(&pair.clone), Mode::Execute);

    assert_eq!(summary.counts().done, 1, "{:?}", summary.reports);
    assert_eq!(
        head_of(&pair.clone),
        before,
        "a fetch must leave the branch where it was"
    );

    // But it did update the remote-tracking ref, which is the point.
    let tracking = scan::read(&pair.clone)
        .expect("a readable clone")
        .tracking
        .expect("tracking");

    assert_eq!(tracking.behind, 1);
}

#[test]
fn fetching_is_safe_on_a_dirty_repository() {
    let pair = clone_pair();

    commit(&pair.origin, "second");
    std::fs::write(pair.clone.join("file.txt"), "unsaved").expect("a file");

    let summary = actions::fetch_all(&comparisons_for(&pair.clone), Mode::Execute);

    assert_eq!(summary.counts().done, 1);
    assert_eq!(
        std::fs::read_to_string(pair.clone.join("file.txt")).expect("the file"),
        "unsaved",
        "fetching must not touch the working tree"
    );
}

#[test]
fn one_failure_does_not_stop_the_others() {
    let pair = clone_pair();
    let broken = pair.clone.parent().expect("a parent").join("not-a-repo");
    std::fs::create_dir_all(&broken).expect("a directory");

    commit(&pair.origin, "second");
    git(&pair.clone, &["fetch", "--quiet"]);

    let mut comparisons = comparisons_for(&pair.clone);

    // A comparison pointing at something that is not a repository at all.
    let mut broken_comparison = comparisons[0].clone();
    broken_comparison.path = Some(broken);
    comparisons.push(broken_comparison);

    let summary = actions::update_all(&comparisons, Mode::Execute);

    assert_eq!(summary.reports.len(), 2, "both should be reported");
    assert_eq!(
        summary.counts().done,
        1,
        "the healthy repository should still have been updated"
    );
    assert!(summary.has_failures(), "and the run should report failure");
}

#[test]
fn cloning_creates_the_layout_and_produces_a_working_clone() {
    let pair = clone_pair();
    let root = tempfile::tempdir().expect("a temporary directory");

    let destination = root.path().join("mcanouil/minato");

    minato::git::clone(&pair.origin.display().to_string(), &destination, false)
        .expect("the clone to succeed");

    assert!(destination.join(".git").exists(), "it should be a clone");
    assert_eq!(
        head_of(&destination),
        head_of(&pair.origin),
        "it should match what it was cloned from"
    );
}

#[test]
fn cloning_refuses_to_write_into_an_occupied_destination() {
    let root = tempfile::tempdir().expect("a temporary directory");
    let occupied = root.path().join("mcanouil/minato");
    std::fs::create_dir_all(&occupied).expect("the directory");
    std::fs::write(occupied.join("precious.txt"), "do not lose me").expect("a file");

    let id = minato::model::RepoId::new(Provider::GitHub, "mcanouil", "minato");
    let comparison = compare::Comparison {
        id: Some(id),
        path: None,
        group: None,
        state: compare::State::RemoteOnly,
        upstream: None,
        local: None,
        remote: None,
    };

    let local = Local {
        roots: vec![root.path().to_owned()],
        layout: "{owner}/{repo}".to_owned(),
        protocol: Protocol::Https,
    };

    let summary = actions::clone_missing(&[comparison], root.path(), &local, false, Mode::Execute);

    assert_eq!(summary.counts().skipped, 1, "{:?}", summary.reports);
    assert_eq!(
        std::fs::read_to_string(occupied.join("precious.txt")).expect("the file"),
        "do not lose me",
        "an occupied destination must be left exactly as it was"
    );
}
