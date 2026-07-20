//! Deciding how a local clone stands against what a provider reports.
//!
//! This is a pure function of what the client and the scanner already gathered.
//! It performs no I/O, which is what makes every state reachable in a test.
//!
//! Safety rules live here rather than in the actions. An action asks whether an
//! operation is permitted; it does not decide it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::model::{Provider, RemoteRepo, RepoId};
use crate::scan::{Head, LocalRepo};

/// How a repository stands, as exactly one answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum State {
    /// The provider knows it and nothing on disk corresponds to it.
    ///
    /// Under the view of local clones as a backup of remote state, this is a
    /// repository with no local copy at all.
    RemoteOnly,

    /// A clone exists on disk that no reported repository corresponds to.
    LocalOnly(LocalOnlyReason),

    /// The branch and its remote-tracking ref are identical.
    InSync,

    /// The branch holds commits the remote-tracking ref does not.
    Ahead {
        /// How many.
        ahead: u32,
    },

    /// The remote-tracking ref holds commits the branch does not.
    Behind {
        /// How many.
        behind: u32,
    },

    /// Each side holds commits the other lacks.
    Diverged {
        /// Commits only the branch has.
        ahead: u32,
        /// Commits only the remote-tracking ref has.
        behind: u32,
    },

    /// A clone matches a reported repository, but the two cannot be compared.
    Incomparable(IncomparableReason),
}

/// Why a clone corresponds to no reported repository.
///
/// The remedies differ, so the cause is kept rather than flattened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LocalOnlyReason {
    /// The clone has no `origin` remote, so it was never published.
    NoRemote,

    /// The remote points somewhere `minato` does not recognise.
    UnsupportedHost,

    /// The remote is understood, but its owner is not configured, so the
    /// provider was never asked about it.
    OwnerNotTracked,

    /// The owner is configured, yet the provider did not report the
    /// repository.
    ///
    /// It was deleted, renamed, made private, or is otherwise invisible to the
    /// token in use. These are not distinguished, because nothing gathered so
    /// far can tell them apart.
    MissingRemotely,
}

/// Why a matched clone cannot be compared against its remote.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IncomparableReason {
    /// The clone has no commits yet.
    NoCommits,

    /// A commit is checked out directly, so no branch is being tracked.
    DetachedHead,

    /// The checked-out branch follows nothing, so there is no ref to compare
    /// against.
    NoUpstreamBranch,
}

/// Facts about the clone on disk, holding independently of the primary state.
///
/// A repository can be behind *and* dirty, and both matter, so these are not
/// folded into [`State`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalFlags {
    /// Tracked files hold uncommitted changes.
    pub dirty: bool,

    /// The working tree holds files git is not tracking.
    pub untracked: bool,

    /// A commit is checked out rather than a branch.
    pub detached_head: bool,
}

/// Facts the provider reports about the repository.
///
/// These are absent rather than false when there is no reported repository, so
/// that "not a fork" stays distinguishable from "nothing to ask".
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteFlags {
    /// The repository is archived, and so never expected to change.
    pub archived: bool,

    /// The repository is private.
    pub private: bool,

    /// The repository is a fork.
    pub fork: bool,
}

/// One repository, as both sides see it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comparison {
    /// Which repository this is, absent when a clone's remote cannot be read.
    pub id: Option<RepoId>,

    /// Where the clone is, absent when there is none.
    pub path: Option<PathBuf>,

    /// The single answer for this repository.
    pub state: State,

    /// Facts about the clone, absent when there is none.
    pub local: Option<LocalFlags>,

    /// Facts the provider reported, absent when it reported nothing.
    pub remote: Option<RemoteFlags>,
}

impl Comparison {
    /// Whether this repository can be fast-forwarded without a decision from
    /// the user.
    ///
    /// Only a repository that is strictly behind qualifies, and only when no
    /// tracked file has been modified. Untracked files do not count: a
    /// fast-forward succeeds alongside them, and git refuses by itself in the
    /// one case where it would overwrite one.
    #[must_use]
    pub const fn can_fast_forward(&self) -> bool {
        let Some(local) = self.local else {
            return false;
        };

        matches!(self.state, State::Behind { .. }) && !local.dirty
    }

