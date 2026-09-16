use super::*;
use layer2_core::bridge::outbox::management::*;

fn completed(item: &mut OutboxItem, outcome: DeliveryOutcome) {
    let now = item.updated_at_ms;
    let attempt = item
        .claim("earlier-worker".into(), now, now + 1000, None)
        .unwrap();
    item.begin_send("earlier-worker", &attempt, now).unwrap();
    item.complete(&attempt, outcome, now + 1).unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn terminal_reactivation_obeys_capacity_and_replays_exact_recovery_after_release() {
    let f = enabled_fixture().await;
    let store = Arc::new(store(&f).await);
    let service = BridgeOutboxService::new(f.definitions.clone(), store.clone());
    let mut rejected = retained_item(&f, "rejected-retry");
    completed(
        &mut rejected,
        DeliveryOutcome::DefinitiveRejection(DeliveryFailure::ProviderFailure),
    );
    seed(&store, &rejected).await;
    let mut sent = retained_item(&f, "sent-history");
    completed(
        &mut sent,
        DeliveryOutcome::Confirmed(DeliveryReceipt {
            nyxid_message_id: "confirmed-original".into(),
            platform_message_id: Some("confirmed-provider".into()),
        }),
    );
    seed(&store, &sent).await;
    // Existing valid retained rows contribute to occupancy across distinct
    // original responses. They do not manufacture new admission authority.
    let pending: Vec<_> = (0..MongoBridgeReplyToolStore::MAX_OUTSTANDING_ITEMS)
        .map(|n| retained_item(&f, &format!("outstanding-{n}")))
        .collect();
    store
        .items
        .insert_many(
            pending
                .iter()
                .map(|item| records::item_document(item).unwrap()),
        )
        .await
        .unwrap();
    let request = OutboxRecoveryRequest {
        item_id: rejected.id.clone(),
        expected_revision: rejected.revision,
        expected_bridge_revision: f.bridge.revision,
        action: OutboxRecoveryAction::Retry {
            accept_duplicate_risk: false,
        },
    };
    assert!(matches!(
        service
            .command(
                &f.context,
                None,
                &f.bridge.id,
                "retry-at-capacity",
                request.clone()
            )
            .await,
        Err(OutboxManagementError::Delivery(OutboxError::Capacity))
    ));
    assert!(retained(&store, &rejected.id).await == rejected);
    assert_eq!(store.recoveries.count_documents(doc! {}).await.unwrap(), 0);
    let release = OutboxRecoveryRequest {
        item_id: pending[0].id.clone(),
        expected_revision: pending[0].revision,
        expected_bridge_revision: f.bridge.revision,
        action: OutboxRecoveryAction::Abandon {},
    };
    let abandoned = service
        .command(&f.context, None, &f.bridge.id, "release-one", release)
        .await
        .unwrap();
    assert_eq!(abandoned.result.status, OutboxStatus::Abandoned);
    let receipt = service
        .command(
            &f.context,
            None,
            &f.bridge.id,
            "retry-at-capacity",
            request.clone(),
        )
        .await
        .unwrap();
    assert_eq!(receipt.result.status, OutboxStatus::Pending);
    assert_eq!(receipt.result.attempts.len(), rejected.attempts.len());
    let reopened = Arc::new(super::store(&f).await);
    let restarted = BridgeOutboxService::new(f.definitions.clone(), reopened.clone());
    assert_eq!(
        restarted
            .command(&f.context, None, &f.bridge.id, "retry-at-capacity", request)
            .await
            .unwrap(),
        receipt
    );
    assert!(retained(&reopened, &sent.id).await == sent);
    assert_eq!(
        store.items.count_documents(doc! {}).await.unwrap(),
        MongoBridgeReplyToolStore::MAX_OUTSTANDING_ITEMS + 2
    );
    f.client.database(&f.database).drop().await.unwrap();
}
