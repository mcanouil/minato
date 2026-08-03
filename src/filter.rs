//! Narrowing a comparison down to the repositories worth looking at.
//!
//! Filters combine: naming an owner and a state means both must hold. A filter
//! nobody set matches everything, so an unfiltered command is the same code
//! path as a filtered one.

use crate::compare::{Comparison, LocalOnlyReason, State};

/// Which repositories to keep.
#[derive(Debug, Default, Clone)]
pub struct Filter {
    /// Keep only these owners, when any are named.
    pub owners: Vec<String>,

    /// Keep only these groups, when any are named.
    pub groups: Vec<String>,

    /// Keep only these states, when any are named.
    pub states: Vec<StateFilter>,

    /// Keep forks, which are hidden by default.
    pub include_forks: bool,

    /// Keep clones of repositories owned by nobody the user tracks, which are
    /// hidden by default.
    pub include_external: bool,
}

/// Whether `wanted` names `group` or one of the groups beneath it.
///
/// A group is a directory path, so naming `perso` means everything filed under
/// it, `perso/apps` included. Matching runs segment by segment rather than over
/// the whole string, so `pers` does not stand for `perso` and `apps` does not
/// stand for `perso/apps`: a group is named from the root down.
fn covers(wanted: &str, group: &str) -> bool {
    let mut segments = group.split('/');

    wanted.trim_end_matches('/').split('/').all(|named| {
        segments
            .next()
            .is_some_and(|segment| segment.eq_ignore_ascii_case(named))
    })
}

/// A state named on the command line.
///
/// This is coarser than [`State`], because someone asking for what is behind
/// does not want to name a number of commits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum StateFilter {
    /// Reported by the provider, with no local clone.
    NotCloned,
    /// Matching the remote exactly.
    InSync,
    /// Holding commits the remote does not.
    Ahead,
    /// Missing commits the remote has.
    Behind,
    /// Both sides hold commits the other lacks.
    Diverged,
    /// A clone no reported repository corresponds to.
    LocalOnly,
    /// A clone that cannot be compared.
    Incomparable,
    /// Anything that is not in sync, which is what usually wants attention.
    Drifted,
}

impl StateFilter {
    /// Whether this filter accepts a state.
    fn accepts(self, state: &State) -> bool {
        match self {
            Self::NotCloned => matches!(state, State::RemoteOnly),
            Self::InSync => matches!(state, State::InSync),
            Self::Ahead => matches!(state, State::Ahead { .. }),
            Self::Behind => matches!(state, State::Behind { .. }),
            Self::Diverged => matches!(state, State::Diverged { .. }),
            Self::LocalOnly => matches!(state, State::LocalOnly(_)),
            Self::Incomparable => matches!(state, State::Incomparable(_)),
            Self::Drifted => !matches!(state, State::InSync),
        }
    }
}

impl Filter {
    /// Whether a comparison survives every condition.
    ///
    /// Beyond the named owner, group, and state conditions, forks and clones of
    /// untracked owners are dropped unless explicitly included, so the default
    /// view is the repositories the user maintains.
    #[must_use]
    pub fn accepts(&self, comparison: &Comparison) -> bool {
        self.accepts_owner(comparison)
            && self.accepts_group(comparison)
            && self.accepts_state(comparison)
            && self.accepts_fork(comparison)
            && self.accepts_external(comparison)
    }

    /// Keeps only the comparisons that survive.
    ///
    /// This always filters, because forks and external clones are dropped even
    /// when no owner, group, or state was named.
    #[must_use]
    pub fn apply(&self, comparisons: Vec<Comparison>) -> Vec<Comparison> {
        comparisons
            .into_iter()
            .filter(|comparison| self.accepts(comparison))
            .collect()
    }

    /// Whether an owner is one of those named, or none were named.
    ///
    /// This is the owner condition on its own, so a command working from remote
    /// repositories rather than comparisons can honour `--owner` too.
    #[must_use]
    pub fn owner_matches(&self, owner: &str) -> bool {
        self.owners.is_empty()
            || self
                .owners
                .iter()
                .any(|named| named.eq_ignore_ascii_case(owner))
    }

    fn accepts_owner(&self, comparison: &Comparison) -> bool {
        if self.owners.is_empty() {
            return true;
        }

        comparison
            .id
            .as_ref()
            .is_some_and(|id| self.owner_matches(&id.owner))
    }

