use clap::Parser;

/// Overview and sync of Git repositories across hosting providers.
#[derive(Debug, Parser)]
#[command(name = "fleet", version, about)]
struct Cli {}

fn main() {
    let Cli {} = Cli::parse();
}
