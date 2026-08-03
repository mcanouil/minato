//! Which narrowing conditions each command can actually act on.
//!
//! `--owner`, `--group`, `--state`, and `--include-external` are global, so clap
//! accepts them everywhere. Not every command can honour them: a group and a
//! state are facts about a local clone, and a command that never scans has
//! nothing for them to match. Naming one there is refused rather than dropped,
//! because an unnarrowed answer looks like a valid answer to a question that
//! was never asked.

use super::{Cli, Command};

/// A condition that narrows what a command works on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Narrowing {
    /// `--owner`, which is known from the provider alone.
    Owner,
    /// `--group`, which is where a clone sits on disk.
    Group,
    /// `--state`, which is how a clone stands against the remote.
    State,
    /// `--include-external`, which is about clones of untracked owners.
    External,
}

impl Narrowing {
    /// The flag as it is typed.
    const fn flag(self) -> &'static str {
        match self {
            Self::Owner => "--owner",
            Self::Group => "--group",
            Self::State => "--state",
            Self::External => "--include-external",
        }
    }
}

/// Every condition, for the commands that compare clones against the remote.
const EVERY: &[Narrowing] = &[
    Narrowing::Owner,
    Narrowing::Group,
    Narrowing::State,
    Narrowing::External,
];

/// What a command that works from the provider alone can act on.
const REMOTE: &[Narrowing] = &[Narrowing::Owner];

/// What a command that works on no selection at all can act on.
const NONE: &[Narrowing] = &[];

/// Never shown, since a command acting on every condition refuses nothing.
const ACTS_ON_EVERY: &str = "it acts on every condition.";

/// Shown by a command that works from the provider alone.
const WORKS_REMOTELY: &str = "it works from what the provider reports and never scans local clones, so a local condition has nothing to match. `minato status` compares the two and takes it.";

/// Shown by `move`, which is told which repository to act on.
const NAMES_ITS_SUBJECT: &str = "it acts on the one repository you name, so there is nothing to narrow; `--to-group` says where it should land.";

/// Shown by a command that acts on no repository at all.
const TOUCHES_NOTHING: &str = "it acts on no repository, so there is nothing to narrow.";

/// What a command narrows by, and what to say about a condition it does not.
#[derive(Debug)]
struct Narrowable {
    /// The command as it is typed.
    command: &'static str,

    /// The conditions it acts on.
    honoured: &'static [Narrowing],

    /// Why the rest do not apply, shown only when one of them is named.
    note: &'static str,
}

/// What each command narrows by.
///
/// Exhaustive on purpose, with no wildcard arm: the flags are global, so clap
/// accepts them everywhere and each command has to say for itself which ones it
/// acts on. A new command will not compile until it does.
const fn narrowable(command: &Command) -> Narrowable {
    match command {
        Command::Status => Narrowable {
            command: "status",
            honoured: EVERY,
            note: ACTS_ON_EVERY,
        },
        Command::Clone { .. } => Narrowable {
            command: "clone",
            honoured: EVERY,
            note: ACTS_ON_EVERY,
        },
        Command::Fetch { .. } => Narrowable {
            command: "fetch",
            honoured: EVERY,
            note: ACTS_ON_EVERY,
        },
        Command::Update { .. } => Narrowable {
            command: "update",
            honoured: EVERY,
            note: ACTS_ON_EVERY,
        },
        Command::Tui => Narrowable {
            command: "tui",
            honoured: EVERY,
            note: ACTS_ON_EVERY,
        },
        Command::List => Narrowable {
            command: "list",
            honoured: REMOTE,
            note: WORKS_REMOTELY,
        },
        Command::SyncFork { .. } => Narrowable {
            command: "sync-fork",
            honoured: REMOTE,
            note: WORKS_REMOTELY,
        },
        Command::Move { .. } => Narrowable {
            command: "move",
            honoured: NONE,
            note: NAMES_ITS_SUBJECT,
        },
        Command::Refresh => Narrowable {
            command: "refresh",
            honoured: NONE,
            note: TOUCHES_NOTHING,
        },
        Command::Auth { .. } => Narrowable {
            command: "auth status",
            honoured: NONE,
            note: TOUCHES_NOTHING,
        },
        Command::Doctor => Narrowable {
            command: "doctor",
            honoured: NONE,
            note: TOUCHES_NOTHING,
        },
        Command::Completions { .. } => Narrowable {
            command: "completions",
            honoured: NONE,
            note: TOUCHES_NOTHING,
        },
    }
}

/// The conditions the run was narrowed by.
fn named(cli: &Cli) -> Vec<Narrowing> {
    let mut named = Vec::new();

    if !cli.owners.is_empty() {
        named.push(Narrowing::Owner);
    }

    if !cli.groups.is_empty() {
        named.push(Narrowing::Group);
    }

    if !cli.states.is_empty() {
        named.push(Narrowing::State);
    }

    if cli.include_external {
        named.push(Narrowing::External);
    }

    named
}

/// The flags as prose, so a refusal reads as a sentence.
fn name_all(refused: &[Narrowing]) -> String {
    let quoted: Vec<String> = refused
        .iter()
        .map(|condition| format!("`{}`", condition.flag()))
        .collect();

    match quoted.split_last() {
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
        None => String::new(),
    }
}

