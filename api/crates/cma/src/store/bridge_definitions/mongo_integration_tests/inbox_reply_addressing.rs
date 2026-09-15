use super::*;
use crate::bridge::ingress::reply_addressing::{ReplyCapabilities, ReplyLookup};
use layer2_core::bridge::partition::{ChannelAdmissionPolicy, GroupAdmission, IdentityAdmission};

async fn reply_policy(store: &Arc<MongoBridgeStore>, id: &BridgeId) {
    let mut config = config();
    config.admission = BridgeAdmissionPolicy::Scoped {
        policy: ChannelAdmissionPolicy {
            senders: IdentityAdmission::Open,
            chats: IdentityAdmission::Open,
            groups: GroupAdmission::ReplyToBot,
        },
    };
    service(store.clone())
        .mutate(
            &caller(),
            None,
            change(
                id,
                2,
                "reply-policy",
                BridgeChange::Update {
                    config: Box::new(config),
                    previous_work: Some(AcceptedWorkPolicy::DrainOriginal),
                },
            ),
        )
        .await
        .unwrap();
}

fn reply_callback(message: &str) -> (Vec<u8>, String) {
    let (body, _, _) = callback(message, "reply input");
    let mut value: Value = serde_json::from_slice(&body).unwrap();
    value["reply_to_platform_message_id"] = json!("40");
    value["raw_platform_data"]["message"]["reply_to_message"] = json!({
        "message_id":40,"chat":{"id":-100123,"type":"supergroup"},"from":{"id":999,"is_bot":true}
    });
    let body = serde_json::to_vec(&value).unwrap();
    let token = sign(
        json!({"iss":"https://nyx.example","aud":"channel-relay/callback","iat":now(),"exp":now()+300,
        "jti":"callback-1","token_type":"relay_callback","api_key_id":"key-1","message_id":message,"platform":"telegram",
        "body_sha256":format!("{:x}",Sha256::digest(&body))}),
    );
    (body, token)
}

async fn positive_write(
    store: &MongoBridgeStore,
    id: &BridgeId,
    body: &[u8],
    token: &str,
    item: &layer2_core::bridge::outbox::OutboxItem,
) -> InboxWrite {
    let provisional = write(store, id, body, token).await;
    let proof = ReplyLookup::retained(provisional.record())
        .unwrap()
        .confirm(item)
        .unwrap()
        .unwrap();
    let result = write_with_extractor(
        store,
        id,
        body,
        token,
        Duration::from_secs(60),
        &ReplyCapabilities(Some(proof)),
    )
    .await;
    assert_eq!(result.record().admission, InboxAdmission::Accepted);
    result
}

fn sent_document(item: &layer2_core::bridge::outbox::OutboxItem) -> Document {
    item.validate().unwrap();
    doc! {"_id":&item.id,"document_schema_version":crate::store::sandbox_mongo::DOCUMENT_SCHEMA_VERSION,"invocation_id":&item.tool_invocation_id,
    "response_id":item.source.response_id().as_str(),"sequence":item.sequence as i64,
    "revision":item.revision as i64,"state":"sent","updated_at_ms":item.updated_at_ms,
    "next_check_at_ms":item.updated_at_ms,"record":crate::store::sandbox_mongo::record_bson(item).unwrap()}
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn confirmed_reply_acceptance_fences_exact_sent_evidence_and_replay_keeps_original_receipt() {
    let (client, database, store, inbox, id) = setup().await;
    reply_policy(&store, &id).await;
    let item = crate::bridge::ingress::reply_addressing::tests::sent_for_bridge(
        store.bridge(&id).await.unwrap().unwrap(),
    );
    let db = client.database(&database);
    let items = db.collection::<Document>("bridge_outbox_items");
    let original = sent_document(&item);
    // Interfere after capability extraction but before acceptance. No stale
    // positive result may create an inbox row or a permanent refusal receipt.
    for change in ["missing", "corrupt", "substituted"] {
        items.insert_one(original.clone()).await.unwrap();
        let (body, token) = reply_callback(change);
        let pending = positive_write(&store, &id, &body, &token, &item).await;
        let expected = pending.record().receipt();
        match change {
            "missing" => {
                items.delete_one(doc! {"_id":&item.id}).await.unwrap();
            }
            "corrupt" => {
                items
                    .update_one(
                        doc! {"_id":&item.id},
                        doc! {"$set":{"record.text":"corrupt"}},
                    )
                    .await
                    .unwrap();
            }
            _ => {
                items
                    .update_one(
                        doc! {"_id":&item.id},
                        doc! {"$set":{"record.state.receipt.nyxid_message_id":"substituted-send"}},
                    )
                    .await
                    .unwrap();
            }
        }
        assert_eq!(
            inbox.accept(pending).await,
            Err(CallbackError::Unavailable),
            "{change}"
        );
        for collection in [
            "bridge_callback_inbox",
            "bridge_callback_receipts",
            "bridge_callback_replies",
        ] {
            assert_eq!(
                db.collection::<Document>(collection)
                    .count_documents(doc! {"_id":&expected.inbox_id})
                    .await
                    .unwrap(),
                0,
                "{change}: {collection}"
            );
        }
        items.delete_many(doc! {}).await.unwrap();
    }
    items.insert_one(original).await.unwrap();
    let (body, token) = reply_callback("confirmed");
    let accepted = inbox
        .accept(positive_write(&store, &id, &body, &token, &item).await)
        .await
        .unwrap();
    assert_eq!(
        items
            .find_one(doc! {"_id":&item.id})
            .await
            .unwrap()
            .unwrap()
            .get_i64("reply_addressing_fence")
            .unwrap(),
        1
    );
    items.delete_one(doc! {"_id":&item.id}).await.unwrap();
    // Prior receipt recovery precedes fresh addressing. A missing old send
    // neither revokes the accepted input nor changes its original deadline.
    assert_eq!(
        inbox
            .accept(write(&store, &id, &body, &token).await)
            .await
            .unwrap(),
        accepted
    );
    assert_eq!(
        db.collection::<Document>("bridge_callback_receipts")
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    db.drop().await.unwrap();
}
