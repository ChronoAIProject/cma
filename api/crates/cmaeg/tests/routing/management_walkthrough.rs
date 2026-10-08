//! Executable local counterpart of the documented owner management walkthrough.
use super::{
    management_support::*, relay_support::Envelope, routing_fixture as fixture, support::native,
};
use reqwest::Method;
use serde_json::{json, Value};

fn policy(version: &Value) -> Value {
    json!({"expected_version":version,"default_consumer":{"name":"main","label":"Main"},
        "default_targets":["default"],"max_targets":4,"reply_label":"auto"})
}

#[tokio::test]
async fn owner_walkthrough_creates_routes_rotates_and_exits_with_the_flag_on() {
    let mut app = OwnerApp::new(true).await;
    let input = json!({"schema_version":1,"name":"owner-walkthrough","profile":profile("cma_codex"),
        "provider":{"slug":"cma_codex"},
        "sources":[{"type":"telegram","bot_id":"77","bot_username":"golden_bot","admission":{"type":"all"}},
            {"type":"nyxid_relay","issuer":"https://nyx.example","key_id":"key-1","route_ids":[fixture::ROUTE],
                "platform":"lark","admission":{"type":"all"}}],
        "partition":{"version":1,"key":"conversation"},
        "reply":{"auto_final":true,"ack_after_ms":3000,"sinks":[{"type":"source"}]},
        "credentials":{"agent_key":"fixture-default-key","channel_key":"fixture-channel-key",
            "telegram_bot_token":"77:fixture","telegram_webhook_secret":"fixture-webhook"}});
    let created = checked(
        app.ingress
            .client
            .post(format!("{}/v1/channels", app.base))
            .bearer_auth("token-alice")
            .json(&input)
            .send()
            .await
            .unwrap(),
        201,
    )
    .await;
    assert!(created.get("warnings").is_none());
    app.ingress.id = created["channel_id"].as_str().unwrap().into();
    let mut current = app.call(Method::GET, "", None, 200).await;
    current = app
        .call(
            Method::PUT,
            "/routing",
            Some(&policy(&current["version"])),
            200,
        )
        .await;
    let reviewer = app
        .call(
            Method::POST,
            "/consumers",
            Some(&consumer(&current["version"], "reviewer", "cma_codex")),
            201,
        )
        .await;
    current = reviewer["channel"].clone();
    let nyx = app
        .call(
            Method::POST,
            "/consumers",
            Some(&consumer(&current["version"], "nyx", "nyxbot")),
            201,
        )
        .await;
    current = nyx["channel"].clone();
    let reviewer_id = reviewer["consumer"]["id"].as_str().unwrap();
    let nyx_id = nyx["consumer"]["id"].as_str().unwrap();
    assert_eq!(reviewer["consumer"]["pending"], false);
    assert_eq!(nyx["consumer"]["pending"], false);
    let mut routes = policy(&current["version"]);
    routes["rules"] = json!([{"id":"lark-review","when":{"sources":["relay_lark"],"kinds":["message"],"chats":["oc_chat"]},"targets":[reviewer_id,nyx_id]}]);
    routes["default_reply"] = json!({"type":"telegram_chat","source_id":"telegram","chat_id":"42"});
    current = app.call(Method::PUT, "/routing", Some(&routes), 200).await;
    assert_eq!(
        current["warnings"],
        json!([{"pointer":"/routing/default_consumer/mentionable","reason":"mentionable_bypasses_rules"}])
    );
    let version = app.local_jwks().await;
    let plain = checked(app.ingress.native(&native("hello")).await, 202).await;
    assert_eq!(plain["consumers"][0]["consumer_id"], "default");
    let mut lark = Envelope::on("lark");
    lark.payload["conversation"]["id"] = fixture::ROUTE.into();
    lark.reply["conversation_id"] = fixture::ROUTE.into();
    let routed = checked(app.ingress.relay(&lark).await, 202).await;
    assert_eq!(routed["consumers"].as_array().unwrap().len(), 2);
    assert_eq!(routed["consumers"][0]["consumer_id"], reviewer_id);
    assert_eq!(routed["consumers"][1]["consumer_id"], nyx_id);
    let mentioned = checked(
        app.ingress.native(&native("@reviewer review this")).await,
        202,
    )
    .await;
    assert_eq!(mentioned["consumers"][0]["consumer_id"], reviewer_id);
    app.ingress.idle().await;
    assert_eq!(app.ingress.recorder.requests.lock().unwrap().len(), 4);
    let rotated = app.call(Method::PUT,&format!("/consumers/{reviewer_id}"),Some(&json!({
        "expected_version":version,"name":"reviewer","credentials":{"agent_key":"fixture-rotated-reviewer-key"}
    })),200).await;
    assert_eq!(rotated["consumer"]["id"], reviewer["consumer"]["id"]);
    assert_eq!(
        rotated["consumer"]["binding_id"],
        reviewer["consumer"]["binding_id"]
    );
    assert_ne!(
        rotated["consumer"]["agent_key_hash"],
        reviewer["consumer"]["agent_key_hash"]
    );
    let after = checked(
        app.ingress
            .native(&native("@reviewer after rotation"))
            .await,
        202,
    )
    .await;
    assert_eq!(mentioned["conversation_id"], after["conversation_id"]);
    let version = &rotated["channel"]["version"];
    let refused = app
        .call(
            Method::DELETE,
            &format!("/consumers/{reviewer_id}?expected_version={version}"),
            None,
            409,
        )
        .await;
    assert_eq!(
        refused["detail"],
        "/routing/rules/0/targets/0: consumer_in_use"
    );
    let exited = app
        .call(
            Method::DELETE,
            &format!("/routing?expected_version={version}&cascade=true"),
            None,
            200,
        )
        .await;
    assert!(exited["definition"].get("routing").is_none());
    assert!(exited.get("warnings").is_none());
    let legacy = checked(app.ingress.native(&native("plain after exit")).await, 202).await;
    assert!(legacy.get("consumers").is_none());
    println!("owner walkthrough flag-on: create, routing, two providers, Lark rule, default reply, default/rule/mention ingress, rotation, referenced removal refusal, cascade exit passed");
    app.close().await;
}

