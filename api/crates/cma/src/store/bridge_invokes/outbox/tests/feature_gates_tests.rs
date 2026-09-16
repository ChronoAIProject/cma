use super::*;
use layer2_core::bridge::{BridgeConfig, BridgeError, BridgeFeature, BridgePlatform};
use layer2_core::resource_access::ResourceOwner;

struct OutboundDisabled(Arc<dyn BridgeConnectionValidator>);
#[async_trait::async_trait]
impl BridgeConnectionValidator for OutboundDisabled {
    fn check_feature(&self, feature: BridgeFeature, _: &BridgePlatform) -> Result<(), BridgeError> {
        if feature == BridgeFeature::Outbound {
            Err(BridgeError::Unavailable)
        } else {
            Ok(())
        }
    }
    async fn validate_connection(
        &self,
        owner: &ResourceOwner,
        config: &BridgeConfig,
    ) -> Result<BridgeValidation, BridgeError> {
        self.0.validate_connection(owner, config).await
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn outbound_disable_holds_fresh_sends_but_reconciles_crashes_and_expired_authority() {
    let f = enabled_fixture().await;
    let store = MongoBridgeReplyToolStore::new(
        f.store.clone(),
        Arc::new(OutboundDisabled(f.authority.clone())),
    )
    .await
    .unwrap();
    let pending = retained_item(&f, "disabled-pending");
    seed(&store, &pending).await;
    assert!(matches!(
        store.claim_item(&pending.id, "disabled-worker").await,
        Err(OutboxError::Suspended)
    ));
    assert!(retained(&store, &pending.id).await == pending);
    assert_eq!(store.send_rates.count_documents(doc! {}).await.unwrap(), 0);

    let mut expired = retained_item(&f, "disabled-expired");
    let mut source = serde_json::to_value(&expired.source).unwrap();
    source["route_expires_at_ms"] = (now_ms() - 1).into();
    expired.source = serde_json::from_value(source).unwrap();
    expired.id = format!(
        "bro_{}",
        layer2_core::profiles::content_digest(&(
            "cma.bridge.outbox.v1",
            expired.source.fingerprint().unwrap(),
            &expired.tool_invocation_id,
            expired.turn_sequence,
            expired.chunk_index,
            &expired.operation,
        ))
        .unwrap()
    );
    seed(&store, &expired).await;
    assert!(store
        .claim_item(&expired.id, "disabled-worker")
        .await
        .unwrap()
        .is_none());
    let expired = retained(&store, &expired.id).await;
    assert_eq!(
        expired.state,
        DeliveryState::Rejected {
            reason: DeliveryFailure::CredentialExpired
        }
    );
    assert!(expired.attempts.is_empty());

    let mut started = retained_item(&f, "disabled-started");
    let time = now_ms() - 500;
    let attempt = started
        .claim("old-worker".into(), time, time + 100, None)
        .unwrap();
    started
        .begin_send("old-worker", &attempt, time + 1)
        .unwrap();
    seed(&store, &started).await;
    let port = Delivered(std::sync::atomic::AtomicUsize::new(0));
    assert!(!store.deliver_one("disabled-worker", &port).await.unwrap());
    assert!(matches!(
        retained(&store, &started.id).await.state,
        DeliveryState::Unknown {
            reason: DeliveryFailure::WorkerLost,
            ..
        }
    ));
    assert_eq!(port.0.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert!(retained(&store, &pending.id).await == pending);
    f.client.database(&f.database).drop().await.unwrap();
}
