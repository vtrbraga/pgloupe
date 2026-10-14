//! Database connections and lock-state polling.

use std::{
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context as _, bail};
use postgres::{Client, Config, NoTls, SimpleQueryMessage};

/// `application_name` of setup and polling sessions.
pub const CONTROL_APP: &str = "synth:control";

/// Backstop so a bug in a scenario fails with a lock timeout instead of hanging forever.
const SESSION_OPTIONS: &str = "-c lock_timeout=30s";

/// Control sessions neither log nor track their statements, so they leave no trace in the corpus.
const CONTROL_OPTIONS: &str =
    "-c lock_timeout=30s -c log_min_duration_statement=-1 -c pg_stat_statements.track=none";

/// Where the container's server listens on the host.
#[derive(Debug, Clone, Copy)]
pub struct Target {
    /// Host port mapped to 5432.
    pub port: u16,
}

impl Target {
    /// Connects as `user` to `db`, tagging the session with `app`.
    ///
    /// # Errors
    ///
    /// Fails if the connection is refused.
    pub fn connect(self, db: &str, user: &str, app: &str) -> anyhow::Result<Client> {
        self.connect_with(db, user, app, SESSION_OPTIONS)
    }

    fn connect_with(
        self,
        db: &str,
        user: &str,
        app: &str,
        options: &str,
    ) -> anyhow::Result<Client> {
        Config::new()
            .host("127.0.0.1")
            .port(self.port)
            .dbname(db)
            .user(user)
            .application_name(app)
            .options(options)
            .connect(NoTls)
            .with_context(|| format!("connecting to {db} as {user}"))
    }

    /// Connects as superuser with statement logging off, for setup and polling.
    ///
    /// # Errors
    ///
    /// Fails if the connection or the `SET` fails.
    pub fn control(self, db: &str) -> anyhow::Result<Client> {
        self.connect_with(db, "postgres", CONTROL_APP, CONTROL_OPTIONS)
    }
}

/// First column of the first row of `sql`, as text, via the simple protocol.
///
/// # Errors
///
/// Fails if the query fails or returns no non-null value.
pub fn query_scalar(client: &mut Client, sql: &str) -> anyhow::Result<String> {
    let messages = client
        .simple_query(sql)
        .with_context(|| format!("running {sql}"))?;
    messages
        .iter()
        .find_map(|message| match message {
            SimpleQueryMessage::Row(row) => row.get(0).map(str::to_owned),
            _ => None,
        })
        .with_context(|| format!("no value from {sql}"))
}

/// Backend process id of `client`'s session.
///
/// # Errors
///
/// Fails if the query fails.
pub fn backend_pid(client: &mut Client) -> anyhow::Result<i32> {
    query_scalar(client, "SELECT pg_backend_pid()")?
        .parse()
        .context("pg_backend_pid() is not an integer")
}

/// Polls until backend `pid` waits on a lock.
///
/// # Errors
///
/// Fails if polling fails or `timeout` elapses first.
pub fn wait_for_lock_wait(control: &mut Client, pid: i32, timeout: Duration) -> anyhow::Result<()> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        // Simple protocol on purpose: prepared statements would consume numbered
        // statement names and make the logged names depend on polling timing.
        let waiting: i64 = query_scalar(
            control,
            &format!("SELECT count(*) FROM pg_locks WHERE pid = {pid} AND NOT granted"),
        )?
        .parse()
        .context("count(*) is not an integer")?;
        if waiting > 0 {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(10));
    }
    bail!("backend {pid} did not wait on a lock within {timeout:?}")
}
