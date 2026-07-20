//! Overview and sync of Git repositories across hosting providers.
//!
//! The binary in `src/main.rs` is a thin wrapper over this library, so that the
//! comparison, cache, and configuration logic stays reachable from integration
//! tests in `tests/` rather than only through the process boundary.

pub mod cache;
pub mod compare;
pub mod config;
pub mod git;
pub mod github;
pub mod model;
pub mod scan;

use clap::Parser;

/// Overview and sync of Git repositories across hosting providers.
#[derive(Debug, Parser)]
#[command(version)]
pub struct Cli {}
