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

use crate::compare::Comparison;
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
#[derive(Debug, Default, Clone, Serialize)]
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
            let detail = format!("fetch {}", path.display());
            let outcome = mode.permitted(detail, || git::fetch(&path));

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
        .filter(|comparison| comparison.path.is_some())
        .map(|comparison| {
            let path = comparison.path.clone().unwrap_or_default();

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

            Report {
                id: comparison.id.clone(),
                path: Some(path),
                outcome,
            }
        })
        .collect();

    Summary { reports }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compare::{LocalFlags, State};
    use crate::config::Protocol;
    use crate::model::Provider;

    fn comparison(state: State, path: Option<&str>) -> Comparison {
        Comparison {
            id: Some(RepoId::new(Provider::GitHub, "mcanouil", "minato")),
            path: path.map(PathBuf::from),
            state,
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