    /// Why this repository cannot be fast-forwarded, for reporting.
    ///
    /// Returns `None` when it can. A repository that cannot be updated is
    /// exactly the one worth explaining, so nothing is skipped silently.
    #[must_use]
    pub fn blocked_reason(&self) -> Option<&'static str> {
        if self.can_fast_forward() {
            return None;
        }

        Some(match self.state {
            State::Behind { .. } => "the working tree has uncommitted changes",
            State::InSync => "it is already up to date",
            State::Ahead { .. } => "it has local commits the remote does not",
            State::Diverged { .. } => "its history has diverged from the remote",
            State::RemoteOnly => "it has not been cloned",
            State::LocalOnly(_) => "it has no remote to update from",
            State::Incomparable(IncomparableReason::NoCommits) => "it has no commits",
            State::Incomparable(IncomparableReason::DetachedHead) => "no branch is checked out",
            State::Incomparable(IncomparableReason::NoUpstreamBranch) => {
                "the checked-out branch tracks nothing"
            }
        })
    }
}

/// Everything needed to judge a clone whose remote could not be matched.
///
/// Owners come from configuration, and are needed to tell "the provider was
/// never asked about this" from "the provider was asked and did not report it".
#[derive(Debug, Default)]
pub struct TrackedOwners(BTreeSet<(Provider, String)>);

impl TrackedOwners {
    /// Builds the set from configured account names.
    pub fn new(owners: impl IntoIterator<Item = (Provider, String)>) -> Self {
        Self(
            owners
                .into_iter()
                .map(|(provider, owner)| (provider, owner.to_lowercase()))
                .collect(),
        )
    }

    /// Whether this owner was asked about.
    fn contains(&self, id: &RepoId) -> bool {
        self.0.contains(&(id.provider, id.owner.clone()))
    }
}

/// Compares what a provider reported against what is on disk.
///
/// Every reported repository and every clone appears exactly once in the
/// result, so nothing is silently dropped. Results are sorted, so output does
/// not depend on the order either side arrived in.
#[must_use]
pub fn compare(
    remotes: &[RemoteRepo],
    locals: &[LocalRepo],
    tracked: &TrackedOwners,
) -> Vec<Comparison> {
    let by_id: BTreeMap<&RepoId, &RemoteRepo> =
        remotes.iter().map(|remote| (&remote.id, remote)).collect();

    let mut comparisons = Vec::with_capacity(remotes.len() + locals.len());
    let mut cloned: BTreeSet<&RepoId> = BTreeSet::new();

    for local in locals {
        let remote = local.id.as_ref().and_then(|id| by_id.get(id).copied());

        if let Some(id) = local.id.as_ref().filter(|_| remote.is_some()) {
            cloned.insert(id);
        }

        comparisons.push(compare_one(local, remote, tracked));
    }

    for remote in remotes {
        if !cloned.contains(&remote.id) {
            comparisons.push(Comparison {
                id: Some(remote.id.clone()),
                path: None,
                state: State::RemoteOnly,
                local: None,
                remote: Some(remote_flags(remote)),
            });
        }
    }

    comparisons.sort_by(|left, right| {
        left.id
            .as_ref()
            .map(ToString::to_string)
            .cmp(&right.id.as_ref().map(ToString::to_string))
            .then_with(|| left.path.cmp(&right.path))
    });

    comparisons
}

/// Judges one clone.
fn compare_one(
    local: &LocalRepo,
    remote: Option<&RemoteRepo>,
    tracked: &TrackedOwners,
) -> Comparison {
    Comparison {
        id: local.id.clone(),
        path: Some(local.path.clone()),
        state: state_for(local, remote, tracked),
        local: Some(LocalFlags {
            dirty: local.dirty,
            untracked: local.untracked,
            detached_head: matches!(local.head, Head::Detached),
        }),
        remote: remote.map(remote_flags),
    }
}

