use clap::Parser;
use synth::cli::{Cli, Command};

fn main() -> anyhow::Result<()> {
    let Command::Generate(args) = Cli::parse().command;
    let dir = synth::generate::run(&args)?;
    println!("{}", dir.display());
    Ok(())
}
