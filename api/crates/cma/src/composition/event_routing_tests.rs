use super::*;
use axum::{http::HeaderMap, routing::post, Json, Router};
use serde_json::json;

#[test]
fn operator_pause_keeps_its_category_through_binding_event_selection() {
    use layer2_core::sandbox::binding::BindingError;
    assert!(matches!(
        binding_route_error(BindingError::AdmissionPaused),
        BindingRouteError::AdmissionPaused
    ));
    for error in [
        BindingError::Forbidden,
        BindingError::NotFound,
        BindingError::Conflict,
        BindingError::Unavailable("fixture".into()),
    ] {
        assert!(matches!(
            binding_route_error(error),
            BindingRouteError::Unavailable
        ));
    }
}

fn activity() -> serde_json::Value {
    json!({"schema_version":1,"event_id":"event-a",
        "kind":{"type":"message","text":"hello","mentions_bot":false},
        "actor":{"id":"sender-one","kind":"human"},
        "conversation":{"id":"chat-a","kind":"private"},
        "occurred_at":"2026-09-29T00:00:00Z", "source":{"type":"nyxid_relay","platform":"telegram","route_id":"route-a"}})
}
fn identity(activity: &serde_json::Value) -> ActivityIdentity {
    serde_json::from_value(activity.clone()).unwrap()
}
#[test]
fn source_user_affinity_is_stable_across_events_and_chats_but_separates_authority() {
    let mut first = activity();
    let expected = source_user_session("binding-a", 1, &identity(&first)).unwrap();
    first["conversation"]["id"] = "chat-b".into();
    first["event_id"] = "event-b".into();
    assert_eq!(
        source_user_session("binding-a", 1, &identity(&first)).unwrap(),
        expected
    );
    assert_ne!(
        source_user_session("binding-a", 2, &identity(&first)).unwrap(),
        expected
    );
    assert_ne!(
        source_user_session("binding-b", 1, &identity(&first)).unwrap(),
        expected
    );
    first["actor"]["id"] = "sender-two".into();
    assert_ne!(
        source_user_session("binding-a", 1, &identity(&first)).unwrap(),
        expected
    );
}
#[test]
fn system_and_unattested_custom_senders_do_not_acquire_human_affinity() {
    let mut first = activity();
    first["actor"]["kind"] = "system".into();
    assert!(source_user_session("binding", 1, &identity(&first)).is_err());
    first["actor"]["kind"] = "human".into();
    first["source"] = json!({"type":"custom"});
    assert!(source_user_session("binding", 1, &identity(&first)).is_err());
}

#[test]
fn attested_connection_completion_and_native_source_keep_original_actor_affinity() {
    let mut value = activity();
    let expected = source_user_session("binding", 1, &identity(&value)).unwrap();
    value["source"] = json!({"type":"native-fixture", "platform":"telegram", "bot_id":"123"});
    assert_eq!(
        source_user_session("binding", 1, &identity(&value)).unwrap(),
        expected
    );
    value["actor"]["kind"] = "system".into();
    value["kind"] = json!({"type":"event","ce":{"type":"gateway.connection.ready","data":{"connection_ref":"opaque","status":"ready"}}});
    assert_eq!(
        source_user_session("binding", 1, &identity(&value)).unwrap(),
        expected
    );
    value["kind"]["ce"]["type"] = "unrelated.system.event".into();
    assert!(source_user_session("binding", 1, &identity(&value)).is_err());
}

#[tokio::test]
async fn verifier_sends_complete_activity_and_requires_exact_gateway_proof() {
    let mut original = activity();
    original["kind"]["text"] = "x".repeat(32 * 1024).into();
    original["kind"]["attachments"] = json!([{
        "reference":"fixture-image", "kind":"image", "filename":"photo.png",
        "mime_type":"image/png", "size_bytes":1024
    }]);
    original["actor"]["display_name"] = "Visitor".into();
    original["conversation"]["thread_id"] = "thread-a".into();
    let event = ExternalEventContext {
        provider_conversation_id: None,
        source_id: "channel-a".into(),
        conversation_id: "chat-a".into(),
        event_id: "event-a".into(),
        reference: "cmaeg1.ev.fixture".into(),
        payload: json!({"activity":original}),
    };
    for mismatch in [
        None,
        Some("activity_verified"),
        Some("channel_id"),
        Some("conversation_id"),
        Some("provider_binding_id"),
        Some("event_id"),
    ] {
        let expected = event.payload["activity"].clone();
        let app = Router::new().route(
            "/v1/events/cmaeg1.ev.fixture/context",
            post(move |headers: HeaderMap, Json(body): Json<serde_json::Value>| {
                let expected = expected.clone();
                async move {
                    assert_eq!(headers["authorization"], "Bearer fixture-credential");
                    assert_eq!(body, expected, "authenticate the complete original Activity");
                    let mut proof = json!({"channel_id":"channel-a","conversation_id":"chat-a",
                        "provider_binding_id":"binding-a","event_id":"event-a","activity_verified":true});
                    if let Some(field) = mismatch {
                        proof[field] = if field == "activity_verified" {
                            false.into()
                        } else {
                            "wrong-source".into()
                        };
                    }
                    Json(proof)
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let gateway = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let result = verify_activity(
            &reqwest::Client::new(),
            &gateway,
            "binding-a",
            "fixture-credential",
            &event,
        )
        .await;
        server.abort();
        if mismatch.is_some() {
            assert!(matches!(result, Err(BindingRouteError::Forbidden)));
        } else {
            let verified = result.unwrap();
            assert_eq!(verified.event_id, "event-a");
            assert_eq!(verified.actor.id, "sender-one");
            assert_eq!(
                source_user_session("binding-a", 1, &verified).unwrap(),
                source_user_session("binding-a", 1, &identity(&original)).unwrap()
            );
        }
    }
}
