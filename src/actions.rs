//! Doing things to repositories.
//!
//! Every action here is opt-in and narrow. Nothing force-pushes, rebases,
//! discards a change, or touches a working tree that was not asked for.
//!
//! Whether an action is permitted is decided in [`crate::compare`], not here.
//! This module carries it out and reports what happened, including for the
//! repositories it deliberately left alone.

use std::path::{Path, PathBuf};

use rayon::prelude::*;
use serde::Serialize;

use crate::compare::{Comparison, LocalOnlyReason, State};
use crate::config::Local;
use crate::git;
use crate::model::{CloneProtocol, RepoId};

/// What happened to one repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum Outcome {
    /// The action was carried out.
    Done {
        /// What was done, in words.
        detail: String,
    },

    /// The action would have been carried out, but this was a rehearsal.
    Would {
        /// What would have been done.
        detail: String,
    },

    /// The action did not apply, which is not a failure.
    Skipped {
        /// Why it did not apply.
        reason: String,
    },

    /// The action was attempted and failed.
    Failed {
        /// What went wrong.
        error: String,
    },
}

impl Outcome {
    /// Whether this outcome should make the process exit non-zero.
    #[must_use]
    pub const fn is_failure(&self) -> bool {
        matches!(self, Self::Failed { .. })
    }
}

/// One repository, and what happened to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Report {
    /// Which repository.
    pub id: Option<RepoId>,

    /// Where it is, or where it would go.
    pub path: Option<PathBuf>,

    /// What happened.
    pub outcome: Outcome,
}

/// Everything that happened, and whether any of it failed.
#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    /// One entry per repository considered.
    pub reports: Vec<Report>,
}

impl Summary {
    /// Whether anything failed, which decides the exit code.
    #[must_use]
    pub fn has_failures(&self) -> bool {
        self.reports
            .iter()
            .any(|report| report.outcome.is_failure())
    }

    /// How many repositories fell into each kind of outcome.
    #[must_use]
    pub fn counts(&self) -> Counts {
        let mut counts = Counts::default();

        for report in &self.reports {
            match report.outcome {
                Outcome::Done { .. } => counts.done += 1,
                Outcome::Would { .. } => counts.would += 1,
                Outcome::Skipped { .. } => counts.skipped += 1,
                Outcome::Failed { .. } => counts.failed += 1,
            }
        }

        counts
    }
}

/// How many repositories fell into each kind of outcome.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Counts {
    /// Acted on.
    pub done: usize,
    /// Would have been acted on, in a rehearsal.
    pub would: usize,
    /// Left alone, with a reason.
    pub skipped: usize,
    /// Attempted and failed.
    pub failed: usize,
}

/// Whether to carry an action out or only report what it would do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Do it.
    Execute,
    /// Say what would be done, and change nothing.
    DryRun,
}

impl Mode {
    /// Builds the outcome for an action that was permitted.
    fn permitted(self, detail: String, run: impl FnOnce() -> Result<(), git::GitError>) -> Outcome {
        match self {
            Self::DryRun => Outcome::Would { detail },
            Self::Execute => match run() {
                Ok(()) => Outcome::Done { detail },
                Err(error) => Outcome::Failed {
                    error: error.to_string(),
                },
            },
        }
    }
}

/// Where a repository would be cloned to.
///
/// The layout is applied under the first configured root. Placeholders are
/// validated when configuration is loaded, so an unknown one cannot reach here.
#[must_use]
pub fn clone_destination(root: &Path, layout: &str, id: &RepoId) -> PathBuf {
    let rendered = layout
        .replace("{provider}", id.provider.as_str())
        .replace("{owner}", &id.owner)
        .replace("{repo}", &id.name);

    root.join(rendered)
}