    fn accepts_group(&self, comparison: &Comparison) -> bool {
        if self.groups.is_empty() {
            return true;
        }

        comparison
            .group
            .as_ref()
            .is_some_and(|group| self.groups.iter().any(|wanted| covers(wanted, group)))
    }

    fn accepts_state(&self, comparison: &Comparison) -> bool {
        self.states.is_empty()
            || self
                .states
                .iter()
                .any(|state| state.accepts(&comparison.state))
    }

    fn accepts_fork(&self, comparison: &Comparison) -> bool {
        self.include_forks || !comparison.remote.is_some_and(|remote| remote.fork)
    }

    fn accepts_external(&self, comparison: &Comparison) -> bool {
        self.include_external
            || !matches!(
                comparison.state,
                State::LocalOnly(LocalOnlyReason::OwnerNotTracked)
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compare::RemoteFlags;
    use crate::model::{Provider, RepoId};
    use std::path::PathBuf;

    fn comparison(owner: &str, group: Option<&str>, state: State) -> Comparison {
        Comparison {
            id: Some(RepoId::new(Provider::GitHub, owner, "repo")),
            path: Some(PathBuf::from("/code/repo")),
            group: group.map(ToOwned::to_owned),
            state,
            upstream: None,
            local: None,
            remote: None,
        }
    }

    fn fork(owner: &str, state: State) -> Comparison {
        Comparison {
            remote: Some(RemoteFlags {
                fork: true,
                ..RemoteFlags::default()
            }),
            ..comparison(owner, None, state)
        }
    }

    #[test]
    fn a_filter_nobody_set_keeps_everything() {
        let all = vec![
            comparison("mcanouil", Some("perso"), State::InSync),
            comparison("someone", None, State::RemoteOnly),
        ];

        assert_eq!(Filter::default().apply(all.clone()).len(), all.len());
    }

    #[test]
    fn owner_matches_is_case_insensitive_and_open_when_none_are_named() {
        assert!(
            Filter::default().owner_matches("anyone"),
            "naming no owner keeps every owner"
        );

        let named = Filter {
            owners: vec!["McAnouil".to_owned()],
            ..Filter::default()
        };
        assert!(named.owner_matches("mcanouil"));
        assert!(!named.owner_matches("someone-else"));
    }

    #[test]
    fn forks_are_hidden_by_default_and_shown_when_included() {
        let it = vec![fork("mcanouil", State::InSync)];

        assert!(Filter::default().apply(it.clone()).is_empty());

        let including = Filter {
            include_forks: true,
            ..Filter::default()
        };
        assert_eq!(including.apply(it).len(), 1);
    }

    #[test]
    fn a_fork_that_is_also_behind_is_still_hidden() {
        let it = vec![fork("mcanouil", State::Behind { behind: 3 })];

        assert!(Filter::default().apply(it).is_empty());
    }

    #[test]
    fn external_clones_are_hidden_by_default_and_shown_when_included() {
        let it = vec![comparison(
            "someone",
            None,
            State::LocalOnly(LocalOnlyReason::OwnerNotTracked),
        )];

        assert!(Filter::default().apply(it.clone()).is_empty());

        let including = Filter {
            include_external: true,
            ..Filter::default()
        };
        assert_eq!(including.apply(it).len(), 1);
    }

    #[test]
    fn local_only_clones_that_are_not_external_stay_visible() {
        let kept = Filter::default().apply(vec![
            comparison("x", None, State::LocalOnly(LocalOnlyReason::NoRemote)),
            comparison(
                "y",
                None,
                State::LocalOnly(LocalOnlyReason::UnsupportedHost),
            ),
        ]);

        assert_eq!(kept.len(), 2);
    }

    #[test]
    fn naming_a_group_keeps_only_that_group() {
        let filter = Filter {
            groups: vec!["demo".to_owned()],
            ..Filter::default()
        };

        let kept = filter.apply(vec![
            comparison("mcanouil", Some("demo"), State::InSync),
            comparison("mcanouil", Some("perso"), State::InSync),
            comparison("mcanouil", None, State::RemoteOnly),
        ]);

        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].group.as_deref(), Some("demo"));
    }

