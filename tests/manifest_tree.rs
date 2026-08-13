//! Recording a real tree of clones, and putting it back the way it was.
//!
//! These use the real `git` binary and real directories. What a manifest is for
//! is surviving a machine, so the claims worth testing are about disk: what is
//! recorded, what is noticed when the tree drifts, and what a relocation moves.

use std::path::{Path, PathBuf};
use std::process::Command;

use minato::actions::{self, Mode, Outcome};
use minato::config::ResolvedRoots;
use minato::manifest::{Manifest, RelativePath};
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

/// A tree of clones, each with a remote naming a repository on GitHub.
///
/// The remote is rewritten after cloning, so a scan reads the identity Minato
/// would read in a real tree while the objects still come from a local origin.
/// Nothing here reaches the network.
struct Tree {
    origins: tempfile::TempDir,
    root: tempfile::TempDir,
}

impl Tree {
    fn with(clones: &[(&str, &str)]) -> Self {
        let tree = Self {
            origins: tempfile::tempdir().expect("a directory for the origins"),
            root: tempfile::tempdir().expect("a temporary root"),
        };

        for (name, place) in clones {
            tree.clone_into_place(name, place);
        }

        tree
    }

    /// Clones a fresh repository into `place` beneath the tree.
    fn clone_into_place(&self, name: &str, place: &str) {
        let origin = self.origins.path().join(name);
        std::fs::create_dir_all(&origin).expect("the origin directory");
        git(&origin, &["init", "--initial-branch=main", "--quiet"]);
        git(&origin, &["config", "user.name", "Test"]);
        git(&origin, &["config", "user.email", "test@example.com"]);
        git(&origin, &["config", "commit.gpgsign", "false"]);
        std::fs::write(origin.join("file.txt"), name).expect("a file to commit");
        git(&origin, &["add", "."]);
        git(&origin, &["commit", "--quiet", "--message", "first"]);

        let destination = self.path().join(place);
        std::fs::create_dir_all(destination.parent().expect("a parent")).expect("the tree");

        assert!(
            Command::new("git")
                .args(["clone", "--quiet"])
                .arg(&origin)
                .arg(&destination)
                .status()
                .expect("git")
                .success()
        );

        git(
            &destination,
            &[
                "remote",
                "set-url",
                "origin",
                &format!("https://github.com/mcanouil/{name}.git"),
            ],
        );
    }

    fn path(&self) -> &Path {
        self.root.path()
    }

    fn scanned(&self) -> Vec<scan::LocalRepo> {
        scan::scan(
            &ResolvedRoots::from_resolved(vec![self.path().to_owned()]),
            scan::DEFAULT_MAX_DEPTH,
        )
        .repositories
    }

    fn recorded(&self) -> Manifest {
        let mut manifest = Manifest::load(self.path())
            .expect("the manifest to be readable")
            .unwrap_or_default();

        manifest.merge(self.path(), &self.scanned());
        manifest
            .save(self.path())
            .expect("the manifest to be written");

        manifest
    }
}

fn places(manifest: &Manifest) -> Vec<(String, String)> {
    manifest
        .repositories
        .iter()
        .map(|entry| (entry.id.name.clone(), entry.path.to_string()))
        .collect()
}

#[test]
fn recording_a_tree_names_every_clone_by_where_it_sits() {
    let tree = Tree::with(&[("minato", "apps/minato"), ("kata", "tools/kata")]);

    let manifest = tree.recorded();

    assert_eq!(
        places(&manifest),
        [
            ("minato".to_owned(), "apps/minato".to_owned()),
            ("kata".to_owned(), "tools/kata".to_owned()),
        ],
        "each clone is recorded where it sits, sorted by path"
    );
    assert_eq!(
        manifest.repositories[0].group().as_deref(),
        Some("apps"),
        "the group is read back out of the path rather than recorded beside it"
    );

    let read = Manifest::load(tree.path())
        .expect("the manifest to be readable")
        .expect("a manifest to have been written");

    assert_eq!(read, manifest, "what was written is what is read back");
}

