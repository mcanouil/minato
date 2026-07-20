//! The command surface.
//!
//! Every capability lands here first, so a person, a script, or an agent can
//! reach any outcome without an interactive interface. Each command offers a
//! table for reading and `--json` for consuming.

mod render;

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use jiff::Timestamp;
use serde::Serialize;
use std::fmt::Write as _;

use crate::cache::{Cache, DEFAULT_TTL};
use crate::compare::{self, Comparison, State, TrackedOwners};
use crate::config::{self, Config};
use crate::github::auth;
use crate::github::{Account, GitHubClient};
use crate::model::{Provider, RemoteRepo};
use crate::scan;

use render::{Table, describe_age};

/// Overview and sync of Git repositories across hosting providers.
#[derive(Debug, Parser)]
#[command(name = "minato", version, about)]
pub struct Cli {
    /// Emit JSON instead of a table.
    #[arg(long, global = true)]
    pub json: bool,

    /// Ignore cached data and ask the provider again.
    #[arg(long, global = true)]
    pub refresh: bool,

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

    /// Discard cached data so the next run asks the provider again.
    Refresh,

    /// Inspect authentication.
    Auth {
        #[command(subcommand)]
        command: AuthCommand,
    },

    /// Check that configuration and tooling are usable.
    Doctor,
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

    /// Output could not be rendered.
    #[error("cannot render JSON output: {0}")]
    Json(#[from] serde_json::Error),
}

/// Runs a command, returning what to print.
///
/// # Errors
///
/// Returns an error when configuration, authentication, the provider, or the
/// cache prevents an answer. Every variant names what to do next.
pub async fn run(cli: &Cli) -> Result<String, CliError> {
    match &cli.command {
        Command::Auth {
            command: AuthCommand::Status,
        } => Ok(auth_status(cli.json)),
        Command::Doctor => doctor(cli.json),
        Command::Refresh => refresh(cli.json),
        Command::List => list(cli).await,
        Command::Status => status(cli).await,
    }
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

/// Everything gathered for a run, and how fresh it was.
struct Gathered {
    remotes: Vec<RemoteRepo>,
    tracked: TrackedOwners,
    /// How old the oldest cached entry used was, absent when nothing was
    /// served from cache.
    staleness: Option<jiff::SignedDuration>,
}

/// Loads configuration, then fills in the remote side from cache or provider.
async fn gather(cli: &Cli, paths: &Paths, config: &Config) -> Result<Gathered, CliError> {
    let accounts: Vec<Account> = config
        .providers
        .github
        .iter()
        .flat_map(|github| github.users.iter().chain(github.orgs.iter()))
        .map(Account::new)
        .collect();

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

        if !cli.refresh {
            if let Some(cached) = paths.cache.load::<Vec<RemoteRepo>>(&key) {
                if !cached.is_stale(now, DEFAULT_TTL) {
                    let age = cached.age(now);
                    staleness = Some(staleness.map_or(age, |worst| worst.max(age)));
                    remotes.extend(cached.data);
                    continue;
                }
            }
        }

        // The client is built once, and only when something actually needs
        // fetching, so a fully cached run never asks for a token.
        let client = if let Some(client) = &client {
            client
        } else {
            let (token, source) = auth::resolve_token_from_system()?;
            client.insert(GitHubClient::new(token, Some(source))?)
        };

        let fetched = client.repositories(account).await?;
        paths.cache.store(&key, &fetched, now)?;
        remotes.extend(fetched);
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

    if cli.json {
        return Ok(serde_json::to_string_pretty(&gathered.remotes)?);
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

    for repo in &gathered.remotes {
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

    let comparisons = compare::compare(&gathered.remotes, &scanned.repositories, &gathered.tracked);

    if cli.json {
        return Ok(serde_json::to_string_pretty(&comparisons)?);
    }

    let mut table = Table::new(["REPOSITORY", "STATE", "NOTES", "PATH"]);

    for comparison in &comparisons {
        table.push([
            comparison
                .id
                .as_ref()
                .map_or_else(|| "-".to_owned(), ToString::to_string),
            describe_state(&comparison.state),
            describe_notes(comparison),
            comparison
                .path
                .as_ref()
                .map_or_else(String::new, |path| path.display().to_string()),
        ]);
    }

    let mut out = finish(&table, gathered.staleness, "repositories");

    for failure in &scanned.failures {
        let _ = write!(out, "\n{failure}");
    }

    Ok(out)
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
        State::RemoteOnly => "not cloned".to_owned(),
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
    let mut notes = Vec::new();

    if let Some(local) = comparison.local {
        if local.dirty {
            notes.push("dirty");
        }
        if local.untracked {
            notes.push("untracked files");
        }
    }

    if let Some(remote) = comparison.remote {
        if remote.archived {
            notes.push("archived");
        }
        if remote.private {
            notes.push("private");
        }
        if remote.fork {
            notes.push("fork");
        }
    }

    notes.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn the_command_surface_is_well_formed() {
        Cli::command().debug_assert();
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
    fn a_state_reads_without_needing_a_legend() {
        use compare::LocalOnlyReason;

        assert_eq!(describe_state(&State::RemoteOnly), "not cloned");
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
    fn an_unknown_subcommand_is_rejected() {
        assert!(Cli::try_parse_from(["minato", "destroy-everything"]).is_err());
    }
}
