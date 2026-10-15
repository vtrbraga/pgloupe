//! `logs fetch` end to end: a fake `aws`, and a real server for the `sql` provider.

#![allow(clippy::unwrap_used, clippy::expect_used, reason = "test code")]

mod support;

use std::{fs, path::PathBuf, process::Command};

use support::fake_cli::{FakeCli, Reply};

fn run(args: &[&str], envs: &[(&str, &str)]) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_pgloupe"));
    command.args(args);
    for (k, v) in envs {
        command.env(k, v);
    }
    command.output().expect("run pgloupe")
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pgloupe-fetch-{}-{name}", std::process::id()));
    drop(fs::remove_dir_all(&dir));
    dir
}

fn list(files: &[(&str, i64, i64)]) -> String {
    let entries: Vec<String> = files
        .iter()
        .map(|(n, w, s)| format!(r#"{{"LogFileName":"{n}","LastWritten":{w},"Size":{s}}}"#))
        .collect();
    format!(r#"{{"DescribeDBLogFiles":[{}]}}"#, entries.join(","))
}

fn portion(data: &str, marker: &str, pending: bool) -> String {
    format!(
        r#"{{"LogFileData":{},"Marker":"{marker}","AdditionalDataPending":{pending}}}"#,
        serde_json::to_string(data).unwrap()
    )
}

/// What RDS writes with its default `log_line_prefix` (`%t:%r:%u@%d:[%p]:`).
const RDS_LINES: &str = "2026-10-14 10:00:01 UTC:10.0.0.5(40001):app@shop:[301]:LOG:  duration: 120.500 ms  statement: SELECT * FROM orders WHERE id = 7\n\
2026-10-14 10:00:02 UTC:10.0.0.5(40001):app@shop:[301]:LOG:  duration: 3.100 ms  statement: SELECT 1\n\
2026-10-14 10:00:03 UTC:10.0.0.5(40001):app@shop:[301]:ERROR:  relation \"nope\" does not exist\n";

#[test]
fn fetched_rds_logs_are_analyzed_with_the_inferred_prefix_and_reruns_continue() {
    let (first, second) = RDS_LINES.split_at(RDS_LINES.find("2026-10-14 10:00:03").unwrap());
    let fake = FakeCli::new("fetch-aws")
        .on(
            &["sts get-caller-identity"],
            vec![Reply::Out("{}".to_owned())],
        )
        .on(
            &["--version"],
            vec![Reply::Out("aws-cli/2.17\n".to_owned())],
        )
        .on(
            &["describe-db-log-files"],
            vec![
                Reply::Out(list(&[(
                    "error/postgresql.log.2026-10-14-10",
                    5_000,
                    i64::try_from(first.len()).unwrap(),
                )])),
                Reply::Out(list(&[(
                    "error/postgresql.log.2026-10-14-10",
                    6_000,
                    i64::try_from(RDS_LINES.len()).unwrap(),
                )])),
            ],
        )
        .on(
            &["--marker 0 --no-paginate"],
            vec![Reply::Out(portion(first, "1:1", false))],
        )
        .on(
            &["--marker 1:1 --no-paginate"],
            vec![Reply::Out(portion(second, "1:2", false))],
        );
    let program = fake.program();
    let dir = scratch("aws");
    let args = |extra: &[&str]| -> Vec<String> {
        let mut v: Vec<String> = [
            "logs",
            "fetch",
            "--provider",
            "aws-rds",
            "--instance",
            "prod",
            "--bin",
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
        v.push(program.display().to_string());
        v.extend(
            ["--since", "2026-10-14", "--out", &dir.display().to_string()]
                .iter()
                .map(|s| (*s).to_owned()),
        );
        v.extend(extra.iter().map(|s| (*s).to_owned()));
        v
    };
    let a = args(&[]);
    let one = run(&a.iter().map(String::as_str).collect::<Vec<_>>(), &[]);
    assert!(
        one.status.success(),
        "{}",
        String::from_utf8_lossy(&one.stderr)
    );
    assert!(
        !String::from_utf8_lossy(&one.stderr).contains("experimental"),
        "aws-rds is supported"
    );
    let two = run(&a.iter().map(String::as_str).collect::<Vec<_>>(), &[]);
    assert!(
        two.status.success(),
        "{}",
        String::from_utf8_lossy(&two.stderr)
    );
    let file = dir.join("error_postgresql.log.2026-10-14-10");
    assert_eq!(fs::read_to_string(&file).unwrap(), RDS_LINES);

    let analyzed = run(
        &[
            "logs",
            "analyze",
            file.to_str().unwrap(),
            "--output",
            "json",
        ],
        &[],
    );
    assert!(
        analyzed.status.success(),
        "{}",
        String::from_utf8_lossy(&analyzed.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&analyzed.stdout).unwrap();
    assert_eq!(report["overview"]["records"], 3);
    assert_eq!(report["dimensions"]["database"][0]["value"], "shop");
    assert_eq!(report["errors"][0]["sqlstate"], serde_json::Value::Null);
}

#[test]
fn a_missing_aws_cli_and_a_missing_instance_fail_with_the_fix() {
    let dir = scratch("missing");
    let out = run(
        &[
            "logs",
            "fetch",
            "--provider",
            "aws-rds",
            "--instance",
            "x",
            "--bin",
            "/nonexistent/aws",
            "--out",
            dir.to_str().unwrap(),
        ],
        &[],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("install the AWS CLI"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let out = run(
        &[
            "logs",
            "fetch",
            "--provider",
            "aws-rds",
            "--out",
            dir.to_str().unwrap(),
        ],
        &[],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("--instance"));
    let out = run(
        &[
            "logs",
            "fetch",
            "--provider",
            "aws-rds",
            "--instance",
            "x",
            "--since",
            "whenever",
            "--out",
            dir.to_str().unwrap(),
        ],
        &[],
    );
    assert_eq!(out.status.code(), Some(1));
}

#[test]
#[ignore = "needs Docker"]
fn the_sql_provider_reads_the_servers_log_and_continues() -> Result<(), String> {
    let pg = support::Pg::start(
        16,
        &[
            "logging_collector=on",
            "log_min_duration_statement=0",
            "log_destination=stderr",
        ],
    )?;
    pg.sql("SELECT 'marker-one-xyz'")?;
    let dir = scratch("sql");
    let dsn = pg.url();
    let go = || {
        run(
            &[
                "logs",
                "fetch",
                "--provider",
                "sql",
                "--since",
                "1h",
                "--out",
                dir.to_str().unwrap_or_default(),
            ],
            &[("PGLOUPE_DSN", &dsn)],
        )
    };
    std::thread::sleep(std::time::Duration::from_secs(1));
    let first = go();
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(String::from_utf8_lossy(&first.stderr).contains("experimental"));
    let text: String = fs::read_dir(&dir)
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .map(|e| fs::read_to_string(e.path()).unwrap_or_default())
        .collect();
    assert!(text.contains("marker-one-xyz"), "{text}");
    pg.sql("SELECT 'marker-two-xyz'")?;
    std::thread::sleep(std::time::Duration::from_secs(1));
    let second = go();
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    let again: String = fs::read_dir(&dir)
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .map(|e| fs::read_to_string(e.path()).unwrap_or_default())
        .collect();
    assert!(again.contains("marker-two-xyz"));
    assert_eq!(
        again.matches("marker-one-xyz").count(),
        text.matches("marker-one-xyz").count(),
        "no duplicates"
    );
    assert!(!String::from_utf8_lossy(&second.stderr).contains(&pg.password));
    Ok(())
}

#[test]
fn experimental_providers_warn_every_time_and_ask_for_what_they_need() {
    let dir = scratch("experimental");
    let heroku = FakeCli::new("cli-heroku")
        .on(&["--version"], vec![Reply::Out("heroku/9\n".to_owned())])
        .on(&["auth:whoami"], vec![Reply::Out("me\n".to_owned())])
        .on(
            &["logs"],
            vec![Reply::Out(
                "2026-10-14T10:00:01.000000+00:00 app[postgres.9]: LOG:  hello\n".to_owned(),
            )],
        );
    let program = heroku.program();
    let out = run(
        &[
            "logs",
            "fetch",
            "--provider",
            "heroku",
            "--instance",
            "shop",
            "--bin",
            program.to_str().unwrap(),
            "--since",
            "2026-10-14",
            "--out",
            dir.to_str().unwrap(),
        ],
        &[],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("provider heroku is experimental"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    for (provider, needs) in [
        ("azure-flexible", "--resource-group"),
        ("gcp-cloudsql", "--project"),
        ("aiven", "--project"),
        ("aws-cloudwatch", "--log-group or --instance"),
    ] {
        let out = run(
            &[
                "logs",
                "fetch",
                "--provider",
                provider,
                "--out",
                dir.to_str().unwrap(),
            ],
            &[],
        );
        assert_eq!(out.status.code(), Some(1), "{provider}");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains(needs),
            "{provider}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[test]
#[ignore = "needs Docker"]
fn export_collects_snapshots_and_logs_and_builds_the_report() -> Result<(), String> {
    let pg = support::Pg::start(16, &[])?;
    pg.sql("CREATE TABLE t (a int); SELECT count(*) FROM t")?;
    let fake = FakeCli::new("export-aws")
        .on(
            &["sts get-caller-identity"],
            vec![Reply::Out("{}".to_owned())],
        )
        .on(
            &["--version"],
            vec![Reply::Out("aws-cli/2.17\n".to_owned())],
        )
        .on(
            &["describe-db-log-files"],
            vec![Reply::Out(list(&[(
                "error/postgresql.log.2026-10-14-10",
                5_000,
                i64::try_from(RDS_LINES.len()).unwrap(),
            )]))],
        )
        .on(
            &["--marker 0 --no-paginate"],
            vec![Reply::Out(portion(RDS_LINES, "1:1", false))],
        )
        .on(
            &["--marker 1:1 --no-paginate"],
            vec![Reply::Out(portion("", "1:1", false))],
        );
    let program = fake.program();
    let dir = scratch("export");
    let report = dir.join("report.html");
    let dsn = pg.url();
    let go = || {
        run(
            &[
                "export",
                "--provider",
                "aws-rds",
                "--instance",
                "prod",
                "--bin",
                program.to_str().unwrap_or_default(),
                "--since",
                "2026-10-14",
                "--out",
                dir.to_str().unwrap_or_default(),
                "--report",
                report.to_str().unwrap_or_default(),
            ],
            &[("PGLOUPE_DSN", &dsn)],
        )
    };
    let first = go();
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(String::from_utf8_lossy(&first.stderr).contains("only one snapshot"));
    std::thread::sleep(std::time::Duration::from_millis(1_100));
    pg.sql("SELECT count(*) FROM t WHERE a > 3")?;
    let second = go();
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    let page = fs::read_to_string(&report).map_err(|e| e.to_string())?;
    assert!(
        page.contains("pgloupe-data") && page.contains("pg_stat_statements"),
        "report missing"
    );
    assert_eq!(
        fs::read_dir(dir.join("snapshots"))
            .map_err(|e| e.to_string())?
            .count(),
        6,
        "two snapshots, their info files and their settings"
    );
    for out in [&first, &second] {
        let all = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(!all.contains(&pg.password));
    }
    Ok(())
}

#[test]
fn watch_refuses_an_interval_that_would_hammer_the_server() {
    for (interval, window) in [("1s", "24h"), ("0s", "24h"), ("5m", "0s")] {
        let out = run(
            &[
                "watch",
                "--conn",
                "postgres://u@127.0.0.1:1/db",
                "--provider",
                "aws-rds",
                "--instance",
                "x",
                "--interval",
                interval,
                "--window",
                window,
                "--once",
            ],
            &[],
        );
        assert!(!out.status.success(), "{interval} {window}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(
            err.contains("--interval") || err.contains("--window"),
            "{err}"
        );
    }
}

#[test]
#[ignore = "needs Docker"]
#[expect(
    clippy::too_many_lines,
    reason = "one end-to-end scenario read top to bottom"
)]
fn watch_serves_a_report_that_gains_its_statistics_side_on_the_second_cycle() -> Result<(), String>
{
    use std::io::{BufRead as _, BufReader};
    use std::process::Stdio;

    let pg = support::Pg::start(16, &[])?;
    pg.sql("CREATE TABLE t (a int); SELECT count(*) FROM t")?;
    let fake = FakeCli::new("watch-aws")
        .on(
            &["sts get-caller-identity"],
            vec![Reply::Out("{}".to_owned())],
        )
        .on(
            &["--version"],
            vec![Reply::Out("aws-cli/2.17\n".to_owned())],
        )
        .on(
            &["describe-db-log-files"],
            vec![Reply::Out(list(&[(
                "error/postgresql.log.2026-10-14-10",
                5_000,
                i64::try_from(RDS_LINES.len()).unwrap(),
            )]))],
        )
        .on(
            &["--marker 0 --no-paginate"],
            vec![Reply::Out(portion(RDS_LINES, "1:1", false))],
        )
        .on(
            &["--marker 1:1 --no-paginate"],
            vec![Reply::Out(portion("", "1:1", false))],
        );
    let program = fake.program();
    let dir = scratch("watch");
    let mut child = Command::new(env!("CARGO_BIN_EXE_pgloupe"))
        .args([
            "watch",
            "--provider",
            "aws-rds",
            "--instance",
            "prod",
            "--bin",
            program.to_str().unwrap_or_default(),
            "--listen",
            "127.0.0.1:0",
            "--interval",
            "10s",
            "--window",
            "9999d",
            "--state-dir",
            dir.to_str().unwrap_or_default(),
        ])
        .env("PGLOUPE_DSN", pg.url())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let mut lines = BufReader::new(child.stdout.take().ok_or("no stdout")?).lines();
    let first = lines
        .next()
        .ok_or("no output")?
        .map_err(|e| e.to_string())?;
    let url = first
        .strip_prefix("serving ")
        .and_then(|rest| rest.split(' ').next())
        .ok_or_else(|| format!("unexpected first line {first:?}"))?
        .trim_end_matches('/')
        .to_owned();
    let get = |path: &str| -> Result<String, String> {
        let mut response = ureq::get(&format!("{url}{path}"))
            .config()
            .http_status_as_error(false)
            .build()
            .call()
            .map_err(|e| e.to_string())?;
        response
            .body_mut()
            .read_to_string()
            .map_err(|e| e.to_string())
    };
    let started = std::time::Instant::now();
    let mut generation = 0;
    while started.elapsed() < std::time::Duration::from_secs(40) {
        std::thread::sleep(std::time::Duration::from_millis(500));
        if let Ok(status) = get("/status") {
            let value: serde_json::Value = serde_json::from_str(&status).unwrap_or_default();
            generation = value["generation"].as_u64().unwrap_or(0);
            if generation >= 2 {
                break;
            }
        }
        if generation == 1 {
            let _ = pg.sql("SELECT count(*) FROM t WHERE a > 5");
        }
    }
    let json = get("/report.json");
    let page = get("/");
    let last_status = get("/status").unwrap_or_default();
    drop(child.kill());
    drop(child.wait());
    assert!(
        generation >= 2,
        "only {generation} cycles in 40 s; status {last_status}"
    );
    let report: serde_json::Value = serde_json::from_str(&json?).map_err(|e| e.to_string())?;
    assert_eq!(report["kind"], "pgloupe.report");
    assert!(
        report["inputs"]["stats"].is_object(),
        "statistics side after two snapshots: {}",
        report["inputs"]
    );
    assert_eq!(report["logs"]["overview"]["records"], 3);
    assert!(page?.contains("pgloupe-data"));
    assert!(!json_text_has(&report, &pg.password));
    Ok(())
}

fn json_text_has(value: &serde_json::Value, needle: &str) -> bool {
    value.to_string().contains(needle)
}
