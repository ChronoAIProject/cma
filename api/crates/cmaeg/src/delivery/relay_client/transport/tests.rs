use super::*;
use crate::telemetry::turn_trace::{testing, Context, SpanId};
use axum::{body::Bytes, routing::post, Router};
use serde_json::Value;
use std::sync::{Arc, Mutex};

#[tokio::test]
async fn relay_http_trace_keeps_request_bytes_and_502_is_unknown_without_resend() {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let handler = captured.clone();
    let app = Router::new().route(
        "/private-path-canary",
        post(move |headers: HeaderMap, body: Bytes| {
            let handler = handler.clone();
            async move {
                let status = if headers.contains_key("fixture-fail") {
                    StatusCode::BAD_GATEWAY
                } else {
                    StatusCode::OK
                };
                handler.lock().unwrap().push((headers, body.to_vec()));
                (status, "private-response-canary")
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url: Url = format!(
        "http://{}/private-path-canary",
        listener.local_addr().unwrap()
    )
    .parse()
    .unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let timeout = Duration::from_secs(2);
    let transport = HttpTransport::new(timeout).unwrap();
    let (writer, records, _guard) = testing::capture();
    let parent = Context {
        turn_id: Some(uuid::Uuid::new_v4()),
        span_id: SpanId::fresh(),
        parent_span_id: None,
        request_id: uuid::Uuid::new_v4(),
        ingress_request_id: Some(uuid::Uuid::new_v4()),
    };
    let body = b"private-message-media-canary".to_vec();
    let mut headers = HeaderMap::new();
    headers.insert("idempotency-key", "same-execution-key".parse().unwrap());
    headers.insert(
        "authorization",
        "Bearer private-credential-canary".parse().unwrap(),
    );
    transport
        .post(url.clone(), headers.clone(), body.clone(), timeout)
        .await
        .unwrap();
    assert!(records.try_recv().is_err());
    for fail in [false, true] {
        let mut headers = headers.clone();
        if fail {
            headers.insert("fixture-fail", "1".parse().unwrap());
        }
        let trace = Operation::start(writer.clone(), Stage::ChannelPart, Some(parent), None);
        let context = trace.context();
        let reply = transport
            .exchange(url.clone(), headers, body.clone(), timeout, Some(trace))
            .await
            .unwrap();
        assert_eq!(
            reply.status,
            if fail {
                StatusCode::BAD_GATEWAY
            } else {
                StatusCode::OK
            }
        );
        let start: Value = serde_json::from_slice(&records.recv_timeout(timeout).unwrap()).unwrap();
        let end: Value = serde_json::from_slice(&records.recv_timeout(timeout).unwrap()).unwrap();
        assert_eq!(start["span_id"], end["span_id"]);
        assert_eq!(start["request_id"], end["request_id"]);
        assert_eq!(end["outcome"], if fail { "unknown" } else { "completed" });
        assert_eq!(end["http_status"], reply.status.as_u16());
        assert_eq!(start["parent_span_id"], parent.span_id.to_string());
        assert!(!format!("{start}{end}").contains("private"));
        let calls = captured.lock().unwrap();
        let (sent, bytes) = calls.last().unwrap();
        assert_eq!(bytes, &body);
        assert_eq!(sent["idempotency-key"], "same-execution-key");
        assert_eq!(sent["x-request-id"], context.request_id.to_string());
        assert_eq!(
            sent["traceparent"],
            format!(
                "00-{}-{}-01",
                parent.turn_id.unwrap().simple(),
                context.span_id
            )
        );
    }
    let calls = captured.lock().unwrap();
    assert_eq!(calls.len(), 3);
    assert!(!calls[0].0.contains_key("traceparent"));
    assert!(!calls[0].0.contains_key("x-request-id"));
    assert_eq!(calls[0].1, body);
    assert!(records.try_recv().is_err());
    server.abort();
}
