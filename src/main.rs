use std::process::ExitCode;

use clap::Parser;
use minato::cli::{self, Cli};

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();

    match cli::run(&cli).await {
        Ok(output) => {
            println!("{}", output.trim_end());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("minato: {error}");

            // Print the chain, so a failure names its underlying cause rather
            // than only its outermost description.
            let mut source = std::error::Error::source(&error);
            while let Some(cause) = source {
                eprintln!("  caused by: {cause}");
                source = cause.source();
            }

            ExitCode::FAILURE
        }
    }
}
