use std::process::ExitCode;

use minato::cli;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = cli::parse();

    match cli::run(&cli).await {
        Ok(output) => {
            println!("{}", output.text.trim_end());

            // A batch that reports failures has still run, so its output is
            // printed, but the exit code must say that something went wrong.
            if output.failed {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            }
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
