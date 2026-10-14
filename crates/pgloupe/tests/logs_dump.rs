//! End-to-end tests of `pgloupe logs dump` on the fixtures in `fixtures/logs/`.

use std::{
    fs, io,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use flate2::{Compression, write::GzEncoder};

const PREFIX: &str = "%m [%p] %q%u@%d/%a %Q ";

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/logs")
        .join(name)
}

fn dump(args: &[&str], file: &Path) -> io::Result<Output> {
    Command::new(env!("CARGO_BIN_EXE_pgloupe"))
        .args(["logs", "dump"])
        .arg(file)
        .args(args)
        .output()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn scratch(name: &str, bytes: &[u8]) -> io::Result<PathBuf> {
    let path = std::env::temp_dir().join(format!("pgloupe-logs-{}-{name}", std::process::id()));
    fs::write(&path, bytes)?;
    Ok(path)
}

fn gzip(bytes: &[u8]) -> io::Result<Vec<u8>> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(bytes)?;
    encoder.finish()
}

#[test]
fn csv_dump_matches_the_golden_file() -> io::Result<()> {
    let out = dump(&[], &fixture("sample.csv"))?;
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        fs::read_to_string(fixture("sample.golden.ndjson"))?
    );
    assert!(
        stderr(&out).contains("decoded 8 records from 8 lines"),
        "{}",
        stderr(&out)
    );
    Ok(())
}

#[test]
fn json_dump_matches_the_same_golden_file() -> io::Result<()> {
    let out = dump(&[], &fixture("sample.json"))?;
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        fs::read_to_string(fixture("sample.golden.ndjson"))?
    );
    Ok(())
}

#[test]
fn stderr_dump_matches_its_own_golden_file() -> io::Result<()> {
    let out = dump(&["--prefix", PREFIX], &fixture("sample.log"))?;
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        fs::read_to_string(fixture("sample.stderr.golden.ndjson"))?
    );
    assert!(
        stderr(&out).contains("decoded 8 records from 18 lines"),
        "{}",
        stderr(&out)
    );
    Ok(())
}

#[test]
fn gzip_input_is_decoded_whatever_the_name() -> io::Result<()> {
    let bytes = gzip(&fs::read(fixture("sample.json"))?)?;
    let path = scratch("renamed.log", &bytes)?;
    let out = dump(&[], &path)?;
    fs::remove_file(&path)?;
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        fs::read_to_string(fixture("sample.golden.ndjson"))?
    );
    Ok(())
}

#[test]
fn truncated_gzip_exits_one() -> io::Result<()> {
    let text = fs::read_to_string(fixture("sample.json"))?.repeat(2_000);
    let mut bytes = gzip(text.as_bytes())?;
    bytes.truncate(bytes.len() / 2);
    let path = scratch("truncated.gz", &bytes)?;
    let out = dump(&[], &path)?;
    fs::remove_file(&path)?;
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("could not read"), "{}", stderr(&out));
    assert!(
        !stdout(&out).is_empty(),
        "records before the break are kept"
    );
    Ok(())
}

#[test]
fn limit_stops_early() -> io::Result<()> {
    let out = dump(&["--limit", "2"], &fixture("sample.csv"))?;
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out).lines().count(), 2);
    Ok(())
}

#[test]
fn wrong_prefix_exits_one_and_names_the_first_unmatched_line() -> io::Result<()> {
    let out = dump(
        &["--format", "stderr", "--prefix", "%t [%p] "],
        &fixture("sample.log"),
    )?;
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(stdout(&out), "");
    let message = stderr(&out);
    assert!(message.contains("decoded 0 records"), "{message}");
    assert!(message.contains("first at line 1"), "{message}");
    assert!(message.contains("log_line_prefix"), "{message}");
    Ok(())
}

#[test]
fn stderr_logs_need_a_prefix() -> io::Result<()> {
    let out = dump(&[], &fixture("sample.log"))?;
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("--prefix"), "{}", stderr(&out));
    Ok(())
}

#[test]
fn a_bad_prefix_is_reported_with_the_escape() -> io::Result<()> {
    let out = dump(
        &["--format", "stderr", "--prefix", "%m %z "],
        &fixture("sample.log"),
    )?;
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("%z"), "{}", stderr(&out));
    Ok(())
}

#[test]
fn wrong_format_exits_one_instead_of_printing_nothing() -> io::Result<()> {
    let out = dump(&["--format", "csv"], &fixture("sample.json"))?;
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(stdout(&out), "");
    assert!(stderr(&out).contains("skipped"), "{}", stderr(&out));
    Ok(())
}

#[test]
fn empty_input_is_not_an_error() -> io::Result<()> {
    let path = scratch("empty.log", b"")?;
    let out = dump(&[], &path)?;
    fs::remove_file(&path)?;
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("decoded 0 records from 0 lines"),
        "{}",
        stderr(&out)
    );
    Ok(())
}

#[test]
fn binary_garbage_exits_one() -> io::Result<()> {
    let path = scratch("garbage.bin", &[0u8, 159, 146, 150, b'\n', 255, 254, b'\n'])?;
    let out = dump(&["--format", "json"], &path)?;
    fs::remove_file(&path)?;
    assert_eq!(out.status.code(), Some(1));
    Ok(())
}

#[test]
fn log_tz_resolves_ambiguous_abbreviations() -> io::Result<()> {
    let line = b"2026-10-14 07:20:09.289 BRT [42] LOG:  hello\n";
    let path = scratch("brt.log", line)?;
    let without = dump(&["--format", "stderr", "--prefix", "%m [%p] "], &path)?;
    assert_eq!(without.status.code(), Some(1));
    assert!(
        stderr(&without).contains("unknown timezone abbreviation 'BRT'"),
        "{}",
        stderr(&without)
    );
    let with = dump(
        &[
            "--format", "stderr", "--prefix", "%m [%p] ", "--log-tz", "-03",
        ],
        &path,
    )?;
    fs::remove_file(&path)?;
    assert!(with.status.success(), "{}", stderr(&with));
    assert!(
        stdout(&with).contains("\"ts\":\"2026-10-14T10:20:09.289Z\""),
        "{}",
        stdout(&with)
    );
    Ok(())
}

#[test]
fn bad_log_tz_is_a_usage_error() -> io::Result<()> {
    let out = dump(&["--log-tz", "BRT"], &fixture("sample.csv"))?;
    assert_eq!(out.status.code(), Some(2));
    Ok(())
}

#[test]
fn missing_file_exits_one_with_the_path() -> io::Result<()> {
    let out = dump(&[], Path::new("/nonexistent/postgresql.log"))?;
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("/nonexistent/postgresql.log"),
        "{}",
        stderr(&out)
    );
    Ok(())
}
