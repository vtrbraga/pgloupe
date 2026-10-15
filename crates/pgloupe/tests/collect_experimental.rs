//! The experimental connectors against fake CLIs that replay payloads written from each vendor's
//! documentation. They show the connectors read what the docs say; they do not prove the real
//! services answer that way.
#![allow(clippy::unwrap_used, clippy::expect_used, reason = "test code")]

mod support;

use std::{path::PathBuf, time::Duration};

use jiff::Timestamp;
use pgloupe::collect::provider::{
    LogProvider, ProviderState, Request,
    aiven::Aiven,
    aws::{AwsCli, AwsRds},
    azure::AzureFlexible,
    cloudwatch::AwsCloudWatch,
    gcp::GcpCloudSql,
    heroku::Heroku,
    sql::SqlLogdir,
};
use support::fake_cli::{FakeCli, MemorySink, Reply};

fn request() -> Request {
    Request {
        since: "2026-10-14T00:00:00Z".parse::<Timestamp>().unwrap(),
        until: None,
    }
}

fn text(sink: &MemorySink, name: &str) -> String {
    String::from_utf8_lossy(&sink.files[name]).into_owned()
}

#[test]
fn cloudwatch_pages_by_token_and_resumes_without_duplicates() {
    let page1 = r#"{"events":[
        {"eventId":"e1","timestamp":1791972001000,"message":"2026-10-14 10:00:01 UTC:::[1]:LOG:  duration: 5 ms  statement: SELECT 1"},
        {"eventId":"e2","timestamp":1791972002000,"message":"2026-10-14 10:00:02 UTC:::[1]:ERROR:  boom\nSTATEMENT:  SELECT 2"}
      ],"nextToken":"tok-2"}"#;
    let page2 = r#"{"events":[{"eventId":"e3","timestamp":1791975601000,"message":"next hour"}]}"#;
    let again = r#"{"events":[
        {"eventId":"e3","timestamp":1791975601000,"message":"next hour"},
        {"eventId":"e4","timestamp":1791975602000,"message":"brand new"}]}"#;
    let fake = FakeCli::new("cw")
        .on(
            &["filter-log-events", "--next-token tok-2"],
            vec![Reply::Out(page2.to_owned())],
        )
        .on(
            &["filter-log-events"],
            vec![Reply::Out(page1.to_owned()), Reply::Out(again.to_owned())],
        );
    let provider = AwsCloudWatch::new(
        AwsCli::new(fake.program(), Some("eu-west-1".to_owned()), None)
            .with_retry_delay(Duration::ZERO),
        AwsCloudWatch::instance_group("prod"),
    );
    assert!(provider.experimental());
    let mut sink = MemorySink::default();
    let mut state = ProviderState::new();
    provider.fetch(&request(), &mut sink, &mut state).unwrap();
    let hour10 = text(&sink, "_aws_rds_instance_prod_postgresql-20261014-10.log");
    assert!(
        hour10.contains("SELECT 1\n") && hour10.contains("boom\nSTATEMENT:  SELECT 2\n"),
        "{hour10}"
    );
    assert_eq!(
        text(&sink, "_aws_rds_instance_prod_postgresql-20261014-11.log"),
        "next hour\n"
    );
    provider.fetch(&request(), &mut sink, &mut state).unwrap();
    assert_eq!(
        text(&sink, "_aws_rds_instance_prod_postgresql-20261014-11.log"),
        "next hour\nbrand new\n",
        "the repeated event is skipped, the new one added"
    );
    let calls = fake.calls();
    assert!(
        calls[0].contains("--log-group-name /aws/rds/instance/prod/postgresql"),
        "{calls:?}"
    );
    assert!(calls.iter().any(|c| c.contains("--next-token tok-2")));
}

