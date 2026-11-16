//! Real router responses prove coverage outside event ingress and console routes.
use axum::{routing::get, Router};
use cmaeg::telemetry;

async fn failing_handler() -> &'static str {
    panic!("handler failure fixture");
}

fn sample(text: &str, name: &str, labels: &[&str]) -> f64 {
    text.lines()
        .find(|line| {
            line.starts_with(&format!("{name}{{"))
                && labels.iter().all(|label| line.contains(label))
        })
        .unwrap_or_else(|| panic!("missing {name} with {labels:?}"))
        .rsplit_once(' ')
        .unwrap()
        .1
        .parse()
        .unwrap()
}

#[tokio::test]
async fn every_route_counts_headers_once_and_excludes_request_values() {
    let metrics = telemetry::install().unwrap();
    let app = telemetry::http::instrument(telemetry::http::instrument(
        Router::new()
            .route("/objects/{id}", get(|| async { "ok" }))
            .route("/failure", get(failing_handler)),
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client = reqwest::Client::new();
    for id in ["private-object-one", "private-object-two"] {
        let response = client
            .get(format!("{base}/objects/{id}?query=private-query"))
            .header("x-request-id", "private-request-marker")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(response.text().await.unwrap(), "ok");
    }
    assert_eq!(
        client
            .get(format!("{base}/failure"))
            .send()
            .await
            .unwrap()
            .status(),
        500
    );
    assert_eq!(
        client
            .get(format!("{base}/private-unmatched-path"))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    assert_eq!(
        client
            .request(
                reqwest::Method::from_bytes(b"CUSTOM").unwrap(),
                format!("{base}/objects/private-object-three")
            )
            .send()
            .await
            .unwrap()
            .status(),
        405
    );
    let text = metrics.render();
    for (route, method, status, count) in [
        ("/objects/{id}", "GET", "200", 2.),
        ("/failure", "GET", "500", 1.),
        ("unmatched", "GET", "404", 1.),
        ("/objects/{id}", "OTHER", "405", 1.),
    ] {
        let route = format!("route=\"{route}\"");
        let method = format!("method=\"{method}\"");
        let status = format!("status=\"{status}\"");
        assert_eq!(
            sample(&text, "cmaeg_http_attempts_total", &[&route, &method]),
            count
        );
        assert_eq!(
            sample(&text, "cmaeg_http_in_flight", &[&route, &method]),
            0.
        );
        assert_eq!(
            sample(
                &text,
                "cmaeg_http_responses_total",
                &[&route, &method, &status]
            ),
            count
        );
        assert_eq!(
            sample(
                &text,
                "cmaeg_http_response_header_seconds_count",
                &[&route, &method, &status]
            ),
            count
        );
    }
    for private in [
        "private-object",
        "private-query",
        "private-request-marker",
        "private-unmatched-path",
        "CUSTOM",
    ] {
        assert!(!text.contains(private), "request value leaked into metrics");
    }
    assert!(!text
        .lines()
        .any(|line| line.starts_with("cmaeg_http_cancelled_before_headers_total{")));
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
}
