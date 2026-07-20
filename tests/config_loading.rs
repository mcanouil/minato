//! Loading configuration from a real file on disk.
//!
//! These exercise the library exactly as the binary does, through its public
//! surface, rather than reaching into private internals.

use std::fs;

use minato::config::{Config, ConfigError, Protocol, ValidationError};
use minato::model::{Provider, RepoId};

fn write_config(contents: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let path = dir.path().join("minato.toml");
    fs::write(&path, contents).expect("the configuration to be written");

    (dir, path)
}

#[test]
fn loads_a_configuration_from_disk() {
    let (_dir, path) = write_config(
        r#"
[providers.github]
users = ["mcanouil"]

[local]
roots = ["~/Projects"]
protocol = "https"

[tags]
reference = ["github:mcanouil/minato"]
"#,
    );

    let config = Config::load_from(&path).expect("a valid configuration");

    assert_eq!(config.local.protocol, Protocol::Https);
    assert_eq!(
        config.tags["reference"],
        [RepoId::new(Provider::GitHub, "mcanouil", "minato")]
    );
}

#[test]
fn rejects_a_configuration_that_parses_but_cannot_be_acted_on() {
    let (_dir, path) = write_config(
        r#"
[local]
roots = ["~/Projects"]
"#,
    );

    let error = Config::load_from(&path).expect_err("a validation failure");

    assert!(matches!(
        error,
        ConfigError::Invalid {
            source: ValidationError::NoProviders,
            ..
        }
    ));
    assert!(
        error.to_string().contains(&path.display().to_string()),
        "the error should name the offending file, got: {error}"
    );
}

#[test]
fn reports_malformed_toml_against_the_file_it_came_from() {
    let (_dir, path) = write_config("this is not toml");

    let error = Config::load_from(&path).expect_err("a parse failure");

    assert!(matches!(error, ConfigError::Parse { .. }));
    assert!(
        error.to_string().contains(&path.display().to_string()),
        "the error should name the offending file, got: {error}"
    );
}

#[test]
fn the_sample_configuration_round_trips_through_a_file() {
    let (_dir, path) = write_config(&Config::sample());

    Config::load_from(&path).expect("the sample configuration to be loadable");
}
