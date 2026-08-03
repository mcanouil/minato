//! The command surface.
//!
//! Every capability lands here first, so a person, a script, or an agent can
//! reach any outcome without an interactive interface. Each command offers a
//! table for reading and `--json` for consuming.

mod narrowing;
mod render;

use std::path::{Path, PathBuf};

use clap::{CommandFactory, Parser, Subcommand};
use jiff::{SignedDuration, Timestamp};
use serde::Serialize;
use std::fmt::Write as _;

use crate::actions::{self, Mode};
use crate::cache::{Cache, Cached};
use crate::compare::{self, Comparison, State, TrackedOwners};
use crate::config::{self, Config};
use crate::filter::{self, Filter};
use crate::github::auth;
use crate::github::{Account, GitHubClient, GitHubError};
use crate::model::{Provider, RemoteRepo};
use crate::scan;

use render::{Table, describe_age};

/// Overview and sync of Git repositories across hosting providers.
#[derive(Debug, Parser)]
#[command(name = "minato", version, about)]
// Each bool is an independent, order-free command-line flag, which is the
// natural shape for a CLI rather than a state machine to be split up.
#[allow(clippy::struct_excessive_bools)]
pub struct Cli {
    /// Emit JSON instead of a table.
    #[arg(long, global = true)]
    pub json: bool,

    /// Ignore cached data and ask the provider again.
    #[arg(long, global = true)]
    pub refresh: bool,

    /// Keep only repositories owned by these accounts.
    ///
    /// Every command that works on a selection takes this. One that names its
    /// subject outright, or touches no repository at all, refuses it rather
    /// than ignoring it.
    #[arg(long = "owner", global = true, value_name = "OWNER")]
    pub owners: Vec<String>,

    /// Keep only repositories in these groups, and in the groups beneath them.
    ///
    /// A group is a directory path beneath a root, written with `/`, so
    /// `--group perso` keeps `perso` and everything filed under it, including
    /// `perso/apps`, while `--group perso/apps` keeps only that bucket.
    ///
    /// A group is where a clone sits, so only the commands that scan take
    /// this: `status`, `clone`, `fetch`, `update`, and `tui`. Anywhere else it
    /// is refused rather than ignored.
    #[arg(long = "group", global = true, value_name = "GROUP")]
    pub groups: Vec<String>,

    /// Keep only repositories in these states.
    ///
    /// A state is how a clone stands against the remote, so only the commands
    /// that scan take this: `status`, `clone`, `fetch`, `update`, and `tui`.
    /// Anywhere else it is refused rather than ignored.
    #[arg(long = "state", global = true, value_name = "STATE")]
    pub states: Vec<filter::StateFilter>,

    /// Include forks, which are hidden by default.
    #[arg(long, global = true)]
    pub include_forks: bool,

    /// Include clones of repositories owned by nobody you track, which are
    /// hidden by default.
    ///
    /// This describes a clone, so only the commands that scan take it:
    /// `status`, `clone`, `fetch`, `update`, and `tui`. Anywhere else it is
    /// refused rather than ignored.
    #[arg(long, global = true)]
    pub include_external: bool,

    #[command(subcommand)]
    pub command: Command,
}

/// What to do.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// List every repository, with what the provider reports about it.
    List,

    /// Show how local clones stand against what the provider reports.
    Status,

    /// Clone repositories that have no local copy.
    Clone {
        /// Where to put them. Defaults to the first configured root.
        ///
        /// Where a repository belongs is a judgement its identity does not
        /// carry, so it is chosen here rather than derived.
        #[arg(long, value_name = "DIRECTORY")]
        into: Option<PathBuf>,

        /// Put them in this group, which is the directory path that group
        /// already occupies beneath a root, such as `perso/apps`.
        ///
        /// This is not the same as the `--group` filter, which selects by
        /// where a clone already sits. A repository that has not been cloned
        /// is in no group, so filtering by one would match nothing.
        #[arg(long = "into-group", value_name = "GROUP", conflicts_with = "into")]
        group: Option<String>,

        /// Report what would be cloned, and change nothing.
        #[arg(long)]
        dry_run: bool,

        /// Clone with a truncated history.
        #[arg(long)]
        shallow: bool,
    },

    /// Fetch every local clone. This never touches a working tree.
    Fetch {
        /// Report what would be fetched, and change nothing.
        #[arg(long)]
        dry_run: bool,
    },

    /// Fast-forward clones that are strictly behind and clean.
    Update {
        /// Report what would be updated, and change nothing.
        #[arg(long)]
        dry_run: bool,
    },

    /// Sync forks with their upstream, fast-forward only.
    ///
    /// Only a fork strictly behind its upstream is synced, through GitHub's
    /// merge-upstream. A fork holding its own commits is reported and left
    /// alone rather than merged, so history is never rewritten.
    SyncFork {
        /// Report what would be synced, and change nothing.
        #[arg(long)]
        dry_run: bool,
    },

    /// Move one repository into a group, which moves it on disk.
    ///
    /// Deliberately one repository at a time: this changes the filesystem, so
    /// it is never a side effect of anything else.
    Move {
        /// Which repository, named by identity, owner/name, or bare name.
        #[arg(value_name = "REPOSITORY")]
        repository: String,

        /// The group to move it into, which is a directory path beneath its
        /// root, such as `perso/apps`.
        #[arg(long = "to-group", value_name = "GROUP")]
        group: String,

        /// Report what would move, and change nothing.
        #[arg(long)]
        dry_run: bool,
    },

    /// Discard cached data so the next run asks the provider again.
    Refresh,

    /// Inspect authentication.
    Auth {
        #[command(subcommand)]
        command: AuthCommand,
    },

    /// Check that configuration and tooling are usable.
    Doctor,

    /// Print a shell completion script.
    ///
    /// The script goes to standard output and where to put it goes to standard
    /// error, so `minato completions zsh > _minato` writes the script alone
    /// while the instructions still reach the terminal. Run it without
    /// redirecting to read them, or see
    /// <https://m.canouil.dev/minato/get-started/#enable-completion>.
    ///
    /// It is generated from the command definitions, so it cannot describe a
    /// command the binary does not have, and it needs regenerating after an
    /// upgrade that adds one.
    Completions {
        /// Which shell to generate for.
        shell: clap_complete::Shell,
    },

    /// Browse repositories interactively.
    Tui,
}

/// Authentication subcommands.
#[derive(Debug, Subcommand)]
pub enum AuthCommand {
    /// Report whether a token was found, and where it came from.
    Status,
}

