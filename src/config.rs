//! Loading, validating, and locating the configuration file.
//!
//! Configuration is plain TOML so that it can be read, edited, and diffed by
//! hand. It never holds a token: it may only name where a token comes from.
//!
//! [`Config`] mirrors the file verbatim, `~` and all, so that a command which
//! edits configuration can write it back without machine-locking a path the
//! user wrote as portable. Expansion happens on the way out, through
//! [`Config::resolved_roots`].

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use jiff::SignedDuration;
use serde::{Deserialize, Serialize};

use crate::model::RepoId;

/// Directory layout applied beneath the directory a clone is placed in.
///
/// The default is a flat name, because where a repository belongs is a human
/// judgement that its identity does not carry. A clone is filed under a chosen
/// directory, not under a tree derived from its owner.
pub const DEFAULT_LAYOUT: &str = "{repo}";

/// The environment variable that overrides the configuration file location.
pub const CONFIG_ENV: &str = "MINATO_CONFIG";

/// Placeholders a layout may use.
const LAYOUT_PLACEHOLDERS: [&str; 3] = ["provider", "owner", "repo"];

/// A complete `minato` configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// The providers to enumerate repositories from.
    #[serde(default)]
    pub providers: Providers,

    /// Where local clones live, and how new ones are laid out.
    pub local: Local,

    /// User-defined groupings, keyed by tag name.
    #[serde(default)]
    pub tags: BTreeMap<String, Vec<RepoId>>,

    /// How the cached copy of provider data is kept fresh.
    #[serde(default)]
    pub cache: CacheSettings,
}

/// Per-provider settings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Providers {
    /// GitHub settings, absent when GitHub is not configured.
    pub github: Option<GitHub>,
}

/// Which GitHub accounts to enumerate repositories for.
///
/// The token is deliberately absent: it is resolved at run time from the
/// environment, the `gh` CLI, or the operating system keyring.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitHub {
    /// Users whose repositories are tracked.
    #[serde(default)]
    pub users: Vec<String>,

    /// Organisations whose repositories are tracked.
    #[serde(default)]
    pub orgs: Vec<String>,
}

impl GitHub {
    /// Whether any account is configured.
    fn is_empty(&self) -> bool {
        self.users.is_empty() && self.orgs.is_empty()
    }
}

/// Where local clones live, and how new ones are laid out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Local {
    /// Directories scanned for existing clones, exactly as written.
    pub roots: Vec<PathBuf>,

    /// Layout applied beneath the directory a clone is placed in.
    #[serde(default = "default_layout")]
    pub layout: String,

    /// Protocol used for new clones.
    #[serde(default)]
    pub protocol: Protocol,
}

fn default_layout() -> String {
    DEFAULT_LAYOUT.to_owned()
}

/// The protocol used when cloning.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    /// Clone over SSH, using the existing SSH agent.
    #[default]
    Ssh,
    /// Clone over HTTPS, using the existing credential helper.
    Https,
}

/// How long the cached copy of provider data stays fresh.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CacheSettings {
    /// How long cached data stays fresh before a run refetches it.
    ///
    /// Written as a friendly duration, for example `"15m"` or `"1h30m"`. A
    /// zero duration refetches on every run.
    #[serde(default = "default_ttl")]
    pub ttl: SignedDuration,
}

impl Default for CacheSettings {
    fn default() -> Self {
        Self { ttl: default_ttl() }
    }
}

fn default_ttl() -> SignedDuration {
    crate::cache::DEFAULT_TTL
}

/// A configuration that parsed but does not describe usable settings.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ValidationError {
    /// No provider has any account configured.
    #[error(
        "no provider is configured, so there is nothing to compare against; add at least one account, for example:\n\n[providers.github]\nusers = [\"your-username\"]"
    )]
    NoProviders,

    /// No local root was given, so no clone could ever be found.
    #[error(
        "`local.roots` is empty, so no local clone can be found; add at least one directory, for example:\n\n[local]\nroots = [\"~/Projects\"]"
    )]
    NoRoots,

    /// The layout would place every repository at the same path.
    #[error(
        "`local.layout` must contain `{{repo}}` so that each repository gets its own directory; got `{layout}`"
    )]
    LayoutMissingRepo {
        /// The offending layout.
        layout: String,
    },

    /// The layout used a placeholder Minato does not substitute.
    #[error(
        "`local.layout` uses `{{{placeholder}}}`, which is not a placeholder Minato knows; use any of {}",
        known_placeholders()
    )]
    LayoutUnknownPlaceholder {
        /// The unrecognised placeholder, without its braces.
        placeholder: String,
    },

    /// The cache lifetime was written as a negative duration.
    #[error("`cache.ttl` must not be negative; got {ttl}")]
    NegativeCacheTtl {
        /// The offending duration.
        ttl: SignedDuration,
    },
}

