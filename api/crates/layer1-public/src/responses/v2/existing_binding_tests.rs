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
async fn borrowed_binding_queue_lifetime_is_pinned_at_admission_and_replay() {
    use layer2_core::{
        ids::AgentResponseId,
        sandbox::agent_io::{AgentCommandStore, AgentCommandWorkerStore, CommandFailureCode},
    };
    let (app, store, access, original) = fixture().await;
    let authority = access
        .resolve(&original, "conversation-external")
        .await
        .unwrap();
    let mut binding = authority.binding;
    binding.binding_id = "binding-long-queue".into();
    binding.key_sha256 = "c".repeat(64);
    binding.level = 1;
    binding.revision = 1;
    binding.profile.queue_lifetime_seconds = Some(86400);
    binding.existing_agent = Some(ExistingBindingAgent {
        agent_id: authority.conversation.agent_id,
        worktree_id: authority.conversation.worktree_id,
    });
    store.save_binding(None, binding.clone()).await.unwrap();
    let conversation =
        BindingConversation::new(&binding, "queue".into(), layer2_core::now_rfc3339()).unwrap();
    store
        .save_binding_conversation(binding.revision, None, conversation)
        .await
        .unwrap();
    let principal = access
        .authenticate(&binding.key_sha256)
        .await
        .unwrap()
        .unwrap();
    let submit = |key: &str| {
        let mut request = request(
            "/api/v2/responses",
            &principal,
            json!({"conversation":"agt_fixture","stream":true,"input":"Run the full sync"}),
        );
        request
            .headers_mut()
            .insert("Idempotency-Key", key.parse().unwrap());
        request
    };
    let mut original_record = None;
    for (key, expected) in [("first", 86400), ("first", 86400), ("next", 3600)] {
        let response = app.clone().oneshot(submit(key)).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let id = AgentResponseId(
            response.headers()["x-cma-response-id"]
                .to_str()
                .unwrap()
                .to_owned(),
        );
        let record = store.get_response_command(&id).await.unwrap().unwrap();
        let seconds = layer2_core::seconds_from_rfc3339(&record.queue_deadline).unwrap()
            - layer2_core::seconds_from_rfc3339(&record.receipt.admitted_at).unwrap();
        assert_eq!(seconds, expected);
        if let Some(first) = &original_record {
            if key == "first" {
                assert_eq!(&record, first);
            }
        } else {
            original_record = Some(record);
            binding = store.binding(&binding.binding_id).await.unwrap().unwrap();
            let mut policy = binding.profile.clone();
            policy.queue_lifetime_seconds = Some(3600);
            let next = binding
                .reconfigure(
                    policy,
                    binding.key_sha256.clone(),
                    layer2_core::now_rfc3339(),
                )
                .unwrap();
            store
                .save_binding(Some(binding.revision), next)
                .await
                .unwrap();
        }
    }
    // Both long-lived commands occupy the same unchanged 64-slot Agent lane.
    for index in 2..64 {
        assert_eq!(
            app.clone()
                .oneshot(submit(&format!("queued-{index}")))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
    }
    let full = app.clone().oneshot(submit("queue-full")).await.unwrap();
    assert_eq!(full.status(), StatusCode::TOO_MANY_REQUESTS);
    let bytes = axum::body::to_bytes(full.into_body(), 4096).await.unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&bytes).unwrap()["error"]["code"],
        "agent_command_queue_full"
    );
    let original = original_record.unwrap();
    assert!(original.work.dispatch.is_none());
    store
        .reject_unclaimed_command(
            &original.receipt.envelope.command_id,
            original.revision,
            CommandFailureCode::QueueTimeout,
            original.queue_deadline.clone(),
        )
        .await
        .unwrap();
    let response_id = original.receipt.response_id.as_ref().unwrap();
    let mut read = request(
        &format!("/api/v2/responses/{response_id}"),
        &principal,
        json!({}),
    );
    *read.method_mut() = axum::http::Method::GET;
    let response = app.clone().oneshot(read).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    let failed: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(failed["status"], "failed");
    assert_eq!(failed["error"]["code"], "agent_command_queue_timeout");
    let expired = store
        .get_response_command(response_id)
        .await
        .unwrap()
        .unwrap();
    assert!(expired.work.dispatch.is_none());
    let replay = app.oneshot(submit("first")).await.unwrap();
    assert_eq!(
        replay.headers()["x-cma-response-id"].to_str().unwrap(),
        response_id.as_str()
    );
    assert_eq!(
        store
            .get_response_command(response_id)
            .await
            .unwrap()
            .unwrap(),
        expired
    );
}

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
