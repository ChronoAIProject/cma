use super::*;
#[path = "channel_reply_fixture.rs"]
mod fixture;
use fixture::ReplyFixture;
use layer2_core::bridge::invoke::RevokeBridgeInvokeGrant;
use layer2_core::sandbox::agent_io::*;
use serde_json::json;

#[path = "channel_reply_delivery_tests.rs"]
mod delivery;
#[path = "channel_reply_generation_test.rs"]
mod generation;
#[path = "channel_reply_reset_tests.rs"]
mod reset;

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn real_channel_reply_tool_queues_multiple_messages_before_turn_end_and_recovers_after_completion(
) {
    let f = ReplyFixture::new().await;
    let context = f.context().await;
    assert!(f.items().await.is_empty());
    let ack = f.reply(&context, "ack-1", "I have started.").await;
    let progress = f
        .reply(&context, "progress-1", "The first step is done.")
        .await;
    for result in [&ack, &progress] {
        assert_eq!(result["isError"], false, "{result}");
        assert_eq!(result["structuredContent"]["status"], "queued");
    }
    assert_ne!(
        ack["structuredContent"]["receipt_id"],
        progress["structuredContent"]["receipt_id"]
    );
    let command = f
        .resources
        .get_command(&f.command.receipt.envelope.command_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(command.status, AgentResponseStatus::InProgress);
    let items = f.items().await;
    assert_eq!(items.len(), 2);
    let first = items[0].get_document("record").unwrap();
    let second = items[1].get_document("record").unwrap();
    assert_eq!(
        second.get_str("predecessor").unwrap(),
        first.get_str("id").unwrap()
    );
    for row in [first, second] {
        let source = row.get_document("source").unwrap();
        assert_eq!(source.get_str("message_id").unwrap(), f.inbox.message_id);
        assert_eq!(
            source.get_str("response_id").unwrap(),
            f.command.receipt.response_id.as_ref().unwrap().as_str()
        );
        assert_eq!(source.get_str("chat_id").unwrap(), "-100123");
        assert!(!format!("{row:?}").contains(HUMAN_SECRET));
        assert!(!format!("{row:?}").contains(RUNTIME_SECRET));
    }
    assert_eq!(f.reply(&context, "ack-1", "I have started.").await, ack);
    let conflict = f
        .reply(&context, "ack-1", "Different intentional message.")
        .await;
    assert_eq!(conflict["isError"], true);
    assert_eq!(
        conflict["structuredContent"]["code"],
        "request_key_conflict"
    );
    f.finish().await;
    let empty = f.call("read_channel_context", json!({})).await;
    assert!(empty["structuredContent"]["reply_context"].is_null());
    // A completed turn creates no automatic final-answer outbox item.
    assert_eq!(f.items().await.len(), 2);
    assert_eq!(f.reply(&context, "ack-1", "I have started.").await, ack);
    assert_eq!(
        f.reply(&context, "late-new-key", "Too late.").await["isError"],
        true
    );
    assert_eq!(f.items().await.len(), 2);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn real_channel_reply_context_survives_inbox_ttl_but_revocation_blocks_new_tool_messages() {
    let f = ReplyFixture::new().await;
    let context = f.context().await;
    // Simulate actual TTL deletion of the temporary callback row. The command
    // transaction retained the exact accepted public source independently.
    f.client
        .database(&f.database)
        .collection::<mongodb::bson::Document>("bridge_callback_inbox")
        .delete_one(doc! {"_id":&f.inbox.id})
        .await
        .unwrap();
    assert_eq!(f.context().await, context);
    let progress = f.reply(&context, "after-inbox-ttl", "Still working.").await;
    assert_eq!(progress["isError"], false, "{progress}");
    let bridge_revision = f
        .invokes
        .authorize(layer2_core::bridge::invoke::BridgeInvokeRequest {
            grant_id: f.grant.id.clone(),
            pin: f.grant.pin.clone(),
            target: f.grant.target.ids.clone(),
            effect: layer2_core::bridge::invoke::BridgeInvokeEffect::ExecuteCommand {
                command_id: f.command.receipt.envelope.command_id.clone(),
            },
        })
        .await
        .unwrap()
        .bridge()
        .revision;
    f.invokes
        .revoke(
            &caller(),
            None,
            RevokeBridgeInvokeGrant {
                grant_id: f.grant.id.clone(),
                expected_bridge_revision: bridge_revision,
                expected_revision: f.grant.revision,
                idempotency_key: "reply-grant-revoke".into(),
            },
        )
        .await
        .unwrap();
    let refused = f
        .reply(&context, "after-revoke", "This must not send.")
        .await;
    assert_eq!(refused["isError"], true, "{refused}");
    assert_eq!(f.items().await.len(), 1);
    // Read-only recovery keeps the exact original receipt after revocation.
    assert_eq!(
        f.reply(&context, "after-inbox-ttl", "Still working.").await,
        progress
    );
    f.close().await;
}
