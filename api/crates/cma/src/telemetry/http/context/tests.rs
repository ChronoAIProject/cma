use super::*;
use axum::{body::Body, http::HeaderValue};

fn request() -> Request {
    Request::builder()
        .extension(Enabled(true))
        .uri("/synthetic")
        .body(Body::empty())
        .unwrap()
}

#[test]
fn absent_diagnostic_option_does_not_observe_even_a_valid_header() {
    let mut request = request();
    request.extensions_mut().remove::<Enabled>();
    request
        .headers_mut()
        .insert(HEADER, "0123456789abcdef0123456789abcdef".parse().unwrap());
    assert!(Read::start(&mut request, ROUTE).is_none());
    assert!(request
        .extensions()
        .get::<Arc<OnceLock<Instant>>>()
        .is_none());
}

#[test]
fn bounded_unique_correlation_is_hashed_without_changing_request_headers() {
    let mut request = request();
    let value = "0123456789abcdef0123456789abcdef";
    request.headers_mut().insert(HEADER, value.parse().unwrap());
    let read = Read::start(&mut request, ROUTE).unwrap();
    assert_eq!(
        read.hash.as_ref(),
        format!("{:x}", Sha256::digest(value.as_bytes()))
    );
    assert_eq!(request.headers()[HEADER], value);
    assert!(request
        .extensions()
        .get::<Arc<OnceLock<Instant>>>()
        .is_some());
}

#[test]
fn missing_invalid_oversized_and_duplicate_correlations_get_independent_hashes() {
    for values in [
        vec![],
        vec![HeaderValue::from_static("private-host-or-credential")],
        vec![HeaderValue::from_static("ABCDEF0123456789ABCDEF0123456789")],
        vec![HeaderValue::from_bytes(&[b'a'; 4096]).unwrap()],
        vec![HeaderValue::from_static("0123456789abcdef0123456789abcdef"); 2],
    ] {
        let mut request = request();
        for value in values {
            request.headers_mut().append(HEADER, value);
        }
        let first = Read::start(&mut request, ROUTE).unwrap();
        let second = Read::start(&mut request, ROUTE).unwrap();
        assert_eq!(first.hash.len(), 64);
        assert_ne!(first.hash, second.hash);
    }
}

#[test]
fn observation_is_scoped_to_retained_context_get() {
    let mut request = request();
    assert!(Read::start(&mut request, "/other").is_none());
    *request.method_mut() = Method::POST;
    assert!(Read::start(&mut request, ROUTE).is_none());
    assert!(request
        .extensions()
        .get::<Arc<OnceLock<Instant>>>()
        .is_none());
}

