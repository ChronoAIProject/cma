use super::binding_tests::{fixture, request};
use crate::rest::agents_tests::{insert_agent, seeded_agent};
use axum::http::StatusCode;
use layer2_core::sandbox::binding::{
    BindingConversation, BindingConversationState, BindingStore, ExistingBindingAgent,
    EXISTING_AGENT_CONVERSATION,
};
use serde_json::{json, Value};
use tower::ServiceExt;

#[tokio::test]
async fn borrowed_binding_resolves_gateway_test_alias_without_event_context() {
    let (app, store, access, original) = fixture().await;
    insert_agent(
        &store,
        seeded_agent(
            "agt_foreign",
            layer2_core::sandbox::AgentStatus::Ready,
            2,
            None,
        ),
        vec![],
    )
    .await;
    let original_conversation = access
        .resolve(&original, "conversation-external")
        .await
        .unwrap();
    let mut borrowed = original_conversation.binding;
    borrowed.binding_id = "binding-borrowed".into();
    borrowed.key_sha256 = "b".repeat(64);
    borrowed.level = 1;
    borrowed.revision = 1;
    borrowed.existing_agent = Some(ExistingBindingAgent {
        agent_id: original_conversation.conversation.agent_id,
        worktree_id: original_conversation.conversation.worktree_id,
    });
    store.save_binding(None, borrowed.clone()).await.unwrap();
    let alias = "conv_e59f2ffc745ce1995bca981c707d1991";
    let mut conversation =
        BindingConversation::new(&borrowed, alias.into(), layer2_core::now_rfc3339()).unwrap();
    assert_eq!(conversation.conversation_id, EXISTING_AGENT_CONVERSATION);
    let principal = access
        .authenticate(&borrowed.key_sha256)
        .await
        .unwrap()
        .unwrap();
    // This is the management /test envelope: no external event to trigger
    // event routing, and the original transport alias from the preceding PUT.
    let body = |alias: &str| {
        json!({"conversation":alias,"stream":true,"metadata":{},
            "input":[{"role":"user","content":[{"type":"input_text","text":"CMAEG channel test"}]}]})
    };
    let response = app
        .clone()
        .oneshot(request("/api/v2/responses", &principal, body(alias)))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    store
        .save_binding_conversation(borrowed.revision, None, conversation.clone())
        .await
        .unwrap();
    let mut response_id = None;
    for alias in [alias, EXISTING_AGENT_CONVERSATION, "agt_fixture"] {
        let response = app
            .clone()
            .oneshot(request("/api/v2/responses", &principal, body(alias)))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let id = response.headers()["x-cma-response-id"]
            .to_str()
            .unwrap()
            .to_owned();
        assert_eq!(response_id.get_or_insert(id.clone()), &id);
    }
    for (caller, alias) in [
        (&principal, "agt_foreign"),
        (&principal, "short"),
        (&principal, "conversation/invalid"),
        (&original, alias),
    ] {
        let response = app
            .clone()
            .oneshot(request("/api/v2/responses", caller, body(alias)))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let problem: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(problem["error"]["code"], "conversation_not_found");
    }
    for state in [
        BindingConversationState::Retiring,
        BindingConversationState::Retired,
    ] {
        let revision = conversation.revision;
        conversation.state = state;
        conversation.revision += 1;
        store
            .save_binding_conversation(borrowed.revision, Some(revision), conversation.clone())
            .await
            .unwrap();
    }
    let response = app
        .oneshot(request("/api/v2/responses", &principal, body(alias)))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