/// Clones every repository that has no local copy.
///
/// A repository is only cloned when nothing exists at its destination. An
/// occupied destination is reported rather than written into, since the thing
/// already there was not put there by this run.
#[must_use]
pub fn clone_missing(
    comparisons: &[Comparison],
    root: &Path,
    local: &Local,
    shallow: bool,
    mode: Mode,
) -> Summary {
    let protocol = match local.protocol {
        crate::config::Protocol::Ssh => CloneProtocol::Ssh,
        crate::config::Protocol::Https => CloneProtocol::Https,
    };

    let reports = comparisons
        .par_iter()
        .filter(|comparison| matches!(comparison.state, crate::compare::State::RemoteOnly))
        .map(|comparison| {
            let Some(id) = comparison.id.clone() else {
                return Report {
                    id: None,
                    path: None,
                    outcome: Outcome::Skipped {
                        reason: "it has no identity to clone from".to_owned(),
                    },
                };
            };

            let destination = clone_destination(root, &local.layout, &id);
            let url = id.clone_url(protocol);

            if destination.exists() {
                return Report {
                    id: Some(id),
                    path: Some(destination.clone()),
                    outcome: Outcome::Skipped {
                        reason: format!("{} already exists", destination.display()),
                    },
                };
            }

            let detail = format!("clone {url} into {}", destination.display());
            let outcome = mode.permitted(detail, || git::clone(&url, &destination, shallow));

            Report {
                id: Some(id),
                path: Some(destination),
                outcome,
            }
        })
        .collect();

    Summary { reports }
}

/// Fetches every local clone, updating remote-tracking refs only.
///
/// This never touches a working tree, so it is always safe to run and needs no
/// rehearsal to be trustworthy. A dry run is still offered, for symmetry with
/// the actions that do change something.
#[must_use]
pub fn fetch_all(comparisons: &[Comparison], mode: Mode) -> Summary {
    let reports = comparisons
        .par_iter()
        .filter_map(|comparison| {
            let path = comparison.path.clone()?;

            // A clone with no `origin` remote has nothing to fetch from, so
            // `git fetch` would only fail; report it as skipped instead, which
            // keeps one never-published clone from failing the whole batch.
            let outcome = if matches!(
                comparison.state,
                State::LocalOnly(LocalOnlyReason::NoRemote)
            ) {
                Outcome::Skipped {
                    reason: "it has no remote to fetch from".to_owned(),
                }
            } else {
                let detail = format!("fetch {}", path.display());
                mode.permitted(detail, || git::fetch(&path))
            };

            Some(Report {
                id: comparison.id.clone(),
                path: Some(path),
                outcome,
            })
        })
        .collect();

    Summary { reports }
}

/// Fast-forwards every clone that is strictly behind and clean.
///
/// Everything else is reported with the reason it was left alone, because a
/// repository that cannot be updated is the one worth knowing about.
#[must_use]
pub fn update_all(comparisons: &[Comparison], mode: Mode) -> Summary {
    let reports = comparisons
        .par_iter()
        .filter_map(|comparison| {
            let path = comparison.path.clone()?;

            let outcome = if comparison.can_fast_forward() {
                let detail = format!("fast-forward {}", path.display());
                mode.permitted(detail, || git::fast_forward(&path))
            } else {
                Outcome::Skipped {
                    reason: comparison
                        .blocked_reason()
                        .unwrap_or("it cannot be fast-forwarded")
                        .to_owned(),
                }
            };

            Some(Report {
                id: comparison.id.clone(),
                path: Some(path),
                outcome,
            })
        })
        .collect();

    Summary { reports }
}

