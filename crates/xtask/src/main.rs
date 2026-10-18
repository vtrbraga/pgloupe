//! `cargo xtask generate` writes the man pages, shell completions and command reference;
//! `cargo xtask check` fails when the files in the repository are out of date;
//! `cargo xtask bench` writes benchmark logs and times pgloupe on them.

mod bench;

use std::{
    collections::BTreeMap,
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
    process::ExitCode,
};

use anyhow::{Context as _, bail};
use clap::{Command, CommandFactory as _};
use clap_complete::Shell;
use pgloupe::cli::Cli;

/// Files to ship in the release archives.
const ASSETS: &str = "dist-assets";
/// The generated page of the book.
const COMMANDS_PAGE: &str = "docs/src/reference/commands.md";

#[expect(clippy::print_stderr, reason = "output is the point of this command")]
fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> anyhow::Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    match std::env::args().nth(1).as_deref() {
        Some("generate") => {
            for (path, bytes) in outputs()? {
                let target = root.join(&path);
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::write(&target, bytes).with_context(|| format!("writing {}", path.display()))?;
            }
            Ok(())
        }
        Some("check") => {
            let stale: Vec<String> = outputs()?
                .into_iter()
                .filter(|(path, bytes)| fs::read(root.join(path)).ok().as_ref() != Some(bytes))
                .map(|(path, _)| path.display().to_string())
                .collect();
            if stale.is_empty() {
                Ok(())
            } else {
                bail!(
                    "out of date, run `cargo xtask generate`: {}",
                    stale.join(", ")
                )
            }
        }
        Some("bench") => bench::run(&root, &std::env::args().skip(2).collect::<Vec<_>>()),
        _ => bail!("usage: cargo xtask <generate|check|bench>"),
    }
}

/// Every generated file, by path from the repository root.
fn outputs() -> anyhow::Result<BTreeMap<PathBuf, Vec<u8>>> {
    let mut files = BTreeMap::new();
    let mut command = Cli::command();
    command.build();

    // One man page per command: `pgloupe.1`, `pgloupe-report.1`, ...
    for (name, command) in commands(&command) {
        let mut page = Vec::new();
        clap_mangen::Man::new(command).render(&mut page)?;
        files.insert(
            Path::new(ASSETS).join(format!("man/{}.1", name.replace(' ', "-"))),
            page,
        );
    }

    for shell in [Shell::Bash, Shell::Zsh, Shell::Fish, Shell::PowerShell] {
        let mut script = Vec::new();
        clap_complete::generate(shell, &mut Cli::command(), "pgloupe", &mut script);
        let file = match shell {
            Shell::Bash => "pgloupe.bash",
            Shell::Zsh => "_pgloupe",
            Shell::Fish => "pgloupe.fish",
            _ => "_pgloupe.ps1",
        };
        files.insert(Path::new(ASSETS).join("completions").join(file), script);
    }

    files.insert(
        PathBuf::from(COMMANDS_PAGE),
        commands_page(&command)?.into_bytes(),
    );
    Ok(files)
}

/// A command and all its subcommands, with their full names.
fn commands(root: &Command) -> Vec<(String, Command)> {
    let mut found = Vec::new();
    collect(root, root.get_name(), &mut found);
    found
}

fn collect(command: &Command, name: &str, found: &mut Vec<(String, Command)>) {
    found.push((name.to_owned(), command.clone()));
    for sub in command.get_subcommands().filter(|s| s.get_name() != "help") {
        collect(sub, &format!("{name} {}", sub.get_name()), found);
    }
}

/// The reference page: the long help of every command.
fn commands_page(root: &Command) -> anyhow::Result<String> {
    let mut page = String::from(
        "# Command reference\n\n\
         Generated from the program; `pgloupe <command> --help` shows the same text.\n",
    );
    for (name, mut command) in commands(root) {
        write!(
            page,
            "\n## `{name}`\n\n```text\n{}\n```\n",
            command.render_long_help().to_string().trim_end()
        )?;
    }
    Ok(page)
}
