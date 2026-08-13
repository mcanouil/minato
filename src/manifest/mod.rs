//! The record of where clones sit, kept in the root it describes.
//!
//! A scan derives the tree from disk on every run, which answers what is here
//! now but not what should be. The manifest is the other half: a file naming
//! each repository and the place it occupies beneath the root, so a lost or
//! rebuilt machine can be brought back to the same tree.
//!
//! It lives at the root rather than in the configuration, so the file itself is
//! the root reference: paths are relative to the directory holding it, and a
//! restore targets that directory whatever it is called on this machine. A
//! folder synced between machines therefore carries its own layout with it, and
//! nothing has to agree about absolute paths.
//!
//! Writing is additive. An entry whose clone is absent is kept rather than
//! dropped, since a machine holding part of a tree must not be able to erase
//! the rest of it from a file shared with the machines that hold the whole
//! thing. Removing a repository is [`Manifest::forget`], which is asked for.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::model::RepoId;
use crate::scan::LocalRepo;

/// The name of the manifest file, in the root it describes.
pub const FILE_NAME: &str = ".minato.toml";

/// The format version this build writes, and the highest it can read.
pub const VERSION: u32 = 1;

/// How a recorded path is written, quoted in every parse failure.
const PATH_FORM: &str = "write it relative to the manifest, with `/`, for example `apps/minato`";

/// The record of a tree, as found in the root it describes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// The format version the file was written with.
    pub version: u32,

    /// The repositories recorded, one per clone.
    #[serde(default, rename = "repository")]
    pub repositories: Vec<Entry>,
}

/// One repository, and the place it occupies beneath the root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    /// Which repository, written `provider:owner/name`.
    pub id: RepoId,

    /// Where its clone sits, relative to the manifest.
    pub path: RelativePath,
}

impl Entry {
    /// The group this entry falls in: every directory above the clone.
    ///
    /// The group is derived rather than recorded, so that the path stays the
    /// one statement of where a repository sits and the two can never disagree.
    #[must_use]
    pub fn group(&self) -> Option<String> {
        self.path.group()
    }
}

/// A path beneath the root, written with `/` and holding no way out of it.
///
/// The manifest arrives over whatever the user syncs with, so its paths are
/// input rather than something this program wrote. Building one is where that
/// is asserted: an absolute path, a `..`, or an empty segment is refused here,
/// and nowhere downstream has to wonder whether a recorded path can escape the
/// root it is joined to.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct RelativePath(String);

impl RelativePath {
    /// Builds a recorded path, refusing anything that could leave the root.
    ///
    /// # Errors
    ///
    /// Returns an error naming what is wrong when the path is empty, absolute,
    /// holds a backslash, or has a segment that is empty, `.`, or `..`.
    pub fn new(path: &str) -> Result<Self, InvalidPathError> {
        if path.is_empty() {
            return Err(InvalidPathError::Empty);
        }

        // A backslash is a legal character in a directory name on Unix and a
        // separator on Windows, so a path holding one means different trees on
        // different machines. Refusing it keeps a manifest portable.
        if path.contains('\\') {
            return Err(InvalidPathError::Backslash {
                path: path.to_owned(),
            });
        }

        if path.starts_with('/') {
            return Err(InvalidPathError::Absolute {
                path: path.to_owned(),
            });
        }

        for segment in path.split('/') {
            if segment.is_empty() || segment == "." || segment == ".." {
                return Err(InvalidPathError::Segment {
                    path: path.to_owned(),
                    segment: segment.to_owned(),
                });
            }
        }

        Ok(Self(path.to_owned()))
    }

    /// Describes where a clone sits relative to `root`.
    ///
    /// Returns `None` when the clone is not beneath the root, or when its path
    /// holds something a recorded path may not, such as a component that is not
    /// valid text.
    #[must_use]
    pub fn between(root: &Path, path: &Path) -> Option<Self> {
        let relative = path.strip_prefix(root).ok()?;

        let joined = relative
            .components()
            .map(|component| component.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");

        Self::new(&joined).ok()
    }

    /// The path as written, with `/` separators.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Where this sits beneath `root`, as a path for this platform.
    ///
    /// Segments are joined one at a time rather than handed to `Path::join` as
    /// a single string, so a recorded path means the same tree everywhere.
    #[must_use]
    pub fn to_path(&self, root: &Path) -> PathBuf {
        self.0
            .split('/')
            .fold(root.to_owned(), |path, segment| path.join(segment))
    }

    /// The directories above the clone, joined with `/`.
    ///
    /// A clone sitting directly in the root has no group, which matches what a
    /// scan reports for the same tree.
    #[must_use]
    pub fn group(&self) -> Option<String> {
        let (group, _) = self.0.rsplit_once('/')?;

        Some(group.to_owned())
    }
}

impl fmt::Display for RelativePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<RelativePath> for String {
    fn from(path: RelativePath) -> Self {
        path.0
    }
}

impl TryFrom<String> for RelativePath {
    type Error = InvalidPathError;

