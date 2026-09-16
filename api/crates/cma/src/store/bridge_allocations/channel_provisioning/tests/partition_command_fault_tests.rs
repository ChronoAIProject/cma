use super::*;

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn partition_command_journal_failure_rolls_back_head_usage_and_generation() {
    let f = ReplyFixture::new().await;
    seed_original(&f).await;
    let service = BridgePartitionCommandService::new(f.svc.clone(), adapter(&f));
    let request = request(&f, &f.allocation, PartitionCommandAction::Reset).await;
    let database = f.client.database(&f.database);
    let records = database.collection::<Document>("bridge_allocations");
    let before = records
        .find_one(doc! {"_id":&f.allocation.id})
        .await
        .unwrap()
        .unwrap();
    database.run_command(doc!{"collMod":"bridge_partition_command_receipts","validator":{"blocked":{"$exists":true}},"validationLevel":"strict","validationAction":"error"}).await.unwrap();
    assert!(service
        .command(
            &caller(),
            None,
            &f.allocation.pin.bridge_id,
            "journal-fail",
            request.clone()
        )
        .await
        .is_err());
    let after = records
        .find_one(doc! {"_id":&f.allocation.id})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(before.get("record"), after.get("record"));
    assert_eq!(
        before.get("channel_usage_binding"),
        after.get("channel_usage_binding")
    );
    assert_eq!(records.count_documents(doc! {}).await.unwrap(), 1);
    database.run_command(doc!{"collMod":"bridge_partition_command_receipts","validator":{},"validationLevel":"strict"}).await.unwrap();
    let receipt = service
        .command(
            &caller(),
            None,
            &f.allocation.pin.bridge_id,
            "journal-fail",
            request,
        )
        .await
        .unwrap();
    assert_eq!(receipt.successor.unwrap().generation, 2);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn partition_command_detached_proof_corruption_cannot_authorize_original_reply() {
    let f = ReplyFixture::new().await;
    seed_original(&f).await;
    let context = f.context().await;
    let service = BridgePartitionCommandService::new(f.svc.clone(), adapter(&f));
    let request = request(&f, &f.allocation, PartitionCommandAction::Detach).await;
    let receipt = service
        .command(
            &caller(),
            None,
            &f.allocation.pin.bridge_id,
            "detach-proof",
            request,
        )
        .await
        .unwrap();
    let receipts = f
        .client
        .database(&f.database)
        .collection::<Document>("bridge_partition_command_receipts");
    let original = receipts
        .find_one(doc! {"_id":&receipt.id})
        .await
        .unwrap()
        .unwrap();
    receipts
        .update_one(
            doc! {"_id":&receipt.id},
            doc! {"$set":{"journal_sha256":"0".repeat(64)}},
        )
        .await
        .unwrap();
    assert_eq!(
        f.reply(&context, "corrupt-detach-proof", "Must stay held.")
            .await["isError"],
        true
    );
    receipts
        .replace_one(doc! {"_id":&receipt.id}, original)
        .await
        .unwrap();
    assert_eq!(
        f.reply(&context, "repaired-detach-proof", "Original allowed.")
            .await["isError"],
        false
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn partition_command_requires_original_source_and_preserves_it_after_callback_expiry() {
    let f = ReplyFixture::new().await;
    let service = BridgePartitionCommandService::new(f.svc.clone(), adapter(&f));
    let request = request(&f, &f.allocation, PartitionCommandAction::Detach).await;
    assert!(matches!(
        service
            .command(
                &caller(),
                None,
                &f.allocation.pin.bridge_id,
                "no-original",
                request.clone()
            )
            .await,
        Err(PartitionCommandError::SourceUnavailable)
    ));
    seed_original(&f).await;
    service
        .command(
            &caller(),
            None,
            &f.allocation.pin.bridge_id,
            "no-original",
            request,
        )
        .await
        .unwrap();
    f.client
        .database(&f.database)
        .collection::<Document>("bridge_callback_inbox")
        .delete_one(doc! {"_id":f.allocation.first_source.inbox_id()})
        .await
        .unwrap();
    let current = f
        .allocations
        .current(&f.allocation.identity)
        .await
        .unwrap()
        .unwrap();
    let reset = service
        .command(
            &caller(),
            None,
            &f.allocation.pin.bridge_id,
            "reset-archived-source",
            support::request(&f, &current, PartitionCommandAction::Reset).await,
        )
        .await
        .unwrap();
    assert_eq!(reset.successor.unwrap().generation, 2);
    f.close().await;
}