/// Why a repository could not be moved.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MoveError {
    /// No repository matched what was asked for.
    #[error("no repository matches `{wanted}`; run `minato status` to see what there is")]
    NoMatch {
        /// What was asked for.
        wanted: String,
    },

    /// More than one repository matched, so the choice would be a guess.
    #[error("`{wanted}` matches {count} repositories; name one of them exactly: {matches}")]
    Ambiguous {
        /// What was asked for.
        wanted: String,
        /// How many matched.
        count: usize,
        /// The ones that matched.
        matches: String,
    },

    /// The repository has no local clone to move.
    #[error("`{wanted}` has no local clone to move; clone it first")]
    NotCloned {
        /// What was asked for.
        wanted: String,
    },

    /// The clone does not sit under a configured root.
    #[error(
        "`{}` is not under any configured root, so there is no group tree to move it within",
        path.display()
    )]
    OutsideRoots {
        /// Where the clone is.
        path: PathBuf,
    },

    /// Something already occupies the destination.
    #[error("`{}` already exists; nothing was moved", destination.display())]
    DestinationExists {
        /// Where it would have gone.
        destination: PathBuf,
    },

    /// It is already where it was asked to go.
    #[error("`{wanted}` is already in `{group}`")]
    AlreadyThere {
        /// What was asked for.
        wanted: String,
        /// The group it is already in.
        group: String,
    },

    /// The group is not a directory path beneath a root.
    #[error(
        "`{group}` is not a valid group; a group is a directory path beneath a root, written with `/`, so no part of it can be empty, `.` or `..`, and it cannot contain `\\`"
    )]
    InvalidGroup {
        /// The offending group.
        group: String,
    },

    /// The move itself failed.
    #[error("cannot move `{}` to `{}`: {message}", from.display(), to.display())]
    Failed {
        /// Where it was.
        from: PathBuf,
        /// Where it was going.
        to: PathBuf,
        /// What the operating system reported.
        message: String,
    },
}

/// The directory a group occupies beneath `root`.
///
/// A group is written with `/` whatever the platform separates paths with, so
/// its segments are joined one at a time rather than handed to `Path::join` as
/// a single string.
#[must_use]
pub fn group_path(root: &Path, group: &str) -> PathBuf {
    group
        .split('/')
        .fold(root.to_owned(), |directory, segment| {
            directory.join(segment)
        })
}

/// Where a repository would end up when moved into a group.
///
/// It keeps its directory name, which is not always the repository name: a
/// clone matched by its remote can sit in a directory called anything.
#[must_use]
pub fn move_destination(path: &Path, root: &Path, group: &str) -> PathBuf {
    let name = path
        .file_name()
        .map_or_else(|| path.to_owned(), PathBuf::from);

    group_path(root, group).join(name)
}

/// Whether a group names a directory path beneath a root.
fn is_valid_group(group: &str) -> bool {
    !group.is_empty()
        && !group.contains('\\')
        && group
            .split('/')
            .all(|segment| !segment.is_empty() && segment != "." && segment != "..")
}

/// Finds the one repository the user meant.
///
/// Matching is deliberately strict about ambiguity: moving the wrong
/// repository is a filesystem change, so a guess is worse than a question.
///
/// # Errors
///
/// Returns an error when nothing matches, or when more than one does.
pub fn find_one<'a>(
    comparisons: &'a [Comparison],
    wanted: &str,
) -> Result<&'a Comparison, MoveError> {
    let lowered = wanted.to_lowercase();

    let matches: Vec<&Comparison> = comparisons
        .iter()
        .filter(|comparison| {
            comparison.id.as_ref().is_some_and(|id| {
                id.to_string() == lowered
                    || id.name == lowered
                    || format!("{}/{}", id.owner, id.name) == lowered
            })
        })
        .collect();

    match matches.len() {
        0 => Err(MoveError::NoMatch {
            wanted: wanted.to_owned(),
        }),
        1 => Ok(matches[0]),
        count => Err(MoveError::Ambiguous {
            wanted: wanted.to_owned(),
            count,
            matches: matches
                .iter()
                .filter_map(|comparison| comparison.id.as_ref())
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", "),
        }),
    }
}

