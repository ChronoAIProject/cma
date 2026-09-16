use super::*;
use crate::store::channel_history::{
    self, ChannelHistoryAssessment, ChannelHistoryPolicy, HistoryHold,
};
use crate::store::sandbox_mongo::start_transaction;
use mongodb::bson::Document;
#[path = "history_outbox_seal_tests.rs"]
mod outbox_seal;

async fn allocation(f: &ReplyFixture) -> Document {
    f.client
        .database(&f.database)
        .collection::<Document>("bridge_allocations")
        .find_one(doc! {"_id": &f.allocation.id})
        .await
        .unwrap()
        .unwrap()
}
async fn assessment(
    f: &ReplyFixture,
) -> Result<ChannelHistoryAssessment, crate::store::bridge_allocations::MutationError> {
    let db = f.client.database(&f.database);
    let mut session = f.client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    let document = db
        .collection::<Document>("bridge_allocations")
        .find_one(doc! {"_id": &f.allocation.id})
        .session(&mut session)
        .await
        .unwrap()
        .unwrap();
    let result = channel_history::assess(&db, &f.allocation, &document, &mut session).await;
    session.abort_transaction().await.unwrap();
    result
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_history_new_policy_is_pinned_and_replay_reset_preserve_both_generations() {
    let f = Fixture::new().await;
    let db = f.client.database(&f.database);
    let rows = db.collection::<Document>("bridge_allocations");
    let original = rows
        .find_one(doc! {"_id": &f.record.id})
        .await
        .unwrap()
        .unwrap();
    let policy = channel_history::policy(&original, &f.record)
        .unwrap()
        .unwrap();
    assert_eq!(policy, ChannelHistoryPolicy::capture(&f.record).unwrap());
    assert_eq!(
        original.get_i64("channel_history_reference_count").unwrap(),
        1
    );
    let (admission, authority, _) =
        proposal(&f.svc, &f.bridges, &f.record.pin.bridge_id, "12345", None).await;
    let replay = f
        .allocations
        .reserve(AllocationReservation::new(admission, authority, now()).unwrap())
        .await
        .unwrap();
    assert!(replay == f.record);
    assert_eq!(
        rows.find_one(doc! {"_id": &f.record.id})
            .await
            .unwrap()
            .unwrap(),
        original
    );
    let (admission, authority, manager) = proposal(
        &f.svc,
        &f.bridges,
        &f.record.pin.bridge_id,
        "12345",
        Some(&f.record),
    )
    .await;
    let reset = f
        .allocations
        .reserve(
            AllocationReservation::reset(admission, authority, f.record.clone(), manager, now())
                .unwrap(),
        )
        .await
        .unwrap();
    let old = rows
        .find_one(doc! {"_id": &f.record.id})
        .await
        .unwrap()
        .unwrap();
    let next = rows
        .find_one(doc! {"_id": &reset.id})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        old.get("channel_history_policy"),
        original.get("channel_history_policy")
    );
    assert_eq!(
        channel_history::policy(&next, &reset).unwrap(),
        Some(ChannelHistoryPolicy::capture(&reset).unwrap())
    );
    assert_eq!(next.get_i64("channel_history_reference_count").unwrap(), 1);
    assert_eq!(
        db.collection::<Document>("channel_history_references")
            .count_documents(doc! {})
            .await
            .unwrap(),
        2
    );
    f.client.database(&f.database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_history_command_source_reply_and_receipts_survive_ttl_and_exact_replay() {
    let f = ReplyFixture::new().await;
    let db = f.client.database(&f.database);
    let before = assessment(&f).await.unwrap();
    assert_eq!(before.reference_count, 4);
    assert_eq!(before.hold, None);
    assert!(!before.inventory_sealed);
    let context = f.context().await;
    let reply = f
        .reply(&context, "history-first", "Retain this explicit reply.")
        .await;
    assert_eq!(reply["isError"], false, "{reply}");
    db.collection::<Document>("bridge_callback_inbox")
        .delete_one(doc! {"_id": &f.inbox.id})
        .await
        .unwrap();
    f.finish().await;
    assert_eq!(
        f.reply(&context, "history-first", "Retain this explicit reply.")
            .await,
        reply
    );
    let after = assessment(&f).await.unwrap();
    assert_eq!(after.reference_count, 6);
    assert_eq!(after.hold, None);
    assert_eq!(before.policy, after.policy);
    assert!(!after.inventory_sealed);
    assert_eq!(
        allocation(&f)
            .await
            .get_i64("channel_history_reference_count")
            .unwrap(),
        6
    );
    let mut indexes = db
        .collection::<Document>("channel_history_references")
        .list_indexes()
        .await
        .unwrap();
    use futures_util::TryStreamExt;
    while let Some(index) = indexes.try_next().await.unwrap() {
        assert!(index.options.unwrap_or_default().expire_after.is_none());
    }
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_history_reference_failure_rolls_back_explicit_reply_and_count() {
    let f = ReplyFixture::new().await;
    let db = f.client.database(&f.database);
    let before = allocation(&f).await;
    let context = f.context().await;
    db.run_command(doc! {"collMod":"channel_history_references", "validator":{"denied":{"$exists":true}}, "validationLevel":"strict", "validationAction":"error"}).await.unwrap();
    let reply = f
        .reply(&context, "history-rollback", "No partial reply.")
        .await;
    assert_eq!(reply["isError"], true, "{reply}");
    assert!(f.items().await.is_empty());
    assert_eq!(
        db.collection::<Document>("bridge_outbox_invocations")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    let after = allocation(&f).await;
    assert_eq!(
        after.get("channel_history_reference_count"),
        before.get("channel_history_reference_count")
    );
    assert_eq!(
        after.get("channel_history_policy"),
        before.get("channel_history_policy")
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_history_missing_reference_target_and_legacy_policy_are_typed_holds() {
    let f = ReplyFixture::new().await;
    let db = f.client.database(&f.database);
    let references = db.collection::<Document>("channel_history_references");
    let reference = references
        .find_one(doc! {"allocation_id": &f.allocation.id})
        .await
        .unwrap()
        .unwrap();
    let id = reference.get_str("_id").unwrap();
    references.delete_one(doc! {"_id": id}).await.unwrap();
    assert_eq!(
        assessment(&f).await.unwrap().hold,
        Some(HistoryHold::ReferenceAccountingIncomplete)
    );
    references.insert_one(reference.clone()).await.unwrap();
    let receipts = db.collection::<Document>("bridge_callback_receipts");
    let receipt = receipts
        .find_one(doc! {"_id": &f.inbox.id})
        .await
        .unwrap()
        .unwrap();
    receipts
        .delete_one(doc! {"_id": &f.inbox.id})
        .await
        .unwrap();
    assert_eq!(
        assessment(&f).await.unwrap().hold,
        Some(HistoryHold::ReferencedRecordMissing)
    );
    receipts.insert_one(receipt).await.unwrap();
    db.collection::<Document>("bridge_allocations")
        .update_one(
            doc! {"_id": &f.allocation.id},
            doc! {"$unset":{"channel_history_policy":""}},
        )
        .await
        .unwrap();
    let context = f.context().await;
    let reply = f
        .reply(
            &context,
            "legacy-retain",
            "Keep the missing policy explicit.",
        )
        .await;
    assert_eq!(reply["isError"], false, "{reply}");
    let held = assessment(&f).await.unwrap();
    assert_eq!(held.hold, Some(HistoryHold::LegacyPolicyMissing));
    assert_eq!(held.policy, None);
    assert!(!held.inventory_sealed);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_history_swapped_policy_and_changed_permanent_source_are_rejected() {
    let f = ReplyFixture::new().await;
    let db = f.client.database(&f.database);
    let rows = db.collection::<Document>("bridge_allocations");
    let original = allocation(&f).await;
    rows.update_one(
        doc! {"_id": &f.allocation.id},
        doc! {"$set":{"channel_history_policy.allocation_sha256":"0".repeat(64)}},
    )
    .await
    .unwrap();
    assert!(assessment(&f).await.is_err());
    rows.update_one(doc! {"_id": &f.allocation.id}, doc! {"$set":{"channel_history_policy":original.get("channel_history_policy").unwrap().clone()}}).await.unwrap();
    db.collection::<Document>("agent_commands")
        .update_one(
            doc! {"_id": f.command.receipt.envelope.command_id.as_str()},
            doc! {"$set":{"channel_source.inbox.envelope.text":"changed retained source"}},
        )
        .await
        .unwrap();
    assert!(assessment(&f).await.is_err());
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_history_tombstone_accounting_is_atomic_and_survives_late_upload_retry() {
    let mut f = Fixture::new().await;
    let allocation = f.ready().await;
    crate::store::checkpoint_objects::history_test_support::check(
        &f.resources.checkpoint_objects,
        &f.client,
        &f.client.database(&f.database),
        &allocation,
    )
    .await;
    f.client.database(&f.database).drop().await.unwrap();
}
