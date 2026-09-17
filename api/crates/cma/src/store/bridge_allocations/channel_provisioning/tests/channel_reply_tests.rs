use super::*;
#[path = "channel_reply_fixture.rs"]
mod fixture;
use fixture::ReplyFixture;
use layer2_core::bridge::invoke::RevokeBridgeInvokeGrant;
use layer2_core::sandbox::agent_io::*;
use serde_json::json;

#[path = "channel_control_admission_tests.rs"]
mod control_admission;
#[path = "channel_control_execution_tests.rs"]
mod control_execution;
pub(crate) use control_execution::compatibility;
#[path = "channel_control_reference_tests.rs"]
mod control_reference;
#[path = "channel_reply_delivery_tests.rs"]
mod delivery;
#[path = "channel_reply_edit_tests.rs"]
mod edits;
#[path = "channel_reply_generation_test.rs"]
mod generation;
#[path = "channel_history_tests.rs"]
mod history;
#[path = "partition_command_tests.rs"]
mod partition_commands;
#[path = "partition_status_tests.rs"]
mod partition_status;
#[path = "channel_reply_reset_tests.rs"]
mod reset;
#[path = "channel_supersession_tests.rs"]
mod supersession;

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn retained_channel_grant_selection_preserves_fresh_authority_and_never_remints_revocation() {
    use crate::bridge::channel_work::matches_retained_channel_grant;
    use layer2_core::bridge::invoke::*;
    let f = ReplyFixture::new().await;
    let store = crate::store::MongoBridgeInvokeStore::open(
        f.bridges.clone(),
        f.resources.clone(),
        &f.database,
    )
    .await
    .unwrap();
    let original = store.grant(&f.grant.id).await.unwrap().unwrap();
    assert!(matches_retained_channel_grant(
        &original,
        &f.inbox,
        &f.allocation
    ));
    let mut manual = original.clone();
    manual.channel = None;
    assert!(!matches_retained_channel_grant(
        &manual,
        &f.inbox,
        &f.allocation
    ));
    let mut another = f.allocation.clone();
    another.identity.generation += 1;
    assert!(!matches_retained_channel_grant(
        &original, &f.inbox, &another
    ));
    let mut other_pin = f.inbox.clone();
    other_pin.pin.config_revision += 1;
    assert!(!matches_retained_channel_grant(
        &original,
        &other_pin,
        &f.allocation
    ));
    let request = || BridgeInvokeRequest {
        grant_id: original.id.clone(),
        pin: f.inbox.pin.clone(),
        target: f.allocation.placement.target().clone(),
        effect: BridgeInvokeEffect::AdmitMessage {
            inbox_id: f.inbox.id.clone(),
        },
    };
    let bridge_revision = f
        .invokes
        .authorize(request())
        .await
        .unwrap()
        .bridge()
        .revision;
    f.invokes
        .revoke(
            &caller(),
            None,
            RevokeBridgeInvokeGrant {
                grant_id: original.id.clone(),
                expected_bridge_revision: bridge_revision,
                expected_revision: original.revision,
                idempotency_key: "retain-revoked-grant".into(),
            },
        )
        .await
        .unwrap();
    let revoked = store.grant(&original.id).await.unwrap().unwrap();
    assert!(
        matches_retained_channel_grant(&revoked, &f.inbox, &f.allocation),
        "retain denied identity instead of entering grant issuance"
    );
    assert!(matches!(
        f.invokes.authorize(request()).await,
        Err(BridgeInvokeError::Revoked)
    ));
    f.close().await;
}
#[path = "wildcard_chat_tests.rs"]
mod wildcard_chat;

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
