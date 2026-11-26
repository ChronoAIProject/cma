use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn uncertain_claim_and_ack_recover_from_majority_state_with_original_identity() {
    let f = Fixture::new(true).await;
    let stream = delivery::stream(&f, "owner-without-billing-enrollment").await;
    let app = format!("usage-delivery-{}", uuid::Uuid::new_v4());
    let mut options =
        ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap())
            .await
            .unwrap();
    options.app_name = Some(app.clone());
    let peer = Client::with_options(options).unwrap();
    let store = CaptureStore::open(&peer, f.db.name(), config())
        .await
        .unwrap();
    let destination = "a".repeat(64);
    assert!(!store.producer_blocked(&destination).await.unwrap());
    let admin = f.client.database("admin");
    let fail = |command| {
        doc! {"configureFailPoint":"failCommand","mode":"alwaysOn", "data":{
        "failCommands":[command],"appName":&app,"writeConcernError":{"code":64_i32,"codeName":"WriteConcernFailed","errmsg":"injected uncertain usage delivery"}}}
    };
    admin.run_command(fail("findAndModify")).await.unwrap();
    let claim = store.claim_delivery(&stream, &destination).await;
    admin
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    assert!(claim.is_err());
    crate::store::test_visibility::majority_barrier(&f.client, f.db.name()).await;
    let rows = f.db.collection::<Document>(OUTBOX);
    let retained = rows
        .find_one(doc! {"stream_id":&stream,"sequence":1_i64})
        .await
        .unwrap()
        .unwrap();
    assert!(retained.contains_key("delivery"));
    assert!(store
        .claim_delivery(&stream, &destination)
        .await
        .unwrap()
        .is_none());
    rows.update_one(
        doc! {"_id":retained.get_str("_id").unwrap()},
        doc! {"$set":{
        "delivery.lease_until":mongodb::bson::DateTime::from_millis(0),
        "delivery.due_at":mongodb::bson::DateTime::from_millis(0)}},
    )
    .await
    .unwrap();
    let attempt = store
        .claim_delivery(&stream, &destination)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(attempt.event.event_id, retained.get_str("_id").unwrap());
    assert_eq!(
        attempt.event.payload,
        *retained.get_binary_generic("payload").unwrap()
    );
    admin.run_command(fail("update")).await.unwrap();
    let ack = store
        .finish_delivery(
            &attempt,
            DeliveryOutcome::Accepted(delivery::receipt(&attempt)),
        )
        .await;
    admin
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    assert!(ack.is_err());
    let saved = rows
        .find_one(doc! {"_id":&attempt.event.event_id})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        saved
            .get_document("delivery")
            .unwrap()
            .get_str("status")
            .unwrap(),
        "accepted"
    );
    assert!(saved
        .get_document("delivery")
        .unwrap()
        .get_document("receipt")
        .is_ok());
    // The injected concern error can return before the local write reaches the
    // majority commit point. The next worker must wait for that visibility.
    let mut recovered = None;
    for _ in 0..100 {
        recovered = store.claim_delivery(&stream, &destination).await.unwrap();
        if recovered.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(
        recovered
            .expect("majority ACK must release the next event")
            .event
            .sequence,
        2
    );
    assert_eq!(f.count(OUTBOX).await, 2);
    f.db.drop().await.unwrap();
}
