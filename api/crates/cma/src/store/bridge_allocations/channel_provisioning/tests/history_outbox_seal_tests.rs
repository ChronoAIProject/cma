use super::*;
use layer2_core::bridge::outbox::{management::*, *};

struct Relay(DeliveryOutcome);
#[async_trait::async_trait]
impl RelayDeliveryPort for Relay {
    async fn deliver(&self, request: RelayDeliveryRequest) -> DeliveryOutcome {
        request.require_current().unwrap();
        self.0.clone()
    }
}
async fn inventory(f: &ReplyFixture) -> Result<usize, HistoryHold> {
    let mut session = f.client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    let result = channel_history::assess_outbox(
        &f.client.database(&f.database),
        &f.allocation,
        &mut session,
    )
    .await
    .unwrap()
    .map(|rows| rows.len());
    session.abort_transaction().await.unwrap();
    result
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_finalization_outbox_keeps_audited_terminal_states_and_holds_unknown() {
    for outcome in [
        DeliveryOutcome::Confirmed(DeliveryReceipt {
            nyxid_message_id: "retained-sent".into(),
            platform_message_id: None,
        }),
        DeliveryOutcome::DefinitiveRejection(DeliveryFailure::RequestRejected),
        DeliveryOutcome::Unknown(DeliveryFailure::ResponseLost),
    ] {
        let f = ReplyFixture::new().await;
        let context = f.context().await;
        assert_eq!(
            f.reply(&context, "sealed-outbox", "Keep the actual outcome.")
                .await["isError"],
            false
        );
        assert_eq!(inventory(&f).await, Err(HistoryHold::OutboxUnresolved));
        assert!(f
            .replies
            .deliver_one("history-finalization", &Relay(outcome.clone()))
            .await
            .unwrap());
        let db = f.client.database(&f.database);
        let service = BridgeOutboxService::new(f.svc.clone(), f.replies.clone());
        let bridge = f
            .bridges
            .bridge(&f.allocation.pin.bridge_id)
            .await
            .unwrap()
            .unwrap();
        let item =
            crate::store::bridge_invokes::outbox::read_cleanup_item(&f.items().await[0]).unwrap();
        if matches!(outcome, DeliveryOutcome::Unknown(_)) {
            assert_eq!(inventory(&f).await, Err(HistoryHold::OutboxUnresolved));
            let receipt = service
                .command(
                    &caller(),
                    None,
                    &bridge.id,
                    "abandon-before-seal",
                    OutboxRecoveryRequest {
                        item_id: item.id.clone(),
                        expected_revision: item.revision,
                        expected_bridge_revision: bridge.revision,
                        action: OutboxRecoveryAction::Abandon {},
                    },
                )
                .await
                .unwrap();
            assert_eq!(receipt.result.status, OutboxStatus::Abandoned);
        }
        assert_eq!(inventory(&f).await, Ok(1));
        let row = f.items().await.remove(0);
        let item = crate::store::bridge_invokes::outbox::read_cleanup_item(&row).unwrap();
        db.collection::<Document>("bridge_outbox_items")
            .update_one(
                doc! {"_id": &item.id},
                doc! {"$set": {"channel_history_seal": "f".repeat(64)}},
            )
            .await
            .unwrap();
        assert!(service
            .command(
                &caller(),
                None,
                &bridge.id,
                "retry-after-seal",
                OutboxRecoveryRequest {
                    item_id: item.id.clone(),
                    expected_revision: item.revision,
                    expected_bridge_revision: bridge.revision,
                    action: OutboxRecoveryAction::Retry {
                        accept_duplicate_risk: true
                    }
                }
            )
            .await
            .is_err());
        assert_eq!(f.items().await[0].get("record"), row.get("record"));
        db.collection::<Document>("bridge_outbox_audits")
            .delete_one(doc! {"_id": format!("{}:{}", item.id, item.revision)})
            .await
            .unwrap();
        assert_eq!(
            inventory(&f).await,
            Err(HistoryHold::ReferencedRecordMissing)
        );
        f.close().await;
    }
}
