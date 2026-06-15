use clap::Parser;
use gitcommitgenerator::{Cli, resolve_config, run};
use std::process::ExitCode;

fn main() -> ExitCode {
    let cli = Cli::parse();

    match resolve_config(cli).and_then(|config| run(&config)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Error: {error:#}");
            ExitCode::FAILURE
        }
    }
}
