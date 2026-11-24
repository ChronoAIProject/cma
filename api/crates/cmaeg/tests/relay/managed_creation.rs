use super::support::*;
use cmaeg_core::{activity::ActivityKind, seal::ReplyAddress};
use serde_json::{json, Value};

pub fn creation() -> Envelope {
    let mut envelope = Envelope::new();
    envelope.payload["conversation"]["platform_id"] = "7".into();
    envelope.payload["content"] = json!({"type":"unknown","text":null});
    envelope.payload["raw_platform_data"] = json!({"update_id":123,"message":{"message_id":11,"date":now()-30,"chat":{"id":7,"type":"private"},"from":{"id":7,"is_bot":false},"managed_bot_created":{"bot":{"id":88,"is_bot":true,"first_name":"Created","username":"created_bot","token":"not-public"}}}});
    envelope
}
#[tokio::test]
async fn signed_creation_receipt_becomes_stable_event_with_original_identity_and_reply() {
    let fixture = Fixture::new(|_| {}, Default::default()).await;
    let envelope = creation();
    for _ in 0..2 {
        assert_eq!(
            fixture.send(&envelope).await.json::<Value>().await.unwrap()["status"],
            "accepted"
        );
    }
    let requests = fixture.recorder.requests.lock().unwrap();
    let request = &requests[0];
    assert_eq!(request.activity, requests[1].activity);
    assert_eq!(request.activity.actor.id, "7");
    assert_eq!(request.activity.conversation.id, "7");
    let ActivityKind::Event { ce, .. } = &request.activity.kind else {
        panic!("expected creation event")
    };
    assert_eq!(ce.source, "nyxid:route:route-1");
    assert_eq!(ce.r#type, "telegram.managed_bot_created");
    assert_eq!(ce.data.as_ref().unwrap()["bot"]["id"], "88");
    assert!(!serde_json::to_string(ce).unwrap().contains("not-public"));
    let opened = request
        .event_ref
        .open(&fixture.state.master_keys, now())
        .unwrap()
        .payload;
    assert!(opened.verifies_activity(&request.activity));
    assert!(
        matches!(&opened.reply,ReplyAddress::NyxidRelay{route_id,message_id,..}if route_id=="route-1"&&message_id=="message-1")
    );
    let mut tampered = request.activity.clone();
    let ActivityKind::Event { ce, .. } = &mut tampered.kind else {
        unreachable!()
    };
    ce.data.as_mut().unwrap()["bot"]["id"] = "99".into();
    assert!(!opened.verifies_activity(&tampered));
}
#[tokio::test]
async fn creation_receipt_requires_signed_exact_human_private_original_message() {
    let fixture = Fixture::new(|_| {}, Default::default()).await;
    for (pointer, value) in [
        ("/raw_platform_data/message/from/is_bot", json!(true)),
        ("/raw_platform_data/message/chat/type", json!("group")),
        (
            "/raw_platform_data/message/forward_origin",
            json!({"type":"user"}),
        ),
        ("/raw_platform_data/message/text", json!("fake creation")),
        (
            "/raw_platform_data/message/managed_bot_created/bot/id",
            json!(0),
        ),
        ("/sender/platform_id", json!("other")),
    ] {
        let mut envelope = creation();
        let (parent, key) = pointer.rsplit_once('/').unwrap();
        envelope.payload.pointer_mut(parent).unwrap()[key] = value;
        let response = fixture.send(&envelope).await;
        assert_ne!(
            response.json::<Value>().await.unwrap()["status"],
            "accepted",
            "{pointer}"
        );
    }
    let envelope = creation();
    let (mut body, token) = envelope.signed();
    body.push(b' ');
    assert_eq!(
        fixture.request(body, &token).send().await.unwrap().status(),
        401
    );
    assert!(fixture.recorder.requests.lock().unwrap().is_empty());
}
#[tokio::test]
async fn managed_updates_are_not_creation_consent_and_sender_policy_still_applies() {
    let fixture = Fixture::new(
        |v| v["sources"][0]["admission"] = json!({"type":"allowlist","sender_ids":["99"]}),
        Default::default(),
    )
    .await;
    assert_eq!(
        fixture
            .send(&creation())
            .await
            .json::<Value>()
            .await
            .unwrap()["reason"],
        "sender_denied"
    );
    let mut envelope = creation();
    envelope.payload["raw_platform_data"] =
        json!({"update_id":124,"managed_bot":{"user":{"id":7},"bot":{"id":88}}});
    assert_eq!(
        fixture.send(&envelope).await.json::<Value>().await.unwrap()["reason"],
        "system_event"
    );
    assert!(fixture.recorder.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn signed_telegram_relay_member_joins_require_both_gates_and_durable_runtime() {
    use axum::{
        body::{to_bytes, Body},
        extract::{Path, State},
    };
    use cmaeg::http::callbacks;
    let fixture = Fixture::new(
        |definition| {
            let source = &mut definition["sources"][0];
            source["bot_id"] = "77".into();
            source["member_join_chat_ids"] = json!(["-100"]);
            source["admission"] = json!({
                "type":"scoped", "senders":{"type":"open"}, "groups":"all",
                "chats":{"type":"allowlist","ids":["-100","-101"]},
            });
        },
        Default::default(),
    )
    .await;
    let mut envelope = Envelope::new();
    envelope.payload["conversation"]["platform_id"] = "-100".into();
    envelope.payload["conversation"]["type"] = "group".into();
    envelope.payload["content"] = json!({"type":"unknown"});
    envelope.payload["raw_platform_data"] = json!({"update_id":91,"message":{
        "message_id":11,"date":now()-2,"chat":{"id":-100,"type":"supergroup"},
        "from":{"id":7,"is_bot":false},
        "new_chat_members":[{"id":12,"is_bot":false,"first_name":"Ada"}],
    }});
    let (body, token) = envelope.signed();
    let disabled = fixture.send(&envelope).await.json::<Value>().await.unwrap();
    assert_eq!(disabled["reason"], "system_event");
    let mut state = fixture.state.clone();
    state.relay_telegram_member_joins_enabled = true;
    let send = |body, token: &str| {
        callbacks::callback(
            State(state.clone()),
            Path(fixture.id.to_string()),
            fixture.local_request(Body::from(body), token),
        )
    };
    let mut tampered = body.clone();
    tampered.push(b' ');
    assert_eq!(send(tampered, &token).await.status(), 401);
    assert_eq!(send(body, &token).await.status(), 503);
    envelope.payload["conversation"]["platform_id"] = "-101".into();
    envelope.payload["raw_platform_data"]["message"]["chat"]["id"] = (-101).into();
    let (body, token) = envelope.signed();
    let response = send(body, &token).await;
    assert_eq!(response.status(), 202);
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap()["reason"],
        "system_event"
    );
    assert!(fixture.recorder.requests.lock().unwrap().is_empty());
}
