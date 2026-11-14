use super::control::stores;
use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn takeover_after_materialization_snapshot_aborts_original_batch() {
    let (first, second) = stores().await;
    let collection = super::super::super::sandbox_metric_rollups::COLLECTION;
    for key in ["rollups", "replay", "diagnostics"] {
        assert!(first.lease(key, "original", 70).await.unwrap());
        let mut session = first.client.start_session().await.unwrap();
        session.start_transaction().await.unwrap();
        first
            .collection(CONTROL)
            .find_one(doc! {"_id":key})
            .session(&mut session)
            .await
            .unwrap()
            .unwrap();
        let update = doc! {"q":{"_id":key},"u":{"$set":{
            "sandbox_id":key,"day":0_i64,"chunk":0_i64,"value":"old"
        }},"upsert":true};
        first
            .bulk_updates_in_session(collection, vec![update], Some(&mut session))
            .await
            .unwrap();
        // A different pool replaces authority after the first transaction has
        // already staged its read model. The original snapshot is insufficient.
        second
            .collection(CONTROL)
            .update_one(
                doc! {"_id":key},
                doc! {"$set":{"lease_until":now() as i64-1}},
            )
            .await
            .unwrap();
        assert!(second.lease(key, "successor", 70).await.unwrap());
        assert!(first
            .commit_controlled_write(key, "original", &mut session, Ok(()))
            .await
            .is_err());
        assert!(second
            .collection(collection)
            .find_one(doc! {"_id":key})
            .await
            .unwrap()
            .is_none());
        let update = doc! {"q":{"_id":key},"u":{"$set":{
            "sandbox_id":key,"day":0_i64,"chunk":0_i64,"value":"successor"
        }},"upsert":true};
        second
            .controlled_updates(key, "successor", collection, vec![update])
            .await
            .unwrap();
        assert_eq!(
            first
                .collection(collection)
                .find_one(doc! {"_id":key})
                .await
                .unwrap()
                .unwrap()
                .get_str("value")
                .unwrap(),
            "successor"
        );
    }
    first.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn expiry_during_materialization_refuses_commit_without_a_successor() {
    let (first, second) = stores().await;
    assert!(first.lease("rollups", "original", 70).await.unwrap());
    // Set a server-time expiry shortly ahead, then stage the actual update
    // before that deadline. No takeover/write conflict is needed for refusal.
    first
        .collection(CONTROL)
        .update_one(
            doc! {"_id":"rollups"},
            vec![doc! {"$set":{
                "lease_until":{"$add":[server_seconds(),2_i64]}
            }}],
        )
        .await
        .unwrap();
    let mut session = first.client.start_session().await.unwrap();
    session.start_transaction().await.unwrap();
    first
        .bulk_updates_in_session(
            CONTROL,
            vec![doc! {
                "q":{"_id":"infrastructure"},"u":{"$set":{"staged":true}},"upsert":true
            }],
            Some(&mut session),
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let expired = second
                .collection(CONTROL)
                .find_one(doc! {
                    "_id":"rollups","$expr":{"$lte":["$lease_until",server_seconds()]}
                })
                .await
                .unwrap();
            if expired.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();
    assert!(first
        .commit_controlled_write("rollups", "original", &mut session, Ok(()))
        .await
        .is_err());
    assert!(second
        .collection(CONTROL)
        .find_one(doc! {"_id":"infrastructure"})
        .await
        .unwrap()
        .is_none());
    first.db.drop().await.unwrap();
}

fn server_seconds() -> Document {
    doc! {"$toLong":{"$divide":[{"$toLong":"$$NOW"},1000_i64]}}
}
