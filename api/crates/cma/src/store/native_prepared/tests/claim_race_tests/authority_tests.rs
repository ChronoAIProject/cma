use super::*;
use mongodb::bson::Bson;
use std::time::SystemTime;

fn at(offset: i64) -> String {
    let delta = Duration::from_secs(offset.unsigned_abs());
    layer2_core::rfc3339_at(if offset < 0 {
        SystemTime::now() - delta
    } else {
        SystemTime::now() + delta
    })
}

async fn peer(name: &str) -> MongoSandboxStore {
    let client = mongodb::Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    MongoSandboxStore::open(&client, name).await.unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn prepared_caller_clock_cannot_steal_or_finish_expired_authority() {
    let (client, name, first) = database().await;
    let second = peer(&name).await;
    let captured_at = at(-60);
    first
        .reconcile_native_prepared_target(Some(&plan()), 1, &captured_at)
        .await
        .unwrap();
    assert!(
        first
            .claim_native_prepared_work(&captured_at, &at(-30))
            .await
            .unwrap()
            .is_none(),
        "a claim cannot arrive with an already-expired requested deadline"
    );
    let original = first
        .claim_native_prepared_work(&at(0), &at(60))
        .await
        .unwrap()
        .unwrap();
    assert!(
        second
            .claim_native_prepared_work(&at(120), &at(180))
            .await
            .unwrap()
            .is_none(),
        "an ahead caller cannot steal server-live authority"
    );
    let id = original.record.id.as_str();
    // The stored deadline is later than the captured task timestamp but is
    // already past at Mongo. The token/revision remain unchanged until takeover.
    second
        .native_prepared
        .update_one(doc! {"_id":id}, doc! {"$set":{"lease_until":at(-1)}})
        .await
        .unwrap();
    let before = first
        .native_prepared
        .find_one(doc! {"_id":id})
        .await
        .unwrap()
        .unwrap();
    assert!(first
        .finish_native_prepared_work(&original, original.record.clone(), &captured_at, &at(30))
        .await
        .is_err());
    assert_eq!(
        second
            .native_prepared
            .find_one(doc! {"_id":id})
            .await
            .unwrap()
            .unwrap(),
        before
    );
    let next = second
        .claim_native_prepared_work(&at(0), &at(60))
        .await
        .unwrap()
        .unwrap();
    assert!(first
        .renew_native_prepared_work(&original, &at(60))
        .await
        .is_err());
    assert!(first
        .finish_native_prepared_work(&original, original.record.clone(), &captured_at, &at(30))
        .await
        .is_err());
    second
        .finish_native_prepared_work(&next, next.record.clone(), &at(0), &at(30))
        .await
        .unwrap();
    assert!(
        first
            .claim_native_prepared_work(&at(120), &at(180))
            .await
            .unwrap()
            .is_none(),
        "an ahead caller cannot run a future scheduled probe"
    );
    assert_eq!(records(&first).await.len(), 1);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn malformed_prepared_deadlines_never_grant_or_extend_authority() {
    let (client, name, first) = database().await;
    let second = peer(&name).await;
    let captured_at = at(-60);
    first
        .reconcile_native_prepared_target(Some(&plan()), 1, &captured_at)
        .await
        .unwrap();
    let original = first
        .claim_native_prepared_work(&at(0), &at(60))
        .await
        .unwrap()
        .unwrap();
    let id = original.record.id.as_str();
    for bad in [
        Bson::String(String::new()),
        Bson::String("2000-99-01T00:00:00Z".into()),
        Bson::String("9999-99-01T00:00:00Z".into()),
        Bson::Int64(0),
        Bson::DateTime(mongodb::bson::DateTime::from_millis(0)),
        Bson::Array(vec![Bson::String(at(-1))]),
        Bson::Array(vec![Bson::String(at(60))]),
    ] {
        first
            .native_prepared
            .update_one(
                doc! {"_id":id},
                doc! {"$set":{"lease_until":bad.clone(),"check_after":&captured_at}},
            )
            .await
            .unwrap();
        let before = second
            .native_prepared
            .find_one(doc! {"_id":id})
            .await
            .unwrap()
            .unwrap();
        assert!(first
            .renew_native_prepared_work(&original, &at(60))
            .await
            .is_err());
        assert!(first
            .finish_native_prepared_work(&original, original.record.clone(), &captured_at, &at(30))
            .await
            .is_err());
        assert!(second
            .claim_native_prepared_work(&at(120), &at(180))
            .await
            .unwrap()
            .is_none());
        assert_eq!(
            second
                .native_prepared
                .find_one(doc! {"_id":id})
                .await
                .unwrap()
                .unwrap(),
            before
        );
        first
            .native_prepared
            .update_one(
                doc! {"_id":id},
                doc! {"$set":{"lease_until":at(-1),"check_after":bad}},
            )
            .await
            .unwrap();
        assert!(second
            .claim_native_prepared_work(&at(120), &at(180))
            .await
            .unwrap()
            .is_none());
    }
    first
        .native_prepared
        .update_one(
            doc! {"_id":id},
            doc! {"$set":{"lease_until":at(-1),"check_after":&captured_at}},
        )
        .await
        .unwrap();
    let next = second
        .claim_native_prepared_work(&at(0), &at(60))
        .await
        .unwrap()
        .unwrap();
    second
        .finish_native_prepared_work(&next, next.record.clone(), &at(0), &at(30))
        .await
        .unwrap();
    client.database(&name).drop().await.unwrap();
}
