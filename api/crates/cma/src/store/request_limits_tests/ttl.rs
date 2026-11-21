use super::super::*;
use super::{
    clock::freeze,
    fixture::{replay, Fixture},
};
use mongodb::bson::{Bson, DateTime};
use std::sync::Arc;

async fn template(f: &Fixture, rate: u32, tokens: u32) -> Document {
    f.take();
    f.left
        .charge(&format!("template-{rate}-{tokens}"), rate, tokens)
        .await
        .unwrap();
    f.take()
        .into_iter()
        .find(|command| command.contains_key("findAndModify"))
        .unwrap()
}

async fn charge_at(f: &Fixture, template: &Document, key: &str, now: i64) -> bool {
    // Execute the captured production command, replacing only its clock input.
    let mut command = Bson::Document(replay(template.clone(), key));
    freeze(&mut command, DateTime::from_millis(now));
    let reply = f
        .database
        .run_command(command.as_document().unwrap().clone())
        .await
        .unwrap();
    match reply.get("value").unwrap() {
        Bson::Null => true,
        Bson::Document(row) => {
            row.get_i64("wait_ms")
                .or_else(|_| row.get_i32("wait_ms").map(i64::from))
                .unwrap()
                == 0
        }
        other => panic!("unexpected preimage: {other:?}"),
    }
}

async fn row(f: &Fixture, key: &str) -> Document {
    f.left
        .limits
        .find_one(doc! {"_id":key})
        .await
        .unwrap()
        .unwrap()
}

async fn expire_at(f: &Fixture, key: &str, now: i64) -> u64 {
    // Exercise the TTL index's deletion predicate without waiting for its monitor.
    f.left
        .limits
        .delete_one(doc! {"_id":key, "expires_at":{"$lte":DateTime::from_millis(now)}})
        .await
        .unwrap()
        .deleted_count
}

