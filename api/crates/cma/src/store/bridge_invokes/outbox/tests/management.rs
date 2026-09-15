use super::*;
use layer2_core::bridge::{
    outbox::management::*, AcceptedWorkPolicy, BridgeChange, BridgeError, BridgeMutation,
};

fn request(f: &Fixture, item: &OutboxItem, action: OutboxRecoveryAction) -> OutboxRecoveryRequest {
    OutboxRecoveryRequest {
        item_id: item.id.clone(),
        expected_revision: item.revision,
        expected_bridge_revision: f.bridge.revision,
        action,
    }
}
fn make_unknown(item: &mut OutboxItem) {
    let now = item.updated_at_ms;
    let attempt = item
        .claim("earlier-worker".into(), now, now + 100, None)
        .unwrap();
    item.begin_send("earlier-worker", &attempt, now).unwrap();
    item.recover_expired(now + 100).unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn manager_unknown_retry_requires_ack_and_preserves_receipt_attempts_and_permanent_command() {
    let f = enabled_fixture().await;
    let store = Arc::new(store(&f).await);
    let service = BridgeOutboxService::new(f.definitions.clone(), store.clone());
    let mut item = retained_item(&f, "manager-retry");
    make_unknown(&mut item);
    seed(&store, &item).await;
    let missing = request(
        &f,
        &item,
        OutboxRecoveryAction::Retry {
            accept_duplicate_risk: false,
        },
    );
    assert!(matches!(
        service
            .command(&f.context, None, &f.bridge.id, "recovery", missing)
            .await,
        Err(OutboxManagementError::DuplicateRiskRequired)
    ));
    assert_eq!(store.recoveries.count_documents(doc! {}).await.unwrap(), 0);
    let retry = request(
        &f,
        &item,
        OutboxRecoveryAction::Retry {
            accept_duplicate_risk: true,
        },
    );
    let receipt = service
        .command(&f.context, None, &f.bridge.id, "recovery", retry.clone())
        .await
        .unwrap();
    assert_eq!(receipt.result.status, OutboxStatus::Pending);
    assert_eq!(receipt.result.receipt_id, item.tool_invocation_id);
    assert!(receipt.result.duplicate_risk);
    assert_eq!(receipt.result.attempts[0].status, OutboxStatus::Unknown);
    let current = store
        .claim_item(&item.id, "next-worker")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.attempts.len(), 2);
    let reopened = Arc::new(super::store(&f).await);
    let restarted = BridgeOutboxService::new(f.definitions.clone(), reopened.clone());
    assert_eq!(
        restarted
            .command(&f.context, None, &f.bridge.id, "recovery", retry.clone())
            .await
            .unwrap(),
        receipt
    );
    let mut changed = retry;
    changed.action = OutboxRecoveryAction::Abandon {};
    assert!(matches!(
        restarted
            .command(&f.context, None, &f.bridge.id, "recovery", changed)
            .await,
        Err(OutboxManagementError::Bridge(
            BridgeError::IdempotencyConflict
        ))
    ));
    assert_eq!(
        reopened.recoveries.count_documents(doc! {}).await.unwrap(),
        1
    );
    assert_eq!(retained(&reopened, &item.id).await.attempts.len(), 2);
    let wire = serde_json::to_string(&receipt).unwrap();
    for private in [
        "original-chat",
        "original-inbound",
        "Progress update",
        "brsec_",
        "fixture-invoke-secret",
    ] {
        assert!(!wire.contains(private));
    }
    f.client.database(&f.database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn manager_concurrent_commands_have_one_cas_winner_and_history_stays_scoped() {
    let f = enabled_fixture().await;
    let store = Arc::new(store(&f).await);
    let service = BridgeOutboxService::new(f.definitions.clone(), store.clone());
    let item = retained_item(&f, "manager-cas");
    seed(&store, &item).await;
    let action = request(&f, &item, OutboxRecoveryAction::Abandon {});
    f.client.database(&f.database).run_command(doc!{"collMod":"bridge_outbox_recoveries","validator":{"forced_failure":true},"validationLevel":"strict","validationAction":"error"}).await.unwrap();
    assert!(service
        .command(&f.context, None, &f.bridge.id, "rollback", action.clone())
        .await
        .is_err());
    assert_eq!(retained(&store, &item.id).await.revision, item.revision);
    assert_eq!(store.recoveries.count_documents(doc! {}).await.unwrap(), 0);
    assert_eq!(store.audits.count_documents(doc! {}).await.unwrap(), 0);
    f.client
        .database(&f.database)
        .run_command(doc! {"collMod":"bridge_outbox_recoveries","validator":{}})
        .await
        .unwrap();
    let (a, b) = tokio::join!(
        service.command(&f.context, None, &f.bridge.id, "first", action.clone()),
        service.command(&f.context, None, &f.bridge.id, "second", action)
    );
    assert_eq!(
        [a.is_ok(), b.is_ok()].into_iter().filter(|ok| *ok).count(),
        1
    );
    assert_eq!(store.recoveries.count_documents(doc! {}).await.unwrap(), 1);
    assert_eq!(retained(&store, &item.id).await.revision, item.revision + 1);
    let mut outsider = f.context.clone();
    outsider.caller.subject = layer2_core::SubjectId::new("outsider");
    outsider.credential.subject = outsider.caller.subject.clone();
    outsider.actor = layer2_core::resource_access::ResourceActor::Human {
        subject: outsider.caller.subject.clone(),
    };
    assert!(service
        .list(&outsider, None, &f.bridge.id, None, 50)
        .await
        .is_err());
    assert!(service
        .detail(&outsider, None, &f.bridge.id, &item.id)
        .await
        .is_err());
    let page = service
        .list(&f.context, None, &f.bridge.id, None, 1)
        .await
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].id, item.id);
    assert!(page.next_cursor.is_none());
    f.client.database(&f.database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn manager_reconciliation_never_invents_sent_and_suspend_allows_history_and_abandon() {
    let f = enabled_fixture().await;
    let store = Arc::new(store(&f).await);
    let service = BridgeOutboxService::new(f.definitions.clone(), store.clone());
    let mut item = retained_item(&f, "manager-reconcile");
    make_unknown(&mut item);
    seed(&store, &item).await;
    let reconcile = request(&f, &item, OutboxRecoveryAction::Reconcile {});
    let receipt = service
        .command(&f.context, None, &f.bridge.id, "reconcile", reconcile)
        .await
        .unwrap();
    assert_eq!(receipt.result.status, OutboxStatus::Unknown);
    assert_eq!(receipt.result.revision, item.revision);
    assert!(!receipt.result.upstream_reconciliation_supported);
    f.definitions
        .mutate(
            &f.context,
            None,
            BridgeMutation {
                id: f.bridge.id.clone(),
                expected_revision: f.bridge.revision,
                idempotency_key: "suspend".into(),
                change: BridgeChange::Disable {
                    accepted_work: AcceptedWorkPolicy::SuspendOriginal,
                },
            },
        )
        .await
        .unwrap();
    assert_eq!(
        service
            .detail(&f.context, None, &f.bridge.id, &item.id)
            .await
            .unwrap()
            .status,
        OutboxStatus::Unknown
    );
    let mut retry = request(
        &f,
        &item,
        OutboxRecoveryAction::Retry {
            accept_duplicate_risk: true,
        },
    );
    retry.expected_bridge_revision += 1;
    assert!(matches!(
        service
            .command(&f.context, None, &f.bridge.id, "retry-suspended", retry)
            .await,
        Err(OutboxManagementError::Delivery(OutboxError::Suspended))
    ));
    let mut abandon = request(&f, &item, OutboxRecoveryAction::Abandon {});
    abandon.expected_bridge_revision += 1;
    let receipt = service
        .command(&f.context, None, &f.bridge.id, "abandon", abandon)
        .await
        .unwrap();
    assert_eq!(receipt.result.status, OutboxStatus::Abandoned);
    assert_eq!(receipt.result.attempts[0].status, OutboxStatus::Unknown);
    f.client.database(&f.database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn manager_organization_demotion_refuses_reads_and_receipt_recovery() {
    let mut f = Fixture::organization().await;
    f.service
        .issue(
            &f.context,
            Some("fixture-credential"),
            f.issue("outbox-consent"),
        )
        .await
        .unwrap();
    f.enable("outbox-enable").await;
    let store = Arc::new(store(&f).await);
    let service = BridgeOutboxService::new(f.definitions.clone(), store.clone());
    let item = retained_item(&f, "manager-org");
    seed(&store, &item).await;
    let action = request(&f, &item, OutboxRecoveryAction::Abandon {});
    service
        .command(
            &f.context,
            Some("fixture-credential"),
            &f.bridge.id,
            "abandon",
            action.clone(),
        )
        .await
        .unwrap();
    *f.authority.role.lock().unwrap() =
        Some(layer2_core::resource_access::OrganizationRole::Member);
    assert!(service
        .list(
            &f.context,
            Some("fixture-credential"),
            &f.bridge.id,
            None,
            50
        )
        .await
        .is_err());
    assert!(service
        .command(
            &f.context,
            Some("fixture-credential"),
            &f.bridge.id,
            "abandon",
            action
        )
        .await
        .is_err());
    assert_eq!(store.recoveries.count_documents(doc! {}).await.unwrap(), 1);
    f.client.database(&f.database).drop().await.unwrap();
}
