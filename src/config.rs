//! Loading, validating, and locating the configuration file.
//!
//! Configuration is plain TOML so that it can be read, edited, and diffed by
//! hand. It never holds a token: it may only name where a token comes from.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::model::RepoId;

/// Directory layout applied under a root when cloning, unless overridden.
pub const DEFAULT_LAYOUT: &str = "{owner}/{repo}";

/// The environment variable that overrides the configuration file location.
pub const CONFIG_ENV: &str = "FLEET_CONFIG";

/// A complete `fleet` configuration.
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
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.users.is_empty() && self.orgs.is_empty()
    }
}

/// Where local clones live, and how new ones are laid out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Local {
    /// Directories scanned for existing clones.
    pub roots: Vec<PathBuf>,

    /// Layout applied under the first root when cloning.
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
        /// Where `fleet` looked.
        path: PathBuf,
    },

    /// The configuration file could not be read.
    #[error("cannot read the configuration at {}: {source}", path.display())]
    Read {
        /// The file `fleet` tried to read.
        path: PathBuf,
        /// The underlying failure.
        source: io::Error,
    },

    /// The configuration file is not valid TOML, or has unexpected fields.
    #[error("cannot parse the configuration at {}: {source}", path.display())]
    Parse {
        /// The file `fleet` tried to parse.
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
    ///
    /// # Errors
    ///
    /// Returns an error when the text is not valid TOML, or contains fields
    /// `fleet` does not recognise.
    pub fn from_toml(text: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(text)
    }

    /// Checks that the configuration describes settings `fleet` can act on.
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

        if !self.local.layout.contains("{repo}") {
            return Err(ValidationError::LayoutMissingRepo {
                layout: self.local.layout.clone(),
            });
        }

        Ok(())
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
             protocol = \"ssh\"\n"
        )
    }

    /// The roots with a leading `~` expanded against `home`.
    #[must_use]
    pub fn resolved_roots(&self, home: Option<&Path>) -> Vec<PathBuf> {
        self.local
            .roots
            .iter()
            .map(|root| expand_tilde(root, home))
            .collect()
    }
}

/// Resolves where the configuration file lives.
///
/// The first of these that is set wins: the `FLEET_CONFIG` override, then
/// `XDG_CONFIG_HOME`, then `~/.config`. The same `.config` path is used on
/// every platform so that a configuration file can be moved between machines.
///
/// # Errors
///
/// Returns [`ConfigError::NoLocation`] when none of the three is available.
pub fn config_path_from(
    explicit: Option<&Path>,
    xdg_config_home: Option<&Path>,
    home: Option<&Path>,
) -> Result<PathBuf, ConfigError> {
    if let Some(explicit) = explicit {
        return Ok(explicit.to_owned());
    }

    let base = xdg_config_home
        .map(Path::to_owned)
        .or_else(|| home.map(|home| home.join(".config")))
        .ok_or(ConfigError::NoLocation)?;

    Ok(base.join("fleet").join("fleet.toml"))
}

