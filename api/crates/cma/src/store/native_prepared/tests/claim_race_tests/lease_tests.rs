use super::*;
use std::time::SystemTime;

fn after(seconds: u64) -> String {
    layer2_core::rfc3339_at(SystemTime::now() + Duration::from_secs(seconds))
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn native_prepared_two_clients_share_target_and_fence_expired_renewal() {
    let (client, name, first) = database().await;
    let other = mongodb::Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let second = MongoSandboxStore::open(&other, &name).await.unwrap();
    let plan = plan();
    let at = layer2_core::now_rfc3339();
    let (a, b) = tokio::join!(
        first.reconcile_native_prepared_target(Some(&plan), 1, &at),
        second.reconcile_native_prepared_target(Some(&plan), 1, &at),
    );
    a.unwrap();
    b.unwrap();
    assert_eq!(records(&first).await.len(), 1, "target is deployment-wide");
    let until = after(60);
    let (a, b) = tokio::join!(
        first.claim_native_prepared_work(&at, &until),
        second.claim_native_prepared_work(&at, &until),
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_ne!(a.is_some(), b.is_some());
    let original = a.or(b).unwrap();
    first
        .renew_native_prepared_work(&original, &after(60))
        .await
        .unwrap();
    let row = first
        .native_prepared
        .find_one(doc! {"_id":original.record.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    let extended = row.get_str("lease_until").unwrap().to_owned();
    first
        .renew_native_prepared_work(&original, &after(30))
        .await
        .unwrap();
    let row = first
        .native_prepared
        .find_one(doc! {"_id":original.record.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row.get_str("lease_until").unwrap(),
        extended,
        "late renewal never shortens authority"
    );
    for invalid in ["bad timestamp", NOW] {
        assert!(first
            .renew_native_prepared_work(&original, invalid)
            .await
            .is_err());
    }
    first
        .native_prepared
        .update_one(
            doc! {"_id":original.record.id.as_str()},
            doc! {"$set":{"lease_until":NOW}},
        )
        .await
        .unwrap();
    assert!(
        first
            .renew_native_prepared_work(&original, &after(60))
            .await
            .is_err(),
        "expiry alone fences renewal even without a replacement owner"
    );
    assert!(first
        .finish_native_prepared_work(
            &original,
            original.record.clone(),
            &layer2_core::now_rfc3339(),
            &after(30)
        )
        .await
        .is_err());
    let successor = second
        .claim_native_prepared_work(&layer2_core::now_rfc3339(), &after(60))
        .await
        .unwrap()
        .unwrap();
    assert!(first
        .renew_native_prepared_work(&original, &after(60))
        .await
        .is_err());
    assert!(first
        .finish_native_prepared_work(
            &original,
            original.record.clone(),
            &layer2_core::now_rfc3339(),
            &after(30)
        )
        .await
        .is_err());
    second
        .renew_native_prepared_work(&successor, &after(60))
        .await
        .unwrap();
    second
        .finish_native_prepared_work(
            &successor,
            successor.record.clone(),
            &layer2_core::now_rfc3339(),
            &after(30),
        )
        .await
        .unwrap();
    assert!(first
        .renew_native_prepared_work(&successor, &after(60))
        .await
        .is_err());
    assert_eq!(records(&first).await.len(), 1);
    client.database(&name).drop().await.unwrap();
}
