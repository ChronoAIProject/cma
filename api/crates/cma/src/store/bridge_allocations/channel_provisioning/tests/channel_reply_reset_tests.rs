//! Both sides of reset keep the original explicit reply source and authority.
use super::*;
use layer2_core::bridge::outbox::*;
use layer2_core::sandbox::store::AgentStore;
use std::sync::Mutex;

#[path = "channel_reply_reset_fixture.rs"]
mod support;
use support::{reset, Relay};
#[path = "channel_reply_stopped_test.rs"]
mod stopped;

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn queued_and_new_explicit_replies_survive_reset_with_original_source_after_turn_completion()
{
    let f = ReplyFixture::new().await;
    let context = f.context().await;
    let first = f
        .reply(&context, "before-reset-reply", "Already queued.")
        .await;
    assert_eq!(first["isError"], false, "{first}");
    let before = f.items().await;
    let next = reset(&f).await;
    assert_eq!(f.context().await, context);
    let second = f
        .reply(&context, "after-reset-reply", "Still the original turn.")
        .await;
    assert_eq!(second["isError"], false, "{second}");
    let retained = f.items().await;
    assert_eq!(retained[0], before[0]);
    assert_eq!(retained.len(), 2);
    for row in &retained {
        let item: OutboxItem =
            mongodb::bson::deserialize_from_document(row.get_document("record").unwrap().clone())
                .unwrap();
        assert_eq!(item.source.allocation_id(), f.allocation.id);
        assert_eq!(item.source.target(), f.allocation.placement.target());
        assert_eq!(item.source.message_id(), f.inbox.message_id);
        assert_eq!(
            item.source.command_id(),
            &f.command.receipt.envelope.command_id
        );
    }
    f.finish().await;
    let relay = Relay {
        original: f.allocation.clone(),
        command: f.command.clone(),
        message: f.inbox.message_id.clone(),
        calls: Mutex::new(Vec::new()),
    };
    for expected in 1..=2 {
        tokio::time::timeout(Duration::from_secs(12), async {
            while relay.calls.lock().unwrap().len() < expected {
                f.replies
                    .deliver_one("reset-reply-delivery", &relay)
                    .await
                    .unwrap();
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
    }
    assert_eq!(relay.calls.lock().unwrap().len(), 2);
    let recovered = f
        .reply(&context, "before-reset-reply", "Already queued.")
        .await;
    assert_eq!(recovered["isError"], false, "{recovered}");
    assert_eq!(
        recovered["structuredContent"]["receipt_id"],
        first["structuredContent"]["receipt_id"]
    );
    assert_eq!(recovered["structuredContent"]["status"], "sent");
    assert!(f
        .resources
        .get_agent(&next.placement.target().agent_id)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        f.resources
            .get_command(&f.command.receipt.envelope.command_id)
            .await
            .unwrap()
            .unwrap()
            .receipt,
        f.command.receipt
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn retained_replies_after_reset_refuse_revoked_or_suspended_original_authority() {
    for revoke in [false, true] {
        let f = ReplyFixture::new().await;
        let context = f.context().await;
        let reply = f
            .reply(
                &context,
                "queued-before-reset-denial",
                "Do not send after authority loss.",
            )
            .await;
        assert_eq!(reply["isError"], false, "{reply}");
        reset(&f).await;
        let bridge = f
            .bridges
            .bridge(&f.allocation.pin.bridge_id)
            .await
            .unwrap()
            .unwrap();
        if revoke {
            f.invokes
                .revoke(
                    &caller(),
                    None,
                    RevokeBridgeInvokeGrant {
                        grant_id: f.grant.id.clone(),
                        expected_revision: f.grant.revision,
                        expected_bridge_revision: bridge.revision,
                        idempotency_key: "reset-grant-revoke".into(),
                    },
                )
                .await
                .unwrap();
        } else {
            f.svc
                .mutate(
                    &caller(),
                    None,
                    change(
                        &bridge.id,
                        bridge.revision,
                        "reset-work-suspend",
                        BridgeChange::Disable {
                            accepted_work: AcceptedWorkPolicy::SuspendOriginal,
                        },
                    ),
                )
                .await
                .unwrap();
        }
        let refusal = f
            .reply(&context, "new-after-reset-denial", "Must remain held.")
            .await;
        assert_eq!(refusal["isError"], true, "{refusal}");
        let relay = Relay {
            original: f.allocation.clone(),
            command: f.command.clone(),
            message: f.inbox.message_id.clone(),
            calls: Mutex::new(Vec::new()),
        };
        let _ = f.replies.deliver_one("reset-reply-denied", &relay).await;
        assert!(relay.calls.lock().unwrap().is_empty());
        let items = f.items().await;
        assert_eq!(items.len(), 1);
        let item: OutboxItem = mongodb::bson::deserialize_from_document(
            items[0].get_document("record").unwrap().clone(),
        )
        .unwrap();
        assert!(!matches!(
            item.state,
            DeliveryState::Claimed { .. } | DeliveryState::Sending { .. }
        ));
        if revoke {
            assert!(matches!(
                item.state,
                DeliveryState::RetryWait {
                    reason: DeliveryFailure::RouteUnavailable,
                    ..
                }
            ));
        }
        assert_eq!(
            f.client
                .database(&f.database)
                .collection::<mongodb::bson::Document>("bridge_outbox_send_rates")
                .count_documents(doc! {"record.lease":{"$ne":mongodb::bson::Bson::Null}})
                .await
                .unwrap(),
            0
        );
        f.close().await;
    }
}
