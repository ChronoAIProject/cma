use super::*;
use futures_util::TryStreamExt;
use layer2_core::bridge::{invoke::*, outbox::ChannelControlReferenceRecord};
use mongodb::bson::Document;
use serde_json::Value;

#[path = "channel_control_reference_commit.rs"]
mod commit;
#[path = "channel_control_reference_races.rs"]
mod races;

async fn store(f: &ReplyFixture) -> Arc<crate::store::MongoBridgeInvokeStore> {
    Arc::new(
        crate::store::MongoBridgeInvokeStore::open(
            f.bridges.clone(),
            f.resources.clone(),
            &f.database,
        )
        .await
        .unwrap(),
    )
}
async fn consent(
    f: &ReplyFixture,
    revision: u64,
    action: SourceControlAction,
) -> SourceControlReceipt {
    let bridge = f
        .bridges
        .bridge(&f.inbox.pin.bridge_id)
        .await
        .unwrap()
        .unwrap();
    BridgeSourceControlService::new(f.invokes.clone(), store(f).await)
        .submit(
            &caller(),
            None,
            &f.grant.id,
            &format!("reference-consent-{revision}"),
            SourceControlRequest {
                expected_bridge_revision: bridge.revision,
                expected_grant_revision: 1,
                expected_policy_revision: revision,
                action,
            },
        )
        .await
        .unwrap()
}
async fn read(f: &ReplyFixture) -> Value {
    let result = f.call("read_channel_control_context", json!({})).await;
    assert_eq!(result["isError"], false, "{result}");
    let result = result["structuredContent"].clone();
    assert_eq!(result["operations"], json!([]));
    assert_eq!(result["approval_mode"], "dashboard_only");
    result
}
async fn rows(f: &ReplyFixture) -> Vec<Document> {
    f.client
        .database(&f.database)
        .collection("bridge_channel_control_references")
        .find(doc! {})
        .sort(doc! {"_id":1})
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap()
}
fn record(document: &Document) -> ChannelControlReferenceRecord {
    let record: ChannelControlReferenceRecord =
        mongodb::bson::deserialize_from_document(document.get_document("record").unwrap().clone())
            .unwrap();
    record.validate().unwrap();
    record
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_reference_requires_consent_recovers_exact_mapping_and_never_sends() {
    let f = ReplyFixture::new().await;
    let old_context = f.context().await;
    let before = f
        .resources
        .get_command(&f.command.receipt.envelope.command_id)
        .await
        .unwrap()
        .unwrap();
    assert!(read(&f).await["source_control_reference"].is_null());
    assert!(rows(&f).await.is_empty());
    let receipt = consent(&f, 0, SourceControlAction::AllowSenderCancelAndSteer).await;
    let (first, second) = tokio::join!(read(&f), read(&f));
    assert_eq!(first, second);
    assert_eq!(first["availability"], "execution_unavailable");
    let reference = first["source_control_reference"].as_str().unwrap();
    assert_eq!(reference.len(), 36);
    assert!(reference.starts_with("ccr_"));
    let retained = rows(&f).await;
    assert_eq!(retained.len(), 1);
    let retained_record = record(&retained[0]);
    assert_eq!(retained_record.reference, reference);
    assert_eq!(retained_record.binding().consent_receipt, receipt);
    assert_eq!(
        retained_record.binding().source.command_id(),
        &f.command.receipt.envelope.command_id
    );
    assert_eq!(
        retained_record.binding().source.response_id(),
        f.command.receipt.response_id.as_ref().unwrap()
    );
    assert_eq!(
        retained_record.binding().source.allocation_id(),
        f.allocation.id
    );
    assert_eq!(
        retained_record.binding().sender_id,
        f.inbox.envelope.sender_id
    );
    assert_eq!(
        retained_record.binding().turn_sequence,
        f.command.work.dispatch.as_ref().unwrap().turn_sequence
    );
    let reopened =
        crate::store::MongoBridgeReplyToolStore::new(store(&f).await, Arc::new(Authority))
            .await
            .unwrap();
    let session = f.tools.authenticate(&f.credential).await.unwrap();
    assert_eq!(
        reopened.control_context(&session).await.unwrap().as_deref(),
        Some(reference)
    );
    f.client
        .database(&f.database)
        .collection::<Document>("bridge_callback_inbox")
        .delete_one(doc! {"_id":&f.inbox.id})
        .await
        .unwrap();
    assert_eq!(read(&f).await, first);
    assert_eq!(f.context().await, old_context);
    assert_eq!(
        f.resources
            .get_command(&f.command.receipt.envelope.command_id)
            .await
            .unwrap()
            .unwrap(),
        before
    );
    assert!(f.items().await.is_empty());
    f.finish().await;
    assert!(read(&f).await["source_control_reference"].is_null());
    assert_eq!(rows(&f).await, retained);
    assert!(f.items().await.is_empty());
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_reference_reenable_pins_new_complete_consent_and_keeps_original_immutable()
{
    let f = ReplyFixture::new().await;
    consent(&f, 0, SourceControlAction::AllowSenderCancelAndSteer).await;
    let first = read(&f).await;
    let original = rows(&f).await.remove(0);
    consent(&f, 1, SourceControlAction::RevokeSenderControls).await;
    assert!(read(&f).await["source_control_reference"].is_null());
    assert_eq!(rows(&f).await, vec![original.clone()]);
    let next = consent(&f, 2, SourceControlAction::AllowSenderCancelAndSteer).await;
    let renewed = read(&f).await;
    assert_ne!(
        first["source_control_reference"],
        renewed["source_control_reference"]
    );
    let retained = rows(&f).await;
    assert_eq!(retained.len(), 2);
    assert!(retained.contains(&original));
    let new = retained
        .iter()
        .find(|row| {
            row.get_str("_id").unwrap() == renewed["source_control_reference"].as_str().unwrap()
        })
        .unwrap();
    assert_eq!(record(new).binding().consent_receipt, next);
    assert_ne!(
        record(new).binding().consent_sha256,
        record(&original).binding().consent_sha256
    );
    assert!(f.items().await.is_empty());
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_reference_copied_lookup_never_selects_foreign_source_and_rejects_arguments(
) {
    let a = ReplyFixture::new().await;
    consent(&a, 0, SourceControlAction::AllowSenderCancelAndSteer).await;
    let first = read(&a).await;
    let original = rows(&a).await.remove(0);
    let b = ReplyFixture::new().await;
    consent(&b, 0, SourceControlAction::AllowSenderCancelAndSteer).await;
    b.client
        .database(&b.database)
        .collection::<Document>("bridge_channel_control_references")
        .insert_one(original.clone())
        .await
        .unwrap();
    let second = read(&b).await;
    assert_ne!(
        first["source_control_reference"],
        second["source_control_reference"]
    );
    assert_eq!(rows(&b).await.len(), 2);
    assert!(rows(&b).await.contains(&original));
    let response = b
        .raw_call(
            "read_channel_control_context",
            json!({"source_control_reference":first["source_control_reference"]}),
        )
        .await;
    let value: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 32768)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(value["error"]["code"], -32602);
    assert_eq!(rows(&b).await.len(), 2);
    assert!(a.items().await.is_empty() && b.items().await.is_empty());
    a.close().await;
    b.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_reference_missing_consent_history_cannot_recover_old_permission() {
    let f = ReplyFixture::new().await;
    let receipt = consent(&f, 0, SourceControlAction::AllowSenderCancelAndSteer).await;
    let _ = read(&f).await;
    let retained = rows(&f).await;
    f.client
        .database(&f.database)
        .collection::<Document>("bridge_source_control_receipts")
        .delete_one(doc! {"_id":receipt.id})
        .await
        .unwrap();
    assert!(read(&f).await["source_control_reference"].is_null());
    assert_eq!(rows(&f).await, retained);
    assert!(f.items().await.is_empty());
    f.close().await;
}
