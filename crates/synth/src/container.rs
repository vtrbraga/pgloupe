//! Docker-managed Postgres container for one generator run.

use std::{
    ffi::OsStr,
    path::Path,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context as _, bail};

/// Server settings passed as `-c name=value`.
pub const SETTINGS: &[(&str, &str)] = &[
    ("shared_preload_libraries", "pg_stat_statements"),
    ("compute_query_id", "on"),
    ("pg_stat_statements.track", "all"),
    ("pg_stat_statements.track_utility", "on"),
    ("pg_stat_statements.max", "500"),
    ("track_io_timing", "on"),
    ("logging_collector", "on"),
    ("log_destination", "stderr,csvlog,jsonlog"),
    ("log_directory", "/log"),
    ("log_filename", "postgresql.log"),
    ("log_rotation_age", "0"),
    ("log_rotation_size", "0"),
    ("log_file_mode", "0644"),
    ("log_line_prefix", "%m [%p] %q%u@%d/%a %Q "),
    ("log_min_duration_statement", "0"),
    ("log_lock_waits", "on"),
    ("deadlock_timeout", "200ms"),
    ("log_temp_files", "0"),
    ("log_checkpoints", "on"),
    ("log_connections", "on"),
    ("log_disconnections", "on"),
    ("checkpoint_timeout", "1d"),
    ("max_wal_size", "4GB"),
    ("jit", "off"),
    ("lc_messages", "C"),
    ("timezone", "UTC"),
    ("log_timezone", "UTC"),
];

/// Settings for one Postgres major version.
///
/// `jsonlog` exists from 15; version 14 logs to stderr and csvlog only.
#[must_use]
pub fn settings(pg_major: u32) -> Vec<(&'static str, &'static str)> {
    SETTINGS
        .iter()
        .map(|&(name, value)| match name {
            "log_destination" if pg_major < 15 => (name, "stderr,csvlog"),
            _ => (name, value),
        })
        .collect()
}

/// `settings` with `overrides` applied: a name already present keeps its position and takes the
/// new value, a new name is appended.
#[must_use]
pub fn apply_overrides<'a>(
    mut settings: Vec<(&'a str, &'a str)>,
    overrides: &[(&'a str, &'a str)],
) -> Vec<(&'a str, &'a str)> {
    for &(name, value) in overrides {
        match settings.iter_mut().find(|(n, _)| *n == name) {
            Some(slot) => slot.1 = value,
            None => settings.push((name, value)),
        }
    }
    settings
}

/// Locales built at startup so sessions can log in other languages (`lc_messages`).
pub const LOCALES: &[&str] = &["pt_BR", "es_ES", "fr_FR", "de_DE"];

/// Where the generated locales live inside the container.
const LOCALE_DIR: &str = "/locales";

/// Label on generator containers, used to find stale ones.
pub const LABEL: &str = "pgloupe.synth=1";

/// How long to wait for Postgres to accept TCP connections.
const READY_TIMEOUT: Duration = Duration::from_secs(60);

/// Everything needed to build the `docker run` command line.
#[derive(Debug)]
pub struct RunSpec<'a> {
    /// Container name.
    pub name: &'a str,
    /// Image tag, e.g. `postgres:17`.
    pub image: &'a str,
    /// Postgres major version of the image.
    pub pg_major: u32,
    /// `key=value` label for stale cleanup.
    pub label: &'a str,
    /// Host uid the server runs as.
    pub uid: u32,
    /// Host gid the server runs as.
    pub gid: u32,
    /// Absolute host path mounted at `/log`.
    pub log_dir: &'a Path,
    /// Absolute host path mounted read-only at `/dataset`.
    pub dataset_dir: &'a Path,
    /// Settings that replace (or extend) [`SETTINGS`] for this run.
    pub overrides: &'a [(&'a str, &'a str)],
}

/// Builds `docker run` arguments, without the leading `docker`.
#[must_use]
pub fn run_args(spec: &RunSpec<'_>) -> Vec<String> {
    let mut args: Vec<String> = [
        "run",
        "-d",
        "--name",
        spec.name,
        "--label",
        spec.label,
        "-v",
        "/etc/passwd:/etc/passwd:ro",
        "-e",
        "PGDATA=/pgdata",
        "-e",
        "POSTGRES_HOST_AUTH_METHOD=trust",
        // `C.UTF-8` is built into glibc; the other locales are generated below into LOCPATH,
        // which the server (not running as root) can read.
        "-e",
        "LANG=C.UTF-8",
        "-e",
        "LOCPATH=/locales",
        "-p",
        "127.0.0.1::5432",
    ]
    .map(String::from)
    .to_vec();
    args.extend([
        "--user".to_owned(),
        format!("{}:{}", spec.uid, spec.gid),
        "--tmpfs".to_owned(),
        format!("/pgdata:rw,uid={},gid={},mode=0700", spec.uid, spec.gid),
        "-v".to_owned(),
        format!("{}:/log", spec.log_dir.display()),
        "-v".to_owned(),
        format!("{}:/dataset:ro", spec.dataset_dir.display()),
        "--tmpfs".to_owned(),
        format!("{LOCALE_DIR}:rw,uid={},gid={}", spec.uid, spec.gid),
        "--entrypoint".to_owned(),
        "sh".to_owned(),
        spec.image.to_owned(),
        "-c".to_owned(),
        locale_script(),
        "sh".to_owned(),
        "postgres".to_owned(),
    ]);
    for (name, value) in apply_overrides(settings(spec.pg_major), spec.overrides) {
        args.push("-c".to_owned());
        args.push(format!("{name}={value}"));
    }
    args
}