    #[test]
    fn a_group_is_matched_whatever_case_it_is_typed_in() {
        let filter = Filter {
            groups: vec!["DEMO".to_owned()],
            ..Filter::default()
        };

        assert_eq!(
            filter
                .apply(vec![comparison("mcanouil", Some("demo"), State::InSync)])
                .len(),
            1
        );
    }

    #[test]
    fn naming_a_group_keeps_everything_beneath_it_too() {
        let filter = Filter {
            groups: vec!["perso".to_owned()],
            ..Filter::default()
        };

        let kept = filter.apply(vec![
            comparison("mcanouil", Some("perso"), State::InSync),
            comparison("mcanouil", Some("perso/apps"), State::InSync),
            comparison("mcanouil", Some("perso/apps/rust"), State::InSync),
            comparison("mcanouil", Some("pro"), State::InSync),
        ]);

        assert_eq!(kept.len(), 3);
    }

    #[test]
    fn naming_a_nested_group_keeps_only_that_subtree() {
        let filter = Filter {
            groups: vec!["PERSO/Apps".to_owned()],
            ..Filter::default()
        };

        let kept = filter.apply(vec![
            comparison("mcanouil", Some("perso/apps"), State::InSync),
            comparison("mcanouil", Some("perso/apps/rust"), State::InSync),
            comparison("mcanouil", Some("perso"), State::InSync),
            comparison("mcanouil", Some("perso/data"), State::InSync),
        ]);

        assert_eq!(kept.len(), 2, "a nested group matches at every segment");
    }

    #[test]
    fn a_group_is_matched_by_whole_segments_from_the_root_down() {
        for wanted in ["apps", "pers", "perso/app", "so/apps"] {
            let filter = Filter {
                groups: vec![wanted.to_owned()],
                ..Filter::default()
            };

            let kept = filter.apply(vec![comparison(
                "mcanouil",
                Some("perso/apps"),
                State::InSync,
            )]);

            assert!(
                kept.is_empty(),
                "`{wanted}` is not a whole-segment prefix of `perso/apps`, so it should match nothing"
            );
        }
    }

    #[test]
    fn conditions_combine_rather_than_accumulate() {
        let filter = Filter {
            owners: vec!["mcanouil".to_owned()],
            groups: vec!["demo".to_owned()],
            ..Filter::default()
        };

        let kept = filter.apply(vec![
            comparison("mcanouil", Some("demo"), State::InSync),
            comparison("mcanouil", Some("perso"), State::InSync),
            comparison("someone", Some("demo"), State::InSync),
        ]);

        assert_eq!(
            kept.len(),
            1,
            "naming both an owner and a group should require both"
        );
    }

    #[test]
    fn drifted_means_anything_needing_attention() {
        let filter = Filter {
            states: vec![StateFilter::Drifted],
            ..Filter::default()
        };

        let kept = filter.apply(vec![
            comparison("mcanouil", None, State::InSync),
            comparison("mcanouil", None, State::Behind { behind: 1 }),
            comparison("mcanouil", None, State::RemoteOnly),
            comparison(
                "mcanouil",
                None,
                State::Diverged {
                    ahead: 1,
                    behind: 1,
                },
            ),
        ]);

        assert_eq!(kept.len(), 3, "everything except what is in sync");
    }

    #[test]
    fn several_states_may_be_named_at_once() {
        let filter = Filter {
            states: vec![StateFilter::Behind, StateFilter::Diverged],
            ..Filter::default()
        };

        let kept = filter.apply(vec![
            comparison("mcanouil", None, State::InSync),
            comparison("mcanouil", None, State::Behind { behind: 1 }),
            comparison(
                "mcanouil",
                None,
                State::Diverged {
                    ahead: 1,
                    behind: 1,
                },
            ),
        ]);

        assert_eq!(kept.len(), 2);
    }

    #[test]
    fn a_repository_with_no_group_is_excluded_when_a_group_is_named() {
        let filter = Filter {
            groups: vec!["demo".to_owned()],
            ..Filter::default()
        };

        assert!(
            filter
                .apply(vec![comparison("mcanouil", None, State::RemoteOnly)])
                .is_empty(),
            "an uncloned repository is in no group, so it cannot match one"
        );
    }
}