#[test]
fn azure_lists_then_downloads_each_new_file_and_only_adds_what_grew() {
    let list = r#"[
      {"lastModifiedTime":"2026-10-14T10:30:00+00:00","name":"serverlogs/postgresql-2026-10-14_100000.log","sizeInKb":1,"type":"x","url":"u"},
      {"lastModifiedTime":"2026-10-01T10:30:00+00:00","name":"serverlogs/old.log","sizeInKb":1,"type":"x","url":"u"}]"#;
    let fake = FakeCli::new("az")
        .on(&["account show"], vec![Reply::Out(String::new())])
        .on(
            &["--version"],
            vec![Reply::Out("azure-cli 2.60\n".to_owned())],
        )
        .on(&["server-logs list"], vec![Reply::Out(list.to_owned())])
        .on(
            &["server-logs download"],
            vec![
                Reply::File {
                    path: "serverlogs/postgresql-2026-10-14_100000.log".to_owned(),
                    content: "line1\nline2\n".to_owned(),
                },
                Reply::File {
                    path: "serverlogs/postgresql-2026-10-14_100000.log".to_owned(),
                    content: "line1\nline2\nline3\n".to_owned(),
                },
            ],
        );
    let provider = AzureFlexible::new(fake.program(), "rg".to_owned(), "srv".to_owned(), None);
    assert!(provider.experimental());
    provider.check().unwrap();
    let mut sink = MemorySink::default();
    let mut state = ProviderState::new();
    provider.fetch(&request(), &mut sink, &mut state).unwrap();
    provider.fetch(&request(), &mut sink, &mut state).unwrap();
    assert_eq!(
        text(&sink, "serverlogs_postgresql-2026-10-14_100000.log"),
        "line1\nline2\nline3\n"
    );
    let downloads: Vec<String> = fake
        .calls()
        .into_iter()
        .filter(|c| c.contains("download"))
        .collect();
    assert_eq!(downloads.len(), 2);
    assert!(
        downloads[0].contains("--name serverlogs/postgresql-2026-10-14_100000.log"),
        "{downloads:?}"
    );
    assert!(
        !fake.calls().iter().any(|c| c.contains("old.log")),
        "files older than --since are not downloaded"
    );
}

#[test]
fn gcloud_reads_the_cloudsql_log_by_filter_and_skips_repeats() {
    let first = r#"[
      {"timestamp":"2026-10-14T10:00:01.250Z","insertId":"i1","textPayload":"2026-10-14 10:00:01.250 UTC [7]: LOG:  statement: SELECT 1"},
      {"timestamp":"2026-10-14T10:00:02Z","insertId":"i2","textPayload":"2026-10-14 10:00:02 UTC [7]: ERROR:  nope"}]"#;
    let second = r#"[
      {"timestamp":"2026-10-14T10:00:02Z","insertId":"i2","textPayload":"2026-10-14 10:00:02 UTC [7]: ERROR:  nope"},
      {"timestamp":"2026-10-14T10:00:03Z","insertId":"i3","textPayload":"2026-10-14 10:00:03 UTC [7]: LOG:  later"}]"#;
    let fake = FakeCli::new("gcloud")
        .on(
            &["--version"],
            vec![Reply::Out("Google Cloud SDK 480\n".to_owned())],
        )
        .on(
            &["auth list"],
            vec![Reply::Out("me@example.com\n".to_owned())],
        )
        .on(
            &["logging read"],
            vec![Reply::Out(first.to_owned()), Reply::Out(second.to_owned())],
        );
    let provider = GcpCloudSql::new(fake.program(), "proj".to_owned(), "prod".to_owned());
    assert!(provider.experimental());
    provider.check().unwrap();
    let mut sink = MemorySink::default();
    let mut state = ProviderState::new();
    provider.fetch(&request(), &mut sink, &mut state).unwrap();
    provider.fetch(&request(), &mut sink, &mut state).unwrap();
    let all = text(&sink, "cloudsql-prod-20261014-10.log");
    assert_eq!(all.matches("ERROR:  nope").count(), 1, "{all}");
    assert!(all.contains("later"));
    let calls = fake.calls();
    assert!(
        calls
            .iter()
            .any(|c| c.contains("database_id=\"proj:prod\"") && c.contains("--project proj")),
        "{calls:?}"
    );
    assert!(
        calls[calls.len() - 1].contains("timestamp>=\"2026-10-14T10:00:02Z\""),
        "second call resumes from the cursor: {calls:?}"
    );
}

