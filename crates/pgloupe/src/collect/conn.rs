//! Connecting to a server: connection string, password sources, TLS, and a read-only session.

use std::{
    fs,
    path::{Path, PathBuf},
    str::FromStr as _,
    sync::Arc,
};

use postgres::{Client, Config, config::Host};
use rustls::{
    ClientConfig, DigitallySignedStruct, Error as TlsError, SignatureScheme,
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    pki_types::{CertificateDer, ServerName, UnixTime},
};
use tokio_postgres_rustls::MakeRustlsConnect;

use super::CollectError;

/// How the connection treats TLS, as libpq's `sslmode` does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SslMode {
    /// Never use TLS.
    Disable,
    /// Use TLS when the server offers it, else go on without.
    Prefer,
    /// Require TLS; the certificate is not checked (libpq's `require`).
    Require,
    /// Require TLS and check the certificate chain and the host name.
    VerifyFull,
}

/// A parsed `--conn` value and where its password comes from.
#[derive(Debug, Clone)]
pub struct ConnSpec {
    config: Config,
    ssl: SslMode,
    root_cert: Option<PathBuf>,
}

/// Splits `sslmode=…` out of a connection string, since the client library only knows three modes.
///
/// Works on URLs (`?sslmode=verify-full&…`) and key/value strings (`sslmode=verify-full`).
fn split_sslmode(dsn: &str) -> (String, Option<String>) {
    let mut found = None;
    let url = dsn.starts_with("postgres://") || dsn.starts_with("postgresql://");
    let cleaned = if url {
        match dsn.split_once('?') {
            Some((base, query)) => {
                let kept: Vec<&str> = query
                    .split('&')
                    .filter(|pair| {
                        if let Some(value) = pair.strip_prefix("sslmode=") {
                            found = Some(value.to_owned());
                            false
                        } else {
                            true
                        }
                    })
                    .collect();
                if kept.is_empty() {
                    base.to_owned()
                } else {
                    format!("{base}?{}", kept.join("&"))
                }
            }
            None => dsn.to_owned(),
        }
    } else {
        dsn.split_whitespace()
            .filter(|pair| {
                if let Some(value) = pair.strip_prefix("sslmode=") {
                    found = Some(value.trim_matches('\'').to_owned());
                    false
                } else {
                    true
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    };
    (cleaned, found)
}

fn parse_ssl(value: Option<&str>) -> Result<SslMode, CollectError> {
    match value {
        None | Some("prefer") => Ok(SslMode::Prefer),
        Some("disable") => Ok(SslMode::Disable),
        Some("require" | "allow") => Ok(SslMode::Require),
        Some("verify-ca" | "verify-full") => Ok(SslMode::VerifyFull),
        Some(other) => Err(CollectError::Dsn(format!(
            "unknown sslmode {other:?}; use disable, prefer, require or verify-full"
        ))),
    }
}

impl ConnSpec {
    /// Parses `dsn` (a `postgres://` URL or `key=value` pairs).
    ///
    /// The password is taken from the string, else from the environment variable `password_env`
    /// (default `PGPASSWORD`), else from `~/.pgpass`.
    ///
    /// # Errors
    ///
    /// Fails if the string is not a valid connection string or `sslmode` is unknown.
    pub fn parse(
        dsn: &str,
        password_env: Option<&str>,
        root_cert: Option<PathBuf>,
    ) -> Result<ConnSpec, CollectError> {
        let (cleaned, mode) = split_sslmode(dsn);
        let ssl = parse_ssl(
            mode.as_deref()
                .or_else(|| root_cert.is_some().then_some("verify-full")),
        )?;
        let mut config = Config::from_str(&cleaned).map_err(|_unparsed| {
            CollectError::Dsn(
                "not a valid connection string; use postgres://user@host:5432/dbname or key=value pairs"
                    .to_owned(),
            )
        })?;
        if config.get_password().is_none() {
            let from_env = std::env::var(password_env.unwrap_or("PGPASSWORD")).ok();
            let found = from_env.or_else(|| pgpass_password(&config));
            if let Some(password) = found {
                config.password(password);
            }
        }
        if config.get_application_name().is_none() {
            config.application_name("pgloupe");
        }
        Ok(ConnSpec {
            config,
            ssl,
            root_cert,
        })
    }

    /// The server this spec names, without secrets: `host:port/dbname as user`.
    #[must_use]
    pub fn describe(&self) -> String {
        let host = self.config.get_hosts().first().map_or_else(
            || "localhost".to_owned(),
            |h| match h {
                Host::Tcp(name) => name.clone(),
                Host::Unix(path) => path.display().to_string(),
            },
        );
        let port = self.config.get_ports().first().copied().unwrap_or(5432);
        format!(
            "{host}:{port}/{} as {}",
            self.config.get_dbname().unwrap_or("-"),
            self.config.get_user().unwrap_or("-")
        )
    }

    /// Removes this spec's password (and its URL-encoded form) from `text`.
    #[must_use]
    pub fn redact(&self, text: &str) -> String {
        redact_password(text, self.config.get_password().map(<[u8]>::to_vec))
    }

    /// Connects, requires TLS as asked, and makes the session read-only with a short timeout.
    ///
    /// # Errors
    ///
    /// Fails (with the password removed from the message) if the server cannot be reached, TLS
    /// cannot be negotiated as `sslmode` demands, or the login is refused.
    pub fn connect(&self) -> Result<Client, CollectError> {
        let mut config = self.config.clone();
        config.ssl_mode(match self.ssl {
            SslMode::Disable => postgres::config::SslMode::Disable,
            SslMode::Prefer => postgres::config::SslMode::Prefer,
            SslMode::Require | SslMode::VerifyFull => postgres::config::SslMode::Require,
        });
        let failed =
            |error: postgres::Error| CollectError::Connect(self.redact(&error_chain(&error)));
        let mut client = if self.ssl == SslMode::Disable {
            config.connect(postgres::NoTls).map_err(failed)?
        } else {
            let tls = MakeRustlsConnect::new(self.tls_config()?);
            config.connect(tls).map_err(failed)?
        };
        client
            .batch_execute(
                "SET default_transaction_read_only = on; SET statement_timeout = '15s'; \
                 SET idle_in_transaction_session_timeout = '60s'",
            )
            .map_err(|error| CollectError::Query {
                step: "making the session read-only",
                message: self.redact(&error_chain(&error)),
            })?;
        Ok(client)
    }

    fn tls_config(&self) -> Result<ClientConfig, CollectError> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let builder = ClientConfig::builder_with_provider(Arc::clone(&provider))
            .with_safe_default_protocol_versions()
            .map_err(|e| CollectError::Connect(format!("TLS setup: {e}")))?;
        if self.ssl != SslMode::VerifyFull {
            // libpq's `prefer` and `require` encrypt without checking the certificate.
            return Ok(builder
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(AcceptAny { provider }))
                .with_no_client_auth());
        }
        let mut roots = rustls::RootCertStore::empty();
        if let Some(path) = &self.root_cert {
            let pem = fs::read(path).map_err(|source| CollectError::Io {
                path: path.display().to_string(),
                source,
            })?;
            for cert in rustls_native_certs::load_native_certs().certs {
                // A custom bundle adds to the system roots rather than replacing them.
                drop(roots.add(cert));
            }
            let mut reader = std::io::Cursor::new(pem);
            for cert in rustls_pemfile_certs(&mut reader) {
                roots.add(cert).map_err(|e| {
                    CollectError::Dsn(format!("{}: not a usable certificate: {e}", path.display()))
                })?;
            }
        } else {
            for cert in rustls_native_certs::load_native_certs().certs {
                drop(roots.add(cert));
            }
        }
        Ok(builder.with_root_certificates(roots).with_no_client_auth())
    }
}

/// PEM certificates in `reader`, in order (blocks that are not certificates are skipped).
fn rustls_pemfile_certs(reader: &mut impl std::io::BufRead) -> Vec<CertificateDer<'static>> {
    let mut text = String::new();
    if reader.read_to_string(&mut text).is_err() {
        return Vec::new();
    }
    let mut certs = Vec::new();
    for block in text.split("-----BEGIN CERTIFICATE-----").skip(1) {
        let Some((body, _)) = block.split_once("-----END CERTIFICATE-----") else {
            continue;
        };
        let b64: String = body.chars().filter(|c| !c.is_whitespace()).collect();
        if let Some(der) = decode_base64(&b64) {
            certs.push(CertificateDer::from(der));
        }
    }
    certs
}

