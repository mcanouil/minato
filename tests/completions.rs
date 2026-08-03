//! The completion scripts the binary prints.
//!
//! These go through the command surface exactly as the binary does, rather
//! than calling the generator directly, so what is asserted is what a user
//! redirects into their shell.

use clap::{Parser, ValueEnum};
use minato::cli::{self, Cli};

/// Runs a command as the binary would, returning what it would print.
async fn output_of(arguments: &[&str]) -> String {
    let cli = Cli::parse_from(arguments);

    cli::run(&cli)
        .await
        .expect("the command to produce output")
        .text
}

/// Driven by the shells `clap_complete` offers rather than by a list written
/// here, so a shell added by a future version is covered rather than silently
/// missed.
#[tokio::test]
async fn every_shell_generates_a_script_naming_the_binary_and_its_commands() {
    for shell in clap_complete::Shell::value_variants() {
        let script = output_of(&["minato", "completions", &shell.to_string()]).await;

        assert!(!script.trim().is_empty(), "{shell} produced no script");
        assert!(
            script.contains("minato"),
            "{shell} produced a script that never names the binary"
        );
        // A generator that emitted a valid but empty skeleton would satisfy
        // everything above, so require a subcommand only this binary has.
        assert!(
            script.contains("sync-fork"),
            "{shell} produced a script that offers no subcommands"
        );
    }
}

/// Flag values that derive `ValueEnum` complete on their own, and `--state` is
/// the one users reach for most, so its values reaching the script is the
/// difference between completing a filter and typing it out.
///
/// Only bash, zsh, and fish carry them, for the reason the command reference
/// gives.
#[tokio::test]
async fn state_values_reach_the_shells_that_carry_them() {
    for shell in ["bash", "zsh", "fish"] {
        let script = output_of(&["minato", "completions", shell]).await;

        for state in ["not-cloned", "in-sync", "diverged", "drifted"] {
            assert!(
                script.contains(state),
                "the {shell} script never offers {state}"
            );
        }
    }
}

#[test]
fn an_unknown_shell_is_refused_with_the_ones_that_work() {
    let error = Cli::try_parse_from(["minato", "completions", "nonsense"])
        .expect_err("an unknown shell to be rejected");
    let message = error.to_string();

    assert!(message.contains("nonsense"), "{message}");
    assert!(
        message.contains("bash") && message.contains("zsh"),
        "{message}"
    );
}
