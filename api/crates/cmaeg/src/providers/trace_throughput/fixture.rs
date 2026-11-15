use super::*;
use axum::{http::HeaderMap, routing::post, Json, Router};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

pub(super) async fn start(
    delay_ms: u64,
) -> (
    ProviderClient,
    String,
    Arc<AtomicUsize>,
    tokio::task::JoinHandle<()>,
) {
    let calls = Arc::new(AtomicUsize::new(0));
    let provider_calls = calls.clone();
    let relay_calls = calls.clone();
    let app = Router::new()
        .route("/responses", post(move |headers: HeaderMap, Json(body): Json<Value>| {
            let calls = provider_calls.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                assert_eq!(body["input"][0]["content"], "private-input-canary");
                if delay_ms > 0 { tokio::time::sleep(Duration::from_millis(delay_ms)).await; }
                let request = uuid::Uuid::new_v4();
                let turn = uuid::Uuid::new_v4();
                let mut frames = vec![json!({"type":"response.created","response":{"id":"resp_benchmark"}})];
                if headers.get("x-nyxid-turn-trace-contract").is_some_and(|h| h == "1") {
                    frames.push(json!({"type":"response.nyxid_trace","response_id":"resp_benchmark","nyxid_trace":{
                        "version":1,"turn_id":turn.to_string(),"trace_id":turn.simple().to_string(),
                        "parent_span_id":"0123456789abcdef","ingress_request_id":request.to_string()
                    }}));
                }
                frames.push(json!({"type":"response.completed","response":{"id":"resp_benchmark","output":[
                    {"type":"message","content":[{"type":"output_text","text":"private-output-canary"}]}
                ]}}));
                let body = frames.into_iter().map(|f| format!("data: {f}\n\n")).collect::<String>();
                ([("content-type","text/event-stream".to_owned()), ("x-request-id",request.to_string()),
                    ("x-nyxid-trace-id", request.to_string())], body)
            }
        }))
        .route("/api/v1/channel-relay/reply", post(move |Json(body): Json<Value>| {
            let calls = relay_calls.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                assert_eq!(body["reply"]["text"], "private-output-canary");
                Json(json!({"message_id":"out_benchmark","platform_message_id":"platform_benchmark"}))
            }
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let registry = ProviderRegistry::parse(&json!([{"slug":"nyxbot","base_url":base,"kind":"responses_http","max_inflight":64,
        "paths":{"agent_card":"/card","binding":"/bindings/{binding_id}","conversation":"/bindings/{binding_id}/conversations/{conversation_id}","responses":"/responses"}}]).to_string()).unwrap();
    let client =
        ProviderClient::new(registry, Duration::from_secs(5), Duration::from_secs(5), 64).unwrap();
    (client, base, calls, server)
}