#[derive(Clone, Default)]
struct Logs(Arc<std::sync::Mutex<Vec<u8>>>);
impl std::io::Write for Logs {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Logs {
    type Writer = Self;
    fn make_writer(&'a self) -> Self {
        self.clone()
    }
}
impl Logs {
    fn records(&self) -> Vec<serde_json::Value> {
        String::from_utf8(self.0.lock().unwrap().clone())
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
}

#[tokio::test(start_paused = true)]
async fn cancellation_before_and_after_handler_entry_has_distinct_timings() {
    use axum::{
        middleware::{from_fn, Next},
        routing::get,
        Router,
    };
    use std::time::Duration;
    use tower::ServiceExt;
    for pre_handler in [true, false] {
        let logs = Logs::default();
        let subscriber = tracing_subscriber::fmt()
            .json()
            .without_time()
            .with_writer(logs.clone())
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);
        let request_span = tracing::info_span!(
            "http.request",
            event_id = "private-event",
            credential = "private-credential"
        );
        let _entered = request_span.enter();
        let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        let _recording = metrics::set_default_local_recorder(&recorder);
        let app = Router::new()
            .route(
                ROUTE,
                get(|request: Request| async move {
                    request
                        .extensions()
                        .get::<Arc<OnceLock<Instant>>>()
                        .unwrap()
                        .get_or_init(Instant::now);
                    std::future::pending::<()>().await;
                })
                .layer(from_fn(move |request: Request, next: Next| async move {
                    if pre_handler {
                        std::future::pending::<()>().await;
                    }
                    tokio::time::sleep(Duration::from_millis(125)).await;
                    next.run(request).await
                })),
            )
            .layer(from_fn(crate::telemetry::http::observe))
            .layer(axum::Extension(Enabled(true)));
        {
            let future = app.oneshot(Request::builder()
                .uri("/api/v1/cmaeg/bindings/private-binding/conversations/private-conversation/events/private-event")
                .header(HEADER, "0123456789abcdef0123456789abcdef")
                .header("authorization", "Bearer private-credential")
                .body(Body::empty()).unwrap());
            tokio::pin!(future);
            assert!(futures_util::poll!(&mut future).is_pending());
            tokio::time::advance(Duration::from_millis(125)).await;
            assert!(futures_util::poll!(&mut future).is_pending());
            tokio::time::advance(Duration::from_secs(3)).await;
        }
        let records = logs.records();
        let phase = &records[1]["fields"];
        assert_eq!(phase["phase"], "pre_handler");
        assert_eq!(
            phase["outcome"],
            if pre_handler {
                "cancelled"
            } else {
                "completed"
            }
        );
        assert_eq!(phase["elapsed_ms"], if pre_handler { 3125 } else { 125 });
        assert_eq!(records[2]["fields"]["phase"], "http_response_headers");
        assert_eq!(records[2]["fields"]["outcome"], "cancelled");
        let hash = format!("{:x}", Sha256::digest(b"0123456789abcdef0123456789abcdef"));
        assert!(records
            .iter()
            .all(|record| record["fields"]["attempt_hash"] == hash));
        let output = format!("{records:?}");
        for private in [
            "private-binding",
            "private-conversation",
            "private-event",
            "private-credential",
            "0123456789abcdef0123456789abcdef",
        ] {
            assert!(!output.contains(private));
        }
        let metrics = handle.render();
        assert!(metrics.contains(&format!(
            "cma_http_before_headers_cancelled_total{{route=\"{ROUTE}\",method=\"GET\"}} 1"
        )));
        assert!(metrics.contains(&format!(
            "cma_http_in_flight{{route=\"{ROUTE}\",method=\"GET\"}} 0"
        )));
    }
}

#[tokio::test(start_paused = true)]
async fn response_before_handler_is_distinct_from_cancellation() {
    let logs = Logs::default();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .without_time()
        .with_writer(logs.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let mut read = Read::start(&mut request(), ROUTE).unwrap();
    tokio::time::advance(std::time::Duration::from_millis(500)).await;
    read.status = Some(403);
    drop(read);
    let records = logs.records();
    assert_eq!(records[1]["fields"]["outcome"], "refused");
    assert_eq!(records[1]["fields"]["elapsed_ms"], 500);
    assert_eq!(records[2]["fields"]["status"], 403);
}

#[test]
fn false_diagnostic_option_does_not_observe_even_a_valid_header() {
    let mut request = request();
    request.extensions_mut().insert(Enabled(false));
    request
        .headers_mut()
        .insert(HEADER, "0123456789abcdef0123456789abcdef".parse().unwrap());
    assert!(Read::start(&mut request, ROUTE).is_none());
    assert!(request
        .extensions()
        .get::<Arc<OnceLock<Instant>>>()
        .is_none());
}

#[test]
fn diagnostic_configuration_defaults_off_independently_of_buffered_reads() {
    use crate::config::FeatureFlags;
    for value in [None, Some("false"), Some("true")] {
        let flags = FeatureFlags::from_lookup(&mut |key| {
            Ok(if key == "CHRONO_CMAEG_CONTEXT_DIAGNOSTICS_ENABLED" {
                value.map(str::to_owned)
            } else {
                None
            })
        })
        .unwrap();
        assert_eq!(flags.context_diagnostics, value == Some("true"));
        assert!(!flags.retained_context_lookup);
    }
    let flags = FeatureFlags::from_lookup(&mut |key| {
        Ok((key == "CHRONO_CMAEG_RETAINED_CONTEXT_LOOKUP_ENABLED").then(|| "true".into()))
    })
    .unwrap();
    assert!(flags.retained_context_lookup);
    assert!(!flags.context_diagnostics);
    for invalid in ["", "TRUE", "1", "private-invalid-option"] {
        let error = FeatureFlags::from_lookup(&mut |key| {
            Ok((key == "CHRONO_CMAEG_CONTEXT_DIAGNOSTICS_ENABLED").then(|| invalid.into()))
        })
        .unwrap_err();
        assert_eq!(
            error,
            "CHRONO_CMAEG_CONTEXT_DIAGNOSTICS_ENABLED must be true or false"
        );
    }
}

#[tokio::test]
async fn disabled_middleware_keeps_headers_context_and_logs_unchanged() {
    use axum::{middleware::from_fn, routing::get, Router};
    use tower::ServiceExt;
    for enabled in [None, Some(false)] {
        let logs = Logs::default();
        let subscriber = tracing_subscriber::fmt()
            .json()
            .without_time()
            .with_writer(logs.clone())
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);
        let app = Router::new()
            .route(
                ROUTE,
                get(|request: Request| async move {
                    assert!(request
                        .extensions()
                        .get::<Arc<OnceLock<Instant>>>()
                        .is_none());
                    assert!(ExternalContextObservation::current().is_none());
                    assert_eq!(DiagnosticContext::current().phase, "unspecified");
                    assert_eq!(
                        request.headers()[HEADER],
                        "0123456789abcdef0123456789abcdef"
                    );
                    axum::http::StatusCode::NOT_FOUND
                }),
            )
            .layer(from_fn(crate::telemetry::http::observe));
        let app = match enabled {
            Some(value) => app.layer(axum::Extension(Enabled(value))),
            None => app,
        };
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/cmaeg/bindings/binding/conversations/conversation/events/event")
                    .header(HEADER, "0123456789abcdef0123456789abcdef")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::NOT_FOUND);
        assert!(logs.records().is_empty());
    }
}