    fn try_from(path: String) -> Result<Self, Self::Error> {
        Self::new(&path)
    }
}

/// A recorded path that does not describe a place beneath the root.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvalidPathError {
    /// The path was empty.
    #[error("a recorded path is empty. {PATH_FORM}")]
    Empty,

    /// The path was absolute, so it would ignore the root entirely.
    #[error(
        "recorded path `{path}` is absolute, so it would not be under the manifest. {PATH_FORM}"
    )]
    Absolute {
        /// The offending path.
        path: String,
    },

    /// The path held a backslash, which separates paths on one platform and
    /// names a directory on another.
    #[error(
        "recorded path `{path}` holds `\\`, which means a different place on each platform. {PATH_FORM}"
    )]
    Backslash {
        /// The offending path.
        path: String,
    },

    /// A segment was empty, `.`, or `..`.
    #[error(
        "recorded path `{path}` has `{segment}` as a segment, which would lead somewhere else in the tree. {PATH_FORM}"
    )]
    Segment {
        /// The offending path.
        path: String,
        /// The segment that was refused.
        segment: String,
    },
}

/// Anything that can go wrong between asking for a manifest and holding one.
#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    /// The file could not be read.
    #[error("cannot read `{}`: {source}", path.display())]
    Read {
        /// The file involved.
        path: PathBuf,
        /// What the operating system reported.
        source: io::Error,
    },

    /// The file could not be written.
    #[error("cannot write `{}`: {source}", path.display())]
    Write {
        /// The file involved.
        path: PathBuf,
        /// What the operating system reported.
        source: io::Error,
    },

    /// The file is not valid TOML, or does not describe a manifest.
    #[error("cannot parse `{}`: {source}", path.display())]
    Parse {
        /// The file involved.
        path: PathBuf,
        /// What the parser reported.
        source: toml::de::Error,
    },

    /// The file was written by a newer Minato.
    #[error(
        "`{}` is version {version}, and this Minato reads up to version {VERSION}. Upgrade Minato to read it",
        path.display()
    )]
    UnsupportedVersion {
        /// The file involved.
        path: PathBuf,
        /// The version it declares.
        version: u32,
    },

    /// The same repository was recorded more than once.
    #[error("`{}` records `{id}` more than once, so where it belongs is ambiguous", path.display())]
    DuplicateId {
        /// The file involved.
        path: PathBuf,
        /// The repository recorded twice.
        id: RepoId,
    },

    /// Two repositories were recorded at the same place.
    #[error("`{}` records two repositories at `{recorded}`, which is one directory", path.display())]
    DuplicatePath {
        /// The file involved.
        path: PathBuf,
        /// The place recorded twice.
        recorded: RelativePath,
    },
}