fn known_placeholders() -> String {
    LAYOUT_PLACEHOLDERS
        .iter()
        .map(|placeholder| format!("`{{{placeholder}}}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A root that cannot be expanded because no home directory is known.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "cannot expand `~` in `{}` because no home directory is set; write an absolute path in `local.roots`, or set HOME",
    root.display()
)]
pub struct UnresolvedRootError {
    /// The root that could not be expanded.
    pub root: PathBuf,
}

/// Anything that can go wrong between asking for the configuration and holding
/// a valid one.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// No configuration file exists at the resolved location.
    #[error(
        "no configuration file at {}\n\nCreate it with:\n\n{}",
        path.display(),
        Config::sample()
    )]
    NotFound {
        /// Where `minato` looked.
        path: PathBuf,
    },

    /// The configuration file could not be read.
    #[error("cannot read the configuration at {}: {source}", path.display())]
    Read {
        /// The file `minato` tried to read.
        path: PathBuf,
        /// The underlying failure.
        source: io::Error,
    },

    /// The configuration file is not valid TOML, or has unexpected fields.
    #[error("cannot parse the configuration at {}: {source}", path.display())]
    Parse {
        /// The file `minato` tried to parse.
        path: PathBuf,
        /// The underlying failure.
        source: toml::de::Error,
    },

    /// The configuration parsed but does not describe usable settings.
    #[error("the configuration at {} is not usable: {source}", path.display())]
    Invalid {
        /// The offending file.
        path: PathBuf,
        /// What was wrong with it.
        source: ValidationError,
    },

    /// The configuration location could not be determined at all.
    #[error(
        "cannot determine where the configuration lives, because neither {CONFIG_ENV}, XDG_CONFIG_HOME, nor a home directory is set; set {CONFIG_ENV} to the path of your configuration file"
    )]
    NoLocation,
}

impl Config {
    /// Parses a configuration from TOML text, without validating it.
    pub(crate) fn from_toml(text: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(text)
    }

    /// Checks that the configuration describes settings `minato` can act on.
    ///
    /// # Errors
    ///
    /// Returns an error naming the first unusable setting found.
    pub fn validate(&self) -> Result<(), ValidationError> {
        let has_provider = self
            .providers
            .github
            .as_ref()
            .is_some_and(|github| !github.is_empty());

        if !has_provider {
            return Err(ValidationError::NoProviders);
        }

        if self.local.roots.is_empty() {
            return Err(ValidationError::NoRoots);
        }

        if self.cache.ttl.is_negative() {
            return Err(ValidationError::NegativeCacheTtl {
                ttl: self.cache.ttl,
            });
        }

        validate_layout(&self.local.layout)
    }

    /// Reads and validates the configuration at `path`.
    ///
    /// # Errors
    ///
    /// Returns an error when the file is missing, unreadable, malformed, or
    /// describes unusable settings. Every variant names the path involved.
    pub fn load_from(path: &Path) -> Result<Self, ConfigError> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                return Err(ConfigError::NotFound {
                    path: path.to_owned(),
                });
            }
            Err(source) => {
                return Err(ConfigError::Read {
                    path: path.to_owned(),
                    source,
                });
            }
        };

        let config = Self::from_toml(&text).map_err(|source| ConfigError::Parse {
            path: path.to_owned(),
            source,
        })?;

        config.validate().map_err(|source| ConfigError::Invalid {
            path: path.to_owned(),
            source,
        })?;

        Ok(config)
    }

    /// A commented starting configuration, shown when none exists.
    #[must_use]
    pub fn sample() -> String {
        format!(
            "[providers.github]\n\
             users = [\"your-username\"]\n\
             orgs = []\n\
             \n\
             [local]\n\
             roots = [\"~/Projects\"]\n\
             layout = \"{DEFAULT_LAYOUT}\"\n\
             protocol = \"ssh\"\n\
             \n\
             [cache]\n\
             ttl = \"15m\"\n"
        )
    }

    /// The roots with a leading `~` expanded against `home`.
    ///
    /// This is the only form a scan should consume, since an unexpanded `~` is
    /// a directory name rather than the home directory.
    ///
    /// # Errors
    ///
    /// Returns an error naming the offending root when it starts with `~` and
    /// no home directory is known, rather than silently scanning a literal `~`.
    pub fn resolved_roots(
        &self,
        home: Option<&Path>,
    ) -> Result<ResolvedRoots, UnresolvedRootError> {
        self.local
            .roots
            .iter()
            .map(|root| expand_tilde(root, home))
            .collect::<Result<Vec<_>, _>>()
            .map(ResolvedRoots::from_resolved)
    }
}