/// Anything that stops a command from producing an answer.
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    /// Configuration could not be loaded.
    #[error(transparent)]
    Config(#[from] config::ConfigError),

    /// No token could be found.
    #[error(transparent)]
    Auth(#[from] auth::NoTokenError),

    /// The provider could not be reached.
    #[error(transparent)]
    GitHub(#[from] crate::github::GitHubError),

    /// A configured root could not be resolved.
    #[error(transparent)]
    Roots(#[from] config::UnresolvedRootError),

    /// The cache could not be written.
    #[error(transparent)]
    Cache(#[from] crate::cache::CacheError),

    /// Configuration described nothing usable.
    #[error(transparent)]
    Validation(#[from] config::ValidationError),

    /// The terminal could not be driven.
    ///
    /// The cause is not interpolated here, because the caller prints the whole
    /// chain and would otherwise show it twice.
    #[error("cannot run the interactive browser")]
    Terminal {
        /// What went wrong.
        #[source]
        source: std::io::Error,
    },

    /// A repository could not be moved.
    #[error(transparent)]
    Move(#[from] actions::MoveError),

    /// A group did not name a directory path beneath a root.
    #[error(transparent)]
    Group(#[from] actions::InvalidGroupError),

    /// A command was narrowed by a condition it cannot act on.
    #[error(transparent)]
    Narrowing(#[from] narrowing::InapplicableNarrowingError),

    /// Output could not be rendered.
    #[error("cannot render JSON output: {0}")]
    Json(#[from] serde_json::Error),
}

/// What a command produced, and whether any repository failed.
///
/// The two are separate because a batch that reports several failures still
/// produces output worth printing; the failures decide the exit code, not
/// whether anything is shown.
#[derive(Debug)]
pub struct Output {
    /// What to print.
    pub text: String,

    /// Whether any repository failed, which must make the process exit
    /// non-zero even though the command itself ran.
    pub failed: bool,
}

impl Cli {
    /// The filter the user asked for.
    fn filter(&self) -> Filter {
        Filter {
            owners: self.owners.clone(),
            groups: self.groups.clone(),
            states: self.states.clone(),
            include_forks: self.include_forks,
            include_external: self.include_external,
        }
    }
}

impl From<String> for Output {
    fn from(text: String) -> Self {
        Self {
            text,
            failed: false,
        }
    }
}

/// Runs a command, returning what to print.
///
/// # Errors
///
/// Returns an error when configuration, authentication, the provider, or the
/// cache prevents an answer. Every variant names what to do next.
pub async fn run(cli: &Cli) -> Result<Output, CliError> {
    // Before anything is loaded or fetched, so a run narrowed by a condition
    // its command cannot act on is refused rather than answered without it.
    narrowing::check(cli)?;

    match &cli.command {
        Command::Auth {
            command: AuthCommand::Status,
        } => Ok(auth_status(cli.json).into()),
        Command::Doctor => doctor(cli.json).map(Into::into),
        Command::Completions { shell } => {
            // On standard error, so a redirect into the shell's completion
            // directory captures the script and leaves the instructions on
            // the terminal, where they are of use.
            eprintln!("{}", completion_hint(*shell));
            Ok(completions(*shell).into())
        }
        Command::Tui => tui(cli).await,
        Command::Refresh => refresh(cli.json).map(Into::into),
        Command::List => list(cli).await.map(Into::into),
        Command::Status => status(cli).await.map(Into::into),
        Command::Clone {
            into,
            group,
            dry_run,
            shallow,
        } => {
            act(
                cli,
                Act::Clone {
                    into: into.clone(),
                    group: group.clone(),
                    shallow: *shallow,
                },
                mode(*dry_run),
            )
            .await
        }
        Command::Fetch { dry_run } => act(cli, Act::Fetch, mode(*dry_run)).await,
        Command::Update { dry_run } => act(cli, Act::Update, mode(*dry_run)).await,
        Command::SyncFork { dry_run } => sync_fork(cli, mode(*dry_run)).await,
        Command::Move {
            repository,
            group,
            dry_run,
        } => move_one(cli, repository, group, mode(*dry_run)).await,
    }
}

/// Which action to carry out.
enum Act {
    Clone {
        into: Option<PathBuf>,
        group: Option<String>,
        shallow: bool,
    },
    Fetch,
    Update,
}

const fn mode(dry_run: bool) -> Mode {
    if dry_run { Mode::DryRun } else { Mode::Execute }
}

/// Runs an action over everything the comparison found.
///
/// Failures do not stop the batch: every repository is reported, and the
/// process exits non-zero only at the end.
async fn act(cli: &Cli, action: Act, mode: Mode) -> Result<Output, CliError> {
    let paths = paths()?;
    let config = Config::load_from(&paths.config)?;
    let gathered = gather(cli, &paths, &config).await?;

    let roots = config.resolved_roots(paths.home.as_deref())?;
    let scanned = scan::scan(&roots, scan::DEFAULT_MAX_DEPTH);
    let comparisons = cli.filter().apply(compare::compare(
        &gathered.remotes,
        &scanned.repositories,
        &gathered.tracked,
    ));

    let summary = match action {
        Act::Clone {
            into,
            group,
            shallow,
        } => {
            let root = roots
                .first()
                .cloned()
                .ok_or(config::ValidationError::NoRoots)?;

            // A group names a directory that already exists somewhere beneath
            // a root, so cloning into a group means cloning where its
            // repositories already live rather than repeating the path.
            let destination = clone_destination(&roots, &root, into, group.as_deref())?;

            actions::clone_missing(&comparisons, &destination, &config.local, shallow, mode)
        }
        Act::Fetch => actions::fetch_all(&comparisons, mode),
        Act::Update => actions::update_all(&comparisons, mode),
    };

    let mut output = summary_output(cli.json, &summary)?;

    // A root that could not be read would otherwise make every repository look
    // uncloned with no hint why, so the scan's notes travel with the result.
    if !cli.json {
        append_scan_notes(&mut output.text, &scanned);
    }

    Ok(output)
}

/// Renders a batch summary as JSON or a table, carrying the failure flag.
fn summary_output(as_json: bool, summary: &actions::Summary) -> Result<Output, CliError> {
    let failed = summary.has_failures();

    let text = if as_json {
        serde_json::to_string_pretty(summary)?
    } else {
        render_summary(summary)
    };

    Ok(Output { text, failed })
}

/// Returns the client, building it on first use so a fully cached or rehearsed
/// run never asks for a token.
fn client_or_init(slot: &mut Option<GitHubClient>) -> Result<&GitHubClient, CliError> {
    if slot.is_none() {
        let (token, source) = auth::resolve_token_from_system()?;
        *slot = Some(GitHubClient::new(token, Some(source))?);
    }

    Ok(slot.as_ref().expect("the client was just built"))
}

/// Renders what happened, including everything deliberately left alone.
fn render_summary(summary: &actions::Summary) -> String {
    if summary.reports.is_empty() {
        return "Nothing to do.".to_owned();
    }

    let mut table = Table::new(["", "REPOSITORY", "DETAIL"]);

    for report in &summary.reports {
        let (marker, detail) = match &report.outcome {
            actions::Outcome::Done { detail } => ("ok", detail.clone()),
            actions::Outcome::Would { detail } => ("--", format!("would {detail}")),
            actions::Outcome::Skipped { reason } => ("  ", format!("skipped: {reason}")),
            actions::Outcome::Failed { error } => ("!!", format!("failed: {error}")),
        };

        table.push([
            marker.to_owned(),
            report
                .id
                .as_ref()
                .map_or_else(|| "-".to_owned(), ToString::to_string),
            detail,
        ]);
    }

    let counts = summary.counts();
    let mut out = table.to_string();

    let _ = write!(
        out,
        "\n{} done, {} would, {} skipped, {} failed.\n",
        counts.done, counts.would, counts.skipped, counts.failed
    );

    out
}

/// Syncs forks that are strictly behind their upstream, fast-forward only.
///
/// A fork holding its own commits cannot be fast-forwarded, so it is reported
/// and left alone rather than merged. Only GitHub's merge-upstream is used, so
/// nothing is cloned or pushed and no history is rewritten.
async fn sync_fork(cli: &Cli, mode: Mode) -> Result<Output, CliError> {
    let paths = paths()?;
    let config = Config::load_from(&paths.config)?;
    let gathered = gather(cli, &paths, &config).await?;

    let filter = cli.filter();
    let mut reports = Vec::new();
    let mut client = None;

    for repo in &gathered.remotes {
        if !repo.is_fork {
            continue;
        }

        if !filter.owner_matches(&repo.id.owner) {
            continue;
        }

        let Some(standing) = repo.metadata.upstream else {
            continue;
        };

        // A fork level with its upstream needs nothing, so it is not reported.
        if !standing.is_behind() {
            continue;
        }

        if !standing.can_fast_forward() {
            reports.push(sync_report(
                repo,
                actions::Outcome::Skipped {
                    reason: format!(
                        "diverged: {} ahead of upstream and {} behind",
                        standing.ahead, standing.behind
                    ),
                },
            ));
            continue;
        }

        let Some(branch) = repo.default_branch.as_deref() else {
            reports.push(sync_report(
                repo,
                actions::Outcome::Skipped {
                    reason: "no default branch to sync".to_owned(),
                },
            ));
            continue;
        };

        let detail = format!(
            "sync {} commits from upstream onto {branch}",
            standing.behind
        );

        let outcome = match mode {
            Mode::DryRun => actions::Outcome::Would { detail },
            Mode::Execute => {
                // The client is built once, and only when a fork actually needs
                // syncing, so a rehearsal never asks for a token.
                let client = client_or_init(&mut client)?;

                match client.merge_upstream(&repo.id, branch).await {
                    Ok(()) => actions::Outcome::Done { detail },
                    Err(error) => actions::Outcome::Failed {
                        error: error.to_string(),
                    },
                }
            }
        };

        reports.push(sync_report(repo, outcome));
    }

    let summary = actions::Summary { reports };

    summary_output(cli.json, &summary)
}

/// Builds a report for a fork, which has no path since it is a remote action.
fn sync_report(repo: &crate::model::RemoteRepo, outcome: actions::Outcome) -> actions::Report {
    actions::Report {
        id: Some(repo.id.clone()),
        path: None,
        outcome,
    }
}

/// Moves one repository into a group.
async fn move_one(
    cli: &Cli,
    repository: &str,
    group: &str,
    mode: Mode,
) -> Result<Output, CliError> {
    let paths = paths()?;
    let config = Config::load_from(&paths.config)?;
    let gathered = gather(cli, &paths, &config).await?;

    let roots = config.resolved_roots(paths.home.as_deref())?;
    let scanned = scan::scan(&roots, scan::DEFAULT_MAX_DEPTH);
    let comparisons = compare::compare(&gathered.remotes, &scanned.repositories, &gathered.tracked);

    let found = actions::find_one(&comparisons, repository)?;
    let report = actions::move_to_group(found, repository, &roots, group, mode)?;

    if cli.json {
        return Ok(serde_json::to_string_pretty(&report)?.into());
    }

    Ok(match &report.outcome {
        actions::Outcome::Would { detail } => format!("Would {detail}."),
        actions::Outcome::Done { detail } => format!("Did {detail}."),
        other => format!("{other:?}"),
    }
    .into())
}

/// Opens the interactive browser over the same comparison the commands use.
async fn tui(cli: &Cli) -> Result<Output, CliError> {
    let paths = paths()?;
    let config = Config::load_from(&paths.config)?;
    let gathered = gather(cli, &paths, &config).await?;
    let roots = config.resolved_roots(paths.home.as_deref())?;
    let filter = cli.filter();

    let build = |remotes: &[crate::model::RemoteRepo]| {
        let scanned = scan::scan(&roots, scan::DEFAULT_MAX_DEPTH);

        filter.apply(compare::compare(
            remotes,
            &scanned.repositories,
            &gathered.tracked,
        ))
    };

    // Actions run through the same functions the commands use. Clone goes to the
    // first configured root under the layout, as `minato clone` does with no
    // `--into`, so the destination and configuration stay here rather than
    // leaking into the browser.
    let clone_root = roots.first().cloned();
    let apply = |action: &crate::tui::Action, comparison: &Comparison| -> actions::Summary {
        let selection = [comparison.clone()];

        match action {
            crate::tui::Action::Fetch => actions::fetch_all(&selection, Mode::Execute),
            crate::tui::Action::Update => actions::update_all(&selection, Mode::Execute),
            crate::tui::Action::Clone { group } => {
                // Nothing here can fail the whole session: a typed group that
                // leads nowhere is reported on the row it was typed for, the
                // same way a clone that could not run is.
                let skipped = |reason: String| actions::Summary {
                    reports: vec![actions::Report {
                        id: comparison.id.clone(),
                        path: None,
                        outcome: actions::Outcome::Skipped { reason },
                    }],
                };

                let Some(root) = clone_root.as_ref() else {
                    return skipped("no configured root to clone into".to_owned());
                };

                match clone_destination(&roots, root, None, group.as_deref()) {
                    Ok(destination) => actions::clone_missing(
                        &selection,
                        &destination,
                        &config.local,
                        false,
                        Mode::Execute,
                    ),
                    Err(error) => skipped(error.to_string()),
                }
            }
        }
    };

    // The first paint, a reload, and the refresh after an action all rescan the
    // disk through `build`. It deliberately does not refetch: asking the provider
    // again is what `--refresh` is for, and a keystroke should not spend
    // someone's rate limit.
    crate::tui::run(|| build(&gathered.remotes), apply)
        .map_err(|source| CliError::Terminal { source })?;

    Ok(String::new().into())
}

/// Where configuration and the cache live for this run.
struct Paths {
    config: PathBuf,
    cache: Cache,
    home: Option<PathBuf>,
}

fn paths() -> Result<Paths, CliError> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let explicit = std::env::var_os(config::CONFIG_ENV).map(PathBuf::from);
    let xdg = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from);

    let config = config::config_path_from(explicit.as_deref(), xdg.as_deref(), home.as_deref())?;

    let cache_root = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| home.as_ref().map(|home| home.join(".cache")))
        .unwrap_or_else(|| PathBuf::from(".cache"))
        .join("minato");

    Ok(Paths {
        config,
        cache: Cache::new(cache_root),
        home,
    })
}

/// Reports where a token came from, without ever showing it.
fn auth_status(as_json: bool) -> String {
    let found = auth::resolve_token_from_system().ok();

    if as_json {
        #[derive(Serialize)]
        struct Report {
            authenticated: bool,
            source: Option<String>,
        }

        let report = Report {
            authenticated: found.is_some(),
            source: found.as_ref().map(|(_, source)| source.to_string()),
        };

        return serde_json::to_string_pretty(&report).unwrap_or_default();
    }

    match found {
        Some((_, source)) => format!("Authenticated to GitHub using {source}."),
        None => format!("Not authenticated to GitHub.\n\n{}", auth::NoTokenError),
    }
}

/// Checks the things a run depends on, and reports every one.
fn doctor(as_json: bool) -> Result<String, CliError> {
    let mut checks: Vec<(String, bool, String)> = Vec::new();

    let git = crate::git::run(&PathBuf::from("."), &["--version"]);
    checks.push((
        "git".to_owned(),
        git.is_ok(),
        git.map_or_else(
            |error| error.to_string(),
            |version| version.replace("git version ", ""),
        ),
    ));

    let token = auth::resolve_token_from_system();
    checks.push((
        "token".to_owned(),
        token.is_ok(),
        token.map_or_else(
            |error| error.to_string(),
            |(_, source)| format!("found in {source}"),
        ),
    ));

    let resolved = paths();
    match &resolved {
        Ok(paths) => {
            let config = Config::load_from(&paths.config);

            checks.push((
                "configuration".to_owned(),
                config.is_ok(),
                config
                    .as_ref()
                    .map_or_else(ToString::to_string, |_| paths.config.display().to_string()),
            ));

            if let Ok(config) = &config {
                let roots = config.resolved_roots(paths.home.as_deref());

                checks.push((
                    "roots".to_owned(),
                    roots.is_ok(),
                    match &roots {
                        Ok(roots) => roots
                            .iter()
                            .map(|root| {
                                let missing = if root.is_dir() { "" } else { " (missing)" };
                                format!("{}{missing}", root.display())
                            })
                            .collect::<Vec<_>>()
                            .join(", "),
                        Err(error) => error.to_string(),
                    },
                ));
            }

            checks.push((
                "cache".to_owned(),
                true,
                paths.cache.root().display().to_string(),
            ));
        }
        Err(error) => checks.push(("configuration".to_owned(), false, error.to_string())),
    }

    if as_json {
        #[derive(Serialize)]
        struct Check {
            name: String,
            ok: bool,
            detail: String,
        }

        let report: Vec<Check> = checks
            .into_iter()
            .map(|(name, ok, detail)| Check { name, ok, detail })
            .collect();

        return Ok(serde_json::to_string_pretty(&report)?);
    }

    let mut table = Table::new(["", "CHECK", "DETAIL"]);

    for (name, ok, detail) in &checks {
        // Only the first line goes in the table: several checks explain
        // themselves at length, and a multi-line cell would tear the columns
        // apart. The full text follows underneath.
        let summary = detail.lines().next().unwrap_or_default();

        table.push([if *ok { "ok" } else { "!!" }, name.as_str(), summary]);
    }

    let mut out = table.to_string();

    for (name, _, detail) in checks.iter().filter(|(_, ok, _)| !ok) {
        if detail.lines().count() > 1 {
            let _ = write!(out, "\n{name}:\n{detail}\n");
        }
    }

    Ok(out)
}

/// Discards cached data.
fn refresh(as_json: bool) -> Result<String, CliError> {
    let paths = paths()?;
    paths.cache.clear()?;

    if as_json {
        return Ok(serde_json::to_string_pretty(&serde_json::json!({
            "cleared": paths.cache.root().display().to_string(),
        }))?);
    }

    Ok(format!("Cleared {}.", paths.cache.root().display()))
}

/// Generates a completion script for a shell.
///
/// The name completed is the one the command carries rather than the one it
/// was invoked by, so a script written from a build directory, or from a copy
/// under another name, still completes the command as it is installed.
fn completions(shell: clap_complete::Shell) -> String {
    let mut command = Cli::command();
    let name = command.get_name().to_owned();
    let mut script = Vec::new();
    clap_complete::generate(shell, &mut command, name, &mut script);

    String::from_utf8(script).expect("clap_complete writes UTF-8")
}

/// Where the per-shell instructions live, for the hint to point at when the
/// few lines it can spare are not enough.
const COMPLETION_DOCS: &str = "https://m.canouil.dev/minato/get-started/#enable-completion";

/// What to do with the script a shell was just handed.
///
/// Printed to standard error rather than returned with the script, so that
/// `minato completions zsh > _minato` writes the script alone while the
/// instructions still reach the terminal.
fn completion_hint(shell: clap_complete::Shell) -> String {
    use clap_complete::Shell;

    let steps = match shell {
        Shell::Bash => "Write it to ~/.local/share/bash-completion/completions/minato.".to_owned(),
        // oh-my-zsh puts its custom completions directory on $fpath and runs
        // compinit itself, so naming it first spares those users an edit to
        // ~/.zshrc that would do nothing.
        Shell::Zsh => [
            "Write it as _minato in a directory on $fpath.",
            "With oh-my-zsh, ~/.oh-my-zsh/custom/completions needs nothing further.",
            "Otherwise, such as ~/.zfunc, have compinit run after it is added:",
            "  fpath=(~/.zfunc $fpath)",
            "  autoload -Uz compinit && compinit",
        ]
        .join("\n"),
        Shell::Fish => "Write it to ~/.config/fish/completions/minato.fish.".to_owned(),
        Shell::Elvish => [
            "Write it to ~/.config/elvish/lib/minato.elv,",
            "then load it from ~/.config/elvish/rc.elv:",
            "  use minato",
        ]
        .join("\n"),
        Shell::PowerShell => [
            "Evaluate it rather than saving it:",
            "  minato completions powershell | Out-String | Invoke-Expression",
            "Add that line to the file $PROFILE names to have it apply to every session.",
        ]
        .join("\n"),
        // `Shell` is non-exhaustive, so a shell added by a future
        // `clap_complete` still gets pointed somewhere useful rather than
        // failing to compile or saying nothing at all.
        _ => format!("Install it where {shell} looks for completion scripts for minato."),
    };

    format!("{steps}\nSee {COMPLETION_DOCS}")
}

/// Everything gathered for a run, and how fresh it was.
struct Gathered {
    remotes: Vec<RemoteRepo>,
    tracked: TrackedOwners,
    /// How old the oldest cached entry used was, absent when nothing was
    /// served from cache.
    staleness: Option<jiff::SignedDuration>,
}

/// The accounts to enumerate, each login once regardless of case or of being
/// listed under both `users` and `orgs`.
///
/// A login that appears twice would otherwise fetch and clone the same
/// repositories twice, doubling the report and racing two clones into one path.
fn accounts_of(github: &config::GitHub) -> Vec<Account> {
    let mut seen = std::collections::HashSet::new();

    github
        .users
        .iter()
        .chain(github.orgs.iter())
        .filter(|login| seen.insert(login.to_lowercase()))
        .map(Account::new)
        .collect()
}

/// What an account's fetch yielded, once cache fallback has been considered.
#[derive(Debug)]
enum Served {
    /// The provider answered, and this is what it said.
    Fresh(Vec<RemoteRepo>),

    /// The provider was briefly unavailable, so a cached copy is shown instead.
    Stale {
        /// The repositories, from cache.
        data: Vec<RemoteRepo>,
        /// How old that cache is.
        age: SignedDuration,
        /// The outage that forced the fallback, to warn the user about.
        outage: GitHubError,
    },
}

/// Decides what to do with an account whose fetch has finished.
///
/// A passing GitHub outage falls back to any cached copy rather than failing
/// the whole run, so one flaky account does not hide every other. Every other
/// error, and a transient outage with no cache to fall back on, propagates.
fn serve(
    fetched: Result<Vec<RemoteRepo>, GitHubError>,
    cached: Option<Cached<Vec<RemoteRepo>>>,
    now: Timestamp,
) -> Result<Served, GitHubError> {
    match fetched {
        Ok(repositories) => Ok(Served::Fresh(repositories)),
        Err(outage) if outage.is_transient_outage() => match cached {
            Some(cached) => Ok(Served::Stale {
                age: cached.age(now),
                data: cached.data,
                outage,
            }),
            None => Err(outage),
        },
        Err(error) => Err(error),
    }
}

/// Loads configuration, then fills in the remote side from cache or provider.
async fn gather(cli: &Cli, paths: &Paths, config: &Config) -> Result<Gathered, CliError> {
    let accounts = config
        .providers
        .github
        .as_ref()
        .map(accounts_of)
        .unwrap_or_default();

    let tracked = TrackedOwners::new(
        accounts
            .iter()
            .map(|account| (Provider::GitHub, account.login().to_owned())),
    );

    let now = Timestamp::now();
    let mut remotes = Vec::new();
    let mut staleness: Option<jiff::SignedDuration> = None;
    let mut client = None;

    for account in &accounts {
        let key = format!("github-{}", account.login());

        // The cache is loaded whether or not it is fresh: a fresh entry is
        // served straight away, and a stale one is kept in hand in case the
        // fetch below runs into a GitHub outage.
        let cached = paths.cache.load::<Vec<RemoteRepo>>(&key);

        let is_fresh = !cli.refresh
            && cached
                .as_ref()
                .is_some_and(|cached| !cached.is_stale(now, config.cache.ttl));

        if is_fresh {
            let cached = cached.expect("a fresh entry was just loaded");
            let age = cached.age(now);
            staleness = Some(staleness.map_or(age, |worst| worst.max(age)));
            remotes.extend(cached.data);
            continue;
        }

        // The client is built once, and only when something actually needs
        // fetching, so a fully cached run never asks for a token.
        let client = client_or_init(&mut client)?;

        match serve(client.repositories(account).await, cached, now)? {
            Served::Fresh(fetched) => {
                paths.cache.store(&key, &fetched, now)?;
                remotes.extend(fetched);
            }
            Served::Stale { data, age, outage } => {
                eprintln!(
                    "warning: {outage}\nShowing cached repositories for `{}` from {} ago instead.",
                    account.login(),
                    describe_age(age)
                );
                staleness = Some(staleness.map_or(age, |worst| worst.max(age)));
                remotes.extend(data);
            }
        }
    }

    Ok(Gathered {
        remotes,
        tracked,
        staleness,
    })
}

/// Lists repositories with what the provider reports.
async fn list(cli: &Cli) -> Result<String, CliError> {
    let paths = paths()?;
    let config = Config::load_from(&paths.config)?;
    let gathered = gather(cli, &paths, &config).await?;

    let filter = cli.filter();
    let remotes: Vec<_> = gathered
        .remotes
        .iter()
        .filter(|repo| filter.owner_matches(&repo.id.owner) && (cli.include_forks || !repo.is_fork))
        .collect();

    if cli.json {
        return Ok(serde_json::to_string_pretty(&remotes)?);
    }

    let mut table = Table::new([
        "REPOSITORY",
        "LANGUAGE",
        "STARS",
        "ISSUES",
        "PRS",
        "LICENCE",
        "PUSHED",
    ]);

    let now = Timestamp::now();

    for repo in &remotes {
        let metadata = &repo.metadata;

        table.push([
            repo.id.to_string(),
            metadata.language.clone().unwrap_or_else(|| "-".to_owned()),
            metadata.stars.to_string(),
            metadata.open_issues.to_string(),
            metadata.open_pull_requests.to_string(),
            metadata.licence.clone().unwrap_or_else(|| "-".to_owned()),
            metadata.last_pushed.map_or_else(
                || "-".to_owned(),
                |pushed| describe_age(now.duration_since(pushed)),
            ),
        ]);
    }

    Ok(finish(&table, gathered.staleness, "repositories"))
}

/// Shows how clones stand against what the provider reports.
async fn status(cli: &Cli) -> Result<String, CliError> {
    let paths = paths()?;
    let config = Config::load_from(&paths.config)?;
    let gathered = gather(cli, &paths, &config).await?;

    let roots = config.resolved_roots(paths.home.as_deref())?;
    let scanned = scan::scan(&roots, scan::DEFAULT_MAX_DEPTH);

    let comparisons = cli.filter().apply(compare::compare(
        &gathered.remotes,
        &scanned.repositories,
        &gathered.tracked,
    ));

    if cli.json {
        return Ok(serde_json::to_string_pretty(&comparisons)?);
    }

    let mut table = Table::new(["REPOSITORY", "GROUP", "STATE", "NOTES", "PATH"]);

    for comparison in &comparisons {
        table.push([
            comparison
                .id
                .as_ref()
                .map_or_else(|| "-".to_owned(), ToString::to_string),
            comparison.group.clone().unwrap_or_else(|| "-".to_owned()),
            describe_state(&comparison.state),
            describe_notes(comparison),
            comparison
                .path
                .as_ref()
                .map_or_else(String::new, |path| path.display().to_string()),
        ]);
    }

    let mut out = finish(&table, gathered.staleness, "repositories");

    append_scan_notes(&mut out, &scanned);

    Ok(out)
}

/// Appends the roots a scan could not read, and the paths it deliberately
/// skipped, to a command's text output, so they are never inferred from a
/// short or empty result.
fn append_scan_notes(out: &mut String, scanned: &scan::Scan) {
    for failure in &scanned.failures {
        let _ = write!(out, "\n{failure}");
    }

    for link in &scanned.skipped_symlinks {
        let _ = write!(
            out,
            "\nNot following the symlink {}; move the clones out from behind it, or point a root at its target.",
            link.display()
        );
    }

    for bare in &scanned.skipped_bare {
        let _ = write!(
            out,
            "\nThe bare repository {} has no working tree, so it is not compared as a clone.",
            bare.display()
        );
    }
}

/// Where new clones should land.
///
/// A named directory is taken as given, since `--into` is the way to say where
/// outright. A named group is the directory it already occupies somewhere
/// beneath a root, or a new one beneath `root` when it holds nothing yet.
/// Naming neither clones into `root` itself.
///
/// # Errors
///
/// Returns an error when the group does not name a directory path beneath a
/// root, so `--into-group` refuses what `--to-group` refuses rather than
/// quietly building a path leading out of the tree.
fn clone_destination(
    roots: &[PathBuf],
    root: &Path,
    into: Option<PathBuf>,
    group: Option<&str>,
) -> Result<PathBuf, actions::InvalidGroupError> {
    if let Some(into) = into {
        return Ok(into);
    }

    let Some(group) = group else {
        return Ok(root.to_owned());
    };

    actions::check_group(group)?;

    Ok(directory_for_group(roots, group).unwrap_or_else(|| actions::group_path(root, group)))
}

/// Finds the directory a group already occupies beneath one of the roots.
fn directory_for_group(roots: &[PathBuf], group: &str) -> Option<PathBuf> {
    roots
        .iter()
        .map(|root| actions::group_path(root, group))
        .find(|candidate| candidate.is_dir())
}

/// Adds the cache-age note that keeps stale data honest.
fn finish(table: &Table, staleness: Option<jiff::SignedDuration>, noun: &str) -> String {
    if table.is_empty() {
        return format!("No {noun} found.");
    }

    let mut out = table.to_string();

    if let Some(age) = staleness {
        let _ = write!(
            out,
            "\nShowing cached data from {}. Use --refresh to ask GitHub again.\n",
            describe_age(age)
        );
    }

    out
}

/// States, phrased so that the reason is visible without a legend.
fn describe_state(state: &State) -> String {
    use compare::{IncomparableReason, LocalOnlyReason};

    match state {
        State::RemoteOnly => "not backed up".to_owned(),
        State::InSync => "in sync".to_owned(),
        State::Ahead { ahead } => format!("ahead {ahead}"),
        State::Behind { behind } => format!("behind {behind}"),
        State::Diverged { ahead, behind } => format!("diverged +{ahead}/-{behind}"),
        State::LocalOnly(reason) => match reason {
            LocalOnlyReason::NoRemote => "local only, no remote".to_owned(),
            LocalOnlyReason::UnsupportedHost => "local only, unsupported host".to_owned(),
            LocalOnlyReason::OwnerNotTracked => "local only, owner not configured".to_owned(),
            LocalOnlyReason::MissingRemotely => "local only, gone from GitHub".to_owned(),
        },
        State::Incomparable(reason) => match reason {
            IncomparableReason::NoCommits => "no commits".to_owned(),
            IncomparableReason::DetachedHead => "detached head".to_owned(),
            IncomparableReason::NoUpstreamBranch => "branch tracks nothing".to_owned(),
        },
    }
}

/// The flags worth showing beside a state.
fn describe_notes(comparison: &Comparison) -> String {
    let mut notes: Vec<String> = Vec::new();

    if let Some(local) = comparison.local {
        if local.dirty {
            notes.push("dirty".to_owned());
        }
        if local.untracked {
            notes.push("untracked files".to_owned());
        }
    }

    if let Some(remote) = comparison.remote {
        if remote.archived {
            notes.push("archived".to_owned());
        }
        if remote.private {
            notes.push("private".to_owned());
        }
        if remote.fork {
            notes.push("fork".to_owned());
        }
    }

    // How a fork stands against its parent is the reason to care that it is a
    // fork at all, so it is spelled out rather than left as a flag.
    if let Some(upstream) = comparison.upstream
        && upstream.is_behind()
    {
        notes.push(format!("{} behind upstream", upstream.behind));
    }

    notes.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_surface_is_well_formed() {
        Cli::command().debug_assert();
    }

    /// Driven by the shells `clap_complete` offers rather than by a list
    /// written here, so a shell added by a future version is caught rather
    /// than left with the fallback wording.
    #[test]
    fn every_shell_is_told_where_its_script_goes() {
        use clap::ValueEnum;

        for shell in clap_complete::Shell::value_variants() {
            let hint = completion_hint(*shell);

            assert!(
                hint.contains("minato"),
                "the {shell} hint never names the binary: {hint}"
            );
            assert!(
                hint.contains(COMPLETION_DOCS),
                "the {shell} hint never points at the instructions: {hint}"
            );
        }
    }

    /// The destination is the whole point of the hint, so each shell has to
    /// name its own rather than share a generic sentence.
    #[test]
    fn each_shell_names_the_destination_its_own_convention_expects() {
        use clap_complete::Shell;

        for (shell, expected) in [
            (Shell::Bash, "bash-completion/completions/minato"),
            (Shell::Zsh, "_minato"),
            (Shell::Fish, "completions/minato.fish"),
            (Shell::Elvish, "minato.elv"),
            (Shell::PowerShell, "Invoke-Expression"),
        ] {
            let hint = completion_hint(shell);

            assert!(
                hint.contains(expected),
                "the {shell} hint never names {expected}: {hint}"
            );
        }
    }

    /// zsh and elvish need a step beyond writing the file, and a hint that
    /// stopped at the path would leave completion silently not working.
    ///
    /// The lines asserted are the ones meant to be pasted, not the prose
    /// around them: naming compinit in a sentence while printing a broken
    /// command would read as correct and not work.
    #[test]
    fn the_shells_that_need_a_second_step_say_so() {
        let zsh = completion_hint(clap_complete::Shell::Zsh);

        for line in [
            "fpath=(~/.zfunc $fpath)",
            "autoload -Uz compinit && compinit",
        ] {
            assert!(
                zsh.contains(line),
                "the zsh hint never prints `{line}`: {zsh}"
            );
        }

        // The directory oh-my-zsh already has on $fpath, where the compinit
        // lines above are unnecessary.
        assert!(
            zsh.contains("~/.oh-my-zsh/custom/completions"),
            "the zsh hint never names the oh-my-zsh directory: {zsh}"
        );

        let elvish = completion_hint(clap_complete::Shell::Elvish);
        assert!(
            elvish.contains("use minato"),
            "the elvish hint never mentions loading the module: {elvish}"
        );
    }

    /// The hint goes to standard error precisely so that a redirect captures
    /// the script alone, so nothing of it may leak into the returned text.
    #[test]
    fn the_hint_stays_out_of_the_script() {
        use clap::ValueEnum;

        for shell in clap_complete::Shell::value_variants() {
            assert!(
                !completions(*shell).contains(COMPLETION_DOCS),
                "the {shell} script carries the hint that belongs on stderr"
            );
        }
    }

    #[test]
    fn a_nested_group_resolves_to_the_directory_it_already_occupies() {
        let root = tempfile::tempdir().expect("a temporary directory");
        std::fs::create_dir_all(root.path().join("perso/apps")).expect("a group directory");

        let roots = vec![PathBuf::from("/nonexistent"), root.path().to_owned()];

        assert_eq!(
            directory_for_group(&roots, "perso/apps"),
            Some(root.path().join("perso/apps"))
        );
        assert_eq!(
            directory_for_group(&roots, "perso/data"),
            None,
            "a group with no directory anywhere is left for the caller to create"
        );
    }

    #[test]
    fn a_new_group_is_created_beneath_the_first_root() {
        let roots = vec![PathBuf::from("/code")];

        assert_eq!(
            clone_destination(&roots, Path::new("/code"), None, Some("perso/apps"))
                .expect("a destination"),
            PathBuf::from("/code/perso/apps")
        );
    }

    #[test]
    fn a_named_directory_is_taken_as_given() {
        let roots = vec![PathBuf::from("/code")];

        assert_eq!(
            clone_destination(
                &roots,
                Path::new("/code"),
                Some(PathBuf::from("/elsewhere")),
                None
            )
            .expect("a destination"),
            PathBuf::from("/elsewhere")
        );

        assert_eq!(
            clone_destination(&roots, Path::new("/code"), None, None).expect("a destination"),
            PathBuf::from("/code"),
            "naming neither clones into the root itself"
        );
    }

    #[test]
    fn cloning_refuses_a_group_that_would_lead_out_of_the_root() {
        let roots = vec![PathBuf::from("/code")];

        for bad in ["../evil", "perso/../evil", "perso//apps", ""] {
            assert!(
                clone_destination(&roots, Path::new("/code"), None, Some(bad)).is_err(),
                "group `{bad}` should be refused rather than cloned into"
            );
        }
    }

    #[test]
    fn json_and_refresh_are_available_on_every_command() {
        for arguments in [
            vec!["minato", "list", "--json"],
            vec!["minato", "status", "--json"],
            vec!["minato", "status", "--refresh"],
            vec!["minato", "doctor", "--json"],
            vec!["minato", "auth", "status", "--json"],
        ] {
            let parsed = Cli::try_parse_from(&arguments)
                .unwrap_or_else(|error| panic!("{arguments:?} should parse: {error}"));

            assert!(parsed.json || parsed.refresh);
        }
    }

    #[test]
    fn the_include_flags_are_available_on_every_command() {
        for command in ["list", "status", "tui"] {
            let parsed =
                Cli::try_parse_from(["minato", command, "--include-forks", "--include-external"])
                    .unwrap_or_else(|error| panic!("{command} should parse: {error}"));

            assert!(parsed.include_forks);
            assert!(parsed.include_external);
        }
    }

    fn cached_repositories(names: &[&str], fetched_at: Timestamp) -> Cached<Vec<RemoteRepo>> {
        Cached {
            version: crate::cache::SCHEMA_VERSION,
            fetched_at,
            data: names
                .iter()
                .map(|name| RemoteRepo {
                    id: crate::model::RepoId::new(Provider::GitHub, "mcanouil", name),
                    default_branch: Some("main".to_owned()),
                    is_private: false,
                    is_archived: false,
                    is_fork: false,
                    upstream: None,
                    metadata: crate::model::Metadata::default(),
                })
                .collect(),
        }
    }

    fn now() -> Timestamp {
        "2026-07-20T12:00:00Z".parse().expect("a timestamp")
    }

    #[test]
    fn serve_shows_a_successful_fetch_without_touching_the_cache() {
        let cached = cached_repositories(&["stale"], now() - SignedDuration::from_hours(1));

        let served = serve(Ok(Vec::new()), Some(cached), now()).expect("a successful fetch");

        assert!(
            matches!(served, Served::Fresh(repositories) if repositories.is_empty()),
            "a fetch that succeeds must be shown as-is, never the cache"
        );
    }

    #[test]
    fn serve_falls_back_to_cache_when_github_has_a_passing_outage() {
        let fetched_at = now() - SignedDuration::from_hours(1);
        let cached = cached_repositories(&["one"], fetched_at);

        let outage = Err(GitHubError::Unexpected {
            account: "mcanouil".to_owned(),
            status: 502,
        });

        let Served::Stale { data, age, .. } =
            serve(outage, Some(cached), now()).expect("a fallback rather than a failure")
        else {
            panic!("a 502 with a warm cache should serve the cache");
        };

        assert_eq!(data.len(), 1, "the cached repositories must be served");
        assert_eq!(
            age,
            SignedDuration::from_hours(1),
            "the fallback must report how old the served data is"
        );
    }

    #[test]
    fn serve_fails_on_an_outage_when_there_is_nothing_cached() {
        let outage = Err(GitHubError::Unexpected {
            account: "mcanouil".to_owned(),
            status: 502,
        });

        let error = serve(outage, None, now()).expect_err("no cache means no fallback");

        assert!(matches!(error, GitHubError::Unexpected { status: 502, .. }));
    }

    #[test]
    fn serve_never_papers_over_an_error_the_user_must_fix() {
        let cached = cached_repositories(&["one"], now() - SignedDuration::from_hours(1));

        let rejected = Err(GitHubError::Unauthorised { token_source: None });

        let error = serve(rejected, Some(cached), now())
            .expect_err("a rejected token is not an outage to hide behind stale data");

        assert!(matches!(error, GitHubError::Unauthorised { .. }));
    }

    #[test]
    fn accounts_are_listed_once_regardless_of_case_or_duplication() {
        let github = config::GitHub {
            users: vec!["mcanouil".to_owned(), "MCANOUIL".to_owned()],
            orgs: vec!["mcanouil".to_owned(), "posit".to_owned()],
        };

        let logins: Vec<_> = accounts_of(&github)
            .iter()
            .map(|account| account.login().to_owned())
            .collect();

        assert_eq!(logins, ["mcanouil", "posit"]);
    }

    #[test]
    fn scan_notes_surface_failures_and_skipped_paths() {
        let scanned = scan::Scan {
            repositories: Vec::new(),
            failures: vec![scan::ScanError {
                root: PathBuf::from("/bad/root"),
                message: "no such directory".to_owned(),
            }],
            skipped_symlinks: vec![PathBuf::from("/root/projects")],
            skipped_bare: vec![PathBuf::from("/root/mirror.git")],
        };

        let mut out = String::new();
        append_scan_notes(&mut out, &scanned);

        assert!(out.contains("/bad/root"), "a failure names its root: {out}");
        assert!(
            out.contains("/root/projects"),
            "a skipped symlink is named: {out}"
        );
        assert!(
            out.contains("/root/mirror.git"),
            "a skipped bare repository is named: {out}"
        );
    }

    #[test]
    fn a_state_reads_without_needing_a_legend() {
        use compare::LocalOnlyReason;

        assert_eq!(describe_state(&State::RemoteOnly), "not backed up");
        assert_eq!(describe_state(&State::Behind { behind: 3 }), "behind 3");
        assert_eq!(
            describe_state(&State::Diverged {
                ahead: 1,
                behind: 2
            }),
            "diverged +1/-2"
        );
        assert_eq!(
            describe_state(&State::LocalOnly(LocalOnlyReason::MissingRemotely)),
            "local only, gone from GitHub"
        );
    }

    #[test]
    fn clone_accepts_a_chosen_destination() {
        let parsed = Cli::try_parse_from(["minato", "clone", "--into", "/code/perso"])
            .expect("the command to parse");

        let Command::Clone { into, .. } = parsed.command else {
            panic!("expected the clone command");
        };

        assert_eq!(into, Some(PathBuf::from("/code/perso")));
    }

    #[test]
    fn clone_falls_back_to_the_configured_root() {
        let parsed = Cli::try_parse_from(["minato", "clone"]).expect("the command to parse");

        let Command::Clone { into, .. } = parsed.command else {
            panic!("expected the clone command");
        };

        assert_eq!(into, None, "no destination means use the configured root");
    }

    #[test]
    fn a_clone_destination_and_a_group_filter_are_different_things() {
        let by_group = Cli::try_parse_from(["minato", "clone", "--into-group", "demo"])
            .expect("a group destination to parse");

        let Command::Clone { group, .. } = &by_group.command else {
            panic!("expected the clone command");
        };

        assert_eq!(group.as_deref(), Some("demo"));
        assert!(
            by_group.groups.is_empty(),
            "choosing a destination must not also filter by group"
        );

        let filtered =
            Cli::try_parse_from(["minato", "status", "--group", "demo"]).expect("a filter");

        assert_eq!(filtered.groups, ["demo"]);
    }

    #[test]
    fn a_destination_and_a_group_cannot_both_be_given() {
        assert!(
            Cli::try_parse_from(["minato", "clone", "--into", "/code", "--into-group", "demo"])
                .is_err(),
            "two ways of saying where would be ambiguous"
        );
    }

    #[test]
    fn a_batch_failure_is_carried_out_to_the_exit_code() {
        let summary = actions::Summary {
            reports: vec![actions::Report {
                id: None,
                path: None,
                outcome: actions::Outcome::Failed {
                    error: "it went wrong".to_owned(),
                },
            }],
        };

        assert!(
            summary.has_failures(),
            "a failed repository must be able to reach the exit code"
        );

        let output = Output {
            text: render_summary(&summary),
            failed: summary.has_failures(),
        };

        assert!(output.failed);
        assert!(output.text.contains("1 failed"));
    }

    #[test]
    fn a_command_that_is_not_a_batch_never_reports_a_batch_failure() {
        let output: Output = "some text".to_owned().into();

        assert!(!output.failed);
    }

    #[test]
    fn an_unknown_subcommand_is_rejected() {
        assert!(Cli::try_parse_from(["minato", "destroy-everything"]).is_err());
    }
}