impl Manifest {
    /// An empty manifest, in the version this build writes.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            version: VERSION,
            repositories: Vec::new(),
        }
    }

    /// Where the manifest of `root` is.
    #[must_use]
    pub fn path_in(root: &Path) -> PathBuf {
        root.join(FILE_NAME)
    }

    /// Reads the manifest in `root`, if there is one.
    ///
    /// An absent file is not a failure: it is the state of every root that has
    /// never been recorded, and the commands that write one say so themselves.
    ///
    /// # Errors
    ///
    /// Returns an error when the file exists but cannot be read, parsed, or
    /// trusted.
    pub fn load(root: &Path) -> Result<Option<Self>, ManifestError> {
        let path = Self::path_in(root);

        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(ManifestError::Read { path, source }),
        };

        Self::from_toml(&text, &path).map(Some)
    }

    /// Parses a manifest, checking what a type cannot state on its own.
    ///
    /// # Errors
    ///
    /// Returns an error when the text is not a manifest, declares a version
    /// this build cannot read, or records a repository or a place twice.
    pub fn from_toml(text: &str, path: &Path) -> Result<Self, ManifestError> {
        let manifest: Self = toml::from_str(text).map_err(|source| ManifestError::Parse {
            path: path.to_owned(),
            source,
        })?;

        // The version is checked before anything else is trusted: a file from a
        // newer Minato may hold entries this build would misread, and reporting
        // the version is more use than reporting what it made of them.
        if manifest.version > VERSION {
            return Err(ManifestError::UnsupportedVersion {
                path: path.to_owned(),
                version: manifest.version,
            });
        }

        manifest.check_unique(path)?;

        Ok(manifest)
    }

    /// Checks that no repository, and no place, is recorded twice.
    fn check_unique(&self, path: &Path) -> Result<(), ManifestError> {
        let mut seen_ids = BTreeSet::new();
        let mut seen_paths = BTreeSet::new();

        for entry in &self.repositories {
            if !seen_ids.insert(entry.id.clone()) {
                return Err(ManifestError::DuplicateId {
                    path: path.to_owned(),
                    id: entry.id.clone(),
                });
            }

            if !seen_paths.insert(entry.path.clone()) {
                return Err(ManifestError::DuplicatePath {
                    path: path.to_owned(),
                    recorded: entry.path.clone(),
                });
            }
        }

        Ok(())
    }

    /// Writes the manifest into `root`, sorted by path.
    ///
    /// Sorting is what keeps the file mergeable: two machines recording the
    /// same tree write the same lines in the same order, so a change to one
    /// repository is a change to one part of the file rather than to all of it.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be written.
    pub fn save(&mut self, root: &Path) -> Result<(), ManifestError> {
        let path = Self::path_in(root);

        self.repositories.sort_by(|left, right| {
            left.path
                .cmp(&right.path)
                .then_with(|| left.id.cmp(&right.id))
        });

        let text = self.to_toml();

        std::fs::write(&path, text).map_err(|source| ManifestError::Write { path, source })
    }

    /// The manifest as it is written to disk.
    #[must_use]
    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_else(|error| {
            unreachable!("a manifest holds only strings and a number, so it serialises: {error}")
        })
    }

    /// The entry recording `id`, if there is one.
    #[must_use]
    pub fn entry(&self, id: &RepoId) -> Option<&Entry> {
        self.repositories.iter().find(|entry| &entry.id == id)
    }

    /// Records `id` at `path`, replacing where it was recorded before.
    ///
    /// This is what keeps a manifest in step with a clone or a move: the
    /// repository is recorded once, at the place it now occupies.
    pub fn record(&mut self, id: RepoId, path: RelativePath) {
        if let Some(entry) = self.repositories.iter_mut().find(|entry| entry.id == id) {
            entry.path = path;
        } else {
            self.repositories.push(Entry { id, path });
        }
    }

    /// Takes in what a scan found beneath `root`, adding and updating entries.
    ///
    /// Nothing is removed. An entry whose clone is not on this machine is left
    /// alone, since the machine that holds it records the same file.
    pub fn merge(&mut self, root: &Path, scanned: &[LocalRepo]) {
        for local in scanned {
            let Some(id) = local.id.clone() else {
                continue;
            };

            let Some(path) = RelativePath::between(root, &local.path) else {
                continue;
            };

            self.record(id, path);
        }
    }

    /// Removes every entry for `wanted`, and reports what was removed.
    ///
    /// Matching follows the same rules as naming a repository anywhere else: a
    /// full identity, `owner/name`, or a bare name.
    pub fn forget(&mut self, wanted: &str) -> Vec<Entry> {
        let (forgotten, kept) = std::mem::take(&mut self.repositories)
            .into_iter()
            .partition(|entry| entry.id.is_named(wanted));

        self.repositories = kept;

        forgotten
    }

    /// Compares the manifest against what a scan found beneath `root`.
    #[must_use]
    pub fn diff(&self, root: &Path, scanned: &[LocalRepo]) -> Plan {
        let found: BTreeMap<&RepoId, &LocalRepo> = scanned
            .iter()
            .filter(|local| local.path.starts_with(root))
            .filter_map(|local| local.id.as_ref().map(|id| (id, local)))
            .collect();

        let mut recorded = BTreeSet::new();
        let mut missing = Vec::new();
        let mut misplaced = Vec::new();

        for entry in &self.repositories {
            recorded.insert(&entry.id);

            let Some(local) = found.get(&entry.id) else {
                missing.push(entry.clone());
                continue;
            };

            if RelativePath::between(root, &local.path).as_ref() != Some(&entry.path) {
                misplaced.push(Misplaced {
                    entry: entry.clone(),
                    found: local.path.clone(),
                });
            }
        }

        let unrecorded = found
            .iter()
            .filter(|(id, _)| !recorded.contains(*id))
            .filter_map(|(id, local)| {
                RelativePath::between(root, &local.path).map(|path| Entry {
                    id: (*id).clone(),
                    path,
                })
            })
            .collect();

        Plan {
            missing,
            misplaced,
            unrecorded,
        }
    }
}