/// Moves one repository into a group, which means moving its directory.
///
/// A clone does not care where it sits, so this is a plain rename. It is still
/// a change to the user's filesystem, so it is explicit, one repository at a
/// time, and refuses anything it would have to overwrite.
///
/// # Errors
///
/// Returns an error when the repository cannot be identified, has no clone,
/// sits outside every root, is already there, or when the move itself fails.
pub fn move_to_group(
    comparison: &Comparison,
    wanted: &str,
    roots: &[PathBuf],
    group: &str,
    mode: Mode,
) -> Result<Report, MoveError> {
    // A group names a directory path beneath a root, written with `/`. An
    // empty segment, `.` or `..` would move the clone somewhere else in the
    // tree or out of the root entirely, and a backslash is a legal character
    // in a directory name on Unix, so neither is guessed at: both are refused
    // before any path is built.
    if !is_valid_group(group) {
        return Err(MoveError::InvalidGroup {
            group: group.to_owned(),
        });
    }

    let Some(path) = comparison.path.clone() else {
        return Err(MoveError::NotCloned {
            wanted: wanted.to_owned(),
        });
    };

    if comparison.group.as_deref() == Some(group) {
        return Err(MoveError::AlreadyThere {
            wanted: wanted.to_owned(),
            group: group.to_owned(),
        });
    }

    let root = roots
        .iter()
        .find(|root| path.starts_with(root))
        .ok_or_else(|| MoveError::OutsideRoots { path: path.clone() })?;

    let destination = move_destination(&path, root, group);

    if destination.exists() {
        return Err(MoveError::DestinationExists { destination });
    }

    let detail = format!("move {} to {}", path.display(), destination.display());

    let outcome = match mode {
        Mode::DryRun => Outcome::Would { detail },
        Mode::Execute => {
            if let Some(parent) = destination.parent()
                && let Err(error) = std::fs::create_dir_all(parent)
            {
                return Err(MoveError::Failed {
                    from: path,
                    to: destination,
                    message: error.to_string(),
                });
            }

            match std::fs::rename(&path, &destination) {
                Ok(()) => Outcome::Done { detail },
                Err(error) => {
                    return Err(MoveError::Failed {
                        from: path,
                        to: destination,
                        message: error.to_string(),
                    });
                }
            }
        }
    };

    Ok(Report {
        id: comparison.id.clone(),
        path: Some(destination),
        outcome,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compare::{LocalFlags, LocalOnlyReason, State};
    use crate::config::Protocol;
    use crate::model::Provider;

    fn comparison(state: State, path: Option<&str>) -> Comparison {
        Comparison {
            id: Some(RepoId::new(Provider::GitHub, "mcanouil", "minato")),
            path: path.map(PathBuf::from),
            group: None,
            state,
            upstream: None,
            local: path.map(|_| LocalFlags::default()),
            remote: None,
        }
    }

    fn local_config() -> Local {
        Local {
            roots: vec![PathBuf::from("/code")],
            layout: "{owner}/{repo}".to_owned(),
            protocol: Protocol::Ssh,
        }
    }

    #[test]
    fn a_layout_places_a_repository_under_the_root() {
        let id = RepoId::new(Provider::GitHub, "mcanouil", "minato");

        assert_eq!(
            clone_destination(Path::new("/code"), "{owner}/{repo}", &id),
            PathBuf::from("/code/mcanouil/minato")
        );
        assert_eq!(
            clone_destination(Path::new("/code"), "{provider}/{owner}/{repo}", &id),
            PathBuf::from("/code/github/mcanouil/minato")
        );
    }

    #[test]
    fn fetch_skips_a_clone_with_no_remote_rather_than_attempting_it() {
        let summary = fetch_all(
            &[comparison(
                State::LocalOnly(LocalOnlyReason::NoRemote),
                Some("/code/orphan"),
            )],
            Mode::DryRun,
        );

        assert!(
            matches!(summary.reports[0].outcome, Outcome::Skipped { .. }),
            "a clone with no remote has nothing to fetch from, got: {:?}",
            summary.reports[0].outcome
        );
    }

    #[test]
    fn a_rehearsal_reports_without_doing_anything() {
        let summary = clone_missing(
            &[comparison(State::RemoteOnly, None)],
            Path::new("/nonexistent-root"),
            &local_config(),
            false,
            Mode::DryRun,
        );

        assert_eq!(summary.counts().would, 1);
        assert_eq!(summary.counts().done, 0);
        assert!(
            !Path::new("/nonexistent-root").exists(),
            "a rehearsal must not create anything"
        );
    }

    #[test]
    fn only_repositories_without_a_clone_are_cloned() {
        let summary = clone_missing(
            &[
                comparison(State::InSync, Some("/code/mcanouil/minato")),
                comparison(State::Behind { behind: 1 }, Some("/code/mcanouil/other")),
            ],
            Path::new("/code"),
            &local_config(),
            false,
            Mode::DryRun,
        );

        assert!(
            summary.reports.is_empty(),
            "a repository that already has a clone is not a candidate"
        );
    }

    #[test]
    fn nothing_is_updated_unless_it_is_behind_and_clean() {
        let cases = [
            (State::InSync, "it is already up to date"),
            (
                State::Ahead { ahead: 1 },
                "it has local commits the remote does not",
            ),
            (
                State::Diverged {
                    ahead: 1,
                    behind: 1,
                },
                "its history has diverged from the remote",
            ),
        ];

        for (state, expected) in cases {
            let summary = update_all(
                &[comparison(state, Some("/code/mcanouil/minato"))],
                Mode::Execute,
            );

            assert_eq!(
                summary.reports[0].outcome,
                Outcome::Skipped {
                    reason: expected.to_owned()
                },
                "a repository that is not behind must be left alone, with a reason"
            );
        }
    }

    #[test]
    fn a_dirty_repository_that_is_behind_is_left_alone_with_its_reason() {
        let mut behind = comparison(State::Behind { behind: 2 }, Some("/code/mcanouil/minato"));
        behind.local = Some(LocalFlags {
            dirty: true,
            ..LocalFlags::default()
        });

        let summary = update_all(&[behind], Mode::Execute);

        assert_eq!(
            summary.reports[0].outcome,
            Outcome::Skipped {
                reason: "the working tree has uncommitted changes".to_owned()
            }
        );
    }

    #[test]
    fn a_repository_that_was_never_cloned_is_not_a_candidate_for_updating() {
        let summary = update_all(&[comparison(State::RemoteOnly, None)], Mode::Execute);

        assert!(summary.reports.is_empty());
    }

    #[test]
    fn skipping_is_not_failing() {
        let summary = update_all(
            &[comparison(State::InSync, Some("/code/mcanouil/minato"))],
            Mode::Execute,
        );

        assert!(
            !summary.has_failures(),
            "leaving a repository alone deliberately is not a failure"
        );
        assert_eq!(summary.counts().skipped, 1);
    }

    #[test]
    fn a_failure_is_counted_and_makes_the_run_a_failure() {
        let summary = Summary {
            reports: vec![
                Report {
                    id: None,
                    path: None,
                    outcome: Outcome::Done {
                        detail: "did a thing".to_owned(),
                    },
                },
                Report {
                    id: None,
                    path: None,
                    outcome: Outcome::Failed {
                        error: "it went wrong".to_owned(),
                    },
                },
            ],
        };

        assert!(summary.has_failures());
        assert_eq!(summary.counts().done, 1);
        assert_eq!(summary.counts().failed, 1);
    }
}

#[cfg(test)]
mod moving {
    use super::*;
    use crate::compare::State;
    use crate::model::Provider;

    fn cloned(name: &str, group: Option<&str>, path: &str) -> Comparison {
        Comparison {
            id: Some(RepoId::new(Provider::GitHub, "mcanouil", name)),
            path: Some(PathBuf::from(path)),
            group: group.map(ToOwned::to_owned),
            state: State::InSync,
            upstream: None,
            local: None,
            remote: None,
        }
    }

    fn roots() -> Vec<PathBuf> {
        vec![PathBuf::from("/code")]
    }

    #[test]
    fn a_repository_keeps_its_directory_name_when_it_moves() {
        // The directory is not always named after the repository, since a clone
        // is matched by its remote rather than by where it sits.
        assert_eq!(
            move_destination(
                Path::new("/code/old/quarto-vscode"),
                Path::new("/code"),
                "new"
            ),
            PathBuf::from("/code/new/quarto-vscode")
        );
    }

    #[test]
    fn a_repository_can_be_named_by_id_owner_and_name_or_bare_name() {
        let all = [cloned("minato", Some("perso"), "/code/perso/minato")];

        for wanted in ["github:mcanouil/minato", "mcanouil/minato", "minato"] {
            assert!(
                find_one(&all, wanted).is_ok(),
                "`{wanted}` should identify the repository"
            );
        }
    }

    #[test]
    fn refuses_a_group_name_that_is_not_a_directory_path_beneath_a_root() {
        let repository = cloned("minato", Some("perso"), "/code/perso/minato");

        for bad in [
            "../evil",
            "perso/../evil",
            "perso//apps",
            "/perso",
            "perso/",
            "perso\\apps",
            "..",
            ".",
            "",
        ] {
            assert!(
                matches!(
                    move_to_group(&repository, "minato", &roots(), bad, Mode::DryRun),
                    Err(MoveError::InvalidGroup { .. })
                ),
                "group `{bad}` should be rejected as invalid"
            );
        }
    }

    #[test]
    fn a_nested_group_names_the_directories_it_is_written_with() {
        let repository = cloned("minato", Some("perso"), "/code/perso/minato");

        let report = move_to_group(&repository, "minato", &roots(), "perso/apps", Mode::DryRun)
            .expect("a plan");

        assert_eq!(report.path, Some(PathBuf::from("/code/perso/apps/minato")));
        assert_eq!(
            move_destination(
                Path::new("/code/perso/minato"),
                Path::new("/code"),
                "perso/apps"
            ),
            PathBuf::from("/code/perso/apps/minato")
        );
    }

    #[test]
    fn moving_out_of_a_nested_group_into_its_parent_is_an_ordinary_move() {
        let repository = cloned("minato", Some("perso/apps"), "/code/perso/apps/minato");

        let report =
            move_to_group(&repository, "minato", &roots(), "perso", Mode::DryRun).expect("a plan");

        assert_eq!(report.path, Some(PathBuf::from("/code/perso/minato")));
    }

    #[test]
    fn a_repository_already_in_a_nested_group_is_not_moved_onto_itself() {
        let repository = cloned("minato", Some("perso/apps"), "/code/perso/apps/minato");

        assert!(matches!(
            move_to_group(&repository, "minato", &roots(), "perso/apps", Mode::DryRun),
            Err(MoveError::AlreadyThere { .. })
        ));
    }

    #[test]
    fn naming_something_that_matches_nothing_says_so() {
        let all = [cloned("minato", Some("perso"), "/code/perso/minato")];

        assert!(matches!(
            find_one(&all, "nonexistent"),
            Err(MoveError::NoMatch { .. })
        ));
    }

    #[test]
    fn an_ambiguous_name_is_refused_rather_than_guessed() {
        let all = [
            cloned("quarto", Some("a"), "/code/a/quarto"),
            Comparison {
                id: Some(RepoId::new(Provider::GitHub, "other-owner", "quarto")),
                path: Some(PathBuf::from("/code/b/quarto")),
                group: Some("b".to_owned()),
                state: State::InSync,
                upstream: None,
                local: None,
                remote: None,
            },
        ];

        let error = find_one(&all, "quarto").expect_err("ambiguity");

        assert!(matches!(error, MoveError::Ambiguous { count: 2, .. }));
        assert!(
            error.to_string().contains("other-owner"),
            "the error should list what matched, got: {error}"
        );
    }

    #[test]
    fn a_rehearsal_moves_nothing() {
        let repository = cloned("minato", Some("old"), "/code/old/minato");

        let report =
            move_to_group(&repository, "minato", &roots(), "new", Mode::DryRun).expect("a plan");

        assert!(matches!(report.outcome, Outcome::Would { .. }));
        assert_eq!(report.path, Some(PathBuf::from("/code/new/minato")));
    }

    #[test]
    fn a_repository_already_in_the_group_is_not_moved_onto_itself() {
        let repository = cloned("minato", Some("perso"), "/code/perso/minato");

        assert!(matches!(
            move_to_group(&repository, "minato", &roots(), "perso", Mode::DryRun),
            Err(MoveError::AlreadyThere { .. })
        ));
    }

    #[test]
    fn a_repository_with_no_clone_cannot_be_moved() {
        let mut repository = cloned("minato", None, "/code/perso/minato");
        repository.path = None;

        assert!(matches!(
            move_to_group(&repository, "minato", &roots(), "new", Mode::DryRun),
            Err(MoveError::NotCloned { .. })
        ));
    }

    #[test]
    fn a_clone_outside_every_root_has_no_group_tree_to_move_within() {
        let repository = cloned("minato", None, "/elsewhere/minato");

        assert!(matches!(
            move_to_group(&repository, "minato", &roots(), "new", Mode::DryRun),
            Err(MoveError::OutsideRoots { .. })
        ));
    }

    #[test]
    fn an_occupied_destination_is_refused_and_nothing_is_touched() {
        let root = tempfile::tempdir().expect("a temporary directory");
        let source = root.path().join("old/minato");
        let occupied = root.path().join("new/minato");

        std::fs::create_dir_all(&source).expect("the source");
        std::fs::create_dir_all(&occupied).expect("the destination");
        std::fs::write(occupied.join("precious.txt"), "keep me").expect("a file");

        let repository = Comparison {
            id: Some(RepoId::new(Provider::GitHub, "mcanouil", "minato")),
            path: Some(source.clone()),
            group: Some("old".to_owned()),
            state: State::InSync,
            upstream: None,
            local: None,
            remote: None,
        };

        let error = move_to_group(
            &repository,
            "minato",
            &[root.path().to_owned()],
            "new",
            Mode::Execute,
        )
        .expect_err("a refusal");

        assert!(matches!(error, MoveError::DestinationExists { .. }));
        assert!(source.exists(), "the source must be left where it was");
        assert_eq!(
            std::fs::read_to_string(occupied.join("precious.txt")).expect("the file"),
            "keep me",
            "whatever was already there must be untouched"
        );
    }

    #[test]
    fn moving_relocates_the_directory_and_creates_the_group() {
        let root = tempfile::tempdir().expect("a temporary directory");
        let source = root.path().join("old/minato");
        std::fs::create_dir_all(source.join(".git")).expect("the source");
        std::fs::write(source.join("file.txt"), "contents").expect("a file");

        let repository = Comparison {
            id: Some(RepoId::new(Provider::GitHub, "mcanouil", "minato")),
            path: Some(source.clone()),
            group: Some("old".to_owned()),
            state: State::InSync,
            upstream: None,
            local: None,
            remote: None,
        };

        let report = move_to_group(
            &repository,
            "minato",
            &[root.path().to_owned()],
            "new",
            Mode::Execute,
        )
        .expect("the move to succeed");

        let destination = root.path().join("new/minato");

        assert!(matches!(report.outcome, Outcome::Done { .. }));
        assert!(!source.exists(), "the old location should be gone");
        assert!(
            destination.join(".git").exists(),
            "it should still be a clone"
        );
        assert_eq!(
            std::fs::read_to_string(destination.join("file.txt")).expect("the file"),
            "contents",
            "its contents must survive the move"
        );
    }
}
