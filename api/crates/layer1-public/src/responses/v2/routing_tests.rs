use super::{binding_tests, BindingEventRouter, BindingRouteError};
use axum::{
    body::{to_bytes, Body},
    http::{Method, StatusCode},
};
use layer2_core::{
    sandbox::{
        agent_io::{AgentCommandStore, ExternalEventContext},
        binding::BindingConversationState,
    },
    AgentId,
};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

struct ProviderSelection;
#[async_trait::async_trait]
impl BindingEventRouter for ProviderSelection {
    async fn route(
        &self,
        _: &crate::AuthenticatedResourceCaller,
        event: &mut ExternalEventContext,
    ) -> Result<Option<AgentId>, BindingRouteError> {
        if event.payload["activity"]["actor"]["id"] != "verified-user" {
            return Err(BindingRouteError::Forbidden);
        }
        event.provider_conversation_id = Some("conversation-external".into());
        Ok(Some(AgentId("agt_fixture".into())))
    }
}

fn payload(conversation: &str, event: &str) -> Value {
    json!({"schema_version":1,"channel_id":"channel-one","conversation_id":conversation,
        "event_ref":format!("sealed-ref-{event}"),
        "activity":{"schema_version":1,"event_id":event,"actor":{"id":"verified-user"},
        "kind":{"type":"message","text":"same sender, different chat"}}})
}

