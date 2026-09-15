use super::*;

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn inbox_work_discovery_survives_corruption_and_reopens_with_exact_accepted_content() {
    let (client, database, store, inbox, id) = setup().await;
    let mut receipts = Vec::new();
    for number in 0..4 {
        let (body, token, _) = callback(&format!("work-{number}"), "retained input");
        receipts.push(
            inbox
                .accept(write(&store, &id, &body, &token).await)
                .await
                .map(|value| value.into_message().unwrap())
                .unwrap(),
        );
    }
    let rows = client
        .database(&database)
        .collection::<Document>("bridge_callback_inbox");
    let original = rows
        .find_one(doc! {"_id":&receipts[0].inbox_id})
        .await
        .unwrap()
        .unwrap();
    let mut corrupt = original.clone();
    corrupt.insert("_id", "000-corrupt");
    rows.insert_one(corrupt).await.unwrap();
    let ids = inbox.pending_channel_inputs(None).await.unwrap();
    assert_eq!(ids.first().map(String::as_str), Some("000-corrupt"));
    assert!(inbox.channel_input("000-corrupt").await.is_err());
    let later = inbox
        .pending_channel_inputs(Some("000-corrupt"))
        .await
        .unwrap();
    assert_eq!(later.len(), 4);
    inbox
        .defer_channel_input(
            &receipts[0].inbox_id,
            Duration::from_secs(30),
            "waiting_allocation",
        )
        .await
        .unwrap();
    let reopened =
        MongoCallbackInbox::open(&store, &database, "disposable-callback", &encryption_key())
            .await
            .unwrap();
    assert!(!reopened
        .pending_channel_inputs(None)
        .await
        .unwrap()
        .contains(&receipts[0].inbox_id));
    let retained = reopened
        .channel_input(&receipts[0].inbox_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retained.receipt(), receipts[0]);
    assert_eq!(retained.envelope.text.as_deref(), Some("retained input"));
    assert_eq!(
        rows.find_one(doc! {"_id":&receipts[0].inbox_id})
            .await
            .unwrap()
            .unwrap()
            .get("record"),
        original.get("record")
    );
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn command_correlated_callbacks_release_inbox_capacity_without_deleting_acceptance() {
    let (client, database, store, inbox, id) = setup().await;
    let mut receipts = Vec::new();
    for number in 0..5 {
        let (body, token, _) = callback(&format!("queued-{number}"), "retained input");
        receipts.push(
            inbox
                .accept(write(&store, &id, &body, &token).await)
                .await
                .map(|value| value.into_message().unwrap())
                .unwrap(),
        );
    }
    let (body, token, _) = callback("extra", "next input");
    assert_eq!(
        inbox
            .accept(write(&store, &id, &body, &token).await)
            .await
            .map(|value| value.into_message().unwrap()),
        Err(CallbackError::Capacity)
    );
    let rows = client
        .database(&database)
        .collection::<Document>("bridge_callback_inbox");
    // Disposable fixture models the same command-correlation sidecar committed
    // atomically by MongoChannelCommandStore; acceptance stays byte-identical.
    let original = rows
        .find_one(doc! {"_id":&receipts[0].inbox_id})
        .await
        .unwrap()
        .unwrap();
    rows.update_one(doc! {"_id":&receipts[0].inbox_id}, doc! {"$set":{"command_correlation":{
        "command_id":"cmd_fixture", "response_id":"resp_fixture", "allocation_id":"bral_fixture",
        "body_sha256":original.get_document("record").unwrap().get_document("attestation").unwrap().get_str("body_sha256").unwrap(),
    }}}).await.unwrap();
    inbox
        .accept(write(&store, &id, &body, &token).await)
        .await
        .map(|value| value.into_message().unwrap())
        .unwrap();
    assert_eq!(rows.count_documents(doc! {}).await.unwrap(), 6);
    assert!(inbox
        .channel_input(&receipts[0].inbox_id)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        rows.find_one(doc! {"_id":&receipts[0].inbox_id})
            .await
            .unwrap()
            .unwrap()
            .get("record"),
        original.get("record")
    );
    client.database(&database).drop().await.unwrap();
}
