use clap::Parser;
use synth::cli::{Cli, Command};

fn main() {
    let Command::Generate(args) = Cli::parse().command;
    println!("{}", args.run_dir().display());
}
