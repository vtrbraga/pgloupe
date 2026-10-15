//! TLS modes against a real server with a self-signed certificate. Needs Docker.

#![allow(clippy::unwrap_used, clippy::expect_used, reason = "test code")]

mod support;

use std::fs;

use pgloupe::collect::conn::ConnSpec;

fn uses_tls(spec: &ConnSpec) -> Result<bool, String> {
    let mut client = spec.connect().map_err(|e| e.to_string())?;
    let row = client
        .query_one(
            "SELECT ssl FROM pg_stat_ssl WHERE pid = pg_backend_pid()",
            &[],
        )
        .map_err(|e| e.to_string())?;
    Ok(row.get(0))
}

#[test]
#[ignore = "needs Docker"]
fn tls_modes_do_what_libpq_does() -> Result<(), String> {
    let (pg, cert) = support::Pg::start_tls(17)?;
    let base = format!(
        "postgres://postgres:{}@localhost:{}/postgres",
        pg.password, pg.port
    );
    let root = std::env::temp_dir().join(format!("pgloupe-root-{}.pem", std::process::id()));
    fs::write(&root, cert).map_err(|e| e.to_string())?;

    // prefer and require encrypt without checking the certificate.
    for mode in ["prefer", "require"] {
        let spec = ConnSpec::parse(&format!("{base}?sslmode={mode}"), None, None)
            .map_err(|e| e.to_string())?;
        assert!(uses_tls(&spec)?, "{mode} should use TLS");
    }
    // verify-full with the certificate as root works for the name it was made for.
    let spec = ConnSpec::parse(
        &format!("{base}?sslmode=verify-full"),
        None,
        Some(root.clone()),
    )
    .map_err(|e| e.to_string())?;
    assert!(uses_tls(&spec)?);
    // verify-full without that root fails, and never falls back to plain text.
    let spec = ConnSpec::parse(&format!("{base}?sslmode=verify-full"), None, None)
        .map_err(|e| e.to_string())?;
    let err = spec
        .connect()
        .err()
        .ok_or("verify-full without a trusted root must fail")?
        .to_string();
    assert!(!err.contains(&pg.password), "{err}");
    // disable stays plain.
    let spec = ConnSpec::parse(&format!("{base}?sslmode=disable"), None, None)
        .map_err(|e| e.to_string())?;
    assert!(!uses_tls(&spec)?);
    fs::remove_file(root).map_err(|e| e.to_string())?;
    Ok(())
}
