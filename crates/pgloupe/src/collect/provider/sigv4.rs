//! AWS Signature Version 4 for a `GET` with no query string and no body.

use std::fmt::{self, Write as _};

use hmac::{Hmac, KeyInit as _, Mac as _};
use jiff::Timestamp;
use sha2::{Digest as _, Sha256};

/// An AWS access key, its secret and, for temporary credentials, the session token.
#[derive(Clone)]
pub struct Credentials {
    /// The access key id.
    pub access_key: String,
    /// The secret access key.
    pub secret_key: String,
    /// The session token of temporary credentials (SSO, assumed roles).
    pub session_token: Option<String>,
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("access_key", &self.access_key)
            .finish_non_exhaustive()
    }
}

/// The headers to send with a signed request, besides `Host`.
#[derive(Debug, PartialEq, Eq)]
pub struct Signed {
    /// `(name, value)` pairs: `x-amz-date`, `authorization` and, when there is one, the token.
    pub headers: Vec<(&'static str, String)>,
}

/// Signs `GET https://{host}{path}` for `service` in `region` at `now`. `path` must already be
/// percent-encoded the way it is sent.
#[must_use]
pub fn sign_get(
    credentials: &Credentials,
    region: &str,
    service: &str,
    host: &str,
    path: &str,
    now: Timestamp,
) -> Signed {
    let stamp = now.strftime("%Y%m%dT%H%M%SZ").to_string();
    let date = &stamp[..8];
    let mut canonical_headers = format!("host:{host}\nx-amz-date:{stamp}\n");
    let mut signed_headers = "host;x-amz-date".to_owned();
    if let Some(token) = &credentials.session_token {
        let _ = writeln!(canonical_headers, "x-amz-security-token:{token}");
        signed_headers.push_str(";x-amz-security-token");
    }
    // Services other than S3 encode the path a second time when they canonicalise it.
    let canonical_path = path
        .split('/')
        .map(encode_segment)
        .collect::<Vec<_>>()
        .join("/");
    let canonical = format!(
        "GET\n{canonical_path}\n\n{canonical_headers}\n{signed_headers}\n{}",
        hex(&Sha256::digest(b""))
    );
    let scope = format!("{date}/{region}/{service}/aws4_request");
    let to_sign = format!(
        "AWS4-HMAC-SHA256\n{stamp}\n{scope}\n{}",
        hex(&Sha256::digest(canonical.as_bytes()))
    );
    let mut key = hmac(
        format!("AWS4{}", credentials.secret_key).as_bytes(),
        date.as_bytes(),
    );
    for part in [region, service, "aws4_request"] {
        key = hmac(&key, part.as_bytes());
    }
    let signature = hex(&hmac(&key, to_sign.as_bytes()));
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={signature}",
        credentials.access_key
    );
    let mut headers = vec![("x-amz-date", stamp), ("authorization", authorization)];
    if let Some(token) = &credentials.session_token {
        headers.push(("x-amz-security-token", token.clone()));
    }
    Signed { headers }
}

/// Percent-encodes everything but the unreserved characters `A-Z a-z 0-9 - _ . ~`.
#[must_use]
pub fn encode_segment(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    for byte in segment.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(byte));
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

#[expect(clippy::expect_used, reason = "HMAC accepts a key of any length")]
fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("any key length is valid");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example() -> Credentials {
        Credentials {
            access_key: "AKIDEXAMPLE".to_owned(),
            secret_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".to_owned(),
            session_token: None,
        }
    }

    /// The `get-vanilla` case of the AWS Signature Version 4 test suite.
    #[test]
    fn matches_the_aws_get_vanilla_vector() {
        let now: Timestamp = "2015-08-30T12:36:00Z".parse().unwrap();
        let signed = sign_get(
            &example(),
            "us-east-1",
            "service",
            "example.amazonaws.com",
            "/",
            now,
        );
        assert_eq!(
            signed.headers,
            vec![
                ("x-amz-date", "20150830T123600Z".to_owned()),
                (
                    "authorization",
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, SignedHeaders=host;x-amz-date, Signature=5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31".to_owned()
                ),
            ]
        );
    }

    #[test]
    fn a_session_token_is_signed_and_sent() {
        let mut credentials = example();
        credentials.session_token = Some("token".to_owned());
        let now: Timestamp = "2015-08-30T12:36:00Z".parse().unwrap();
        let signed = sign_get(&credentials, "us-east-1", "rds", "h", "/", now);
        let auth = &signed.headers[1].1;
        assert!(auth.contains("SignedHeaders=host;x-amz-date;x-amz-security-token"));
        assert_eq!(
            signed.headers[2],
            ("x-amz-security-token", "token".to_owned())
        );
    }

    #[test]
    fn debug_never_shows_the_secret() {
        let shown = format!("{:?}", example());
        assert!(shown.contains("AKIDEXAMPLE"));
        assert!(!shown.contains("wJalr"));
    }

    #[test]
    fn segments_encode_everything_but_unreserved_characters() {
        assert_eq!(
            encode_segment("postgresql.log.2026-10-16-20"),
            "postgresql.log.2026-10-16-20"
        );
        assert_eq!(encode_segment("a b/c"), "a%20b%2Fc");
    }
}
