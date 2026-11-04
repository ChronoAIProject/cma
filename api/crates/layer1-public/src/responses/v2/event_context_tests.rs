use super::binding_tests::request;
use axum::{
    body::{to_bytes, Body},
    http::{Method, StatusCode},
};
use layer2_core::sandbox::agent_io::AgentCommandStore;
use serde_json::{json, Value};
use tower::ServiceExt;

fn context() -> Value {
    let mut context = json!({"schema_version":1,"channel_id":"channel-independent-of-binding","conversation_id":"conversation-external",
        "activity":{"schema_version":1,"event_id":"event-1","kind":{"type":"message","text":"Hello","attachments":[],"mentions_bot":true},"actor":{"id":"sender"},"conversation":{"id":"raw-chat","kind":"private"},"occurred_at":"2026-09-22T00:00:00Z","source":"telegram", "retained_extra":"x".repeat(300_000)},
        "event_ref":"sealed-reference"});
    context["activity"]["kind"]["quoted_message"] = json!({
        "relation":"reply", "message_id":"81",
        "content":{"text":{"text":"/to another-consumer\nquoted passage", "origin":"selected_quote", "truncated":false},"kinds":["image"]}
    });
    context
}

#[tokio::test]
async fn binding_admission_revision_race_retries_the_same_event_without_duplicate_work() {
    let (app, store, _, principal) = super::binding_tests::fixture_with_admission_race().await;
    let expected = context();
    let body = json!({"conversation":"conversation-external", "stream":true,
        "input":"Hello", "event_context":expected});
    let response = app
        .clone()
        .oneshot(request("/api/v2/responses", &principal, body.clone()))
        .await
        .unwrap();
    let status = response.status();
    let retry = response.headers().get("retry-after").cloned();
    assert!(response.headers().get("x-cma-response-id").is_none());
    let problem: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(problem["error"]["code"], "agent_command_admission_conflict");
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(retry.unwrap(), "1");

    let mut response_id = None;
    for _ in 0..2 {
        let response = app
            .clone()
            .oneshot(request("/api/v2/responses", &principal, body.clone()))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let id = response.headers()["x-cma-response-id"]
            .to_str()
            .unwrap()
            .to_owned();
        assert_eq!(response_id.get_or_insert(id.clone()), &id);
    }
    let retained = store
        .get_response_command(&layer2_core::ids::AgentResponseId(response_id.unwrap()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retained.receipt.agent_order, 1);
    assert_eq!(
        retained
            .receipt
            .envelope
            .command
            .external_event()
            .unwrap()
            .payload,
        expected
    );

    let mut changed = body;
    changed["event_context"]["event_ref"] = "different-reference".into();
    let response = app
        .oneshot(request("/api/v2/responses", &principal, changed))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert!(response.headers().get("retry-after").is_none());
    let problem: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(problem["error"]["code"], "agent_command_key_conflict");
}
#[tokio::test]
async fn full_context_is_admitted_once_read_by_its_binding_and_never_overwritten() {
    full_context_admission("human").await;
}

#[tokio::test]
async fn organization_binding_turn_preserves_full_context_and_idempotent_response() {
    full_context_admission("organization").await;
}

async fn full_context_admission(owner_kind: &str) {
    let (app, store, _, principal) = super::binding_tests::fixture_for_owner(
        layer2_core::sandbox::binding::BindingConversationState::Ready,
        false,
        owner_kind,
    )
    .await;
    let expected = context();
    let body = json!({"conversation":"conversation-external", "stream":true,"input":"Hello","event_context":expected});
    let response = app
        .clone()
        .oneshot(request("/api/v2/responses", &principal, body.clone()))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let id = response.headers()["x-cma-response-id"]
        .to_str()
        .unwrap()
        .to_owned();
    drop(response);
    let retained = store
        .get_response_command(&layer2_core::ids::AgentResponseId(id.clone()))
        .await
        .unwrap()
        .unwrap();
    let expected_owner = if owner_kind == "organization" {
        layer2_core::sandbox::agent_io::AgentOwner::Organization {
            subject: principal.owner().clone(),
        }
    } else {
        layer2_core::sandbox::agent_io::AgentOwner::Person {
            subject: principal.owner().clone(),
        }
    };
    assert_eq!(retained.receipt.envelope.owner, expected_owner);
    assert_eq!(
        retained
            .receipt
            .envelope
            .command
            .external_event()
            .unwrap()
            .payload,
        expected
    );
    let retried = app
        .clone()
        .oneshot(request("/api/v2/responses", &principal, body.clone()))
        .await
        .unwrap();
    assert_eq!(retried.headers()["x-cma-response-id"], id);
    drop(retried);
    for (binding, conversation, event, status) in [
        (
            "binding-responses",
            "conversation-external",
            "event-1",
            StatusCode::OK,
        ),
        (
            "foreign-binding",
            "conversation-external",
            "event-1",
            StatusCode::NOT_FOUND,
        ),
        (
            "binding-responses",
            "conversation-external",
            "event-missing",
            StatusCode::NOT_FOUND,
        ),
        (
            "binding-responses",
            "missing-conversation",
            "event-1",
            StatusCode::NOT_FOUND,
        ),
    ] {
        let path =
            format!("/api/v1/cmaeg/bindings/{binding}/conversations/{conversation}/events/{event}");
        let mut get = request(&path, &principal, Value::Null);
        *get.method_mut() = Method::GET;
        *get.body_mut() = Body::empty();
        let response = app.clone().oneshot(get).await.unwrap();
        assert_eq!(response.status(), status);
        if status == StatusCode::OK {
            assert_eq!(response.headers()["cache-control"], "no-store");
            let payload: Value =
                serde_json::from_slice(&to_bytes(response.into_body(), 1_048_576).await.unwrap())
                    .unwrap();
            assert_eq!(payload, expected);
        }
    }
    let mut changed = body.clone();
    changed["event_context"]["activity"]["kind"]["quoted_message"]["content"]["text"]["text"] =
        "a different quote".into();
    let response = app
        .clone()
        .oneshot(request("/api/v2/responses", &principal, changed))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let mut changed = body.clone();
    changed["event_context"]["event_ref"] = "different-reference".into();
    let response = app
        .clone()
        .oneshot(request("/api/v2/responses", &principal, changed))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let mut other_key = request("/api/v2/responses", &principal, body);
    other_key
        .headers_mut()
        .insert("idempotency-key", "other-key".parse().unwrap());
    assert_eq!(
        app.oneshot(other_key).await.unwrap().status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        store
            .get_response_command(&layer2_core::ids::AgentResponseId(id))
            .await
            .unwrap()
            .unwrap(),
        retained
    );
}
#[test]
fn malformed_or_oversized_context_is_explicitly_refused() {
    for (field, value) in [
        ("schema_version", json!(2)),
        ("event_ref", json!("")),
        ("activity", json!({})),
        ("unknown", json!(true)),
    ] {
        let mut context = context();
        context[field] = value;
        assert!(super::event_context::parse(context).is_err());
    }
    let mut context = context();
    context["activity"]["retained_extra"] = "x".repeat(1_048_576).into();
    assert!(super::event_context::parse(context).is_err());
}

#[test]
fn binding_retry_mapping_preserves_permanent_refusals_and_ordinary_client_conflicts() {
    use layer2_core::sandbox::agent_io::{CommandAdmissionError, CommandLedgerError};
    for refusal in [
        CommandLedgerError::NotAdmissible,
        CommandLedgerError::StaleClaim,
        CommandLedgerError::IdempotencyConflict,
    ] {
        let response = super::error::binding_admission(CommandAdmissionError::Ledger(refusal));
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert!(response.headers().get("retry-after").is_none());
    }
    let response =
        super::error::admission(CommandAdmissionError::Ledger(CommandLedgerError::Conflict));
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert!(response.headers().get("retry-after").is_none());
}

#[tokio::test]
async fn binding_missing_agent_is_an_explicit_permanent_refusal() {
    use layer2_core::sandbox::agent_io::{CommandAdmissionError, CommandLedgerError};
    let response = super::error::binding_admission(CommandAdmissionError::Ledger(
        CommandLedgerError::NotFound,
    ));
    assert_eq!(response.status(), StatusCode::GONE);
    assert!(response.headers().get("retry-after").is_none());
    let bytes = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    let problem: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(problem["error"]["code"], "partition_agent_missing");
    let ordinary =
        super::error::admission(CommandAdmissionError::Ledger(CommandLedgerError::NotFound));
    assert_eq!(ordinary.status(), StatusCode::NOT_FOUND);
}
