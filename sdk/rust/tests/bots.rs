#[path = "support/wire.rs"]
#[allow(dead_code)]
mod wire;
use cma_sdk::{IdempotencyKey, bots::*};
use serde_json::{Value, json};
use wire::{Expected, WireServer};

fn bot() -> Value {
    json!({"bot_id":"bot_one","owner":"owner","name":"Test bot","nyxid_bot_id":"external-bot",
    "profile":{"id":"profile","revision":1},"provider":"cma_codex","state":"creating","failure":null,
    "operation":{"operation":"create","step":"admitted"},"channel":null,"credentials_expire_at":null,"created_at":"now","updated_at":"now","last_test":null})
}
fn expected(method: &str, path: &str, status: u16, response: Value) -> Expected {
    Expected {
        method: method.into(),
        path: path.into(),
        status,
        headers: wire::headers(true, "application/json"),
        response: serde_json::to_vec(&response).unwrap(),
        ..Default::default()
    }
}
#[tokio::test]
async fn bot_lifecycle_uses_exact_routes_statuses_and_creation_key() {
    let server = WireServer::new().await;
    let body = CreateBot {
        event_worker_profile: None,
        name: "Test bot".into(),
        nyxid_bot_id: "external-bot".into(),
        connection_ref: None,
        image: None,
        repository: None,
        agent: None,
        profile: Some(BotProfileRef {
            id: "profile".into(),
            revision: 1,
        }),
        sandbox_profile: Some(BotProfileRef {
            id: "sbp_research".into(),
            revision: 2,
        }),
        workspace_profile: Some(BotProfileRef {
            id: "wkp_research".into(),
            revision: 3,
        }),
        admission: BotAdmission::Scoped {
            senders: BotIdentityPolicy::Open {},
            chats: BotIdentityPolicy::Open {},
            groups: BotGroupPolicy::Deny,
        },
        reply: BotReply {
            auto_final: true,
            ack_after_ms: 0,
        },
    };
    let mut create = expected("POST", "/api/v1/bots", 201, bot());
    create.body = Some(serde_json::to_value(&body).unwrap());
    create
        .headers
        .insert("idempotency-key".into(), "stable-key".into());
    create
        .headers
        .insert("content-type".into(), "application/json".into());
    server.expect(create);
    assert_eq!(
        server
            .client
            .bots()
            .create(&body, &IdempotencyKey::new("stable-key").unwrap())
            .await
            .unwrap()
            .bot_id,
        "bot_one"
    );
    server.expect(expected("GET", "/api/v1/bots/bot_one", 200, bot()));
    assert!(matches!(
        server.client.bots().get("bot_one").await.unwrap().state,
        BotState::Creating
    ));
    server.expect(expected(
        "GET",
        "/api/v1/bots?after=before&limit=1",
        200,
        json!({"items":[bot()],"next_after":"bot_one"}),
    ));
    assert_eq!(
        server
            .client
            .bots()
            .list(&BotListOptions {
                after: Some("before".into()),
                limit: Some(1)
            })
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    server.expect(expected(
        "GET",
        "/api/v1/bots/nyxid-bots",
        200,
        json!([{ "id":"external-bot","name":"Test bot","platform":"telegram" }]),
    ));
    assert_eq!(server.client.bots().available().await.unwrap().len(), 1);
    server.expect(expected(
        "POST",
        "/api/v1/bots/bot_one/test",
        200,
        json!({"accepted_after_ms":15,"first_text":"Ready","level":"l2","tested_at":"now"}),
    ));
    assert_eq!(
        server
            .client
            .bots()
            .test("bot_one")
            .await
            .unwrap()
            .accepted_after_ms,
        15
    );
    for action in ["disable", "enable", "rotate-keys"] {
        server.expect(expected(
            "POST",
            &format!("/api/v1/bots/bot_one/{action}"),
            202,
            bot(),
        ));
    }
    server.expect(expected("DELETE", "/api/v1/bots/bot_one", 202, bot()));
    server.client.bots().disable("bot_one").await.unwrap();
    server.client.bots().enable("bot_one").await.unwrap();
    server.client.bots().rotate_keys("bot_one").await.unwrap();
    server.client.bots().delete("bot_one").await.unwrap();
    server.assert_drained();
}
