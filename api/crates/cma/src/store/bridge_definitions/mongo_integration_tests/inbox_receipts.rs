use super::*;
use crate::store::bridge_definitions::inbox::receipts::model::{
    AcceptanceDisposition, AcceptedCallbackReceipt,
};
use futures_util::TryStreamExt;

async fn short_retention(store: &Arc<MongoBridgeStore>, id: &BridgeId) {
    let mut config = config();
    config.limits.inbox_retention_seconds = 1;
    config.limits.reply_secret_retention_seconds = 1;
    service(store.clone())
        .mutate(
            &caller(),
            None,
            change(
                id,
                2,
                "short-retention",
                BridgeChange::Update {
                    config: Box::new(config),
                    previous_work: Some(AcceptedWorkPolicy::DrainOriginal),
                },
            ),
        )
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn expired_payload_keeps_original_receipt_without_restoring_admission_after_restart() {
    let (client, database, store, inbox, id) = setup().await;
    short_retention(&store, &id).await;
    let (body, token, reply) = callback("expired-acceptance", "secret accepted input");
    let accepted = inbox
        .accept(write(&store, &id, &body, &token).await)
        .await
        .unwrap();
    let db = client.database(&database);
    let receipts = db.collection::<Document>("bridge_callback_receipts");
    let original = receipts
        .find_one(doc! {"_id":&accepted.inbox_id})
        .await
        .unwrap()
        .unwrap();
    let expected = AcceptedCallbackReceipt::read(&original).unwrap();
    assert!(matches!(
        expected.disposition,
        AcceptanceDisposition::Pending
    ));
    let encoded = serde_json::to_string(&original).unwrap();
    for secret in [
        &reply,
        "secret accepted input",
        "protected-interaction-secret",
        "reply_token",
        "thread_context",
    ] {
        assert!(
            !encoded.contains(secret),
            "safe receipt must omit payload and provider secrets"
        );
    }
    let indexes = receipts
        .list_indexes()
        .await
        .unwrap()
        .try_collect::<Vec<_>>()
        .await
        .unwrap();
    assert!(indexes.iter().all(|index| index
        .options
        .as_ref()
        .and_then(|options| options.expire_after)
        .is_none()));
    tokio::time::sleep(Duration::from_millis(1100)).await;
    db.collection::<Document>("bridge_callback_inbox")
        .delete_one(doc! {
            "_id":&accepted.inbox_id,"expires_at":{"$lte":mongodb::bson::DateTime::now()}
        })
        .await
        .unwrap();
    let reopened =
        MongoCallbackInbox::open(&store, &database, "disposable-callback", &encryption_key())
            .await
            .unwrap();
    let current = receipts
        .find_one(doc! {"_id":&accepted.inbox_id})
        .await
        .unwrap()
        .unwrap();
    let retained = AcceptedCallbackReceipt::read(&current).unwrap();
    assert_eq!(retained.receipt, accepted);
    assert_eq!(retained.expires_at, expected.expires_at);
    assert_eq!(
        retained.disposition,
        AcceptanceDisposition::Expired {
            expired_at: expected.expires_at
        }
    );
    assert_eq!(
        reopened
            .accept(write(&store, &id, &body, &token).await)
            .await
            .unwrap(),
        accepted
    );
    let (changed, changed_token, _) = callback("expired-acceptance", "replacement input");
    assert_eq!(
        reopened
            .accept(write(&store, &id, &changed, &changed_token).await)
            .await,
        Err(CallbackError::ReplayConflict)
    );
    assert!(reopened
        .pending_channel_inputs(None)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        db.collection::<Document>("bridge_callback_inbox")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        receipts
            .find_one(doc! {"_id":&accepted.inbox_id})
            .await
            .unwrap(),
        Some(current)
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn startup_backfill_requires_exact_live_source_and_skips_missing_or_corrupt_evidence() {
    let (client, database, store, inbox, id) = setup().await;
    let mut accepted = Vec::new();
    for message in ["live", "missing", "corrupt-correlation"] {
        let (body, token, _) = callback(message, "legacy input");
        accepted.push(
            inbox
                .accept(write(&store, &id, &body, &token).await)
                .await
                .unwrap(),
        );
    }
    let db = client.database(&database);
    let receipts = db.collection::<Document>("bridge_callback_receipts");
    let rows = db.collection::<Document>("bridge_callback_inbox");
    receipts.delete_many(doc! {}).await.unwrap();
    rows.delete_one(doc! {"_id":&accepted[1].inbox_id})
        .await
        .unwrap();
    rows.update_one(
        doc! {"_id":&accepted[2].inbox_id},
        doc! {"$set":{"command_correlation":"malformed"}},
    )
    .await
    .unwrap();
    let mut corrupt = rows
        .find_one(doc! {"_id":&accepted[0].inbox_id})
        .await
        .unwrap()
        .unwrap();
    corrupt.insert("_id", "000-corrupt-source");
    rows.insert_one(corrupt).await.unwrap();
    MongoCallbackInbox::open(&store, &database, "disposable-callback", &encryption_key())
        .await
        .unwrap();
    assert_eq!(receipts.count_documents(doc! {}).await.unwrap(), 1);
    let recovered = receipts
        .find_one(doc! {"_id":&accepted[0].inbox_id})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        AcceptedCallbackReceipt::read(&recovered).unwrap().receipt,
        accepted[0]
    );
    assert_eq!(rows.count_documents(doc! {}).await.unwrap(), 3);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn disposition_cursor_passes_corrupt_batch_without_fabricating_terminal_results() {
    let (client, database, store, inbox, id) = setup().await;
    short_retention(&store, &id).await;
    let (body, token, _) = callback("later-valid", "retained input");
    let accepted = inbox
        .accept(write(&store, &id, &body, &token).await)
        .await
        .unwrap();
    let db = client.database(&database);
    let receipts = db.collection::<Document>("bridge_callback_receipts");
    let original = receipts
        .find_one(doc! {"_id":&accepted.inbox_id})
        .await
        .unwrap()
        .unwrap();
    for number in 0..33 {
        let mut corrupt = original.clone();
        corrupt.insert("_id", format!("000-corrupt-{number:02}"));
        receipts.insert_one(corrupt).await.unwrap();
    }
    tokio::time::sleep(Duration::from_millis(1100)).await;
    db.collection::<Document>("bridge_callback_inbox")
        .delete_one(doc! {
            "_id":&accepted.inbox_id,"expires_at":{"$lte":mongodb::bson::DateTime::now()}
        })
        .await
        .unwrap();
    let reopened =
        MongoCallbackInbox::open(&store, &database, "disposable-callback", &encryption_key())
            .await
            .unwrap();
    reopened.settle_expired_acceptances().await.unwrap();
    let valid = receipts
        .find_one(doc! {"_id":&accepted.inbox_id})
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        AcceptedCallbackReceipt::read(&valid).unwrap().disposition,
        AcceptanceDisposition::Expired { .. }
    ));
    assert_eq!(
        receipts
            .count_documents(doc! {"disposition":"pending"})
            .await
            .unwrap(),
        33
    );
    assert_eq!(
        receipts
            .count_documents(doc! {"disposition":"expired"})
            .await
            .unwrap(),
        1
    );
    db.drop().await.unwrap();
}