/// Standard base64 (with padding) to bytes.
fn decode_base64(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let (mut acc, mut bits) = (0_u32, 0_u32);
    for byte in text.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(u8::try_from((acc >> bits) & 0xff).ok()?);
        }
    }
    Some(out)
}

/// Accepts any certificate: what libpq's `prefer` and `require` do (they only encrypt).
#[derive(Debug)]
struct AcceptAny {
    provider: Arc<rustls::crypto::CryptoProvider>,
}

impl ServerCertVerifier for AcceptAny {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// `error` and its causes, joined with `: `.
fn error_chain(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}

fn redact_password(text: &str, password: Option<Vec<u8>>) -> String {
    let Some(password) = password.filter(|p| !p.is_empty()) else {
        return text.to_owned();
    };
    let plain = String::from_utf8_lossy(&password).into_owned();
    let encoded: String = plain
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                char::from(b).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect();
    text.replace(&plain, "[password]")
        .replace(&encoded, "[password]")
}

/// The password `~/.pgpass` has for this host, port, database and user, if any.
fn pgpass_password(config: &Config) -> Option<String> {
    let home = std::env::var_os("HOME")?;
    let path = std::env::var_os("PGPASSFILE")
        .map_or_else(|| Path::new(&home).join(".pgpass"), PathBuf::from);
    let text = fs::read_to_string(path).ok()?;
    let host = config.get_hosts().first().map_or_else(
        || "localhost".to_owned(),
        |h| match h {
            Host::Tcp(name) => name.clone(),
            Host::Unix(_) => "localhost".to_owned(),
        },
    );
    let port = config
        .get_ports()
        .first()
        .copied()
        .unwrap_or(5432)
        .to_string();
    pgpass_lookup(
        &text,
        &host,
        &port,
        config.get_dbname().unwrap_or(""),
        config.get_user().unwrap_or(""),
    )
}

/// The first `host:port:db:user:password` line of `file` that matches (`*` matches anything).
fn pgpass_lookup(file: &str, host: &str, port: &str, db: &str, user: &str) -> Option<String> {
    for line in file
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
    {
        let mut fields: Vec<String> = Vec::new();
        let mut current = String::new();
        let mut chars = line.chars();
        while let Some(c) = chars.next() {
            match c {
                '\\' => {
                    if let Some(next) = chars.next() {
                        current.push(next);
                    }
                }
                ':' if fields.len() < 4 => fields.push(std::mem::take(&mut current)),
                _ => current.push(c),
            }
        }
        fields.push(current);
        if fields.len() != 5 {
            continue;
        }
        let ok = |pattern: &str, value: &str| pattern == "*" || pattern == value;
        if ok(&fields[0], host)
            && ok(&fields[1], port)
            && ok(&fields[2], db)
            && ok(&fields[3], user)
        {
            return fields.pop();
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sslmode_is_split_out_of_urls_and_key_value_strings() {
        assert_eq!(
            split_sslmode("postgres://u:p@h:5432/db?sslmode=verify-full&connect_timeout=5"),
            (
                "postgres://u:p@h:5432/db?connect_timeout=5".to_owned(),
                Some("verify-full".to_owned())
            )
        );
        assert_eq!(
            split_sslmode("postgres://u@h/db?sslmode=require"),
            ("postgres://u@h/db".to_owned(), Some("require".to_owned()))
        );
        assert_eq!(
            split_sslmode("host=h dbname=d sslmode=require user=u"),
            (
                "host=h dbname=d user=u".to_owned(),
                Some("require".to_owned())
            )
        );
        assert_eq!(
            split_sslmode("postgres://u@h/db"),
            ("postgres://u@h/db".to_owned(), None)
        );
    }

    #[test]
    fn modes_map_and_unknown_ones_are_refused() {
        assert_eq!(parse_ssl(None).unwrap(), SslMode::Prefer);
        assert_eq!(parse_ssl(Some("verify-ca")).unwrap(), SslMode::VerifyFull);
        assert_eq!(parse_ssl(Some("disable")).unwrap(), SslMode::Disable);
        assert!(matches!(
            parse_ssl(Some("sometimes")),
            Err(CollectError::Dsn(_))
        ));
    }

    #[test]
    fn a_connection_string_is_described_without_its_password() {
        let spec = ConnSpec::parse(
            "postgres://app:s3cret!@db.example.com:6432/shop?sslmode=require",
            None,
            None,
        )
        .unwrap();
        let shown = spec.describe();
        assert_eq!(shown, "db.example.com:6432/shop as app");
        assert!(!shown.contains("s3cret"));
    }

    #[test]
    fn the_password_is_removed_from_messages_in_both_spellings() {
        let spec = ConnSpec::parse("postgres://app:p%40ss%2Fword@h/db", None, None).unwrap();
        let text = spec.redact("password authentication failed for p@ss/word (url: p%40ss%2Fword)");
        assert!(
            !text.contains("p@ss/word") && !text.contains("p%40ss%2Fword"),
            "{text}"
        );
        assert!(text.contains("[password]"));
    }

    #[test]
    fn a_bad_string_is_a_clear_error_that_does_not_echo_it() {
        let err = ConnSpec::parse("postgres://u:topsecret@h:notaport/db", None, None)
            .unwrap_err()
            .to_string();
        assert!(!err.contains("topsecret"), "{err}");
    }

    #[test]
    fn pgpass_lines_match_with_wildcards_and_escapes() {
        let file = "# comment\nother:5432:*:*:no\n*:5432:shop:app:pa\\:ss\nlocalhost:*:*:*:last\n";
        assert_eq!(
            pgpass_lookup(file, "h", "5432", "shop", "app").as_deref(),
            Some("pa:ss")
        );
        assert_eq!(
            pgpass_lookup(file, "localhost", "1", "x", "y").as_deref(),
            Some("last")
        );
        assert_eq!(pgpass_lookup(file, "h", "5432", "nope", "app"), None);
    }

    #[test]
    fn pem_bundles_decode_to_der_certificates() {
        let pem = "-----BEGIN CERTIFICATE-----\nAAEC\nAwQ=\n-----END CERTIFICATE-----\nnoise\n-----BEGIN CERTIFICATE-----\nBQYH\n-----END CERTIFICATE-----\n";
        let certs = rustls_pemfile_certs(&mut std::io::Cursor::new(pem));
        assert_eq!(certs.len(), 2);
        assert_eq!(certs[0].as_ref(), [0, 1, 2, 3, 4]);
        assert_eq!(certs[1].as_ref(), [5, 6, 7]);
    }
}
