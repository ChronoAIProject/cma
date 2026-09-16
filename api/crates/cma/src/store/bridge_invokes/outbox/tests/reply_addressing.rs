use super::*;
use crate::bridge::ingress::reply_addressing::{ReplyAddressingStore, ReplyLookup};
use layer2_core::bridge::{ingress::CallbackError, partition::PartitionSourcePin};

fn lookup(f: &Fixture, parent: &str) -> ReplyLookup {
    ReplyLookup {
        source: serde_json::from_value(json!({"inbox_id":"6".repeat(64),"body_sha256":"7".repeat(64),"work_pin":f.bridge.admission_pin().unwrap()})).unwrap(),
        owner: f.bridge.owner.clone(), issuer:f.bridge.current().unwrap().config.connector.issuer.clone(),
        bot_id:f.bridge.current().unwrap().config.connector.bot_id.clone(),
        platform:f.bridge.current().unwrap().config.connector.platform.clone(),
        chat_id:"original-chat".into(), parent_id:parent.into(),
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn confirmed_sent_reply_addressing_survives_restart_and_refuses_foreign_or_corrupt_evidence()
{
    let f = enabled_fixture().await;
    let original = store(&f).await;
    let item = retained_item(&f, "addressing-sent");
    seed(&original, &item).await;
    let lookup = lookup(&f, "platform-reply");
    assert!(original.confirmed_reply(&lookup).await.unwrap().is_none());
    assert!(original
        .deliver_one(
            "addressing-worker",
            &Delivered(std::sync::atomic::AtomicUsize::new(0))
        )
        .await
        .unwrap());
    assert!(original.confirmed_reply(&lookup).await.unwrap().is_some());
    let reopened = store(&f).await;
    assert!(reopened.confirmed_reply(&lookup).await.unwrap().is_some());
    for field in [
        "owner", "issuer", "bot", "platform", "chat", "parent", "bridge",
    ] {
        let mut wrong = lookup.clone();
        match field {
            "owner"=>wrong.owner = layer2_core::resource_access::ResourceOwner::Person {subject:layer2_core::SubjectId::new("another-owner")},
            "issuer"=>wrong.issuer="https://foreign.example".into(),
            "bot"=>wrong.bot_id="another-bot".into(),
            "platform"=>wrong.platform=layer2_core::bridge::BridgePlatform::Feishu,
            "chat"=>wrong.chat_id="another-chat".into(),
            "parent"=>wrong.parent_id="another-parent".into(),
            _=>wrong.source=serde_json::from_value::<PartitionSourcePin>(json!({"inbox_id":"6".repeat(64),"body_sha256":"7".repeat(64),"work_pin":{"bridge_id":"brg_foreign","config_revision":1}})).unwrap(),
        }
        assert!(
            reopened.confirmed_reply(&wrong).await.unwrap().is_none(),
            "{field}"
        );
    }
    reopened
        .items
        .update_one(
            doc! {"_id":&item.id},
            doc! {"$set":{"record.text":"corrupt retained text"}},
        )
        .await
        .unwrap();
    assert!(matches!(
        reopened.confirmed_reply(&lookup).await,
        Err(CallbackError::Unavailable)
    ));
    f.client.database(&f.database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn pending_claimed_sending_and_unknown_never_qualify_as_confirmed_reply_addressing() {
    let f = enabled_fixture().await;
    let store = store(&f).await;
    for (index, state) in ["pending", "claimed", "sending", "unknown"]
        .into_iter()
        .enumerate()
    {
        let mut item = retained_item(&f, &format!("addressing-{state}"));
        let now = now_ms();
        if index > 0 {
            let attempt = item
                .claim("old-worker".into(), now, now + 30_000, None)
                .unwrap();
            if index > 1 {
                item.begin_send("old-worker", &attempt, now).unwrap();
            }
            if index > 2 {
                item.complete(
                    &attempt,
                    DeliveryOutcome::Unknown(DeliveryFailure::WorkerLost),
                    now,
                )
                .unwrap();
            }
        }
        seed(&store, &item).await;
    }
    assert!(store
        .confirmed_reply(&lookup(&f, "platform-reply"))
        .await
        .unwrap()
        .is_none());
    assert_eq!(store.items.count_documents(doc! {}).await.unwrap(), 4);
    f.client.database(&f.database).drop().await.unwrap();
}
