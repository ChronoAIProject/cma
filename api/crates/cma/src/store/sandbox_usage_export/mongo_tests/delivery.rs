use super::*;
use crate::sandbox_usage_export::Receipt;
use serde_json::json;

pub(super) async fn stream(f: &Fixture, owner: &str) -> String {
    let parent = f.parent(owner).await;
    f.capture()
        .project_one(&source_id(&parent, 1), now())
        .await
        .unwrap();
    f.db.collection::<Document>(STREAMS)
        .find_one(doc! {"owner":owner})
        .await
        .unwrap()
        .unwrap()
        .get_str("_id")
        .unwrap()
        .into()
}

pub(super) fn receipt(attempt: &DeliveryAttempt) -> Receipt {
    serde_json::from_value(json!({"receipt_id":uuid::Uuid::new_v4().to_string(),
        "event_id":attempt.event.event_id,"stream_id":attempt.event.stream_id,
        "sequence":attempt.event.sequence.to_string(),"payload_digest":attempt.event.digest,
        "accepted_at_ms":attempt.event.produced_at_ms.to_string(),"custody":"persisted"}))
    .unwrap()
}

async fn expire(f: &Fixture, attempt: &DeliveryAttempt) {
    f.db.collection::<Document>(OUTBOX)
        .update_one(
            doc! {"_id":&attempt.event.event_id},
            doc! {"$set":{"delivery.lease_until":mongodb::bson::DateTime::from_millis(0),
            "delivery.due_at":mongodb::bson::DateTime::from_millis(0)}},
        )
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn lease_takeover_rejects_stale_ack_and_keeps_frozen_bytes() {
    let f = Fixture::new(true).await;
    let stream = stream(&f, "owner-without-wallet").await;
    let capture = f.capture();
    let destination = "a".repeat(64);
    let first = capture
        .claim_delivery(&stream, &destination)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.event.sequence, 1);
    assert!(capture
        .claim_delivery(&stream, &destination)
        .await
        .unwrap()
        .is_none());
    expire(&f, &first).await;
    assert!(!capture
        .finish_delivery(&first, DeliveryOutcome::Accepted(receipt(&first)))
        .await
        .unwrap());
    let peer = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let reopened = CaptureStore::open(&peer, f.db.name(), config())
        .await
        .unwrap();
    let next = reopened
        .claim_delivery(&stream, &destination)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.event.payload, next.event.payload);
    assert_eq!(first.event.event_id, next.event.event_id);
    assert!(next.generation > first.generation);
    assert_ne!(first.attempt_id, next.attempt_id);
    assert!(!capture
        .finish_delivery(&first, DeliveryOutcome::Accepted(receipt(&first)))
        .await
        .unwrap());
    assert!(reopened
        .finish_delivery(&next, DeliveryOutcome::Accepted(receipt(&next)))
        .await
        .unwrap());
    assert_eq!(
        reopened
            .claim_delivery(&stream, &destination)
            .await
            .unwrap()
            .unwrap()
            .event
            .sequence,
        2
    );
    assert_eq!(f.count(OUTBOX).await, 2);
    f.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn destination_change_and_invalid_receipt_cannot_reassign_or_ack_retained_work() {
    let f = Fixture::new(true).await;
    let stream = stream(&f, "owner-not-enrolled").await;
    let capture = f.capture();
    let first = capture
        .claim_delivery(&stream, &"a".repeat(64))
        .await
        .unwrap()
        .unwrap();
    let mut wrong = receipt(&first);
    wrong.payload_digest = "b".repeat(64);
    assert!(capture
        .finish_delivery(&first, DeliveryOutcome::Accepted(wrong))
        .await
        .is_err());
    expire(&f, &first).await;
    assert!(capture
        .claim_delivery(&stream, &"b".repeat(64))
        .await
        .is_err());
    let resumed = capture
        .claim_delivery(&stream, &first.destination)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.event.payload, resumed.event.payload);
    assert_eq!(first.event.event_id, resumed.event.event_id);
    f.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn retry_and_quarantine_block_only_their_stream_and_forbidden_holds_producer() {
    let f = Fixture::new(true).await;
    let a = stream(&f, "free-owner").await;
    let b = stream(&f, "zero-balance-owner").await;
    let capture = f.capture();
    let destination = "a".repeat(64);
    let first = capture
        .claim_delivery(&a, &destination)
        .await
        .unwrap()
        .unwrap();
    assert!(capture
        .finish_delivery(
            &first,
            DeliveryOutcome::Retry {
                seconds: 60,
                reason: "rate_limited"
            }
        )
        .await
        .unwrap());
    assert!(capture
        .claim_delivery(&a, &destination)
        .await
        .unwrap()
        .is_none());
    let second = capture
        .claim_delivery(&b, &destination)
        .await
        .unwrap()
        .unwrap();
    assert!(capture
        .finish_delivery(&second, DeliveryOutcome::Hold("identity_conflict"))
        .await
        .unwrap());
    expire(&f, &second).await;
    assert!(capture
        .claim_delivery(&b, &destination)
        .await
        .unwrap()
        .is_none());
    expire(&f, &first).await;
    let first = capture
        .claim_delivery(&a, &destination)
        .await
        .unwrap()
        .unwrap();
    assert!(capture
        .finish_delivery(&first, DeliveryOutcome::ProducerForbidden)
        .await
        .unwrap());
    let reopened = CaptureStore::open(&f.client, f.db.name(), config())
        .await
        .unwrap();
    assert!(reopened.producer_blocked(&destination).await.unwrap());
    assert!(reopened.producer_blocked(&"b".repeat(64)).await.is_err());
    assert_eq!(f.count(OUTBOX).await, 4);
    f.db.drop().await.unwrap();
}