/// Expands a leading `~` against `home`, leaving every other path untouched.
#[must_use]
pub fn expand_tilde(path: &Path, home: Option<&Path>) -> PathBuf {
    let Some(home) = home else {
        return path.to_owned();
    };

    let Ok(rest) = path.strip_prefix("~") else {
        return path.to_owned();
    };

    home.join(rest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Provider;

    fn valid_toml() -> &'static str {
        r#"
[providers.github]
users = ["mcanouil"]
orgs = ["some-org"]

[local]
roots = ["~/Projects"]
layout = "{provider}/{owner}/{repo}"
protocol = "https"

[tags]
reference = ["github:mcanouil/fleet"]
"#
    }

    #[test]
    fn parses_a_complete_configuration() {
        let config = Config::from_toml(valid_toml()).unwrap();
        let github = config.providers.github.unwrap();

        assert_eq!(github.users, ["mcanouil"]);
        assert_eq!(github.orgs, ["some-org"]);
        assert_eq!(config.local.roots, [PathBuf::from("~/Projects")]);
        assert_eq!(config.local.protocol, Protocol::Https);
        assert_eq!(
            config.tags["reference"],
            [RepoId::new(Provider::GitHub, "mcanouil", "fleet")]
        );
    }

    #[test]
    fn applies_defaults_for_omitted_settings() {
        let config = Config::from_toml(
            r#"
[providers.github]
users = ["mcanouil"]

[local]
roots = ["~/Projects"]
"#,
        )
        .unwrap();

        assert_eq!(config.local.layout, DEFAULT_LAYOUT);
        assert_eq!(config.local.protocol, Protocol::Ssh);
        assert!(config.tags.is_empty());
        assert!(config.providers.github.unwrap().orgs.is_empty());
    }

    #[test]
    fn rejects_an_unknown_field_and_names_it() {
        let error = Config::from_toml(
            r#"
[providers.github]
user = ["mcanouil"]

[local]
roots = ["~/Projects"]
"#,
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
            r#"
[providers.github]
users = ["mcanouil"]

[local]
roots = ["~/Projects"]

[tags]
reference = ["mcanouil/fleet"]
"#,
        )
        .unwrap_err();

        assert!(
            error.to_string().contains("provider"),
            "the error should explain the expected form, got: {error}"
        );
    }

    #[test]
    fn requires_a_configured_provider() {
        let config = Config::from_toml(
            r#"
[local]
roots = ["~/Projects"]
"#,
        )
        .unwrap();

        assert_eq!(config.validate(), Err(ValidationError::NoProviders));
    }

    #[test]
    fn treats_a_provider_without_accounts_as_unconfigured() {
        let config = Config::from_toml(
            r#"
[providers.github]

[local]
roots = ["~/Projects"]
"#,
        )
        .unwrap();

        assert_eq!(config.validate(), Err(ValidationError::NoProviders));
    }

    #[test]
    fn requires_at_least_one_root() {
        let config = Config::from_toml(
            r#"
[providers.github]
users = ["mcanouil"]

[local]
roots = []
"#,
        )
        .unwrap();

        assert_eq!(config.validate(), Err(ValidationError::NoRoots));
    }

    #[test]
    fn rejects_a_layout_that_would_collide() {
        let config = Config::from_toml(
            r#"
[providers.github]
users = ["mcanouil"]

[local]
roots = ["~/Projects"]
layout = "{owner}"
"#,
        )
        .unwrap();

        assert!(matches!(
            config.validate(),
            Err(ValidationError::LayoutMissingRepo { .. })
        ));
    }

    #[test]
    fn accepts_a_valid_configuration() {
        assert_eq!(Config::from_toml(valid_toml()).unwrap().validate(), Ok(()));
    }

    #[test]
    fn the_sample_configuration_is_itself_valid() {
        let config = Config::from_toml(&Config::sample()).unwrap();

        assert_eq!(config.validate(), Ok(()));
    }

    #[test]
    fn reports_a_missing_file_with_the_path_it_looked_at() {
        let path = Path::new("/nonexistent/fleet/fleet.toml");
        let error = Config::load_from(path).unwrap_err();

        assert!(matches!(error, ConfigError::NotFound { .. }));
        assert!(
            error.to_string().contains("/nonexistent/fleet/fleet.toml"),
            "the error should name the path, got: {error}"
        );
        assert!(
            error.to_string().contains("[providers.github]"),
            "the error should show a sample configuration, got: {error}"
        );
    }

    #[test]
    fn the_override_wins_over_every_other_location() {
        let path = config_path_from(
            Some(Path::new("/explicit/fleet.toml")),
            Some(Path::new("/xdg")),
            Some(Path::new("/home/user")),
        )
        .unwrap();

        assert_eq!(path, PathBuf::from("/explicit/fleet.toml"));
    }

    #[test]
    fn falls_back_from_xdg_to_the_home_directory() {
        assert_eq!(
            config_path_from(None, Some(Path::new("/xdg")), Some(Path::new("/home/user"))).unwrap(),
            PathBuf::from("/xdg/fleet/fleet.toml")
        );
        assert_eq!(
            config_path_from(None, None, Some(Path::new("/home/user"))).unwrap(),
            PathBuf::from("/home/user/.config/fleet/fleet.toml")
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
        let home = Path::new("/home/user");

        assert_eq!(
            expand_tilde(Path::new("~/Projects"), Some(home)),
            PathBuf::from("/home/user/Projects")
        );
        assert_eq!(
            expand_tilde(Path::new("~"), Some(home)),
            PathBuf::from("/home/user")
        );
        assert_eq!(
            expand_tilde(Path::new("/absolute"), Some(home)),
            PathBuf::from("/absolute")
        );
        assert_eq!(
            expand_tilde(Path::new("relative/path"), Some(home)),
            PathBuf::from("relative/path")
        );
    }

    #[test]
    fn leaves_a_tilde_alone_when_there_is_no_home_directory() {
        assert_eq!(
            expand_tilde(Path::new("~/Projects"), None),
            PathBuf::from("~/Projects")
        );
    }

    #[test]
    fn does_not_expand_a_tilde_inside_a_username() {
        let home = Path::new("/home/user");

        assert_eq!(
            expand_tilde(Path::new("~other/Projects"), Some(home)),
            PathBuf::from("~other/Projects")
        );
    }

    #[test]
    fn resolves_every_root_against_the_home_directory() {
        let config = Config::from_toml(valid_toml()).unwrap();

        assert_eq!(
            config.resolved_roots(Some(Path::new("/home/user"))),
            [PathBuf::from("/home/user/Projects")]
        );
    }
}
