use super::*;
use crate::{
    delivery::{
        intents::DeliveryOutcome,
        relay_client::{Credential, RelayClient},
    },
    telemetry::turn_trace::{scope, Association, BoundOperation},
};
use axum::{http::Uri, Json};
use cmaeg_core::{
    channel::{Platform, Secret},
    seal::ReplyAddress,
};
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};
use tracing::instrument::WithSubscriber;
use tracing_subscriber::prelude::*;

#[tokio::test]
async fn commonmark_routing_and_trace_gates_preserve_keyboard_bytes_and_uncertainty() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let fail = Arc::new(AtomicBool::new(false));
    let captured = calls.clone();
    let failed = fail.clone();
    let app = Router::new().fallback(move |uri: Uri, headers: HeaderMap, body: Bytes| {
        let (captured, failed) = (captured.clone(), failed.clone());
        async move {
            captured
                .lock()
                .unwrap()
                .push((uri.path().to_owned(), headers, body.to_vec()));
            (
                if failed.load(Ordering::SeqCst) {
                    StatusCode::BAD_GATEWAY
                } else {
                    StatusCode::OK
                },
                Json(json!({"message_id":"out-fixture","platform_message_id":"platform-fixture"})),
            )
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let timeout = Duration::from_secs(2);
    let credential = Secret::new(format!("nyx_{}", uuid::Uuid::new_v4().simple()));
    let address = ReplyAddress::NyxidRelay {
        platform: Platform::Telegram,
        message_id: "private-target-canary".into(),
        route_id: "private-route-canary".into(),
        reply_token: Secret::new("private-reply-canary"),
        reply_token_expires_at: i64::MAX,
        thread_reply: false,
    };
    let source = "**private-text-canary** [link](https://example.test/a_b) \\*literal\\*";
    let keyboard =
        json!({"inline_keyboard":[[{"text":"Choose","callback_data":"private-callback-canary"}]]});
    let (writer, records, _guard) = testing::capture();
    let mut first_body = None;
    for commonmark in [false, true] {
        let relay = RelayClient::new(&base, timeout)
            .unwrap()
            .with_commonmark_relay(commonmark);
        for tracing in [false, true] {
            let _installed = tracing.then(|| testing::install(writer.clone()));
            let subscriber = tracing_subscriber::registry().with(scope::layer());
            async {
                for rejected in [false, true] {
                    fail.store(rejected, Ordering::SeqCst);
                    let association = Association {
                        turn_id: uuid::Uuid::new_v4(),
                        parent_span_id: Some(SpanId::fresh()),
                        ingress_request_id: uuid::Uuid::new_v4(),
                    };
                    let trace = BoundOperation::start(Some(association), Stage::Platform);
                    assert_eq!(trace.is_some(), tracing);
                    let send = relay.send_rich(
                        &address,
                        Credential::ChannelKey(&credential),
                        source,
                        Some(&keyboard),
                    );
                    let (outcome, _) = if let Some(trace) = trace {
                        let result = trace.within(send).await;
                        trace.finish(if rejected {
                            Outcome::Unknown
                        } else {
                            Outcome::Confirmed
                        });
                        result
                    } else {
                        send.await
                    };
                    assert_eq!(
                        matches!(outcome, DeliveryOutcome::Confirmed { .. }),
                        !rejected
                    );
                    if rejected {
                        assert!(matches!(outcome, DeliveryOutcome::Unknown { .. }));
                    }
                    let (path, headers, bytes) = calls.lock().unwrap().last().unwrap().clone();
                    assert_eq!(
                        path,
                        if commonmark {
                            "/api/v1/channel-relay/reply/commonmark"
                        } else {
                            "/api/v1/channel-relay/reply"
                        }
                    );
                    assert_eq!(headers.contains_key("traceparent"), tracing);
                    assert_eq!(headers.contains_key("x-request-id"), tracing);
                    if tracing {
                        assert!(headers["traceparent"]
                            .to_str()
                            .unwrap()
                            .starts_with(&format!("00-{}-", association.turn_id.simple())));
                    }
                    let body: Value = serde_json::from_slice(&bytes).unwrap();
                    assert_eq!(body["reply"]["text"], source);
                    assert_eq!(body["reply"]["metadata"]["reply_markup"], keyboard);
                    assert_eq!(first_body.get_or_insert(bytes.clone()), &bytes);
                    let spans: Vec<Value> = (0..if tracing { 5 } else { 0 })
                        .map(|_| {
                            serde_json::from_slice(&records.recv_timeout(timeout).unwrap()).unwrap()
                        })
                        .collect();
                    if tracing {
                        let end = spans
                            .iter()
                            .find(|record| {
                                record["stage"] == "channel_part" && record["phase"] == "end"
                            })
                            .unwrap();
                        assert_eq!(
                            end["outcome"],
                            if rejected { "unknown" } else { "completed" }
                        );
                        assert!(!serde_json::to_string(&spans).unwrap().contains("private-"));
                    }
                    assert!(records.try_recv().is_err());
                }
            }
            .with_subscriber(subscriber)
            .await;
        }
    }
    assert_eq!(calls.lock().unwrap().len(), 8, "no retry or route fallback");
    assert_eq!(writer.dropped(), 0);
    server.abort();
}