#[test]
fn aiven_and_heroku_read_recent_entries_and_dedupe_across_runs() {
    let aiven_out = r#"{"logs":[
      {"time":"2026-10-14T10:00:01Z","unit":"postgres","msg":"2026-10-14 10:00:01 UTC [3]: LOG:  one"},
      {"time":"2026-10-14T10:00:02Z","unit":"postgres","msg":"2026-10-14 10:00:02 UTC [3]: LOG:  two"}]}"#;
    let fake = FakeCli::new("avn")
        .on(&["--version"], vec![Reply::Out("avn 4\n".to_owned())])
        .on(&["user", "info"], vec![Reply::Out("{}".to_owned())])
        .on(
            &["service", "logs", "pg-prod"],
            vec![Reply::Out(aiven_out.to_owned())],
        );
    let aiven = Aiven::new(fake.program(), "myproj".to_owned(), "pg-prod".to_owned());
    assert!(aiven.experimental());
    aiven.check().unwrap();
    let mut sink = MemorySink::default();
    let mut state = ProviderState::new();
    aiven.fetch(&request(), &mut sink, &mut state).unwrap();
    aiven.fetch(&request(), &mut sink, &mut state).unwrap();
    assert_eq!(
        text(&sink, "aiven-pg-prod-20261014-10.log").lines().count(),
        2
    );
    assert!(fake.calls().iter().any(|c| c.contains("--project myproj")));

    let heroku_out = "2026-10-14T10:00:01.000000+00:00 app[postgres.9]: [DATABASE] LOG:  hello\n\
                      2026-10-14T10:00:02.000000+00:00 app[postgres.9]: [DATABASE] LOG:  world\n";
    let fake = FakeCli::new("heroku")
        .on(&["--version"], vec![Reply::Out("heroku/9\n".to_owned())])
        .on(&["auth:whoami"], vec![Reply::Out("me\n".to_owned())])
        .on(
            &["logs", "--ps postgres", "--app shop"],
            vec![Reply::Out(heroku_out.to_owned())],
        );
    let heroku = Heroku::new(fake.program(), "shop".to_owned());
    assert!(heroku.experimental());
    heroku.check().unwrap();
    let mut sink = MemorySink::default();
    let mut state = ProviderState::new();
    heroku.fetch(&request(), &mut sink, &mut state).unwrap();
    heroku.fetch(&request(), &mut sink, &mut state).unwrap();
    assert_eq!(
        text(&sink, "heroku-shop-20261014-10.log"),
        "[DATABASE] LOG:  hello\n[DATABASE] LOG:  world\n"
    );
}

#[test]
fn only_aws_rds_is_called_supported() {
    let program = PathBuf::from("/nonexistent");
    let supported = AwsRds::new(program.clone(), "x".to_owned(), None, None);
    assert!(!supported.experimental());
    let others: Vec<(&str, bool)> = vec![
        (
            "aws-cloudwatch",
            AwsCloudWatch::new(AwsCli::new(program.clone(), None, None), "g".to_owned())
                .experimental(),
        ),
        (
            "azure-flexible",
            AzureFlexible::new(program.clone(), "r".to_owned(), "s".to_owned(), None)
                .experimental(),
        ),
        (
            "gcp-cloudsql",
            GcpCloudSql::new(program.clone(), "p".to_owned(), "i".to_owned()).experimental(),
        ),
        (
            "aiven",
            Aiven::new(program.clone(), "p".to_owned(), "s".to_owned()).experimental(),
        ),
        (
            "heroku",
            Heroku::new(program.clone(), "a".to_owned()).experimental(),
        ),
    ];
    for (name, experimental) in others {
        assert!(
            experimental,
            "{name} must stay experimental until confirmed on a real account"
        );
    }
    let spec = pgloupe::collect::conn::ConnSpec::parse("postgres://u@h/db", None, None).unwrap();
    assert!(SqlLogdir::new(spec).experimental());
}

#[test]
fn a_missing_tool_names_how_to_install_it_for_every_vendor() {
    let missing = PathBuf::from("/nonexistent/tool");
    let checks: Vec<(&str, Result<(), pgloupe::collect::provider::FetchError>)> = vec![
        (
            "az",
            AzureFlexible::new(missing.clone(), "r".to_owned(), "s".to_owned(), None).check(),
        ),
        (
            "gcloud",
            GcpCloudSql::new(missing.clone(), "p".to_owned(), "i".to_owned()).check(),
        ),
        (
            "avn",
            Aiven::new(missing.clone(), "p".to_owned(), "s".to_owned()).check(),
        ),
        (
            "heroku",
            Heroku::new(missing.clone(), "a".to_owned()).check(),
        ),
        (
            "aws",
            AwsCloudWatch::new(AwsCli::new(missing, None, None), "g".to_owned()).check(),
        ),
    ];
    for (name, result) in checks {
        let message = result.unwrap_err().to_string();
        assert!(message.contains("install"), "{name}: {message}");
    }
}