#[tokio::test]
#[ignore = "requires disposable MongoDB 8.0.13 replica set"]
async fn expiration_covers_single_and_prepaid_debt_through_maximum_configuration() {
    let f = Fixture::new().await;
    let now = DateTime::now().timestamp_millis() + 86_400_000;
    for rate in [1, 10, 120, 600, 60_001, 120_000, 120_001, 120_002, u32::MAX] {
        let interval = i64::from(60_000_u32.div_ceil(rate));
        for tokens in [1, rate] {
            let command = template(&f, rate, tokens).await;
            let key = format!("boundary-{rate}-{tokens}");
            f.left.limits.delete_one(doc! {"_id":&key}).await.unwrap();
            // This preimage leaves exactly this charge's credits available.
            f.left
                .limits
                .insert_one(doc! {
                    "_id":&key, "document_schema_version":1,
                    "tat":now + interval * i64::from(rate - tokens),
                    "expires_at":DateTime::from_millis(now + 120_000)
                })
                .await
                .unwrap();
            assert!(charge_at(&f, &command, &key, now).await);
            let accepted = row(&f, &key).await;
            let tat = now + interval * i64::from(rate);
            assert_eq!(accepted.get_i64("tat").unwrap(), tat);
            assert_eq!(
                accepted
                    .get_datetime("expires_at")
                    .unwrap()
                    .timestamp_millis(),
                (now + 120_000).max(tat)
            );
            assert!(!charge_at(&f, &command, &key, now).await);
            assert_eq!(
                row(&f, &key).await,
                accepted,
                "refusal must preserve debt and expiration"
            );
        }
    }
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable MongoDB 8.0.13 replica set"]
async fn expiration_matches_retained_admission_and_shortened_ttl_is_a_negative_control() {
    let f = Fixture::new().await;
    let now = DateTime::now().timestamp_millis() + 86_400_000;
    for rate in [600, 120_000, 120_002, u32::MAX] {
        let command = template(&f, rate, rate).await;
        let keys = [
            format!("candidate-{rate}"),
            format!("retained-{rate}"),
            format!("shortened-{rate}"),
        ];
        for key in &keys {
            assert!(charge_at(&f, &command, key, now).await);
        }
        let accepted = row(&f, &keys[0]).await;
        let expiry = accepted
            .get_datetime("expires_at")
            .unwrap()
            .timestamp_millis();
        assert_eq!(expire_at(&f, &keys[0], expiry - 1).await, 0);
        if rate > 120_000 {
            assert_eq!(expire_at(&f, &keys[0], now + 120_000).await, 0);
            f.left
                .limits
                .update_one(
                    doc! {"_id":&keys[2]},
                    doc! {"$set":{
                        "expires_at":DateTime::from_millis(now + 120_000)
                    }},
                )
                .await
                .unwrap();
            assert_eq!(expire_at(&f, &keys[2], now + 120_000).await, 1);
            let partial = template(&f, rate, 120_001).await;
            assert!(!charge_at(&f, &partial, &keys[1], now + 120_000).await);
            assert!(!charge_at(&f, &partial, &keys[0], now + 120_000).await);
            assert!(
                charge_at(&f, &partial, &keys[2], now + 120_000).await,
                "the old fixed TTL grants credits the retained bucket refuses"
            );
        }
        assert_eq!(expire_at(&f, &keys[0], expiry).await, 1);
        assert!(charge_at(&f, &command, &keys[0], expiry).await);
        assert!(charge_at(&f, &command, &keys[1], expiry).await);
        let fresh = row(&f, &keys[0]).await;
        let retained = row(&f, &keys[1]).await;
        assert_eq!(
            fresh.get_i64("tat").unwrap(),
            retained.get_i64("tat").unwrap()
        );
        assert_eq!(
            fresh.get_datetime("expires_at").unwrap(),
            retained.get_datetime("expires_at").unwrap()
        );
    }
    f.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires disposable MongoDB 8.0.13 replica set"]
async fn concurrent_single_and_prepaid_charges_retain_the_final_debt_horizon() {
    let mut f = Fixture::new().await;
    let now = DateTime::now().timestamp_millis() + 86_400_000;
    Arc::get_mut(&mut f.left)
        .unwrap()
        .set_test_clock_millis(now);
    Arc::get_mut(&mut f.right)
        .unwrap()
        .set_test_clock_millis(now);
    for rate in [120_002, u32::MAX] {
        for tokens in [1, 4] {
            let key = format!("concurrent-{rate}-{tokens}");
            assert_eq!(f.left.charge(&key, rate, rate - 16).await.unwrap(), None);
            let barrier = Arc::new(tokio::sync::Barrier::new(33));
            let mut tasks = tokio::task::JoinSet::new();
            for index in 0..32 {
                let store = if index % 2 == 0 {
                    f.left.clone()
                } else {
                    f.right.clone()
                };
                let (key, barrier) = (key.clone(), barrier.clone());
                tasks.spawn(async move {
                    barrier.wait().await;
                    store.charge(&key, rate, tokens).await.unwrap()
                });
            }
            barrier.wait().await;
            let mut admitted = 0;
            while let Some(result) = tasks.join_next().await {
                admitted += u32::from(result.unwrap().is_none());
            }
            assert_eq!(admitted * tokens, 16);
            let accepted = row(&f, &key).await;
            assert_eq!(accepted.get_i64("tat").unwrap(), now + i64::from(rate));
            assert_eq!(
                accepted
                    .get_datetime("expires_at")
                    .unwrap()
                    .timestamp_millis(),
                now + i64::from(rate)
            );
        }
    }
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable MongoDB 8.0.13 replica set with failpoints"]
async fn uncertain_acknowledgement_keeps_charged_debt_and_its_expiration() {
    let mut f = Fixture::new().await;
    let now = DateTime::now().timestamp_millis() + 86_400_000;
    Arc::get_mut(&mut f.left)
        .unwrap()
        .set_test_clock_millis(now);
    Arc::get_mut(&mut f.right)
        .unwrap()
        .set_test_clock_millis(now);
    f.admin
        .run_command(doc! {
            "configureFailPoint":"failCommand", "mode":{"times":1}, "data":{
                "failCommands":["findAndModify"], "appName":&f.app,
                "writeConcernError":{"code":64, "codeName":"WriteConcernFailed",
                    "errmsg":"synthetic acknowledgement uncertainty"}
            }
        })
        .await
        .unwrap();
    let outcome = f.left.charge("uncertain", u32::MAX, u32::MAX).await;
    f.clear_failpoint().await;
    assert!(
        matches!(&outcome, Err(StoreError::Unavailable(message)) if message.contains("synthetic acknowledgement uncertainty")),
        "{outcome:?}"
    );
    // The injected error skips the majority wait. An independent majority write
    // makes the preceding charge observable to the store's majority readers.
    assert_eq!(f.right.check("majority-barrier", 1).await.unwrap(), None);
    let charged = row(&f, "uncertain").await;
    assert_eq!(charged.get_i64("tat").unwrap(), now + i64::from(u32::MAX));
    assert_eq!(
        charged
            .get_datetime("expires_at")
            .unwrap()
            .timestamp_millis(),
        now + i64::from(u32::MAX)
    );
    assert_eq!(f.right.check("uncertain", u32::MAX).await.unwrap(), Some(1));
    assert_eq!(row(&f, "uncertain").await, charged);
    f.close().await;
}