impl Default for Manifest {
    fn default() -> Self {
        Self::new()
    }
}

/// How the manifest and the tree beneath the root disagree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Plan {
    /// Recorded, with no clone anywhere beneath the root.
    pub missing: Vec<Entry>,

    /// Recorded, with a clone sitting somewhere other than the recorded place.
    pub misplaced: Vec<Misplaced>,

    /// Cloned beneath the root, and recorded nowhere.
    pub unrecorded: Vec<Entry>,
}

impl Plan {
    /// Whether the manifest and the tree agree.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.missing.is_empty() && self.misplaced.is_empty() && self.unrecorded.is_empty()
    }
}

/// A clone that is not where the manifest records it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Misplaced {
    /// What the manifest records.
    pub entry: Entry,

    /// Where the clone actually is.
    pub found: PathBuf,
}

/// The nearest root at or above `start`, meaning the nearest manifest.
///
/// A manifest names the root it describes by sitting in it, so finding one is
/// how a command run from inside a tree knows which tree it is in. The search
/// stops at the first one found, so a manifest nested inside another describes
/// the tree around it rather than being shadowed by its parent.
#[must_use]
pub fn find_upward(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|directory| Manifest::path_in(directory).is_file())
        .map(Path::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Provider;
    use crate::scan::Head;

    fn id(name: &str) -> RepoId {
        RepoId::new(Provider::GitHub, "mcanouil", name)
    }

    fn entry(name: &str, path: &str) -> Entry {
        Entry {
            id: id(name),
            path: RelativePath::new(path).expect("the fixture path to be valid"),
        }
    }

    fn local(name: Option<&str>, path: &str) -> LocalRepo {
        LocalRepo {
            path: PathBuf::from(path),
            id: name.map(id),
            remote_url: None,
            head: Head::Branch("main".to_owned()),
            tracking: None,
            dirty: false,
            group: None,
            untracked: false,
        }
    }

    fn manifest(entries: Vec<Entry>) -> Manifest {
        Manifest {
            version: VERSION,
            repositories: entries,
        }
    }

    #[test]
    fn a_manifest_round_trips_through_toml() {
        let mut written = manifest(vec![entry("minato", "apps/minato"), entry("kata", "kata")]);
        let text = written.to_toml();

        let read = Manifest::from_toml(&text, Path::new("/code/.minato.toml"))
            .expect("what was just written to parse");

        written
            .repositories
            .sort_by(|left, right| left.path.cmp(&right.path));
        let mut read = read;
        read.repositories
            .sort_by(|left, right| left.path.cmp(&right.path));

        assert_eq!(read, written);
    }

    #[test]
    fn a_newer_version_is_refused_rather_than_half_read() {
        let text = format!("version = {}\n", VERSION + 1);

        assert!(
            matches!(
                Manifest::from_toml(&text, Path::new("/code/.minato.toml")),
                Err(ManifestError::UnsupportedVersion { version, .. }) if version == VERSION + 1
            ),
            "a manifest from a newer Minato is refused by version"
        );
    }

    #[test]
    fn an_unknown_field_is_refused() {
        let text = "version = 1\nroots = [\"~/Projects\"]\n";

        assert!(
            matches!(
                Manifest::from_toml(text, Path::new("/code/.minato.toml")),
                Err(ManifestError::Parse { .. })
            ),
            "a manifest holding a field Minato does not know is refused"
        );
    }

    #[test]
    fn a_repository_recorded_twice_is_refused() {
        let text = "version = 1\n\
                    [[repository]]\nid = \"github:mcanouil/minato\"\npath = \"apps/minato\"\n\
                    [[repository]]\nid = \"github:mcanouil/minato\"\npath = \"tools/minato\"\n";

        assert!(
            matches!(
                Manifest::from_toml(text, Path::new("/code/.minato.toml")),
                Err(ManifestError::DuplicateId { .. })
            ),
            "a repository recorded in two places has no single place to restore to"
        );
    }

    #[test]
    fn two_repositories_in_one_place_are_refused() {
        let text = "version = 1\n\
                    [[repository]]\nid = \"github:mcanouil/minato\"\npath = \"apps/thing\"\n\
                    [[repository]]\nid = \"github:mcanouil/kata\"\npath = \"apps/thing\"\n";

        assert!(
            matches!(
                Manifest::from_toml(text, Path::new("/code/.minato.toml")),
                Err(ManifestError::DuplicatePath { .. })
            ),
            "two repositories cannot occupy one directory"
        );
    }

    #[test]
    fn a_path_that_could_leave_the_root_is_refused() {
        for path in [
            "/etc",
            "../elsewhere",
            "apps/../..",
            "apps//minato",
            "apps\\minato",
            "",
        ] {
            assert!(
                RelativePath::new(path).is_err(),
                "`{path}` can lead outside the root, so it is not a recorded path"
            );
        }
    }

    #[test]
    fn a_path_beneath_the_root_is_accepted_and_joined_one_segment_at_a_time() {
        let path = RelativePath::new("apps/minato").expect("a path beneath the root");

        assert_eq!(
            path.to_path(Path::new("/code")),
            PathBuf::from("/code/apps/minato")
        );
        assert_eq!(path.group().as_deref(), Some("apps"));
        assert_eq!(
            RelativePath::new("minato")
                .expect("a clone in the root")
                .group(),
            None
        );
    }

    #[test]
    fn a_recorded_path_is_read_back_out_of_a_scanned_one() {
        assert_eq!(
            RelativePath::between(Path::new("/code"), Path::new("/code/apps/minato"))
                .as_ref()
                .map(RelativePath::as_str),
            Some("apps/minato")
        );
        assert_eq!(
            RelativePath::between(Path::new("/code"), Path::new("/elsewhere/minato")),
            None
        );
    }

    #[test]
    fn saving_sorts_by_path_so_the_file_merges() {
        let mut manifest = manifest(vec![
            entry("minato", "apps/minato"),
            entry("kata", "apps/kata"),
            entry("brand", "quarto/brand"),
        ]);
        let root = tempfile::tempdir().expect("a temporary root");

        manifest
            .save(root.path())
            .expect("the manifest to be written");

        let text = std::fs::read_to_string(Manifest::path_in(root.path()))
            .expect("the manifest to be readable");
        let order: Vec<&str> = text
            .lines()
            .filter_map(|line| line.strip_prefix("path = "))
            .collect();

        assert_eq!(
            order,
            ["\"apps/kata\"", "\"apps/minato\"", "\"quarto/brand\""]
        );
    }

    #[test]
    fn an_absent_manifest_is_not_a_failure() {
        let root = tempfile::tempdir().expect("a temporary root");

        assert!(
            Manifest::load(root.path())
                .expect("an absent manifest to be readable")
                .is_none(),
            "a root that has never been recorded has no manifest, which is not an error"
        );
    }

    #[test]
    fn merging_adds_what_is_new_updates_what_moved_and_keeps_what_is_absent() {
        let mut manifest = manifest(vec![
            entry("minato", "apps/minato"),
            entry("kata", "apps/kata"),
        ]);

        manifest.merge(
            Path::new("/code"),
            &[
                local(Some("minato"), "/code/tools/minato"),
                local(Some("brand"), "/code/quarto/brand"),
                local(None, "/code/unknown"),
            ],
        );

        let mut recorded: Vec<(String, String)> = manifest
            .repositories
            .iter()
            .map(|entry| (entry.id.name.clone(), entry.path.to_string()))
            .collect();
        recorded.sort();

        assert_eq!(
            recorded,
            [
                ("brand".to_owned(), "quarto/brand".to_owned()),
                ("kata".to_owned(), "apps/kata".to_owned()),
                ("minato".to_owned(), "tools/minato".to_owned()),
            ],
            "a clone that moved is recorded where it now is, one absent from this machine is kept, and one with no identity is skipped"
        );
    }

    #[test]
    fn merging_twice_records_the_same_tree() {
        let mut once = manifest(vec![entry("minato", "apps/minato")]);
        let scanned = [local(Some("kata"), "/code/apps/kata")];

        once.merge(Path::new("/code"), &scanned);
        let mut twice = once.clone();
        twice.merge(Path::new("/code"), &scanned);

        assert_eq!(twice, once, "merging is settled after the first pass");
    }

    #[test]
    fn a_diff_separates_what_is_missing_misplaced_and_unrecorded() {
        let manifest = manifest(vec![
            entry("minato", "apps/minato"),
            entry("kata", "apps/kata"),
        ]);

        let plan = manifest.diff(
            Path::new("/code"),
            &[
                local(Some("kata"), "/code/tools/kata"),
                local(Some("brand"), "/code/quarto/brand"),
                local(Some("elsewhere"), "/other/elsewhere"),
            ],
        );

        assert_eq!(
            plan.missing
                .iter()
                .map(|entry| entry.id.name.clone())
                .collect::<Vec<_>>(),
            ["minato"]
        );
        assert_eq!(
            plan.misplaced
                .iter()
                .map(|misplaced| (misplaced.entry.id.name.clone(), misplaced.found.clone()))
                .collect::<Vec<_>>(),
            [("kata".to_owned(), PathBuf::from("/code/tools/kata"))]
        );
        assert_eq!(
            plan.unrecorded
                .iter()
                .map(|entry| entry.id.name.clone())
                .collect::<Vec<_>>(),
            ["brand"],
            "a clone outside the root belongs to another tree and is not reported here"
        );
    }

    #[test]
    fn forgetting_removes_by_any_way_of_naming_a_repository() {
        for wanted in ["minato", "mcanouil/minato", "github:mcanouil/minato"] {
            let mut manifest = manifest(vec![
                entry("minato", "apps/minato"),
                entry("kata", "apps/kata"),
            ]);

            let forgotten = manifest.forget(wanted);

            assert_eq!(forgotten.len(), 1, "`{wanted}` names one repository");
            assert_eq!(
                manifest
                    .repositories
                    .iter()
                    .map(|entry| entry.id.name.clone())
                    .collect::<Vec<_>>(),
                ["kata"],
                "forgetting `{wanted}` leaves the rest of the tree recorded"
            );
        }
    }

    #[test]
    fn the_nearest_manifest_above_a_directory_names_the_root() {
        let root = tempfile::tempdir().expect("a temporary root");
        let nested = root.path().join("apps").join("minato").join("src");
        std::fs::create_dir_all(&nested).expect("the tree to be created");
        std::fs::write(Manifest::path_in(root.path()), "version = 1\n")
            .expect("a manifest to be written");

        assert_eq!(find_upward(&nested).as_deref(), Some(root.path()));
        assert_eq!(
            find_upward(Path::new("/")),
            None,
            "a directory with no manifest above it is in no recorded tree"
        );
    }
}

#[cfg(test)]
mod properties {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// The claim the whole type exists for: whatever a manifest holds, a
        /// place accepted from it lands under the tree it was read from.
        #[test]
        fn an_accepted_place_never_leaves_the_tree(text in "\\PC{0,32}") {
            if let Ok(recorded) = RelativePath::new(&text) {
                let root = Path::new("/code");

                prop_assert!(
                    recorded.to_path(root).starts_with(root),
                    "`{text}` was accepted and resolved outside the tree"
                );
            }
        }

        /// A place read out of a scanned path describes that same path, so
        /// recording a tree and restoring it are the same statement.
        #[test]
        fn a_place_read_from_a_scan_resolves_back_to_it(
            segments in prop::collection::vec("[a-z]{1,4}", 1..4),
        ) {
            let root = Path::new("/code");
            let scanned = segments.iter().fold(root.to_owned(), |path, segment| path.join(segment));

            let recorded = RelativePath::between(root, &scanned)
                .expect("a path beneath the root to be describable");

            prop_assert_eq!(recorded.to_path(root), scanned);
        }
    }
}
