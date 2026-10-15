//! The local server: routes, caching, and what it refuses.
#![allow(clippy::unwrap_used, clippy::expect_used, reason = "test code")]

use std::sync::{Arc, RwLock};

use pgloupe::collect::serve::{Published, Shared, bind, parse_listen};

fn get(url: &str) -> (u16, String, Option<String>) {
    match ureq::get(url)
        .config()
        .http_status_as_error(false)
        .build()
        .call()
    {
        Ok(mut response) => {
            let etag = response
                .headers()
                .get("etag")
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            let status = response.status().as_u16();
            (
                status,
                response.body_mut().read_to_string().unwrap_or_default(),
                etag,
            )
        }
        Err(error) => (0, format!("{url}: {error}"), None),
    }
}

fn publish(shared: &Shared) {
    let mut p = shared.write().unwrap();
    p.generation = 3;
    p.updated_at = Some("2026-10-14T12:00:00Z".to_owned());
    p.html = Arc::new("<html>report</html>".to_owned());
    p.json = Arc::new("{\"kind\":\"pgloupe.report\"}".to_owned());
    p.interval_secs = 300;
}

#[test]
fn the_server_answers_its_routes_and_refuses_the_rest() {
    let shared: Shared = Arc::new(RwLock::new(Published::default()));
    let bound = bind(parse_listen("127.0.0.1:0", false).unwrap()).unwrap();
    bound.spawn(Arc::clone(&shared));
    let base = format!("http://{}", bound.addr);

    // Nothing built yet: the page is not ready, the status already answers.
    assert_eq!(get(&format!("{base}/")).0, 503);
    assert_eq!(get(&format!("{base}/healthz")).0, 503);
    let (status, body, _) = get(&format!("{base}/status"));
    assert_eq!(status, 200);
    assert!(body.contains("\"generation\":0"), "{body}");

    publish(&shared);
    let (status, body, _) = get(&format!("{base}/"));
    assert_eq!((status, body.as_str()), (200, "<html>report</html>"));
    assert_eq!(get(&format!("{base}/healthz")).1, "ok");
    let (status, body, etag) = get(&format!("{base}/report.json"));
    assert_eq!(status, 200);
    assert!(body.contains("pgloupe.report"));
    assert_eq!(etag.as_deref(), Some("\"3\""));
    let (status, status_body, _) = get(&format!("{base}/status"));
    assert_eq!(status, 200);
    assert!(
        status_body.contains("\"generation\":3") && status_body.contains("\"interval_secs\":300"),
        "{status_body}"
    );

    // A matching ETag is not sent again.
    let response = ureq::get(&format!("{base}/report.json"))
        .header("If-None-Match", "\"3\"")
        .config()
        .http_status_as_error(false)
        .build()
        .call()
        .unwrap();
    assert_eq!(response.status().as_u16(), 304);

    // Nothing is served from disk, and only reads are allowed.
    assert_eq!(get(&format!("{base}/../../etc/passwd")).0, 404);
    assert_eq!(get(&format!("{base}/etc/passwd")).0, 404);
    let post = ureq::post(&format!("{base}/"))
        .config()
        .http_status_as_error(false)
        .build()
        .send_empty()
        .unwrap();
    assert_eq!(post.status().as_u16(), 405);
    // A page on another domain pointed at 127.0.0.1 must not read the report (DNS rebinding).
    let rebound = ureq::get(&format!("{base}/report.json"))
        .header("Host", "evil.example:8080")
        .config()
        .http_status_as_error(false)
        .build()
        .call()
        .unwrap();
    assert_eq!(rebound.status().as_u16(), 421);
    bound.unblock();
}

#[test]
fn remote_addresses_need_permission() {
    parse_listen("0.0.0.0:0", false).unwrap_err();
    parse_listen("0.0.0.0:0", true).unwrap();
}
