#![expect(
    clippy::unwrap_used,
    clippy::panic,
    reason = "test helpers panic on setup failure"
)]

mod common;

use std::{
    fs,
    path::{Path, PathBuf},
};

use csv::StringRecord;
use synth::{
    cli::{GenerateArgs, PgMajor},
    generate,
    manifest::{EventKind, Expect, Manifest},
    scenario::{self, Ctx, Phase, Scenario},
};

const SEVERITY: usize = 11;
const SQLSTATE: usize = 12;
const MESSAGE: usize = 13;
const DETAIL: usize = 14;
const QUERY: usize = 19;
const APP: usize = 22;

/// Args writing into a fresh temp dir, sharing the repo's download cache.
fn args(only: &[&str]) -> GenerateArgs {
    GenerateArgs {
        pg: PgMajor::V17,
        seed: 42,
        only: only.iter().map(|s| (*s).to_owned()).collect(),
        force: false,
        out: common::scratch_dir("out"),
        cache: Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/cache"),
    }
}

/// Generates a corpus for `only` and returns its directory.
fn generate_corpus(only: &[&str]) -> PathBuf {
    let args = args(only);
    let scenarios = scenario::select(&args.only).unwrap();
    generate::run_with(&args, &scenarios).unwrap()
}

/// Reads a CSV without headers (csvlog).
fn read_csv(path: &Path) -> Vec<StringRecord> {
    csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .from_path(path)
        .unwrap()
        .records()
        .map(Result::unwrap)
        .collect()
}

/// Reads a CSV with a header row (snapshots).
fn read_csv_with_headers(path: &Path) -> (StringRecord, Vec<StringRecord>) {
    let mut reader = csv::Reader::from_path(path).unwrap();
    let headers = reader.headers().unwrap().clone();
    let rows = reader.records().map(Result::unwrap).collect();
    (headers, rows)
}

/// Column index by header name.
fn column(headers: &StringRecord, name: &str) -> usize {
    headers
        .iter()
        .position(|h| h == name)
        .unwrap_or_else(|| panic!("no column {name}"))
}

fn matches(row: &StringRecord, expect: &Expect) -> bool {
    let message = &row[MESSAGE];
    let app_matches = row[APP] == expect.app;
    let duration_line =
        |kind: &str| app_matches && message.starts_with("duration: ") && message.contains(kind);
    let bound_parameters = || row[DETAIL].starts_with("Parameters: ");
    match expect.event {
        EventKind::Statement => duration_line(" statement: "),
        EventKind::Parse => duration_line("  parse "),
        EventKind::Bind => duration_line("  bind ") && bound_parameters(),
        EventKind::Execute => duration_line("  execute ") && bound_parameters(),
        EventKind::LockWait => app_matches && message.contains(" still waiting for "),
        EventKind::LockAcquired => {
            app_matches && message.contains(" acquired ") && message.contains(" after ")
        }
        EventKind::Deadlock => {
            app_matches && &row[SEVERITY] == "ERROR" && &row[SQLSTATE] == "40P01"
        }
        EventKind::TempFile => app_matches && message.starts_with("temporary file: "),
        EventKind::Checkpoint => message == "checkpoint starting: immediate force wait",
        EventKind::ConnectionAuthorized => {
            message.starts_with("connection authorized: ")
                && message.contains(&format!("application_name={}", expect.app))
        }
        EventKind::Disconnection => app_matches && message.starts_with("disconnection: "),
        EventKind::Error => {
            app_matches
                && &row[SEVERITY] == "ERROR"
                && Some(&row[SQLSTATE]) == expect.sqlstate.as_deref()
                && !row[QUERY].is_empty()
        }
    }
}

/// Asserts every manifest expectation against `log/postgresql.csv`.
fn assert_log_matches_manifest(dir: &Path) {
    let manifest = Manifest::read(dir).unwrap();
    let rows = read_csv(&dir.join("log/postgresql.csv"));
    for expect in &manifest.expect {
        let count = u32::try_from(rows.iter().filter(|r| matches(r, expect)).count()).unwrap();
        assert!(
            count >= expect.min && expect.max.is_none_or(|max| count <= max),
            "{} {:?} {:?}: got {count}, expected {}..={:?}",
            expect.scenario,
            expect.event,
            expect.sqlstate,
            expect.min,
            expect.max,
        );
    }
}

#[test]
#[ignore = "needs docker"]
fn empty_run_writes_layout_snapshots_and_manifest() {
    let args = args(&[]);
    let dir = generate::run_with(&args, &[]).unwrap();
    for file in [
        "log/postgresql.log",
        "log/postgresql.csv",
        "log/postgresql.json",
        "manifest.json",
    ] {
        assert!(dir.join(file).is_file(), "missing {file}");
    }
    for n in 0..3 {
        for stem in ["pgss", "pgss_info", "pg_stat_database"] {
            assert!(
                dir.join(format!("{stem}_{n}.csv")).is_file(),
                "missing {stem}_{n}.csv"
            );
        }
    }
    assert!(!dir.with_extension("tmp").exists());
    let manifest = Manifest::read(&dir).unwrap();
    assert_eq!(manifest.pg_major, 17);
    assert!(manifest.server_version_num >= 170_000);
    assert_eq!(manifest.snapshots_at.len(), 3);
    assert!(manifest.image_digest.contains("sha256:"));
}