#[tokio::test]
async fn owner_walkthrough_refuses_gated_writes_and_keeps_exits_with_the_flag_off() {
    let app = OwnerApp::new(false).await;
    let current = app.call(Method::GET, "", None, 200).await;
    let version = &current["version"];
    for (method, suffix, body) in [
        (Method::PUT, "/routing".to_owned(), Some(policy(version))),
        (
            Method::POST,
            "/consumers".to_owned(),
            Some(consumer(version, "other", "cma_codex")),
        ),
        (
            Method::PUT,
            format!("/consumers/{}", fixture::REVIEWER),
            Some(json!({"expected_version":version,"name":"reviewer"})),
        ),
        (
            Method::POST,
            format!("/test?consumer={}", fixture::REVIEWER),
            None,
        ),
    ] {
        assert_eq!(
            app.call(method, &suffix, body.as_ref(), 400).await["detail"],
            "multi_consumer_disabled"
        );
    }
    let plain = checked(app.ingress.native(&native("hello")).await, 202).await;
    assert!(plain.get("consumers").is_none());
    let refused = checked(app.ingress.native(&native("@reviewer x")).await, 202).await;
    assert_eq!(refused["reason"], "consumer_unavailable");
    let removed = app
        .call(
            Method::DELETE,
            &format!(
                "/consumers/{}?expected_version={version}",
                fixture::REVIEWER
            ),
            None,
            200,
        )
        .await;
    let exited = app
        .call(
            Method::DELETE,
            &format!(
                "/routing?expected_version={}&cascade=true",
                removed["version"]
            ),
            None,
            200,
        )
        .await;
    assert!(exited["definition"].get("routing").is_none());
    assert!(exited.get("warnings").is_none());
    assert!(
        checked(app.ingress.native(&native("plain after exit")).await, 202)
            .await
            .get("consumers")
            .is_none()
    );
    println!("owner walkthrough flag-off: routing/add/update/test refused, legacy ingress retained, additional addressing refused, consumer removal and cascade exit passed");
    app.close().await;
}