#[test]
fn a_clone_that_moved_is_reported_and_left_alone_until_it_is_relocated() {
    let tree = Tree::with(&[("minato", "apps/minato")]);
    let manifest = tree.recorded();

    let recorded = tree.path().join("apps").join("minato");
    let moved = tree.path().join("elsewhere").join("minato");
    std::fs::create_dir_all(moved.parent().expect("a parent")).expect("the directory");
    std::fs::rename(&recorded, &moved).expect("the clone to be moved by hand");

    let plan = manifest.diff(tree.path(), &tree.scanned());

    assert!(plan.missing.is_empty(), "the clone is still in the tree");
    assert_eq!(plan.misplaced.len(), 1, "{plan:?}");
    assert_eq!(plan.misplaced[0].found, moved);
    assert!(
        moved.exists(),
        "reporting drift moves nothing, since someone put it there"
    );

    let report = actions::move_to_path(
        Some(plan.misplaced[0].entry.id.clone()),
        &plan.misplaced[0].found,
        plan.misplaced[0].entry.path.to_path(tree.path()),
        Mode::Execute,
    )
    .expect("the relocation to be made");

    assert!(matches!(report.outcome, Outcome::Done { .. }));
    assert!(recorded.join(".git").exists(), "the clone is back in place");
    assert!(
        manifest.diff(tree.path(), &tree.scanned()).is_empty(),
        "and the tree matches what is recorded"
    );
}

#[test]
fn a_clone_this_machine_does_not_hold_is_reported_and_kept_on_the_next_write() {
    let tree = Tree::with(&[("minato", "apps/minato"), ("kata", "tools/kata")]);
    tree.recorded();

    std::fs::remove_dir_all(tree.path().join("tools").join("kata")).expect("a clone to be removed");

    let manifest = Manifest::load(tree.path())
        .expect("the manifest to be readable")
        .expect("a manifest to have been written");
    let plan = manifest.diff(tree.path(), &tree.scanned());

    assert_eq!(
        plan.missing
            .iter()
            .map(|entry| entry.id.name.clone())
            .collect::<Vec<_>>(),
        ["kata"],
        "what is recorded and not here is what a restore would clone"
    );

    assert_eq!(
        places(&tree.recorded()),
        [
            ("minato".to_owned(), "apps/minato".to_owned()),
            ("kata".to_owned(), "tools/kata".to_owned()),
        ],
        "writing keeps what another machine may hold rather than dropping it"
    );
}

#[test]
fn a_clone_that_is_not_recorded_is_reported_and_then_recorded() {
    let tree = Tree::with(&[("minato", "apps/minato")]);
    let manifest = tree.recorded();

    tree.clone_into_place("brand", "quarto/brand");

    let plan = manifest.diff(tree.path(), &tree.scanned());

    assert_eq!(
        plan.unrecorded
            .iter()
            .map(|entry| entry.id.name.clone())
            .collect::<Vec<_>>(),
        ["brand"]
    );

    assert!(
        tree.recorded()
            .diff(tree.path(), &tree.scanned())
            .is_empty(),
        "writing takes in what the tree holds"
    );
}

#[test]
fn a_recorded_place_cannot_reach_outside_the_tree() {
    let tree = Tree::with(&[("minato", "apps/minato")]);
    let text = format!(
        "version = 1\n\n[[repository]]\nid = \"github:mcanouil/minato\"\npath = \"{}\"\n",
        "../../etc"
    );
    std::fs::write(Manifest::path_in(tree.path()), text).expect("a manifest to be written");

    assert!(
        Manifest::load(tree.path()).is_err(),
        "a manifest arriving over a sync tool cannot name a place outside the tree it describes"
    );
}

#[test]
fn a_recorded_place_resolves_beneath_the_tree_it_was_read_from() {
    let recorded = RelativePath::new("apps/minato").expect("a place beneath the root");

    assert_eq!(
        recorded.to_path(Path::new("/somewhere/else")),
        PathBuf::from("/somewhere/else/apps/minato"),
        "a tree restores under whatever it is called on this machine"
    );
}
