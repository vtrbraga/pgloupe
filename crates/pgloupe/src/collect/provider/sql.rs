//! Reading the server's own log directory over SQL (`pg_ls_logdir`, `pg_read_binary_file`).
//!
//! **Experimental:** the role needs `pg_monitor` for the listing and superuser or
//! `pg_read_server_files` for the read; managed services usually forbid it.

use jiff::Timestamp;

use super::{FetchError, LogProvider, LogSink, ProviderState, Request, Summary};
use crate::collect::conn::ConnSpec;

/// Bytes read per call.
const CHUNK: i64 = 1 << 20;

/// Where the log files are read from.
#[derive(Debug, Clone)]
pub struct SqlLogdir {
    spec: ConnSpec,
}

impl SqlLogdir {
    /// A provider that connects with `spec`.
    #[must_use]
    pub fn new(spec: ConnSpec) -> Self {
        SqlLogdir { spec }
    }
}

fn failed(spec: &ConnSpec, what: &'static str, error: &postgres::Error) -> FetchError {
    let (message, hint) = error.as_db_error().map_or_else(
        || (error.to_string(), ""),
        |d| {
            (
                d.message().to_owned(),
                if d.code().code() == "42501" {
                    " (the role needs pg_monitor to list the log directory and pg_read_server_files, or superuser, to read the files)"
                } else {
                    ""
                },
            )
        },
    );
    FetchError::Output {
        provider: "sql",
        what,
        detail: spec.redact(&format!("{message}{hint}")),
    }
}

impl LogProvider for SqlLogdir {
    fn name(&self) -> &'static str {
        "sql"
    }

    fn check(&self) -> Result<(), FetchError> {
        let mut client = self.spec.connect().map_err(|e| FetchError::Output {
            provider: "sql",
            what: "connecting",
            detail: e.to_string(),
        })?;
        client
            .query("SELECT name FROM pg_ls_logdir() LIMIT 1", &[])
            .map_err(|e| failed(&self.spec, "pg_ls_logdir()", &e))
            .map(|_| ())
    }

    fn fetch(
        &self,
        request: &Request,
        sink: &mut dyn LogSink,
        state: &mut ProviderState,
    ) -> Result<Summary, FetchError> {
        let mut client = self.spec.connect().map_err(|e| FetchError::Output {
            provider: "sql",
            what: "connecting",
            detail: e.to_string(),
        })?;
        let directory: String = client
            .query_one("SELECT current_setting('log_directory')", &[])
            .map_err(|e| failed(&self.spec, "reading log_directory", &e))?
            .get(0);
        #[expect(clippy::cast_precision_loss, reason = "a Unix time in seconds")]
        let since = request.since.as_second() as f64;
        let files = client
            .query(
                "SELECT name, size, extract(epoch FROM modification)::bigint \
                 FROM pg_ls_logdir() WHERE modification >= to_timestamp($1::float8) \
                 ORDER BY modification, name",
                &[&since],
            )
            .map_err(|e| failed(&self.spec, "pg_ls_logdir()", &e))?;
        let mut summary = Summary::default();
        for row in files {
            let name: String = row.get(0);
            let size: i64 = row.get(1);
            let local = super::aws::safe_name(&name);
            let mut offset = i64::try_from(sink.stored(&local)).unwrap_or(0);
            if offset > size {
                sink.reset(&local)?;
                summary.restarted.push(local.clone());
                offset = 0;
            }
            let path = format!("{directory}/{name}");
            let mut wrote = false;
            while offset < size {
                let chunk: Vec<u8> = client
                    .query_one(
                        "SELECT pg_read_binary_file($1, $2, $3)",
                        &[&path, &offset, &CHUNK],
                    )
                    .map_err(|e| failed(&self.spec, "pg_read_binary_file()", &e))?
                    .get(0);
                if chunk.is_empty() {
                    break;
                }
                sink.append(&local, &chunk)?;
                offset += i64::try_from(chunk.len()).unwrap_or(0);
                summary.bytes += chunk.len() as u64;
                wrote = true;
            }
            if wrote {
                summary.files.push(local);
            }
            state.insert(format!("seen:{name}"), Timestamp::now().to_string());
            sink.checkpoint(state)?;
        }
        Ok(summary)
    }
}
