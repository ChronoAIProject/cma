//! Real provider boundaries retain their policy while emitting content-free causes.
use super::*;
use crate::auth::{identity_http, verification_diagnostics::Endpoint};
use layer2_core::diagnostic_context::DiagnosticContext;
use tracing::instrument::WithSubscriber;

#[derive(Clone, Default)]
struct Log(Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for Log {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

async fn capture<F: std::future::Future>(future: F) -> (F::Output, Vec<Value>) {
    let log = Log::default();
    let writer = log.clone();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .with_max_level(tracing::Level::INFO)
        .with_writer(move || writer.clone())
        .finish();
    let context = DiagnosticContext {
        request_hash: "safe-request-hash".into(),
        ..DiagnosticContext::default()
    };
    let result = context.scope(future).with_subscriber(subscriber).await;
    let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
    let events: Vec<Value> = text
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|event| event["fields"]["event"] == "identity_verification_failure")
        .collect();
    for event in &events {
        assert_eq!(event["fields"]["request_hash"], "safe-request-hash");
        assert_eq!(event["span"], json!({"name":"identity_verification"}));
        assert_eq!(event["spans"], json!([{"name":"identity_verification"}]));
        for private in ["must-not-leak", "http://", "owner", "cookie-client"] {
            assert!(!event.to_string().contains(private), "{event}");
        }
    }
    (result, events)
}

fn assert_event(events: &[Value], endpoint: &str, stage: &str, reason: &str, disposition: &str) {
    assert_eq!(events.len(), 1, "{events:?}");
    let fields = &events[0]["fields"];
    assert_eq!(fields["endpoint"], endpoint);
    assert_eq!(fields["stage"], stage);
    assert_eq!(fields["reason"], reason);
    assert_eq!(fields["disposition"], disposition);
}

async fn read(f: &Fixture, deadline: Duration) -> Result<Value, ProviderVerificationError> {
    let metadata = f.auth.metadata().await.unwrap();
    identity_http::json(
        f.auth
            .http
            .get(metadata.userinfo_endpoint.unwrap())
            .bearer_auth("must-not-leak-access-token"),
        tokio::time::Instant::now() + deadline,
        Endpoint::Userinfo,
    )
    .await
}

#[tokio::test]
async fn http_status_diagnostics_preserve_refusal_and_retry_classification() {
    for status in [200, 401, 403, 429, 500, 502, 503] {
        let f = Fixture::new().await;
        f.auth.metadata().await.unwrap();
        f.control.userinfo.write().await.status = StatusCode::from_u16(status).unwrap();
        let before = f.control.requests.load(Ordering::SeqCst);
        let (result, events) = capture(read(&f, Duration::from_secs(5))).await;
        assert_eq!(f.control.requests.load(Ordering::SeqCst) - before, 1);
        let disposition = match status {
            200 => {
                assert!(result.is_ok());
                assert!(events.is_empty());
                continue;
            }
            401 | 403 => {
                assert!(matches!(
                    result,
                    Err(ProviderVerificationError::Rejected(_))
                ));
                "rejected"
            }
            429 => {
                assert!(matches!(
                    result,
                    Err(ProviderVerificationError::RateLimited(_))
                ));
                "rate_limited"
            }
            502 | 503 => {
                assert!(matches!(
                    result,
                    Err(ProviderVerificationError::Transient(_))
                ));
                "transient"
            }
            _ => {
                assert!(matches!(
                    result,
                    Err(ProviderVerificationError::Unavailable(_))
                ));
                "unavailable"
            }
        };
        assert_event(&events, "userinfo", "status", "http_status", disposition);
        assert_eq!(events[0]["fields"]["status"], status);
    }
}

#[tokio::test]
async fn partial_oversized_and_invalid_bodies_remain_nonretryable_and_private() {
    for case in 0..7 {
        let f = Fixture::new().await;
        f.auth.metadata().await.unwrap();
        let mut reply = f.control.userinfo.write().await;
        let (stage, reason) = match case {
            0 => {
                reply
                    .headers
                    .insert(header::CONTENT_RANGE, "bytes 0-1/10".parse().unwrap());
                ("response_headers", "partial_response")
            }
            1 => {
                reply.body = "must-not-leak-invalid-json".into();
                ("body", "invalid_body")
            }
            2 => {
                reply.body = r#"{"sub":"must-not-leak","sub":"duplicate"}"#.into();
                ("body", "invalid_body")
            }
            3 => {
                reply.body = "x".repeat(identity_http::MAX_BYTES + 1);
                ("body", "body_too_large")
            }
            4 => {
                reply.body = "x".repeat(identity_http::MAX_BYTES);
                reply.stream_tail = Some((Duration::from_millis(10), Ok("x".into())));
                ("body", "body_too_large")
            }
            5 => {
                reply.body =
                    json!({"error_description":"must-not-leak-provider-detail"}).to_string();
                ("claims", "error_disposition")
            }
            _ => {
                reply.body = "{\"sub\":\"must-not-leak\",".into();
                reply.stream_tail = Some((Duration::from_millis(10), Err("must-not-leak-error")));
                ("body", "body_read")
            }
        };
        drop(reply);
        let (result, events) = capture(async {
            use tracing::Instrument;
            read(&f, Duration::from_secs(5))
                .instrument(tracing::info_span!(
                    "private-parent",
                    secret = "must-not-leak-span"
                ))
                .await
        })
        .await;
        assert!(matches!(
            result,
            Err(ProviderVerificationError::Unavailable(_))
        ));
        assert_event(&events, "userinfo", stage, reason, "unavailable");
        assert_eq!(events[0]["fields"]["status"], 200);
    }
}

#[tokio::test]
async fn header_and_body_deadlines_are_distinct_without_expanding_retry() {
    for body in [false, true] {
        let f = Fixture::new().await;
        f.auth.metadata().await.unwrap();
        let mut reply = f.control.userinfo.write().await;
        if body {
            reply.body = "{".into();
            reply.stream_tail = Some((Duration::from_secs(10), Ok("}".into())));
        } else {
            reply.delay = Duration::from_secs(10);
        }
        drop(reply);
        let (result, events) = capture(read(&f, Duration::from_secs(1))).await;
        if body {
            assert!(matches!(
                result,
                Err(ProviderVerificationError::Unavailable(_))
            ));
            assert_event(&events, "userinfo", "body", "timeout", "unavailable");
        } else {
            assert!(matches!(
                result,
                Err(ProviderVerificationError::Transient("timeout"))
            ));
            assert_event(
                &events,
                "userinfo",
                "response_headers",
                "timeout",
                "transient",
            );
        }
    }
}

#[tokio::test]
async fn reqwest_body_timeout_retains_its_phase_and_fail_closed_policy() {
    let f = Fixture::new().await;
    let metadata = f.auth.metadata().await.unwrap();
    let mut reply = f.control.userinfo.write().await;
    reply.body = "{".into();
    reply.stream_tail = Some((Duration::from_secs(10), Ok("}".into())));
    drop(reply);
    let (result, events) = capture(identity_http::json(
        f.auth
            .http
            .get(metadata.userinfo_endpoint.unwrap())
            .timeout(Duration::from_secs(1)),
        tokio::time::Instant::now() + Duration::from_secs(5),
        Endpoint::Userinfo,
    ))
    .await;
    assert!(matches!(
        result,
        Err(ProviderVerificationError::Unavailable(_))
    ));
    assert_event(&events, "userinfo", "body", "timeout", "unavailable");
}

#[tokio::test]
async fn invalid_claims_configuration_and_capacity_are_separate_failures() {
    for endpoint in [
        "userinfo",
        "introspection",
        "discovery",
        "configuration",
        "capacity",
    ] {
        let f = Fixture::new().await;
        let state = f.seed().await;
        if endpoint != "discovery" {
            f.auth.metadata().await.unwrap();
        }
        let permits;
        let (expected_endpoint, stage, reason) = match endpoint {
            "userinfo" => {
                f.control.userinfo.write().await.body = "{}".into();
                ("userinfo", "claims", "invalid_claims")
            }
            "introspection" => {
                f.control.introspection.write().await.body = json!({"active":true}).to_string();
                ("introspection", "claims", "invalid_claims")
            }
            "discovery" => {
                f.control.discovery.write().await.body = "{}".into();
                ("discovery", "claims", "invalid_claims")
            }
            "configuration" => {
                f.auth
                    .metadata
                    .write()
                    .await
                    .as_mut()
                    .unwrap()
                    .userinfo_endpoint = None;
                ("userinfo", "configuration", "missing_configuration")
            }
            _ => {
                permits = f.auth.verification_slots.acquire_many(64).await.unwrap();
                assert_eq!(permits.num_permits(), 64);
                ("verification", "admission", "verification_capacity")
            }
        };
        let (result, events) = capture(f.request(state)).await;
        assert_unavailable(result);
        assert_event(&events, expected_endpoint, stage, reason, "unavailable");
    }
}

#[tokio::test]
async fn shared_flight_reports_one_provider_failure_for_two_callers() {
    let f = Fixture::new().await;
    let state = f.seed().await;
    f.auth.metadata().await.unwrap();
    let gate = Arc::new(Gate::default());
    let mut reply = f.control.introspection.write().await;
    reply.status = StatusCode::TOO_MANY_REQUESTS;
    reply
        .headers
        .insert(header::RETRY_AFTER, "60".parse().unwrap());
    reply.gate = Some(gate.clone());
    drop(reply);
    let before = f.control.requests.load(Ordering::SeqCst);
    let ((first, second, ()), events) = capture(async {
        tokio::join!(f.request(state.clone()), f.request(state.clone()), async {
            gate.reached.notified().await;
            gate.release.notify_one();
        })
    })
    .await;
    assert_unavailable(first);
    assert_unavailable(second);
    assert_eq!(f.control.requests.load(Ordering::SeqCst) - before, 2);
    assert_event(
        &events,
        "introspection",
        "status",
        "http_status",
        "rate_limited",
    );
    assert_eq!(events[0]["fields"]["status"], 429);
}

#[tokio::test]
async fn caller_and_verification_deadlines_are_observed_and_release_capacity() {
    for caller in [false, true] {
        let f = Fixture::new().await;
        let state = f.seed().await;
        f.auth.metadata().await.unwrap();
        let events = if caller {
            // The leader owns the longer sequence. This caller waits on the
            // same flight, so its short deadline cannot race that sequence's.
            let (_gate, leader) = f.blocked_request(state.clone()).await;
            let (result, events) = capture(request_with_timeout(
                &f.auth,
                &state.subject.clone(),
                state,
                Duration::from_millis(100),
            ))
            .await;
            assert_unavailable(result);
            leader.abort();
            assert!(matches!(leader.await, Err(error) if error.is_cancelled()));
            events
        } else {
            f.control.userinfo.write().await.delay = Duration::from_secs(10);
            f.control.introspection.write().await.delay = Duration::from_secs(10);
            let (result, events) = capture(crate::auth::identity_retry::verify(
                &f.auth,
                None,
                "must-not-leak-access-token",
                None,
                Some("owner"),
                Duration::from_millis(100),
            ))
            .await;
            assert!(matches!(
                result,
                Err(ProviderVerificationError::Unavailable(_))
            ));
            events
        };
        assert_event(
            &events,
            "verification",
            "deadline",
            if caller {
                "caller_deadline"
            } else {
                "verification_deadline"
            },
            "unavailable",
        );
        assert_eq!(f.auth.verification_slots.available_permits(), 64);
    }
}
