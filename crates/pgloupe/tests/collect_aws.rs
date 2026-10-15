//! The `aws-rds` connector against a fake `aws`.

#![allow(clippy::unwrap_used, clippy::expect_used, reason = "test code")]

mod support;

use std::time::Duration;

use jiff::Timestamp;
use pgloupe::collect::provider::{
    FetchError, LogProvider as _, ProviderState, Request, aws::AwsRds,
};
use support::fake_cli::{FakeCli, MemorySink, Reply};

fn provider(fake: &FakeCli) -> AwsRds {
    AwsRds::new(
        fake.program(),
        "prod-db".to_owned(),
        Some("eu-west-1".to_owned()),
        None,
    )
    .with_retry_delay(Duration::ZERO)
}

fn request() -> Request {
    Request {
        since: "2026-10-14T00:00:00Z".parse::<Timestamp>().unwrap(),
        until: None,
    }
}

fn list(files: &[(&str, i64, i64)]) -> String {
    let entries: Vec<String> = files
        .iter()
        .map(|(name, written, size)| {
            format!(r#"{{"LogFileName":"{name}","LastWritten":{written},"Size":{size}}}"#)
        })
        .collect();
    format!(r#"{{"DescribeDBLogFiles":[{}]}}"#, entries.join(","))
}

fn portion(data: &str, marker: &str, pending: bool) -> String {
    format!(
        r#"{{"LogFileData":{},"Marker":"{marker}","AdditionalDataPending":{pending}}}"#,
        serde_json::to_string(data).unwrap()
    )
}

#[test]
fn files_are_listed_paged_and_written_in_order() {
    let fake = FakeCli::new("aws-basic")
        .on(
            &["describe-db-log-files"],
            vec![Reply::Out(list(&[
                ("error/postgresql.log.2026-10-14-11", 2_000, 12),
                ("error/postgresql.log.2026-10-14-10", 1_000, 11),
            ]))],
        )
        .on(
            &["2026-10-14-10", "--marker 0 --no-paginate"],
            vec![Reply::Out(portion("line one\n", "1:9", true))],
        )
        .on(
            &["2026-10-14-10", "--marker 1:9 --no-paginate"],
            vec![Reply::Out(portion("two\n", "1:13", false))],
        )
        .on(
            &["2026-10-14-11", "--marker 0 --no-paginate"],
            vec![Reply::Out(portion("eleven:00\n", "2:10", false))],
        );
    let mut sink = MemorySink::default();
    let mut state = ProviderState::new();
    let summary = provider(&fake)
        .fetch(&request(), &mut sink, &mut state)
        .unwrap();
    assert_eq!(
        sink.files["error_postgresql.log.2026-10-14-10"],
        b"line one\ntwo\n"
    );
    assert_eq!(
        sink.files["error_postgresql.log.2026-10-14-11"],
        b"eleven:00\n"
    );
    assert_eq!(summary.bytes, 9 + 4 + 10);
    assert_eq!(state["marker:error/postgresql.log.2026-10-14-10"], "1:13");
    let calls = fake.calls();
    assert!(
        calls[0].contains("--region eu-west-1")
            && calls[0].contains("--db-instance-identifier prod-db"),
        "{calls:?}"
    );
    assert!(
        calls[0].contains("--file-last-written 1791936000000"),
        "{}",
        calls[0]
    );
}

#[test]
fn a_second_run_continues_from_the_marker_and_skips_finished_files() {
    let fake = FakeCli::new("aws-resume")
        .on(
            &["describe-db-log-files"],
            vec![
                Reply::Out(list(&[("log/a", 1_000, 5)])),
                Reply::Out(list(&[("log/a", 1_500, 9)])),
                Reply::Out(list(&[("log/a", 1_500, 9)])),
            ],
        )
        .on(
            &["--marker 0 --no-paginate"],
            vec![Reply::Out(portion("12345", "1:5", false))],
        )
        .on(
            &["--marker 1:5 --no-paginate"],
            vec![Reply::Out(portion("6789", "1:9", false))],
        );
    let mut sink = MemorySink::default();
    let mut state = ProviderState::new();
    let provider = provider(&fake);
    provider.fetch(&request(), &mut sink, &mut state).unwrap();
    provider.fetch(&request(), &mut sink, &mut state).unwrap();
    assert_eq!(sink.files["log_a"], b"123456789");
    let before = fake.calls().len();
    let summary = provider.fetch(&request(), &mut sink, &mut state).unwrap();
    assert_eq!(summary.bytes, 0, "nothing new");
    assert_eq!(
        fake.calls().len(),
        before + 1,
        "only the listing was asked for"
    );
}

#[test]
fn a_file_that_shrank_is_started_again_and_said_so() {
    let fake = FakeCli::new("aws-shrink")
        .on(
            &["describe-db-log-files"],
            vec![Reply::Out(list(&[("log/a", 9_000, 3)]))],
        )
        .on(
            &["--marker 0 --no-paginate"],
            vec![Reply::Out(portion("new", "1:3", false))],
        );
    let mut sink = MemorySink::default();
    sink.files
        .insert("log_a".to_owned(), b"old content that was longer".to_vec());
    let mut state = ProviderState::new();
    state.insert("marker:log/a".to_owned(), "1:27".to_owned());
    let summary = provider(&fake)
        .fetch(&request(), &mut sink, &mut state)
        .unwrap();
    assert_eq!(sink.files["log_a"], b"new");
    assert_eq!(summary.restarted, ["log_a"]);
}

#[test]
fn throttling_is_retried_and_other_failures_say_what_to_do() {
    let throttled = Reply::Fail { code: 254, stderr: "An error occurred (Throttling) when calling the DescribeDBLogFiles operation: Rate exceeded".to_owned() };
    let fake = FakeCli::new("aws-throttle").on(
        &["describe-db-log-files"],
        vec![throttled, Reply::Out(list(&[]))],
    );
    let mut sink = MemorySink::default();
    provider(&fake)
        .fetch(&request(), &mut sink, &mut ProviderState::new())
        .unwrap();
    assert_eq!(fake.calls().len(), 2);

    let denied = Reply::Fail {
        code: 254,
        stderr: "An error occurred (AccessDenied) when calling the DescribeDBLogFiles operation"
            .to_owned(),
    };
    let fake = FakeCli::new("aws-denied").on(&["describe-db-log-files"], vec![denied]);
    let err = provider(&fake)
        .fetch(
            &request(),
            &mut MemorySink::default(),
            &mut ProviderState::new(),
        )
        .err()
        .unwrap();
    assert!(
        err.to_string().contains("rds:DownloadDBLogFilePortion"),
        "{err}"
    );
}

#[test]
fn a_marker_that_never_advances_is_an_error_not_a_loop() {
    let fake = FakeCli::new("aws-stuck")
        .on(
            &["describe-db-log-files"],
            vec![Reply::Out(list(&[("log/a", 1, 100)]))],
        )
        .on(
            &["--marker 0 --no-paginate"],
            vec![Reply::Out(portion("", "0", true))],
        );
    let err = provider(&fake)
        .fetch(
            &request(),
            &mut MemorySink::default(),
            &mut ProviderState::new(),
        )
        .err()
        .unwrap();
    assert!(err.to_string().contains("does not advance"), "{err}");
}

#[test]
fn hostile_file_names_stay_inside_the_sink() {
    let fake = FakeCli::new("aws-names")
        .on(
            &["describe-db-log-files"],
            vec![Reply::Out(list(&[("../../etc/cron.d/x", 1, 2)]))],
        )
        .on(
            &["--marker 0 --no-paginate"],
            vec![Reply::Out(portion("ok", "1:2", false))],
        );
    let mut sink = MemorySink::default();
    provider(&fake)
        .fetch(&request(), &mut sink, &mut ProviderState::new())
        .unwrap();
    assert_eq!(sink.files.len(), 1);
    for name in sink.files.keys() {
        assert!(!name.contains('/') && !name.contains('\\'), "{name}");
    }
}

#[test]
fn a_missing_cli_and_expired_credentials_explain_themselves() {
    let missing = AwsRds::new("/nonexistent/aws".into(), "x".to_owned(), None, None);
    assert!(matches!(
        missing.check(),
        Err(FetchError::MissingCli { .. })
    ));

    let fake = FakeCli::new("aws-check")
        .on(
            &["--version"],
            vec![Reply::Out("aws-cli/2.17.0\n".to_owned())],
        )
        .on(
            &["sts get-caller-identity"],
            vec![Reply::Fail {
                code: 255,
                stderr:
                    "An error occurred (ExpiredToken) when calling the GetCallerIdentity operation"
                        .to_owned(),
            }],
        );
    let err = provider(&fake).check().unwrap_err().to_string();
    assert!(err.contains("sign in again"), "{err}");
    assert!(!provider(&fake).experimental());
}
