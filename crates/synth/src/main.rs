//! The `synth` command: generates the synthetic Postgres corpus the tests run against.

use clap::Parser;
use synth::cli::{BenchCommand, Cli, Command};

#[expect(clippy::print_stdout, reason = "output is the point of this command")]
fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Generate(args) => {
            let dir = synth::generate::run(&args)?;
            println!("{}", dir.display());
        }
        Command::Bench(BenchCommand::Run(args)) => {
            let dir = synth::bench::run::run(&args)?;
            println!("{}", dir.display());
        }
        Command::Bench(BenchCommand::Verify(args)) => {
            if !synth::bench::verify::run(&args)? {
                std::process::exit(1);
            }
        }
        Command::ExtractLocales(args) => {
            let rows = synth::locales::extract(&args.out)?;
            println!("wrote {rows} translations to {}", args.out.display());
        }
    }
    Ok(())
}