/// Works out the single answer for one clone.
fn state_for(local: &LocalRepo, remote: Option<&RemoteRepo>, tracked: &TrackedOwners) -> State {
    let Some(id) = local.id.as_ref() else {
        return State::LocalOnly(if local.remote_url.is_none() {
            LocalOnlyReason::NoRemote
        } else {
            LocalOnlyReason::UnsupportedHost
        });
    };

    if remote.is_none() {
        return State::LocalOnly(if tracked.contains(id) {
            LocalOnlyReason::MissingRemotely
        } else {
            LocalOnlyReason::OwnerNotTracked
        });
    }

    match &local.head {
        Head::Unborn => return State::Incomparable(IncomparableReason::NoCommits),
        Head::Detached => return State::Incomparable(IncomparableReason::DetachedHead),
        Head::Branch(_) => {}
    }

    let Some(tracking) = local.tracking else {
        return State::Incomparable(IncomparableReason::NoUpstreamBranch);
    };

    match (tracking.ahead, tracking.behind) {
        (0, 0) => State::InSync,
        (ahead, 0) => State::Ahead { ahead },
        (0, behind) => State::Behind { behind },
        (ahead, behind) => State::Diverged { ahead, behind },
    }
}

/// The flags that come from what a provider reports.
fn remote_flags(remote: &RemoteRepo) -> RemoteFlags {
    RemoteFlags {
        archived: remote.is_archived,
        private: remote.is_private,
        fork: remote.is_fork,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Metadata;
    use crate::scan::Tracking;

    fn remote(owner: &str, name: &str) -> RemoteRepo {
        RemoteRepo {
            id: RepoId::new(Provider::GitHub, owner, name),
            default_branch: Some("main".to_owned()),
            is_private: false,
            is_archived: false,
            is_fork: false,
            upstream: None,
            metadata: Metadata::default(),
        }
    }

    fn local(owner: &str, name: &str) -> LocalRepo {
        LocalRepo {
            path: PathBuf::from(format!("/code/{owner}/{name}")),
            id: Some(RepoId::new(Provider::GitHub, owner, name)),
            remote_url: Some(format!("git@github.com:{owner}/{name}.git")),
            head: Head::Branch("main".to_owned()),
            tracking: Some(Tracking {
                ahead: 0,
                behind: 0,
            }),
            dirty: false,
            untracked: false,
        }
    }

    fn tracked() -> TrackedOwners {
        TrackedOwners::new([(Provider::GitHub, "mcanouil".to_owned())])
    }

    fn only(comparisons: Vec<Comparison>) -> Comparison {
        assert_eq!(comparisons.len(), 1, "expected exactly one comparison");

        comparisons.into_iter().next().expect("a comparison")
    }

    #[test]
    fn a_repository_with_no_clone_is_remote_only() {
        let result = only(compare(&[remote("mcanouil", "minato")], &[], &tracked()));

        assert_eq!(result.state, State::RemoteOnly);
        assert_eq!(result.path, None);
    }

    #[test]
    fn a_matching_pair_with_equal_counts_is_in_sync() {
        let result = only(compare(
            &[remote("mcanouil", "minato")],
            &[local("mcanouil", "minato")],
            &tracked(),
        ));

        assert_eq!(result.state, State::InSync);
    }

    #[test]
    fn counts_translate_into_the_four_tracked_states() {
        let cases = [
            (0, 0, State::InSync),
            (2, 0, State::Ahead { ahead: 2 }),
            (0, 3, State::Behind { behind: 3 }),
            (
                1,
                4,
                State::Diverged {
                    ahead: 1,
                    behind: 4,
                },
            ),
        ];

        for (ahead, behind, expected) in cases {
            let mut clone = local("mcanouil", "minato");
            clone.tracking = Some(Tracking { ahead, behind });

            let result = only(compare(
                &[remote("mcanouil", "minato")],
                &[clone],
                &tracked(),
            ));

            assert_eq!(
                result.state, expected,
                "ahead {ahead}, behind {behind} produced the wrong state"
            );
        }
    }

    #[test]
    fn a_clone_of_a_tracked_owner_that_is_not_reported_is_missing_remotely() {
        let result = only(compare(&[], &[local("mcanouil", "gone")], &tracked()));

        assert_eq!(
            result.state,
            State::LocalOnly(LocalOnlyReason::MissingRemotely)
        );
    }

    #[test]
    fn a_clone_of_an_owner_nobody_asked_about_says_so_instead() {
        let result = only(compare(&[], &[local("someone-else", "thing")], &tracked()));

        assert_eq!(
            result.state,
            State::LocalOnly(LocalOnlyReason::OwnerNotTracked),
            "an unconfigured owner must not look like a deleted repository"
        );
    }

    #[test]
    fn a_clone_with_no_remote_is_distinguished_from_one_on_an_unknown_host() {
        let mut no_remote = local("mcanouil", "private-notes");
        no_remote.id = None;
        no_remote.remote_url = None;

        let mut unsupported = local("mcanouil", "elsewhere");
        unsupported.id = None;
        unsupported.remote_url = Some("git@example.com:some/thing.git".to_owned());

        assert_eq!(
            only(compare(&[], &[no_remote], &tracked())).state,
            State::LocalOnly(LocalOnlyReason::NoRemote)
        );
        assert_eq!(
            only(compare(&[], &[unsupported], &tracked())).state,
            State::LocalOnly(LocalOnlyReason::UnsupportedHost)
        );
    }

    #[test]
    fn a_clone_that_cannot_be_compared_says_why() {
        let cases = [
            (Head::Unborn, None, IncomparableReason::NoCommits),
            (Head::Detached, None, IncomparableReason::DetachedHead),
            (
                Head::Branch("wip".to_owned()),
                None,
                IncomparableReason::NoUpstreamBranch,
            ),
        ];

        for (head, tracking, expected) in cases {
            let mut clone = local("mcanouil", "minato");
            clone.head = head;
            clone.tracking = tracking;

            let result = only(compare(
                &[remote("mcanouil", "minato")],
                &[clone],
                &tracked(),
            ));

            assert_eq!(result.state, State::Incomparable(expected));
        }
    }

    #[test]
    fn every_repository_appears_exactly_once() {
        let remotes = [
            remote("mcanouil", "cloned"),
            remote("mcanouil", "not-cloned"),
        ];
        let locals = [local("mcanouil", "cloned"), local("mcanouil", "orphan")];

        let results = compare(&remotes, &locals, &tracked());

        assert_eq!(results.len(), 3, "two reported plus one extra clone");

        let states: Vec<_> = results.iter().map(|result| &result.state).collect();

        assert!(states.contains(&&State::InSync));
        assert!(states.contains(&&State::RemoteOnly));
        assert!(states.contains(&&State::LocalOnly(LocalOnlyReason::MissingRemotely)));
    }

    #[test]
    fn results_do_not_depend_on_the_order_the_inputs_arrived_in() {
        let remotes = [remote("mcanouil", "b"), remote("mcanouil", "a")];
        let locals = [local("mcanouil", "a")];

        let forwards = compare(&remotes, &locals, &tracked());

        let reversed_remotes = [remote("mcanouil", "a"), remote("mcanouil", "b")];
        let backwards = compare(&reversed_remotes, &locals, &tracked());

        assert_eq!(forwards, backwards);
    }

    #[test]
    fn two_clones_of_one_repository_are_both_reported() {
        let mut second = local("mcanouil", "minato");
        second.path = PathBuf::from("/elsewhere/minato");

        let results = compare(
            &[remote("mcanouil", "minato")],
            &[local("mcanouil", "minato"), second],
            &tracked(),
        );

        assert_eq!(results.len(), 2, "neither checkout should be swallowed");
        assert!(
            !results
                .iter()
                .any(|result| result.state == State::RemoteOnly),
            "a repository with two clones is not uncloned"
        );
    }

    #[test]
    fn flags_hold_alongside_the_state_rather_than_replacing_it() {
        let mut clone = local("mcanouil", "minato");
        clone.tracking = Some(Tracking {
            ahead: 0,
            behind: 2,
        });
        clone.dirty = true;
        clone.untracked = true;

        let mut reported = remote("mcanouil", "minato");
        reported.is_archived = true;
        reported.is_private = true;
        reported.is_fork = true;

        let result = only(compare(&[reported], &[clone], &tracked()));

        assert_eq!(result.state, State::Behind { behind: 2 });

        let local = result.local.expect("local facts");
        let reported = result.remote.expect("reported facts");

        assert!(local.dirty);
        assert!(local.untracked);
        assert!(reported.archived);
        assert!(reported.private);
        assert!(reported.fork);
    }

    #[test]
    fn only_a_clean_repository_that_is_behind_may_be_fast_forwarded() {
        let mut behind = local("mcanouil", "minato");
        behind.tracking = Some(Tracking {
            ahead: 0,
            behind: 1,
        });

        let result = only(compare(
            &[remote("mcanouil", "minato")],
            &[behind.clone()],
            &tracked(),
        ));

        assert!(result.can_fast_forward());
        assert_eq!(result.blocked_reason(), None);
    }

    #[test]
    fn an_untracked_file_does_not_block_a_fast_forward() {
        let mut behind = local("mcanouil", "minato");
        behind.tracking = Some(Tracking {
            ahead: 0,
            behind: 1,
        });
        behind.untracked = true;

        let result = only(compare(
            &[remote("mcanouil", "minato")],
            &[behind],
            &tracked(),
        ));

        assert!(
            result.can_fast_forward(),
            "git fast-forwards alongside untracked files, so minato should too"
        );
    }

    #[test]
    fn a_dirty_repository_that_is_behind_is_refused_with_a_reason() {
        let mut behind = local("mcanouil", "minato");
        behind.tracking = Some(Tracking {
            ahead: 0,
            behind: 1,
        });
        behind.dirty = true;

        let result = only(compare(
            &[remote("mcanouil", "minato")],
            &[behind],
            &tracked(),
        ));

        assert!(!result.can_fast_forward());
        assert_eq!(
            result.blocked_reason(),
            Some("the working tree has uncommitted changes")
        );
    }

    #[test]
    fn nothing_else_may_be_fast_forwarded_and_all_of_it_is_explained() {
        let mut diverged = local("mcanouil", "minato");
        diverged.tracking = Some(Tracking {
            ahead: 1,
            behind: 1,
        });

        let mut ahead = local("mcanouil", "minato");
        ahead.tracking = Some(Tracking {
            ahead: 1,
            behind: 0,
        });

        let mut detached = local("mcanouil", "minato");
        detached.head = Head::Detached;
        detached.tracking = None;

        for clone in [diverged, ahead, detached, local("mcanouil", "minato")] {
            let result = only(compare(
                &[remote("mcanouil", "minato")],
                &[clone],
                &tracked(),
            ));

            assert!(!result.can_fast_forward());
            assert!(
                result.blocked_reason().is_some(),
                "every refusal must carry a reason, {:?} did not",
                result.state
            );
        }
    }

    #[test]
    fn reported_facts_are_absent_rather_than_false_when_nothing_was_reported() {
        let result = only(compare(&[], &[local("mcanouil", "gone")], &tracked()));

        assert_eq!(
            result.remote, None,
            "`not a fork` must stay distinguishable from `there is nothing to ask`"
        );
        assert!(result.local.is_some());
    }

    #[test]
    fn a_repository_that_was_never_cloned_carries_no_local_facts() {
        let result = only(compare(&[remote("mcanouil", "minato")], &[], &tracked()));

        assert_eq!(result.local, None);
        assert!(result.remote.is_some());
        assert!(!result.can_fast_forward());
    }

    #[test]
    fn an_owner_is_matched_without_regard_to_the_case_it_was_configured_in() {
        let tracked = TrackedOwners::new([(Provider::GitHub, "McAnouil".to_owned())]);

        assert_eq!(
            only(compare(&[], &[local("mcanouil", "gone")], &tracked)).state,
            State::LocalOnly(LocalOnlyReason::MissingRemotely),
            "configuration case must not change which repositories are recognised"
        );
    }
}

#[cfg(test)]
mod properties {
    use super::*;
    use crate::model::Metadata;
    use crate::scan::Tracking;
    use proptest::prelude::*;

    /// Builds a clone from generated parts, so every shape a scan can produce
    /// is reachable, including ones no fixture would think to write down.
    fn any_local() -> impl Strategy<Value = LocalRepo> {
        (
            prop::option::of("[a-z]{1,4}"),
            prop::option::of("[a-z]{1,4}"),
            0_u32..4,
            0_u32..4,
            prop::bool::ANY,
            prop::bool::ANY,
            0_usize..3,
            prop::bool::ANY,
        )
            .prop_map(
                |(owner, name, ahead, behind, dirty, untracked, head_kind, tracks)| {
                    let id = owner
                        .as_ref()
                        .zip(name.as_ref())
                        .map(|(owner, name)| RepoId::new(Provider::GitHub, owner, name));

                    let head = match head_kind {
                        0 => Head::Branch("main".to_owned()),
                        1 => Head::Detached,
                        _ => Head::Unborn,
                    };

                    LocalRepo {
                        path: PathBuf::from(format!("/code/{owner:?}-{name:?}")),
                        remote_url: id.as_ref().map(|id| format!("git@github.com:{id}")),
                        id,
                        tracking: tracks.then_some(Tracking { ahead, behind }),
                        head,
                        dirty,
                        untracked,
                    }
                },
            )
    }

    fn any_remote() -> impl Strategy<Value = RemoteRepo> {
        ("[a-z]{1,4}", "[a-z]{1,4}").prop_map(|(owner, name)| RemoteRepo {
            id: RepoId::new(Provider::GitHub, &owner, &name),
            default_branch: Some("main".to_owned()),
            is_private: false,
            is_archived: false,
            is_fork: false,
            upstream: None,
            metadata: Metadata::default(),
        })
    }

    proptest! {
        /// Nothing may be dropped: a scan that finds a clone must always see it
        /// in the result, whatever state it turns out to be in.
        #[test]
        fn every_clone_is_accounted_for(
            remotes in prop::collection::vec(any_remote(), 0..6),
            locals in prop::collection::vec(any_local(), 0..6),
        ) {
            let results = compare(&remotes, &locals, &TrackedOwners::default());

            for local in &locals {
                prop_assert!(
                    results.iter().any(|result| result.path.as_ref() == Some(&local.path)),
                    "clone at {} vanished from the comparison",
                    local.path.display()
                );
            }
        }

        /// Every reported repository is either matched to a clone or reported
        /// as having none.
        #[test]
        fn every_reported_repository_is_accounted_for(
            remotes in prop::collection::vec(any_remote(), 0..6),
            locals in prop::collection::vec(any_local(), 0..6),
        ) {
            let results = compare(&remotes, &locals, &TrackedOwners::default());

            for remote in &remotes {
                prop_assert!(
                    results.iter().any(|result| result.id.as_ref() == Some(&remote.id)),
                    "reported repository {} vanished from the comparison",
                    remote.id
                );
            }
        }

        /// The safety rule, stated as a property rather than as examples: the
        /// only thing that may be fast-forwarded is a clean repository that is
        /// strictly behind.
        #[test]
        fn only_clean_and_behind_may_be_fast_forwarded(
            remotes in prop::collection::vec(any_remote(), 0..6),
            locals in prop::collection::vec(any_local(), 0..6),
        ) {
            for result in compare(&remotes, &locals, &TrackedOwners::default()) {
                if result.can_fast_forward() {
                    prop_assert!(
                        matches!(result.state, State::Behind { .. }),
                        "{:?} is not behind yet was judged safe to fast-forward",
                        result.state
                    );
                    prop_assert!(
                        !result.local.expect("local facts").dirty,
                        "a dirty repository was judged safe to fast-forward"
                    );
                }
            }
        }

        /// Anything refused carries a reason, and anything permitted does not,
        /// so no repository is ever skipped without explanation.
        #[test]
        fn refusal_and_reason_always_agree(
            remotes in prop::collection::vec(any_remote(), 0..6),
            locals in prop::collection::vec(any_local(), 0..6),
        ) {
            for result in compare(&remotes, &locals, &TrackedOwners::default()) {
                prop_assert_eq!(
                    result.can_fast_forward(),
                    result.blocked_reason().is_none(),
                    "state {:?} explains itself inconsistently",
                    result.state
                );
            }
        }

        /// Output must not depend on the order either side arrived in.
        #[test]
        fn the_result_does_not_depend_on_input_order(
            remotes in prop::collection::vec(any_remote(), 0..6),
            locals in prop::collection::vec(any_local(), 0..6),
        ) {
            let forwards = compare(&remotes, &locals, &TrackedOwners::default());

            let mut reversed_remotes = remotes.clone();
            reversed_remotes.reverse();
            let mut reversed_locals = locals.clone();
            reversed_locals.reverse();

            let backwards = compare(&reversed_remotes, &reversed_locals, &TrackedOwners::default());

            prop_assert_eq!(forwards.len(), backwards.len());
        }
    }
}