/// Shell that builds the locales, then hands over to the image's own entrypoint with
/// `postgres -c ...` as its arguments.
fn locale_script() -> String {
    let names = LOCALES.join(" ");
    format!(
        "for l in {names}; do localedef -i $l -f UTF-8 {LOCALE_DIR}/$l.UTF-8 2>/dev/null; done; exec docker-entrypoint.sh \"$@\""
    )
}

/// Runs `docker <args>` and returns trimmed stdout.
///
/// # Errors
///
/// Fails if docker cannot be spawned or exits non-zero; the error names the command and its stderr.
pub fn docker<S: AsRef<OsStr>>(args: &[S]) -> anyhow::Result<String> {
    let shown = args
        .iter()
        .map(|a| a.as_ref().to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ");
    let shown = match shown.char_indices().nth(300) {
        Some((cut, _)) => format!("{}…", &shown[..cut]),
        None => shown,
    };
    let output = Command::new("docker")
        .args(args)
        .output()
        .context("spawning docker; is Docker running and on PATH?")?;
    if !output.status.success() {
        bail!(
            "`docker {shown}` failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Removes every container carrying `label` (left behind by interrupted runs).
///
/// # Errors
///
/// Fails if docker cannot list or remove containers.
pub fn remove_stale(label: &str) -> anyhow::Result<()> {
    let filter = format!("label={label}");
    let ids = docker(&["ps", "-aq", "--filter", filter.as_str()])?;
    for id in ids.lines() {
        docker(&["rm", "-f", id])?;
    }
    Ok(())
}

/// Repo digest of a local image, e.g. `postgres@sha256:…`.
///
/// # Errors
///
/// Fails if the image is not present locally.
pub fn image_digest(image: &str) -> anyhow::Result<String> {
    docker(&[
        "image",
        "inspect",
        "--format",
        "{{index .RepoDigests 0}}",
        image,
    ])
}

/// Host port from `docker port` output such as `127.0.0.1:49153`.
fn parse_port(output: &str) -> anyhow::Result<u16> {
    let line = output
        .lines()
        .next()
        .context("docker port printed nothing")?;
    let (_, port) = line
        .rsplit_once(':')
        .with_context(|| format!("unexpected docker port output {line:?}"))?;
    port.parse()
        .with_context(|| format!("bad port in {line:?}"))
}

/// A running Postgres container, removed on drop.
#[derive(Debug)]
pub struct PgContainer {
    name: String,
    port: u16,
}

impl PgContainer {
    /// Starts the container and waits until Postgres accepts TCP connections.
    ///
    /// # Errors
    ///
    /// Fails if docker fails or the server is not ready within 60 s; the container is removed.
    pub fn start(spec: &RunSpec<'_>) -> anyhow::Result<Self> {
        if let Err(e) = docker(&run_args(spec)) {
            // `docker run` can fail after creating the container.
            let _ = docker(&["rm", "-f", spec.name]);
            return Err(e);
        }
        let mut container = Self {
            name: spec.name.to_owned(),
            port: 0,
        };
        container.wait_ready()?;
        container.port = parse_port(&docker(&["port", container.name.as_str(), "5432"])?)?;
        Ok(container)
    }

    /// Host port mapped to the server's 5432.
    #[must_use]
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Container name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Runs a command in the container with statement logging disabled for libpq clients.
    ///
    /// # Errors
    ///
    /// Fails if the command exits non-zero.
    pub fn exec_unlogged(&self, cmd: &[&str]) -> anyhow::Result<String> {
        let mut args = vec![
            "exec",
            "-e",
            "PGOPTIONS=-c log_min_duration_statement=-1 -c pg_stat_statements.track=none",
            self.name.as_str(),
        ];
        args.extend_from_slice(cmd);
        docker(&args)
    }

    /// Stops the server cleanly so the logging collector flushes.
    ///
    /// # Errors
    ///
    /// Fails if `docker stop` fails.
    pub fn stop(&self) -> anyhow::Result<()> {
        docker(&["stop", "-t", "30", self.name.as_str()]).map(drop)
    }

    fn wait_ready(&self) -> anyhow::Result<()> {
        let deadline = Instant::now() + READY_TIMEOUT;
        while Instant::now() < deadline {
            if docker(&["inspect", "-f", "{{.State.Running}}", self.name.as_str()])? != "true" {
                bail!(
                    "container {} exited before becoming ready; last logs:\n{}",
                    self.name,
                    self.logs()
                );
            }
            let ready = Command::new("docker")
                .args([
                    "exec",
                    self.name.as_str(),
                    "pg_isready",
                    "-q",
                    "-h",
                    "127.0.0.1",
                    "-U",
                    "postgres",
                ])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success());
            if ready {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(500));
        }
        bail!(
            "postgres in {} not ready after {READY_TIMEOUT:?}; last logs:\n{}",
            self.name,
            self.logs()
        )
    }

    /// Last lines of the container's stdout and stderr, for error messages.
    fn logs(&self) -> String {
        match Command::new("docker")
            .args(["logs", "--tail", "50", self.name.as_str()])
            .output()
        {
            Ok(out) => format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            ),
            Err(e) => format!("(could not read logs: {e})"),
        }
    }
}

impl Drop for PgContainer {
    #[expect(clippy::print_stderr, reason = "progress is reported on the terminal")]
    fn drop(&mut self) {
        if let Err(e) = docker(&["rm", "-f", self.name.as_str()]) {
            eprintln!("warning: could not remove container {}: {e:#}", self.name);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> RunSpec<'static> {
        RunSpec {
            name: "pgloupe-synth-test",
            image: "postgres:17",
            pg_major: 17,
            label: LABEL,
            uid: 1000,
            gid: 1001,
            log_dir: Path::new("/abs/log"),
            dataset_dir: Path::new("/abs/dataset"),
            overrides: &[],
        }
    }

    fn has_pair(args: &[String], flag: &str, value: &str) -> bool {
        args.windows(2).any(|w| w[0] == flag && w[1] == value)
    }

    #[test]
    fn runs_as_host_user_with_tmpfs_pgdata() {
        let args = run_args(&spec());
        assert!(has_pair(&args, "--user", "1000:1001"));
        assert!(has_pair(
            &args,
            "--tmpfs",
            "/pgdata:rw,uid=1000,gid=1001,mode=0700"
        ));
        assert!(has_pair(&args, "-e", "PGDATA=/pgdata"));
        assert!(has_pair(&args, "-v", "/etc/passwd:/etc/passwd:ro"));
    }

    #[test]
    fn mounts_log_and_dataset_dirs() {
        let args = run_args(&spec());
        assert!(has_pair(&args, "-v", "/abs/log:/log"));
        assert!(has_pair(&args, "-v", "/abs/dataset:/dataset:ro"));
        assert!(has_pair(&args, "--label", LABEL));
        assert!(has_pair(&args, "-p", "127.0.0.1::5432"));
    }

    #[test]
    fn locales_are_built_before_the_image_entrypoint_takes_over() {
        let args = run_args(&spec());
        assert!(has_pair(&args, "-e", "LOCPATH=/locales"));
        assert!(has_pair(&args, "--entrypoint", "sh"));
        let script = args.iter().find(|a| a.contains("localedef")).unwrap();
        for locale in LOCALES {
            assert!(script.contains(locale), "{locale}");
        }
        assert!(script.ends_with("exec docker-entrypoint.sh \"$@\""));
    }

    #[test]
    fn every_setting_follows_the_image_as_dash_c() {
        let args = run_args(&spec());
        let image_at = args.iter().position(|a| a == "postgres:17").unwrap();
        for (name, value) in SETTINGS {
            let pos = args
                .windows(2)
                .position(|w| w[0] == "-c" && w[1] == format!("{name}={value}"))
                .unwrap_or_else(|| panic!("missing -c {name}={value}"));
            assert!(pos > image_at, "{name} must come after the image");
        }
    }

    #[test]
    fn postgres_14_has_no_jsonlog() {
        let log_destination = |major| {
            settings(major)
                .into_iter()
                .find(|(name, _)| *name == "log_destination")
                .map(|(_, value)| value)
        };
        assert_eq!(log_destination(14), Some("stderr,csvlog"));
        assert_eq!(log_destination(15), Some("stderr,csvlog,jsonlog"));
        assert_eq!(log_destination(18), Some("stderr,csvlog,jsonlog"));
    }

    #[test]
    fn overrides_replace_in_place_and_append_new_names() {
        let base = vec![("a", "1"), ("b", "2")];
        let out = apply_overrides(base, &[("b", "9"), ("c", "3")]);
        assert_eq!(out, [("a", "1"), ("b", "9"), ("c", "3")]);
    }

    #[test]
    fn run_args_use_the_overridden_value_only() {
        let mut spec = spec();
        spec.overrides = &[("log_rotation_size", "256MB")];
        let args = run_args(&spec);
        assert!(has_pair(&args, "-c", "log_rotation_size=256MB"));
        assert!(!has_pair(&args, "-c", "log_rotation_size=0"));
    }

    #[test]
    fn parses_docker_port_output() {
        assert_eq!(parse_port("127.0.0.1:49153\n").unwrap(), 49153);
        assert_eq!(parse_port("127.0.0.1:5\n[::1]:6\n").unwrap(), 5);
        parse_port("").unwrap_err();
        parse_port("127.0.0.1:notaport").unwrap_err();
    }
}
