use super::*;
use crate::providers::{ProviderRegistry, ResponsesEvent, ToolsAdvertisement, TurnRequest};
use axum::{
    body::Bytes,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{post, put},
    Json, Router,
};
use cmaeg_core::{
    channel::Secret,
    ids::{ConversationId, IdempotencyKey},
};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};

type CapturedRequest = (HeaderMap, Vec<u8>);

#[derive(Clone, Default)]
struct Requests(Arc<Mutex<Vec<CapturedRequest>>>);

async fn fixture(
    enabled: bool,
    echo: Option<Value>,
    reject: bool,
) -> (ProviderClient, Requests, tokio::task::JoinHandle<()>) {
    let captured = Requests::default();
    let binding = captured.clone();
    let responses = captured.clone();
    let app = Router::new()
        .route("/bindings/{binding_id}", put(move |headers: HeaderMap, body: Bytes| {
            let captured = binding.clone();
            let echo = echo.clone();
            async move {
                captured.0.lock().unwrap().push((headers, body.to_vec()));
                if reject {
                    return (StatusCode::UNPROCESSABLE_ENTITY, Json(json!({"code":"unknown_field"}))).into_response();
                }
                let mut value = json!({"binding_id":"binding_one","level":"l1","honoured":[],"refused":[]});
                if let Some(echo) = echo { value["capabilities"] = echo; }
                Json(value).into_response()
            }
        }))
        .route("/responses", post(move |headers: HeaderMap, body: Bytes| {
            let captured = responses.clone();
            async move {
                let negotiated = headers.get("x-nyxid-turn-trace-contract").is_some();
                captured.0.lock().unwrap().push((headers, body.to_vec()));
                let mut frames = vec![json!({"type":"response.created","response":{"id":"resp_synthetic"}})];
                if negotiated {
                    frames.push(json!({"type":"response.nyxid_trace","response_id":"resp_synthetic","nyxid_trace":{
                        "version":1,"turn_id":"11111111-1111-4111-8111-111111111111",
                        "trace_id":"11111111111141118111111111111111","parent_span_id":"0123456789abcdef",
                        "ingress_request_id":"22222222-2222-4222-8222-222222222222"}}));
                }
                frames.push(json!({"type":"response.completed","response":{"id":"resp_synthetic","output":[]}}));
                let body = frames.into_iter().map(|frame| format!("data: {frame}\n\n")).collect::<String>();
                ([("content-type", "text/event-stream")], body)
            }
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let registry = ProviderRegistry::parse(&json!([{"slug":"nyxbot","base_url":origin,"kind":"responses_http","max_inflight":2,
        "paths":{"agent_card":"/card","binding":"/bindings/{binding_id}","conversation":"/bindings/{binding_id}/conversations/{conversation_id}","responses":"/responses"}}]).to_string()).unwrap();
    let mut client =
        ProviderClient::new(registry, Duration::from_secs(2), Duration::from_secs(2), 2).unwrap();
    client.turn_trace_enabled = enabled;
    (client, captured, server)
}

fn binding_request() -> BindingRequest {
    let profile =
        crate::channels::routing_fixture::definition(crate::channels::routing_fixture::routed())
            .profile;
    BindingRequest {
        schema_version: 1,
        target: Default::default(),
        profile_digest: profile.digest(),
        profile,
        agent_key: Secret::new(format!("nyx_{}", uuid::Uuid::new_v4().simple())),
        tools: ToolsAdvertisement {
            native_mcp_url: "https://gateway.example/mcp".into(),
            nyxid_service_slug: "cmaeg".into(),
        },
    }
}

#[tokio::test]
async fn trace_binding_only_exact_echo_enables_live_header_and_reader() {
    for (enabled, echo, expected) in [
        (false, None, false),
        (
            false,
            Some(json!([super::super::trace_contract::CAPABILITY])),
            false,
        ),
        (true, None, false),
        (true, Some(Value::Null), false),
        (true, Some(json!([])), false),
        (true, Some(json!("nyxid_turn_trace_v1")), false),
        (true, Some(json!({"nyxid_turn_trace_v1":true})), false),
        (true, Some(json!(["unknown-private-capability"])), false),
        (true, Some(json!(["nyxid_turn_trace_v1", 42])), false),
        (true, Some(json!(["nyxid_turn_trace_v1", "unknown"])), false),
        (true, Some(json!(["nyxid_turn_trace_v1"])), true),
    ] {
        let (client, captured, server) = fixture(enabled, echo, false).await;
        assert_eq!(
            client.turn_trace_capability(),
            enabled.then_some("nyxid_turn_trace_v1")
        );
        let mut provider = crate::channels::routing_fixture::definition(
            crate::channels::routing_fixture::routed(),
        )
        .provider;
        provider.slug = ProviderSlug::new("nyxbot").unwrap();
        provider.binding_id = BindingId::new("binding_one").unwrap();
        let request = binding_request();
        let result = client
            .bind(&provider.slug, &provider.binding_id, &request)
            .await
            .unwrap();
        provider.capabilities = result.capabilities;
        assert_eq!(client.nyxid_trace_contract(&provider), expected);
        let turn = TurnRequest {
            nyxid_trace: client.nyxid_trace_contract(&provider),
            timing: None,
            conversation_id: ConversationId::new("conv_synthetic").unwrap(),
            input: vec![json!({"role":"user","content":"private-input-canary"})],
            idempotency_key: IdempotencyKey::new("same-execution-key").unwrap(),
            metadata: BTreeMap::new(),
            agent_key: request.agent_key.clone(),
            event_context: None,
        };
        let mut stream = client
            .start_turn(&provider.slug, turn.clone())
            .await
            .unwrap();
        assert_eq!(stream.accepted.await.unwrap().response_id, "resp_synthetic");
        let mut traced = false;
        while let Some(event) = stream.events.recv().await {
            traced |= matches!(event, ResponsesEvent::NyxidTrace);
        }
        assert_eq!(traced, expected);
        assert_eq!(stream.trace.is_some(), expected);
        let canonical = stream.trace.as_ref().and_then(|slot| slot.get());
        assert_eq!(canonical.is_some(), expected);
        if let Some(canonical) = canonical {
            assert_eq!(
                canonical.turn_id.to_string(),
                "11111111-1111-4111-8111-111111111111"
            );
        }
        let requests = captured.0.lock().unwrap();
        assert_eq!(requests.len(), 2);
        let mut sent_binding: Value = serde_json::from_slice(&requests[0].1).unwrap();
        assert_eq!(sent_binding.get("capabilities").is_some(), enabled);
        sent_binding.as_object_mut().unwrap().remove("capabilities");
        assert_eq!(
            sent_binding,
            json!({"schema_version":request.schema_version,"profile":request.profile,
            "profile_digest":request.profile_digest,"agent_key":request.agent_key.as_str(),"tools":request.tools})
        );
        assert_eq!(
            requests[1]
                .0
                .get("x-nyxid-turn-trace-contract")
                .map(|header| header.to_str().unwrap()),
            expected.then_some("1")
        );
        assert_eq!(requests[1].0["idempotency-key"], "same-execution-key");
        assert!(!requests[1].0.contains_key("x-request-id"));
        assert!(!requests[1].0.contains_key("traceparent"));
        assert_eq!(requests[1].1, canonical_json(json!({"conversation":turn.conversation_id,"stream":true,"input":turn.input,"metadata":turn.metadata})).into_bytes());
        server.abort();
    }
}

#[tokio::test]
async fn trace_binding_rejection_has_no_fallback_put_or_inferred_negotiation() {
    let (client, captured, server) = fixture(true, None, true).await;
    let result = client
        .bind(
            &ProviderSlug::new("nyxbot").unwrap(),
            &BindingId::new("binding_one").unwrap(),
            &binding_request(),
        )
        .await;
    assert!(matches!(
        result,
        Err(ProviderError::Rejected { status: 422, .. })
    ));
    let calls = captured.0.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        serde_json::from_slice::<Value>(&calls[0].1).unwrap()["capabilities"],
        json!(["nyxid_turn_trace_v1"])
    );
    server.abort();
}
