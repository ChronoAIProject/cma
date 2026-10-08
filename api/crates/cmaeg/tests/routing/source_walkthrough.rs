//! Executable local owner walkthrough for the source-management HTTP contract.
use super::{
    custom::event, management_support::*, relay_support::Envelope, routing_fixture as fixture,
    support::native,
};
use reqwest::Method;
use serde_json::{json, Value};

fn relay(platform: &str, route: &str) -> Value {
    let mut source = fixture::relay(platform, route);
    source.as_object_mut().unwrap().remove("jwks_url");
    source
}
fn callback(platform: &str, route: &str) -> Envelope {
    let mut envelope = Envelope::on(platform);
    envelope.payload["conversation"]["id"] = route.into();
    envelope.reply["conversation_id"] = route.into();
    envelope
}

#[tokio::test]
async fn source_owner_walkthrough_admits_new_sources_then_revokes_lark_immediately() {
    let mut app = OwnerApp::with_sources(true, true).await;
    let created = checked(app.ingress.client.post(format!("{}/v1/channels", app.base))
        .bearer_auth("token-alice").json(&json!({
            "schema_version":1,"name":"dynamic-source-walkthrough","profile":profile("cma_codex"),
            "provider":{"slug":"cma_codex"},"sources":[fixture::telegram()],
            "partition":{"version":1,"key":"conversation"},
            "reply":{"auto_final":true,"ack_after_ms":3000,"sinks":[{"type":"source"}]},
            "credentials":{"agent_key":"fixture-owner-agent","telegram_bot_token":"77:fixture",
                "telegram_webhook_secret":"fixture-webhook"}
        })).send().await.unwrap(), 201).await;
    app.ingress.id = created["channel_id"].as_str().unwrap().into();
    let chat_before = checked(
        app.ingress
            .native(&native("before source attachment"))
            .await,
        202,
    )
    .await;
    let lark = app
        .call(
            Method::POST,
            "/sources",
            Some(&json!({
                "expected_version":created["version"],"source":relay("lark",fixture::ROUTE),
                "credentials":{"channel_key":"fixture-shared-relay-key"}
            })),
            201,
        )
        .await;
    assert_eq!(lark["source_id"], "relay_lark");
    let feishu_route = "44c35208-fc32-42a5-96ed-683adf569214";
    let feishu = app
        .call(
            Method::POST,
            "/sources",
            Some(&json!({
                "expected_version":lark["version"],"source":relay("feishu",feishu_route),
                "credentials":{"channel_key":"fixture-shared-relay-key"}
            })),
            201,
        )
        .await;
    assert_eq!(feishu["source_id"], "relay_feishu");
    let version = app.local_jwks().await;
    checked(
        app.ingress.relay(&callback("lark", fixture::ROUTE)).await,
        202,
    )
    .await;
    checked(
        app.ingress.relay(&callback("feishu", feishu_route)).await,
        202,
    )
    .await;
    let custom = app.call(Method::POST, "/sources", Some(&json!({
        "expected_version":version,"source":fixture::custom(),"partition_custom_key":"event_subject",
        "credentials":{"hmac_secrets":[{"key_id":"key-1","secret":"Zml4dHVyZQ=="}]}
    })), 201).await;
    assert_eq!(custom["source_id"], "custom");
    assert_eq!(
        custom["definition"]["partition"]["custom_key"],
        "event_subject"
    );
    let mut order = event(None);
    order["subject"] = "order-42".into();
    checked(app.ingress.custom(order, false).await, 202).await;
    let chat_after = checked(
        app.ingress.native(&native("after source attachment")).await,
        202,
    )
    .await;
    assert_eq!(
        chat_before["conversation_id"],
        chat_after["conversation_id"]
    );
    let policy = app.call(Method::PUT, "/routing", Some(&json!({
        "expected_version":custom["version"],"default_consumer":{"name":"main"},
        "default_targets":["default"],"default_reply":{"type":"telegram_chat","source_id":"telegram","chat_id":"42"}
    })), 200).await;
    let detached = app
        .call(
            Method::DELETE,
            &format!("/sources/relay_lark?expected_version={}", policy["version"]),
            None,
            200,
        )
        .await;
    let refused = checked(
        app.ingress.relay(&callback("lark", fixture::ROUTE)).await,
        400,
    )
    .await;
    assert_eq!(refused["detail"], "no matching relay source");
    checked(
        app.ingress.relay(&callback("feishu", feishu_route)).await,
        202,
    )
    .await;
    let protected = app
        .call(
            Method::DELETE,
            &format!("/sources/telegram?expected_version={}", detached["version"]),
            None,
            409,
        )
        .await;
    assert_eq!(protected["detail"], "default_reply_source_in_use");
    app.ingress.idle().await;
    assert_eq!(app.ingress.recorder.requests.lock().unwrap().len(), 6);
    println!("source walkthrough: native Telegram, shared Lark/Feishu relays and callbacks, custom HMAC event_subject, unchanged chat conversation, immediate Lark revocation, retained Feishu and protected default reply passed");
    app.close().await;
}
