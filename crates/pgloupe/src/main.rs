use std::process::ExitCode;

use clap::Parser as _;
use pgloupe::cli::{Cli, run};

fn main() -> ExitCode {
    match run(&Cli::parse()) {
        Ok(output) => {
            print!("{output}");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}
