//! Real signed callbacks and majority transactions, in disposable databases only.
use super::redelivery::proven_write;
use super::*;
use crate::store::bridge_definitions::inbox::receipts::model::AcceptedCallbackReceipt;
use crate::store::sandbox_mongo::{record_bson, DOCUMENT_SCHEMA_VERSION};
use futures_util::TryStreamExt;

#[path = "inbox_controls/accepted.rs"]
mod accepted;
#[path = "inbox_controls/failures.rs"]
mod failures;
#[path = "inbox_controls/http.rs"]
mod http;
#[path = "inbox_controls/unclassified.rs"]
mod unclassified;

async fn no_message_effects(client: &Client, database: &str, inbox: &MongoCallbackInbox) {
    for name in [
        "bridge_callback_inbox",
        "bridge_callback_replies",
        "bridge_callback_receipts",
        "bridge_callback_platform_messages",
        "agent_commands",
        "agent_log",
        "command_authorization_audits",
        "bridge_outbox_items",
        "bridge_outbox_invocations",
    ] {
        assert_eq!(
            client
                .database(database)
                .collection::<Document>(name)
                .count_documents(doc! {})
                .await
                .unwrap(),
            0,
            "{name}"
        );
    }
    assert!(inbox.pending_channel_inputs(None).await.unwrap().is_empty());
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn concurrent_reserved_redeliveries_persist_one_refusal_and_never_create_message_work() {
    let (client, database, store, inbox, id) = setup().await;
    let text = "CMA-CONTROL/1 steer ccr_source\nInspect the private migration.";
    let (body, token, reply) = callback("control-original", text);
    let (alias, alias_token, _) = callback("control-alias", text);
    let original = proven_write(&store, &id, &body, &token).await;
    let later = proven_write(&store, &id, &alias, &alias_token).await;
    let (a, b) = tokio::join!(inbox.accept(original), inbox.accept(later));
    let accepted = a.unwrap();
    assert_eq!(b.unwrap(), accepted);
    assert!(
        matches!(&accepted, CallbackAcceptance::ControlRefused(receipt) if receipt.code == ChannelControlRefusal::ChannelControlsDisabled)
    );
    no_message_effects(&client, &database, &inbox).await;
    let db = client.database(&database);
    let decisions = db.collection::<Document>("bridge_callback_dispositions");
    assert_eq!(
        decisions
            .count_documents(doc! {"kind":"callback"})
            .await
            .unwrap(),
        2
    );
    assert_eq!(
        decisions
            .count_documents(doc! {"kind":"provider"})
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        db.collection::<Document>("bridge_callback_rates")
            .find_one(doc! {})
            .await
            .unwrap()
            .unwrap()
            .get_i64("count")
            .unwrap(),
        1
    );
    let stored = decisions
        .find(doc! {})
        .await
        .unwrap()
        .try_collect::<Vec<_>>()
        .await
        .unwrap();
    let encoded = serde_json::to_string(&stored).unwrap();
    for secret in [
        text,
        &reply,
        "Inspect the private migration",
        "protected-interaction-secret",
        "reply_token",
        "thread_context",
        "ccr_source",
    ] {
        assert!(
            !encoded.contains(secret),
            "shared disposition retained text or credentials"
        );
    }
    let reopened =
        MongoCallbackInbox::open(&store, &database, "disposable-callback", &encryption_key())
            .await
            .unwrap();
    for (body, token) in [(&body, &token), (&alias, &alias_token)] {
        let write = proven_write(&store, &id, body, token).await;
        assert_eq!(
            reopened.recover_receipt(&write).await.unwrap(),
            Some(accepted.clone())
        );
        assert_eq!(reopened.accept(write).await.unwrap(), accepted);
    }
    assert_eq!(
        decisions
            .find(doc! {})
            .await
            .unwrap()
            .try_collect::<Vec<_>>()
            .await
            .unwrap(),
        stored
    );
    no_message_effects(&client, &database, &reopened).await;
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn conflicting_message_and_control_share_one_provider_choice() {
    for control_first in [false, true] {
        let (client, database, store, inbox, id) = setup().await;
        let (ordinary, ordinary_token, _) = callback("ordinary", "ordinary input");
        let (control, control_token, _) = callback("reserved", "CMA-CONTROL/1 cancel ccr_source");
        let (first, first_token, second, second_token) = if control_first {
            (&control, &control_token, &ordinary, &ordinary_token)
        } else {
            (&ordinary, &ordinary_token, &control, &control_token)
        };
        inbox
            .accept(proven_write(&store, &id, first, first_token).await)
            .await
            .unwrap();
        assert_eq!(
            inbox
                .accept(proven_write(&store, &id, second, second_token).await)
                .await,
            Err(CallbackError::ReplayConflict)
        );
        let decisions = client
            .database(&database)
            .collection::<Document>("bridge_callback_dispositions");
        assert_eq!(
            decisions
                .count_documents(doc! {"kind":"provider"})
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            decisions
                .count_documents(doc! {"kind":"callback"})
                .await
                .unwrap(),
            1
        );
        if control_first {
            no_message_effects(&client, &database, &inbox).await;
        }
        client.database(&database).drop().await.unwrap();
    }
    let (client, database, store, inbox, id) = setup().await;
    let (ordinary, ordinary_token, _) = callback("racing-ordinary", "ordinary input");
    let (control, control_token, _) =
        callback("racing-reserved", "CMA-CONTROL/1 cancel ccr_source");
    let first = proven_write(&store, &id, &ordinary, &ordinary_token).await;
    let second = proven_write(&store, &id, &control, &control_token).await;
    let (a, b) = tokio::join!(inbox.accept(first), inbox.accept(second));
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    assert!(a == Err(CallbackError::ReplayConflict) || b == Err(CallbackError::ReplayConflict));
    assert_eq!(
        client
            .database(&database)
            .collection::<Document>("bridge_callback_dispositions")
            .count_documents(doc! {"kind":"provider"})
            .await
            .unwrap(),
        1
    );
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn retained_legacy_prefix_message_recovers_without_reclassification_or_backfill() {
    let (client, database, store, inbox, id) = setup().await;
    let (body, token, _) = callback("legacy-reserved", "CMA-CONTROL/1 cancel ccr_legacy");
    let write = proven_write(&store, &id, &body, &token).await;
    let source = write.record();
    let receipt = AcceptedCallbackReceipt::capture(source)
        .unwrap()
        .document()
        .unwrap();
    let row = doc! {"_id":&source.id,"document_schema_version":DOCUMENT_SCHEMA_VERSION,
    "bridge_id":id.as_str(),"admission":"accepted","expires_at":mongodb::bson::DateTime::from_millis(source.expires_at*1000),
    "record":record_bson(source).unwrap()};
    let db = client.database(&database);
    // Exact persisted v1 fixtures model a pre-guard accepted Message.
    db.collection::<Document>("bridge_callback_receipts")
        .insert_one(receipt.clone())
        .await
        .unwrap();
    db.collection::<Document>("bridge_callback_inbox")
        .insert_one(row.clone())
        .await
        .unwrap();
    let proof = write.platform_redelivery().unwrap();
    let first = json!({"key_sha256":proof.key_sha256(),"semantic_sha256":proof.semantic_sha256(),
        "source":{"inbox_id":source.id,"owner":source.owner,"pin":source.pin,"bot_id":source.bot_id,
            "key_id":source.key_id,"message_id":source.message_id,"correlation_id":source.correlation_id,
            "issuer":source.attestation.issuer,"body_sha256":source.attestation.body_sha256},
        "receipt":source.receipt(),"expires_at":source.expires_at});
    db.collection::<Document>("bridge_callback_platform_messages").insert_one(doc! {
        "_id":format!("message:{}",proof.key_sha256()),"document_schema_version":DOCUMENT_SCHEMA_VERSION,
        "kind":"first_message","record_sha256":layer2_core::profiles::content_digest(&first).unwrap(),
        "record":record_bson(&first).unwrap(),
    }).await.unwrap();
    let expected = CallbackAcceptance::Message(source.receipt());
    assert_eq!(inbox.accept(write).await.unwrap(), expected);
    let (alias, alias_token, _) = callback("legacy-alias", "CMA-CONTROL/1 cancel ccr_legacy");
    assert_eq!(
        inbox
            .accept(proven_write(&store, &id, &alias, &alias_token).await)
            .await
            .unwrap(),
        expected
    );
    assert_eq!(
        db.collection::<Document>("bridge_callback_dispositions")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        db.collection::<Document>("bridge_callback_inbox")
            .find_one(doc! {})
            .await
            .unwrap(),
        Some(row)
    );
    db.collection::<Document>("bridge_callback_inbox")
        .delete_many(doc! {})
        .await
        .unwrap();
    let reopened =
        MongoCallbackInbox::open(&store, &database, "disposable-callback", &encryption_key())
            .await
            .unwrap();
    assert_eq!(
        reopened
            .accept(proven_write(&store, &id, &body, &token).await)
            .await
            .unwrap(),
        expected
    );
    assert_eq!(
        reopened
            .recover_receipt(&proven_write(&store, &id, &alias, &alias_token).await)
            .await
            .unwrap(),
        Some(expected)
    );
    assert_eq!(
        db.collection::<Document>("bridge_callback_receipts")
            .find_one(doc! {})
            .await
            .unwrap(),
        Some(receipt)
    );
    db.drop().await.unwrap();
}