#[test]
#[ignore = "needs docker"]
fn existing_output_is_kept_without_force() {
    let mut args = args(&[]);
    let dir = generate::run_with(&args, &[]).unwrap();
    let marker = dir.join("marker");
    fs::write(&marker, "keep").unwrap();
    generate::run_with(&args, &[]).unwrap();
    assert!(
        marker.exists(),
        "rerun without --force must not touch output"
    );
    args.force = true;
    generate::run_with(&args, &[]).unwrap();
    assert!(!marker.exists(), "--force must regenerate");
}

#[test]
#[ignore = "needs docker"]
fn stale_tmp_dir_is_replaced() {
    let args = args(&[]);
    let tmp = args.run_dir().with_extension("tmp");
    fs::create_dir_all(&tmp).unwrap();
    fs::write(tmp.join("junk"), "from an interrupted run").unwrap();
    let dir = generate::run_with(&args, &[]).unwrap();
    assert!(!dir.join("junk").exists());
}

fn fail(_: &mut Ctx) -> anyhow::Result<()> {
    anyhow::bail!("boom")
}

const FAILING: Scenario = Scenario {
    name: "failing",
    phase: Phase::Main,
    run: fail,
    expect: &[],
};

#[test]
#[ignore = "needs docker"]
fn failed_scenario_keeps_tmp_and_leaves_existing_corpus_untouched() {
    let mut args = args(&[]);
    let dir = generate::run_with(&args, &[]).unwrap();
    fs::write(dir.join("marker"), "keep").unwrap();
    args.force = true;
    let err = generate::run_with(&args, &[&FAILING]).unwrap_err();
    assert!(format!("{err:#}").contains("scenario failing"), "{err:#}");
    assert!(
        dir.join("marker").exists(),
        "failed --force run must not replace the corpus"
    );
    assert!(
        dir.with_extension("tmp").join("log").is_dir(),
        "partial output stays for inspection"
    );
}

#[test]
#[ignore = "needs docker"]
fn baseline_matches_manifest() {
    assert_log_matches_manifest(&generate_corpus(&["baseline"]));
}

#[test]
#[ignore = "needs docker"]
fn ddl_matches_manifest() {
    assert_log_matches_manifest(&generate_corpus(&["ddl"]));
}

#[test]
#[ignore = "needs docker"]
fn temp_files_matches_manifest_and_every_query_spills() {
    let dir = generate_corpus(&["temp_files"]);
    assert_log_matches_manifest(&dir);
    let rows = read_csv(&dir.join("log/postgresql.csv"));
    for query in scenario::temp_files::QUERIES {
        let spills = rows
            .iter()
            .filter(|r| r[MESSAGE].starts_with("temporary file: ") && r[QUERY] == **query)
            .count();
        assert!(spills >= 1, "no temp file logged for {query}");
    }
}

#[test]
#[ignore = "needs docker"]
fn errors_matches_manifest() {
    assert_log_matches_manifest(&generate_corpus(&["errors"]));
}

#[test]
#[ignore = "needs docker"]
fn multiline_matches_manifest_and_logs_exact_text() {
    let dir = generate_corpus(&["multiline"]);
    assert_log_matches_manifest(&dir);
    let manifest = Manifest::read(&dir).unwrap();
    assert_eq!(manifest.multiline_texts.len(), 10);
    let rows = read_csv(&dir.join("log/postgresql.csv"));
    for text in &manifest.multiline_texts {
        let suffix = format!(" statement: {text}");
        assert!(
            rows.iter().any(|r| r[MESSAGE].ends_with(&suffix)),
            "statement not logged verbatim: {text:?}"
        );
    }
}

#[test]
#[ignore = "needs docker"]
fn extended_matches_manifest() {
    assert_log_matches_manifest(&generate_corpus(&["extended"]));
}

#[test]
#[ignore = "needs docker"]
fn lock_wait_matches_manifest() {
    assert_log_matches_manifest(&generate_corpus(&["lock_wait"]));
}

#[test]
#[ignore = "needs docker"]
fn deadlock_matches_manifest() {
    assert_log_matches_manifest(&generate_corpus(&["deadlock"]));
}

#[test]
#[ignore = "needs docker"]
fn functions_hide_inner_statements_from_logs_but_not_pgss() {
    let dir = generate_corpus(&["functions"]);
    assert_log_matches_manifest(&dir);
    let (headers, rows) = read_csv_with_headers(&dir.join("pgss_1.csv"));
    let (query, toplevel, calls) = (
        column(&headers, "query"),
        column(&headers, "toplevel"),
        column(&headers, "calls"),
    );
    let inner_insert = rows
        .iter()
        .find(|r| r[query].starts_with("INSERT INTO rental") && &r[toplevel] == "f")
        .expect("nested INSERT tracked with toplevel=false");
    assert_eq!(&inner_insert[calls], "100");
}

#[test]
#[ignore = "needs docker"]
fn connections_matches_manifest() {
    assert_log_matches_manifest(&generate_corpus(&["connections"]));
}
