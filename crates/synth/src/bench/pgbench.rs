//! Running pgbench inside the server container.

use std::path::Path;

use anyhow::{Context as _, bail};

use crate::container::{PgContainer, docker};

/// Where the literal scripts are copied inside the container.
pub const SCRIPT_DIR: &str = "/tmp/bench";

/// Statements one TPC-B transaction sends (`BEGIN`, three `UPDATE`s, `SELECT`, `INSERT`, `END`).
pub const TPCB_STATEMENTS: u64 = 7;

/// Arguments of the built-in TPC-B script over the extended protocol.
#[must_use]
pub fn tpcb_args(clients: u32, secs: u32) -> Vec<String> {
    let mut args = base(clients, secs, "extended");
    args.push("bench".to_owned());
    args
}

/// Arguments for the literal scripts over the simple protocol, one `-f` per script.
#[must_use]
pub fn literal_args(clients: u32, secs: u32, scale: u32, scripts: &[String]) -> Vec<String> {
    let mut args = base(clients, secs, "simple");
    args.extend(["-D".to_owned(), format!("scale={scale}")]);
    for script in scripts {
        args.extend(["-f".to_owned(), format!("{SCRIPT_DIR}/{script}@1")]);
    }
    args.push("bench".to_owned());
    args
}

fn base(clients: u32, secs: u32, protocol: &str) -> Vec<String> {
    [
        "pgbench",
        "-n",
        "-M",
        protocol,
        "-c",
        &clients.to_string(),
        "-j",
        &clients.min(4).to_string(),
        "-T",
        &secs.to_string(),
        "-U",
        "postgres",
    ]
    .map(str::to_owned)
    .to_vec()
}

/// Transactions pgbench reports having processed.
///
/// # Errors
///
/// Fails if the output has no `number of transactions actually processed` line.
pub fn parse_transactions(output: &str) -> anyhow::Result<u64> {
    let line = output
        .lines()
        .find_map(|l| l.strip_prefix("number of transactions actually processed:"))
        .context("pgbench printed no transaction count")?;
    line.trim()
        .parse()
        .with_context(|| format!("bad transaction count {line:?}"))
}

/// Copies the script directory into the container.
///
/// # Errors
///
/// Fails if `docker cp` fails.
pub fn copy_scripts(pg: &PgContainer, host_dir: &Path) -> anyhow::Result<()> {
    let target = format!("{}:{SCRIPT_DIR}", pg.name());
    let source = host_dir.display().to_string();
    docker(&["cp", source.as_str(), target.as_str()]).map(drop)
}

/// Runs one pgbench round in the container as application `app`; returns its transactions.
///
/// # Errors
///
/// Fails if pgbench exits non-zero or prints no count.
pub fn run_round(pg: &PgContainer, app: &str, args: &[String]) -> anyhow::Result<u64> {
    let env = format!("PGAPPNAME={app}");
    let mut cmd = vec!["exec", "-e", env.as_str(), pg.name()];
    cmd.extend(args.iter().map(String::as_str));
    let output = docker(&cmd).with_context(|| format!("pgbench round for {app}"))?;
    if output.is_empty() {
        bail!("pgbench printed nothing for {app}");
    }
    parse_transactions(&output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transactions_are_read_from_the_summary() {
        let out = "transaction type: <builtin: TPC-B>\nscaling factor: 1\nnumber of transactions actually processed: 12345\nlatency average = 1 ms\n";
        assert_eq!(parse_transactions(out).unwrap(), 12345);
        parse_transactions("nothing here").unwrap_err();
    }

    #[test]
    fn tpcb_uses_the_extended_protocol() {
        assert_eq!(
            tpcb_args(8, 30),
            [
                "pgbench", "-n", "-M", "extended", "-c", "8", "-j", "4", "-T", "30", "-U",
                "postgres", "bench"
            ]
        );
    }

    #[test]
    fn literal_scripts_run_over_the_simple_protocol() {
        let args = literal_args(2, 30, 20, &["a.sql".to_owned(), "b.sql".to_owned()]);
        let joined = args.join(" ");
        assert!(joined.contains("-M simple"), "{joined}");
        assert!(joined.contains("-D scale=20"), "{joined}");
        assert!(joined.contains("-f /tmp/bench/a.sql@1"), "{joined}");
        assert!(joined.contains("-f /tmp/bench/b.sql@1"), "{joined}");
        assert!(joined.contains("-j 2"), "{joined}");
        assert_eq!(args.last().map(String::as_str), Some("bench"));
    }
}