/// Refuses a run narrowed by a condition its command cannot act on.
///
/// # Errors
///
/// Returns an error naming every refused flag at once, so a run is not
/// corrected one flag at a time.
pub fn check(cli: &Cli) -> Result<(), InapplicableNarrowingError> {
    let narrowable = narrowable(&cli.command);

    let refused: Vec<Narrowing> = named(cli)
        .into_iter()
        .filter(|condition| !narrowable.honoured.contains(condition))
        .collect();

    if refused.is_empty() {
        return Ok(());
    }

    Err(InapplicableNarrowingError {
        command: narrowable.command,
        flags: name_all(&refused),
        note: narrowable.note,
    })
}

/// A command was narrowed by a condition it cannot act on.
#[derive(Debug, thiserror::Error)]
#[error("cannot narrow `{command}` by {flags}: {note}")]
pub struct InapplicableNarrowingError {
    /// The command as it is typed.
    command: &'static str,

    /// The flags it cannot act on, as they were typed.
    flags: String,

    /// Why they do not apply, and what to do instead.
    note: &'static str,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    /// The refusal for a command line, or a panic naming what was accepted.
    fn refusal(arguments: &[&str]) -> String {
        let cli = Cli::try_parse_from(arguments).expect("the arguments should parse");

        match check(&cli) {
            Err(error) => error.to_string(),
            Ok(()) => panic!("{arguments:?} should be refused"),
        }
    }

    fn accepts(arguments: &[&str]) {
        let cli = Cli::try_parse_from(arguments).expect("the arguments should parse");

        assert!(
            check(&cli).is_ok(),
            "{arguments:?} names only conditions the command acts on, so it should be accepted"
        );
    }

    #[test]
    fn list_refuses_a_group_and_says_which_command_takes_one() {
        let message = refusal(&["minato", "list", "--group", "perso"]);

        assert!(message.contains("`--group`"), "{message}");
        assert!(message.contains("`list`"), "{message}");
        assert!(
            message.contains("minato status"),
            "the refusal should name the command that does compare clones: {message}"
        );
    }

    #[test]
    fn list_refuses_every_condition_it_cannot_act_on() {
        for flag in [
            vec!["--group", "perso"],
            vec!["--state", "drifted"],
            vec!["--include-external"],
        ] {
            let mut arguments = vec!["minato", "list"];
            arguments.extend(flag.iter().copied());

            let message = refusal(&arguments);

            assert!(message.contains(flag[0]), "{arguments:?}: {message}");
        }
    }

    #[test]
    fn list_accepts_the_conditions_it_does_act_on() {
        accepts(&["minato", "list", "--owner", "mcanouil"]);
        accepts(&["minato", "list", "--include-forks"]);
    }

    #[test]
    fn sync_fork_works_remotely_so_it_refuses_a_local_condition() {
        let message = refusal(&["minato", "sync-fork", "--group", "perso", "--dry-run"]);

        assert!(message.contains("`--group`"), "{message}");
        assert!(message.contains("`sync-fork`"), "{message}");
    }

    #[test]
    fn the_commands_that_scan_accept_every_condition() {
        for command in ["status", "clone", "fetch", "update", "tui"] {
            accepts(&[
                "minato",
                command,
                "--owner",
                "mcanouil",
                "--group",
                "perso",
                "--state",
                "drifted",
                "--include-external",
            ]);
        }
    }

    #[test]
    fn move_names_its_repository_outright_so_it_narrows_by_nothing() {
        let message = refusal(&[
            "minato",
            "move",
            "repo",
            "--to-group",
            "demo",
            "--owner",
            "mcanouil",
        ]);

        assert!(message.contains("`--owner`"), "{message}");
        assert!(
            message.contains("--to-group"),
            "the refusal should point at the flag that does say where: {message}"
        );
    }

    #[test]
    fn the_commands_that_touch_no_repository_refuse_every_condition() {
        for arguments in [
            vec!["minato", "refresh", "--group", "perso"],
            vec!["minato", "doctor", "--state", "drifted"],
            vec!["minato", "auth", "status", "--owner", "mcanouil"],
            vec!["minato", "completions", "bash", "--include-external"],
        ] {
            let message = refusal(&arguments);

            assert!(
                message.contains("no repository"),
                "{arguments:?}: {message}"
            );
        }
    }

    #[test]
    fn json_and_refresh_narrow_nothing_so_they_are_accepted_anywhere() {
        accepts(&["minato", "doctor", "--json", "--refresh"]);
        accepts(&["minato", "completions", "bash"]);
    }

    #[test]
    fn every_refused_condition_is_named_in_one_message() {
        let message = refusal(&[
            "minato",
            "list",
            "--group",
            "perso",
            "--state",
            "drifted",
            "--include-external",
        ]);

        for flag in ["`--group`", "`--state`", "`--include-external`"] {
            assert!(
                message.contains(flag),
                "a run should be corrected once rather than a flag at a time: {message}"
            );
        }
    }
}