#[tokio::test]
async fn provider_selection_keeps_both_original_reply_contexts_durable_and_replayable() {
    let (app, store, _, principal) = binding_tests::fixture_for_owner_with_router(
        BindingConversationState::Ready,
        false,
        "organization",
        Some(Arc::new(ProviderSelection)),
    )
    .await;
    for (chat, event) in [("source-chat-A", "event-A"), ("source-chat-B", "event-B")] {
        let original = payload(chat, event);
        let body =
            json!({"conversation":chat,"stream":true,"input":"hello","event_context":original});
        let mut request = binding_tests::request("/api/v2/responses", &principal, body.clone());
        request
            .headers_mut()
            .insert("idempotency-key", event.parse().unwrap());
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let id = response.headers()["x-cma-response-id"]
            .to_str()
            .unwrap()
            .to_owned();
        drop(response);
        let record = store
            .get_response_command(&layer2_core::ids::AgentResponseId(id.clone()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(record.receipt.envelope.agent_id.as_str(), "agt_fixture");
        let retained = record.receipt.envelope.command.external_event().unwrap();
        assert_eq!(retained.payload, original);
        assert_eq!(retained.conversation_id, chat);
        assert_eq!(
            retained.provider_conversation_id.as_deref(),
            Some("conversation-external")
        );
        let mut replay = binding_tests::request("/api/v2/responses", &principal, body);
        replay
            .headers_mut()
            .insert("idempotency-key", event.parse().unwrap());
        assert_eq!(
            app.clone().oneshot(replay).await.unwrap().headers()["x-cma-response-id"],
            id
        );
        // Source reads need no mapping from transport chat to the chosen agent.
        let mut read = binding_tests::request(
            &format!(
                "/api/v1/cmaeg/bindings/binding-responses/conversations/{chat}/events/{event}"
            ),
            &principal,
            Value::Null,
        );
        *read.method_mut() = Method::GET;
        *read.body_mut() = Body::empty();
        let result = app.clone().oneshot(read).await.unwrap();
        assert_eq!(result.status(), StatusCode::OK);
        let returned: Value =
            serde_json::from_slice(&to_bytes(result.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(returned, original);
    }
    let mut bad = payload("source-chat-C", "event-C");
    bad["activity"]["actor"]["id"] = "foreign-user".into();
    let response = app
        .oneshot(binding_tests::request(
            "/api/v2/responses",
            &principal,
            json!({"conversation":"source-chat-C","stream":true,"input":"bad","event_context":bad}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(response.headers().get("x-cma-response-id").is_none());
}

#[test]
fn callers_cannot_inject_provider_session_selection_into_transport_context() {
    let mut value = payload("source-chat", "event");
    value["provider_conversation_id"] = "conversation-external".into();
    assert!(super::event_context::parse(value).is_err());
}

#[tokio::test]
async fn provider_selection_operator_pause_refuses_before_a_response_is_admitted() {
    struct Paused;
    #[async_trait::async_trait]
    impl BindingEventRouter for Paused {
        async fn route(
            &self,
            _: &crate::AuthenticatedResourceCaller,
            _: &mut ExternalEventContext,
        ) -> Result<Option<AgentId>, BindingRouteError> {
            Err(BindingRouteError::AdmissionPaused)
        }
    }
    let (app, _, _, principal) = binding_tests::fixture_for_owner_with_router(
        BindingConversationState::Ready,
        false,
        "human",
        Some(Arc::new(Paused)),
    )
    .await;
    let response = app
        .oneshot(binding_tests::request(
            "/api/v2/responses",
            &principal,
            json!({"conversation":"source-chat", "stream":true, "input":"hello",
                "event_context":payload("source-chat", "paused-event")}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.headers()["retry-after"], "60");
    assert!(!response.headers().contains_key("x-cma-response-id"));
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(body["error"]["code"], "admission_paused");
    assert_eq!(
        body["error"]["message"],
        layer2_core::admission::ADMISSION_PAUSED_GUIDANCE
    );
}

#[tokio::test]
async fn stale_provider_generation_is_rejected_before_source_user_routing_or_command_admission() {
    struct MustNotRoute;
    #[async_trait::async_trait]
    impl BindingEventRouter for MustNotRoute {
        async fn route(
            &self,
            _: &crate::AuthenticatedResourceCaller,
            _: &mut ExternalEventContext,
        ) -> Result<Option<AgentId>, BindingRouteError> {
            panic!("stale generation must not select or create a source-user Agent")
        }
    }
    let (app, _, _, principal) = binding_tests::fixture_for_owner_with_router(
        BindingConversationState::Ready,
        false,
        "human",
        Some(Arc::new(MustNotRoute)),
    )
    .await;
    let response = app.oneshot(binding_tests::request("/api/v2/responses", &principal,
        json!({"conversation":"source-chat", "stream":true, "input":"stale",
            "binding_generation":principal.generation()+1, "event_context":payload("source-chat", "stale-event")})))
        .await.unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert!(response.headers().get("x-cma-response-id").is_none());
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(body["error"]["code"], "binding_generation_changed");
}

#[tokio::test]
async fn current_provider_generation_keeps_source_user_submission_and_replay() {
    let (app, _, _, principal) = binding_tests::fixture_for_owner_with_router(
        BindingConversationState::Ready,
        false,
        "human",
        Some(Arc::new(ProviderSelection)),
    )
    .await;
    let body = json!({"conversation":"source-chat", "stream":true,"input":"hello",
        "binding_generation":principal.generation(), "event_context":payload("source-chat", "fresh-event")});
    let first = app
        .clone()
        .oneshot(binding_tests::request(
            "/api/v2/responses",
            &principal,
            body.clone(),
        ))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let response_id = first.headers()["x-cma-response-id"].clone();
    drop(first);
    let replay = app
        .oneshot(binding_tests::request(
            "/api/v2/responses",
            &principal,
            body,
        ))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(replay.headers()["x-cma-response-id"], response_id);
}

#[tokio::test]
async fn sender_context_errors_match_the_shared_public_contract() {
    let cases: Vec<serde_json::Value> = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../cmaeg/tests/fixtures/provider_sender_refusals.json"
    )))
    .unwrap();
    assert_eq!(cases.len(), 3);
    for case in cases {
        let error = match case["body"]["error"]["code"].as_str().unwrap() {
            "verified_sender_context_invalid" => BindingRouteError::SenderInvalid,
            "verified_sender_context_unsupported" => BindingRouteError::SenderUnsupported,
            "verified_sender_context_unavailable" => BindingRouteError::SenderUnavailable,
            _ => panic!("unknown contract case"),
        };
        let response = super::routing::error(error);
        assert_eq!(
            u64::from(response.status().as_u16()),
            case["status"].as_u64().unwrap()
        );
        assert_eq!(
            response.headers()["content-type"].to_str().unwrap(),
            case["content_type"].as_str().unwrap()
        );
        assert_eq!(
            response
                .headers()
                .get("retry-after")
                .map(|v| v.to_str().unwrap()),
            case["retry_after"].as_str()
        );
        assert!(!response.headers().contains_key("x-cma-response-id"));
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(),
            case["body"]
        );
    }
}
