//! Narrowing a comparison down to the repositories worth looking at.
//!
//! Filters combine: naming an owner and a state means both must hold. A filter
//! nobody set matches everything, so an unfiltered command is the same code
//! path as a filtered one.

use crate::compare::{Comparison, State};

/// Which repositories to keep.
#[derive(Debug, Default, Clone)]
pub struct Filter {
    /// Keep only these owners, when any are named.
    pub owners: Vec<String>,

    /// Keep only these groups, when any are named.
    pub groups: Vec<String>,

    /// Keep only these states, when any are named.
    pub states: Vec<StateFilter>,
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
    /// Whether anything was asked for.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.owners.is_empty() && self.groups.is_empty() && self.states.is_empty()
    }

    /// Whether a comparison survives every named condition.
    #[must_use]
    pub fn accepts(&self, comparison: &Comparison) -> bool {
        self.accepts_owner(comparison)
            && self.accepts_group(comparison)
            && self.accepts_state(comparison)
    }

    /// Keeps only the comparisons that survive.
    #[must_use]
    pub fn apply(&self, comparisons: Vec<Comparison>) -> Vec<Comparison> {
        if self.is_empty() {
            return comparisons;
        }

        comparisons
            .into_iter()
            .filter(|comparison| self.accepts(comparison))
            .collect()
    }

    fn accepts_owner(&self, comparison: &Comparison) -> bool {
        if self.owners.is_empty() {
            return true;
        }

        comparison.id.as_ref().is_some_and(|id| {
            self.owners
                .iter()
                .any(|owner| owner.eq_ignore_ascii_case(&id.owner))
        })
    }

    fn accepts_group(&self, comparison: &Comparison) -> bool {
        if self.groups.is_empty() {
            return true;
        }

        comparison.group.as_ref().is_some_and(|group| {
            self.groups
                .iter()
                .any(|wanted| wanted.eq_ignore_ascii_case(group))
        })
    }

    fn accepts_state(&self, comparison: &Comparison) -> bool {
        self.states.is_empty()
            || self
                .states
                .iter()
                .any(|state| state.accepts(&comparison.state))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    #[test]
    fn a_filter_nobody_set_keeps_everything() {
        let all = vec![
            comparison("mcanouil", Some("perso"), State::InSync),
            comparison("someone", None, State::RemoteOnly),
        ];

        assert_eq!(Filter::default().apply(all.clone()).len(), all.len());
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