/// Roots with every leading `~` expanded, the only form a scan may consume.
///
/// A scan reads these paths as written, so an unexpanded `~` would name a
/// directory rather than the home directory. Building this type is where that
/// expansion is asserted: [`Config::resolved_roots`] performs it, and
/// [`ResolvedRoots::from_resolved`] wraps paths a caller has already resolved.
/// [`scan`](crate::scan::scan) accepts nothing else, so handing it the verbatim
/// [`Config`] roots cannot compile.
#[derive(Debug, PartialEq, Eq)]
pub struct ResolvedRoots(Vec<PathBuf>);

impl ResolvedRoots {
    /// Wraps roots the caller has already expanded to concrete paths.
    #[must_use]
    pub fn from_resolved(roots: Vec<PathBuf>) -> Self {
        Self(roots)
    }
}

impl std::ops::Deref for ResolvedRoots {
    type Target = [PathBuf];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// Checks a layout for a missing or unrecognised placeholder.
fn validate_layout(layout: &str) -> Result<(), ValidationError> {
    for placeholder in placeholders(layout) {
        if !LAYOUT_PLACEHOLDERS.contains(&placeholder) {
            return Err(ValidationError::LayoutUnknownPlaceholder {
                placeholder: placeholder.to_owned(),
            });
        }
    }

    if !layout.contains("{repo}") {
        return Err(ValidationError::LayoutMissingRepo {
            layout: layout.to_owned(),
        });
    }

    Ok(())
}

/// The placeholder names in `layout`, without their braces.
fn placeholders(layout: &str) -> impl Iterator<Item = &str> {
    layout
        .split('{')
        .skip(1)
        .filter_map(|rest| rest.split_once('}'))
        .map(|(placeholder, _)| placeholder)
}

/// Resolves where the configuration file lives.
///
/// The first of these that is set wins: the `MINATO_CONFIG` override, then
/// `XDG_CONFIG_HOME`, then `~/.config`. The same `.config` path is used on
/// every platform so that a configuration file can be moved between machines.
/// An override set to an empty value counts as unset, since an exported but
/// empty variable is a common accident.
///
/// # Errors
///
/// Returns [`ConfigError::NoLocation`] when none of the three is available.
pub fn config_path_from(
    explicit: Option<&Path>,
    xdg_config_home: Option<&Path>,
    home: Option<&Path>,
) -> Result<PathBuf, ConfigError> {
    if let Some(explicit) = explicit.filter(|path| !path.as_os_str().is_empty()) {
        return Ok(explicit.to_owned());
    }

    let base = xdg_config_home
        .filter(|path| !path.as_os_str().is_empty())
        .map(Path::to_owned)
        .or_else(|| home.map(|home| home.join(".config")))
        .ok_or(ConfigError::NoLocation)?;

    Ok(base.join("minato").join("minato.toml"))
}

/// Expands a leading `~` against `home`, leaving every other path untouched.
fn expand_tilde(path: &Path, home: Option<&Path>) -> Result<PathBuf, UnresolvedRootError> {
    let Ok(rest) = path.strip_prefix("~") else {
        return Ok(path.to_owned());
    };

    home.map(|home| home.join(rest))
        .ok_or_else(|| UnresolvedRootError {
            root: path.to_owned(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Provider;

    /// A configuration with `local` and `tags` sections appended to a valid
    /// provider section, so each test states only what it is about.
    fn config_with(rest: &str) -> Config {
        Config::from_toml(&format!(
            "[providers.github]\nusers = [\"mcanouil\"]\n\n{rest}"
        ))
        .expect("the fixture to parse")
    }

    #[test]
    fn parses_a_complete_configuration() {
        let config = Config::from_toml(
            r#"
[providers.github]
users = ["mcanouil"]
orgs = ["some-org"]

[local]
roots = ["~/Projects"]
layout = "{provider}/{owner}/{repo}"
protocol = "https"

[tags]
reference = ["github:mcanouil/minato"]
"#,
        )
        .unwrap();
        let github = config.providers.github.unwrap();

        assert_eq!(github.users, ["mcanouil"]);
        assert_eq!(github.orgs, ["some-org"]);
        assert_eq!(config.local.roots, [PathBuf::from("~/Projects")]);
        assert_eq!(config.local.protocol, Protocol::Https);
        assert_eq!(
            config.tags["reference"],
            [RepoId::new(Provider::GitHub, "mcanouil", "minato")]
        );
    }

    #[test]
    fn applies_defaults_for_omitted_settings() {
        let config = config_with("[local]\nroots = [\"~/Projects\"]\n");

        assert_eq!(config.local.layout, DEFAULT_LAYOUT);
        assert_eq!(config.local.protocol, Protocol::Ssh);
        assert!(config.tags.is_empty());
        assert!(config.providers.github.unwrap().orgs.is_empty());
        assert_eq!(config.cache.ttl, crate::cache::DEFAULT_TTL);
    }

    #[test]
    fn reads_a_configured_cache_lifetime() {
        let config = config_with("[local]\nroots = [\"~/P\"]\n\n[cache]\nttl = \"2h\"\n");

        assert_eq!(config.cache.ttl, SignedDuration::from_hours(2));
    }

    #[test]
    fn rejects_a_negative_cache_lifetime() {
        let config = config_with("[local]\nroots = [\"~/P\"]\n\n[cache]\nttl = \"-5m\"\n");

        assert_eq!(
            config.validate(),
            Err(ValidationError::NegativeCacheTtl {
                ttl: SignedDuration::from_mins(-5),
            })
        );
    }

    #[test]
    fn rejects_an_unknown_field_and_names_it() {
        let error = Config::from_toml(
            "[providers.github]\nuser = [\"mcanouil\"]\n\n[local]\nroots = [\"~/Projects\"]\n",
        )
        .unwrap_err();

        assert!(
            error.to_string().contains("user"),
            "the error should name the offending field, got: {error}"
        );
    }

    #[test]
    fn rejects_a_malformed_repository_in_tags() {
        let error = Config::from_toml(
            "[providers.github]\nusers = [\"mcanouil\"]\n\n[local]\nroots = [\"~/P\"]\n\n[tags]\nreference = [\"mcanouil/minato\"]\n",
        )
        .unwrap_err();

        assert!(
            error.to_string().contains("provider"),
            "the error should explain the expected form, got: {error}"
        );
    }

    #[test]
    fn requires_a_configured_provider() {
        let config = Config::from_toml("[local]\nroots = [\"~/Projects\"]\n").unwrap();

        assert_eq!(config.validate(), Err(ValidationError::NoProviders));
    }

    #[test]
    fn treats_a_provider_without_accounts_as_unconfigured() {
        let config =
            Config::from_toml("[providers.github]\n\n[local]\nroots = [\"~/Projects\"]\n").unwrap();

        assert_eq!(config.validate(), Err(ValidationError::NoProviders));
    }

    #[test]
    fn requires_at_least_one_root() {
        let config = config_with("[local]\nroots = []\n");

        assert_eq!(config.validate(), Err(ValidationError::NoRoots));
    }

    #[test]
    fn rejects_a_layout_that_would_collide() {
        let config = config_with("[local]\nroots = [\"~/P\"]\nlayout = \"{owner}\"\n");

        assert!(matches!(
            config.validate(),
            Err(ValidationError::LayoutMissingRepo { .. })
        ));
    }

    #[test]
    fn rejects_a_mistyped_placeholder_rather_than_creating_it_as_a_directory() {
        let config = config_with("[local]\nroots = [\"~/P\"]\nlayout = \"{onwer}/{repo}\"\n");

        let error = config.validate().unwrap_err();

        assert!(matches!(
            error,
            ValidationError::LayoutUnknownPlaceholder { ref placeholder } if placeholder == "onwer"
        ));
        assert!(
            error.to_string().contains("{owner}"),
            "the error should list the placeholders that do exist, got: {error}"
        );
    }

    #[test]
    fn accepts_every_documented_placeholder() {
        let config =
            config_with("[local]\nroots = [\"~/P\"]\nlayout = \"{provider}/{owner}/{repo}\"\n");

        assert_eq!(config.validate(), Ok(()));
    }

    #[test]
    fn the_override_wins_over_every_other_location() {
        let path = config_path_from(
            Some(Path::new("/explicit/minato.toml")),
            Some(Path::new("/xdg")),
            Some(Path::new("/home/user")),
        )
        .unwrap();

        assert_eq!(path, PathBuf::from("/explicit/minato.toml"));
    }

    #[test]
    fn treats_an_empty_override_as_unset() {
        let path =
            config_path_from(Some(Path::new("")), None, Some(Path::new("/home/user"))).unwrap();

        assert_eq!(path, PathBuf::from("/home/user/.config/minato/minato.toml"));
    }

    #[test]
    fn falls_back_from_xdg_to_the_home_directory() {
        assert_eq!(
            config_path_from(None, Some(Path::new("/xdg")), Some(Path::new("/home/user"))).unwrap(),
            PathBuf::from("/xdg/minato/minato.toml")
        );
        assert_eq!(
            config_path_from(None, None, Some(Path::new("/home/user"))).unwrap(),
            PathBuf::from("/home/user/.config/minato/minato.toml")
        );
    }

    #[test]
    fn reports_when_no_location_can_be_determined() {
        assert!(matches!(
            config_path_from(None, None, None),
            Err(ConfigError::NoLocation)
        ));
    }

    #[test]
    fn expands_a_leading_tilde_only() {
        let home = Some(Path::new("/home/user"));

        assert_eq!(
            expand_tilde(Path::new("~/Projects"), home).unwrap(),
            PathBuf::from("/home/user/Projects")
        );
        assert_eq!(
            expand_tilde(Path::new("~"), home).unwrap(),
            PathBuf::from("/home/user")
        );
        assert_eq!(
            expand_tilde(Path::new("/absolute"), home).unwrap(),
            PathBuf::from("/absolute")
        );
        assert_eq!(
            expand_tilde(Path::new("relative/path"), home).unwrap(),
            PathBuf::from("relative/path")
        );
    }

    #[test]
    fn does_not_expand_a_tilde_inside_a_username() {
        assert_eq!(
            expand_tilde(Path::new("~other/Projects"), Some(Path::new("/home/user"))).unwrap(),
            PathBuf::from("~other/Projects")
        );
    }

    #[test]
    fn refuses_to_scan_a_literal_tilde_when_there_is_no_home_directory() {
        let config = config_with("[local]\nroots = [\"~/Projects\"]\n");

        let error = config.resolved_roots(None).unwrap_err();

        assert_eq!(error.root, PathBuf::from("~/Projects"));
        assert!(
            error.to_string().contains("absolute path"),
            "the error should say what to do instead, got: {error}"
        );
    }

    #[test]
    fn resolves_every_root_against_the_home_directory() {
        let config = config_with("[local]\nroots = [\"~/Projects\", \"/opt/code\"]\n");

        assert_eq!(
            config
                .resolved_roots(Some(Path::new("/home/user")))
                .unwrap(),
            ResolvedRoots::from_resolved(vec![
                PathBuf::from("/home/user/Projects"),
                PathBuf::from("/opt/code")
            ])
        );
    }

    #[test]
    fn keeps_the_configuration_verbatim_so_it_can_be_written_back() {
        let config = config_with("[local]\nroots = [\"~/Projects\"]\n");

        config
            .resolved_roots(Some(Path::new("/home/user")))
            .unwrap();

        assert_eq!(
            config.local.roots,
            [PathBuf::from("~/Projects")],
            "resolving must not rewrite the portable path the user wrote"
        );
    }
}
