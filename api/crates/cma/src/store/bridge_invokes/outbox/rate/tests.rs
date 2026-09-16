use super::super::tests::{enabled_fixture, retained, retained_item, seed, store};
use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn concurrent_replicas_share_one_bot_slot_without_consuming_blocked_attempts() {
    let fixture = enabled_fixture().await;
    let first_store = store(&fixture).await;
    let second_store = store(&fixture).await;
    let first = retained_item(&fixture, "rate-first");
    let second = retained_item(&fixture, "rate-second");
    seed(&first_store, &first).await;
    seed(&first_store, &second).await;
    let (a, b) = tokio::join!(
        first_store.claim_item(&first.id, "replica-a"),
        second_store.claim_item(&second.id, "replica-b")
    );
    let a = a.unwrap();
    let b = b.unwrap();
    assert_eq!(usize::from(a.is_some()) + usize::from(b.is_some()), 1);
    let (held, waiting) = if let Some(item) = a {
        (item, second)
    } else {
        (b.unwrap(), first)
    };
    let slot = first_store
        .send_rates
        .find_one(doc! {"_id":key(&held.source).unwrap()})
        .await
        .unwrap()
        .unwrap();
    let (_, rate) = read(Some(slot)).unwrap();
    assert_eq!(rate.lease.as_ref().unwrap(), &lease(&held).unwrap());
    assert_eq!(retained(&first_store, &waiting.id).await.attempts.len(), 0);
    for _ in 0..20 {
        assert!(second_store
            .claim_item(&waiting.id, "replica-b")
            .await
            .unwrap()
            .is_none());
    }
    assert_eq!(retained(&first_store, &waiting.id).await.attempts.len(), 0);
}

struct Throttled {
    store: Arc<MongoBridgeReplyToolStore>,
    calls: AtomicUsize,
    retry_at: i64,
}
#[async_trait::async_trait]
impl RelayDeliveryPort for Throttled {
    async fn deliver(&self, request: RelayDeliveryRequest) -> DeliveryOutcome {
        self.store.require_sending(&request).await.unwrap();
        self.calls.fetch_add(1, Ordering::SeqCst);
        DeliveryOutcome::RateLimitedUnknown {
            not_before_ms: self.retry_at,
        }
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn provider_cooldown_survives_restart_and_keeps_original_send_unknown() {
    let fixture = enabled_fixture().await;
    let original_store = Arc::new(store(&fixture).await);
    let first = retained_item(&fixture, "rate-response");
    seed(&original_store, &first).await;
    let port = Throttled {
        store: original_store.clone(),
        calls: AtomicUsize::new(0),
        retry_at: now_ms() + 120_000,
    };
    assert!(original_store
        .deliver_one("original-worker", &port)
        .await
        .unwrap());
    let original = retained(&original_store, &first.id).await;
    assert!(matches!(
        original.state,
        DeliveryState::Unknown {
            reason: DeliveryFailure::RateLimited,
            ..
        }
    ));
    let reopened = store(&fixture).await;
    let second = retained_item(&fixture, "later-message");
    seed(&reopened, &second).await;
    assert!(!reopened
        .deliver_one("restarted-worker", &port)
        .await
        .unwrap());
    assert!(reopened
        .claim_item(&first.id, "cannot-retry-unknown")
        .await
        .unwrap()
        .is_none());
    assert_eq!(retained(&reopened, &second.id).await.attempts.len(), 0);
    let row = reopened
        .send_rates
        .find_one(doc! {"_id":key(&first.source).unwrap()})
        .await
        .unwrap()
        .unwrap();
    let (_, rate) = read(Some(row)).unwrap();
    assert_eq!(rate.cooldown_until_ms, port.retry_at);
    assert!(rate.lease.is_none());
    assert_eq!(port.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn bot_identity_shares_pacing_across_keys_routes_and_configuration_generations() {
    let fixture = enabled_fixture().await;
    let original = retained_item(&fixture, "scope");
    let first = key(&original.source).unwrap();
    let mut value = serde_json::to_value(&original.source).unwrap();
    value["config"]["connector"]["key_id"] = "replacement-key".into();
    value["config"]["connector"]["route_ids"] = serde_json::json!(["another-route"]);
    value["pin"]["config_revision"] = 20.into();
    let rotated: OutboxSource = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(key(&rotated).unwrap(), first);
    value["config"]["connector"]["bot_id"] = "another-bot".into();
    let other: OutboxSource = serde_json::from_value(value).unwrap();
    assert_ne!(key(&other).unwrap(), first);
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn late_rate_response_preserves_recovered_unknown_and_fences_newer_bot_lease() {
    let fixture = enabled_fixture().await;
    let original_store = store(&fixture).await;
    let mut old = retained_item(&fixture, "late-rate-response");
    let start = now_ms() - 60_000;
    old.created_at_ms = start;
    old.updated_at_ms = start;
    let attempt = old
        .claim("old-worker".into(), start, start + 100, None)
        .unwrap();
    old.begin_send("old-worker", &attempt, start + 1).unwrap();
    seed(&original_store, &old).await;
    assert!(original_store
        .claim_item(&old.id, "recovery")
        .await
        .unwrap()
        .is_none());
    let recovered = retained(&original_store, &old.id).await;
    assert!(matches!(
        recovered.state,
        DeliveryState::Unknown {
            reason: DeliveryFailure::WorkerLost,
            ..
        }
    ));

    let newer = retained_item(&fixture, "newer-reservation");
    seed(&original_store, &newer).await;
    let current = original_store
        .claim_item(&newer.id, "new-worker")
        .await
        .unwrap()
        .unwrap();
    original_store.require_rate(&current).await.unwrap();
    let retry_at = now_ms() + 120_000;
    original_store
        .complete_delivery(
            &old,
            DeliveryOutcome::RateLimitedUnknown {
                not_before_ms: retry_at,
            },
        )
        .await
        .unwrap();

    let reopened = store(&fixture).await;
    assert_eq!(
        reopened.require_rate(&current).await,
        Err(OutboxError::RateLimited)
    );
    assert!(retained(&reopened, &old.id).await == recovered);
    assert!(retained(&reopened, &newer.id).await == current);
    let (_, rate) = read(
        reopened
            .send_rates
            .find_one(doc! {"_id":key(&old.source).unwrap()})
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(rate.lease.as_ref(), Some(&lease(&current).unwrap()));
    assert_eq!(rate.cooldown_until_ms, retry_at);
    assert!(reopened
        .claim_item(&old.id, "no-auto-retry")
        .await
        .unwrap()
        .is_none());
}
