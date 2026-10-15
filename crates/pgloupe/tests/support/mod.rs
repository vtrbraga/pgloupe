//! A throwaway Postgres in Docker for the tests that need a live server.

#![allow(dead_code, reason = "each test file uses a part")]

use std::{
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

/// A container that is removed when dropped.
pub struct Pg {
    name: String,
    /// Host port mapped to 5432.
    pub port: u16,
    /// The superuser password.
    pub password: String,
}

fn docker(args: &[&str]) -> Result<String, String> {
    let out = Command::new("docker")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("docker: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_owned())
    }
}

impl Pg {
    /// Starts `postgres:<major>` with `pg_stat_statements` preloaded and the extension created.
    pub fn start(major: u32, extra: &[&str]) -> Result<Pg, String> {
        let name = format!("pgloupe-test-{}-{major}-{}", std::process::id(), nanos());
        let password = "s3cret-pw".to_owned();
        let image = format!("postgres:{major}");
        let mut args: Vec<&str> = vec![
            "run",
            "-d",
            "--rm",
            "--name",
            &name,
            "-e",
            "POSTGRES_PASSWORD=s3cret-pw",
            "-p",
            "127.0.0.1::5432",
            &image,
            "-c",
            "shared_preload_libraries=pg_stat_statements",
            "-c",
            "pg_stat_statements.track=all",
        ];
        for setting in extra {
            args.push("-c");
            args.push(setting);
        }
        docker(&args)?;
        let started = Instant::now();
        let port: u16 = loop {
            let mapped = docker(&["port", &name, "5432/tcp"]).unwrap_or_default();
            if let Some(port) = mapped
                .lines()
                .next()
                .and_then(|l| l.rsplit(':').next())
                .and_then(|p| p.parse().ok())
            {
                break port;
            }
            if started.elapsed() > Duration::from_secs(20) {
                return Err(format!("cannot read the port from {mapped:?}"));
            }
            thread::sleep(Duration::from_millis(200));
        };
        let pg = Pg {
            name,
            port,
            password,
        };
        pg.wait()?;
        pg.sql("CREATE EXTENSION IF NOT EXISTS pg_stat_statements")?;
        Ok(pg)
    }

    /// Starts a server with TLS on and a certificate for `localhost` signed by a throwaway CA.
    ///
    /// Returns the container and the CA certificate (PEM), which a client can trust as a root.
    pub fn start_tls(major: u32) -> Result<(Pg, String), String> {
        let name = format!(
            "pgloupe-test-{}-{major}-tls-{}",
            std::process::id(),
            nanos()
        );
        let image = format!("postgres:{major}");
        let script = "set -e; cd /var/lib/postgresql; \\
            openssl req -new -x509 -days 2 -nodes -subj /CN=testca -out ca.crt -keyout ca.key; \\
            openssl req -new -nodes -subj /CN=localhost -out server.csr -keyout server.key; \\
            printf 'subjectAltName=DNS:localhost,IP:127.0.0.1\\nbasicConstraints=CA:FALSE\\n' > ext.cnf; \\
            openssl x509 -req -in server.csr -CA ca.crt -CAkey ca.key -CAcreateserial -days 2 -extfile ext.cnf -out server.crt; \\
            chmod 600 server.key; chown postgres server.key server.crt; \\
            exec docker-entrypoint.sh postgres -c ssl=on \\
              -c ssl_cert_file=/var/lib/postgresql/server.crt \\
              -c ssl_key_file=/var/lib/postgresql/server.key \\
              -c shared_preload_libraries=pg_stat_statements";
        docker(&[
            "run",
            "-d",
            "--rm",
            "--name",
            &name,
            "-e",
            "POSTGRES_PASSWORD=s3cret-pw",
            "-p",
            "127.0.0.1::5432",
            "--entrypoint",
            "bash",
            &image,
            "-c",
            script,
        ])?;
        let started = Instant::now();
        let port: u16 = loop {
            let mapped = docker(&["port", &name, "5432/tcp"]).unwrap_or_default();
            if let Some(port) = mapped
                .lines()
                .next()
                .and_then(|l| l.rsplit(':').next())
                .and_then(|p| p.parse().ok())
            {
                break port;
            }
            if started.elapsed() > Duration::from_secs(20) {
                return Err("no port".to_owned());
            }
            thread::sleep(Duration::from_millis(200));
        };
        let pg = Pg {
            name,
            port,
            password: "s3cret-pw".to_owned(),
        };
        pg.wait()?;
        let cert = docker(&["exec", &pg.name, "cat", "/var/lib/postgresql/ca.crt"])?;
        Ok((pg, cert))
    }

    fn wait(&self) -> Result<(), String> {
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(60) {
            // The image first runs a temporary server on the socket only, then restarts for real:
            // only the TCP listener means the final server is up.
            let up = docker(&[
                "exec",
                &self.name,
                "pg_isready",
                "-h",
                "127.0.0.1",
                "-U",
                "postgres",
            ])
            .is_ok();
            if up && self.sql("SELECT 1").is_ok() {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(300));
        }
        Err("postgres did not become ready".to_owned())
    }

    /// Runs `sql` as the superuser through `psql` inside the container.
    pub fn sql(&self, sql: &str) -> Result<String, String> {
        docker(&[
            "exec", &self.name, "psql", "-U", "postgres", "-tA", "-c", sql,
        ])
    }

    /// A connection URL for the superuser.
    pub fn url(&self) -> String {
        format!(
            "postgres://postgres:{}@127.0.0.1:{}/postgres",
            self.password, self.port
        )
    }
}

impl Drop for Pg {
    fn drop(&mut self) {
        drop(docker(&["rm", "-f", &self.name]));
    }
}

fn nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos())
}
