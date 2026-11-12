use super::*;
use crate::auth::actor_links::{
    ActorActivation, ActorBrowserConsent, ActorLinkOrigin, ActorWakeup, ActorWakeupState,
    EventActor,
};

fn record() -> ActorLinkRecord {
    let actor = EventActor {
        binding_id: "binding".into(),
        channel_id: "channel".into(),
        conversation_id: "source-chat".into(),
        actor_id: "actor-secret".into(),
        platform: "telegram".into(),
    };
    let key = SecretDigest::identify("event-actor-link", &serde_json::to_string(&actor).unwrap())
        .as_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    ActorLinkRecord {
        actor_key: key,
        actor,
        link_id: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
        revision: 3,
        expires_at: 100,
        subject: Some(SubjectId::new("subject")),
        incarnation: Some("incarnation".into()),
        profile: Profile::default(),
        confirmation_digest: None,
        confirmation_failures: 0,
        active: true,
        revoked: false,
        renewed_at: 10,
        activation: ActorActivation::BrowserConsent,
        browser_consent: Some(ActorBrowserConsent {
            attempt_digest: "a".repeat(64),
            browser_digest: "b".repeat(64),
            csrf_token: "c".repeat(32),
            review_revision: 2,
            approved_at: Some(10),
        }),
        origin: Some(ActorLinkOrigin {
            binding_generation: 7,
            provider_conversation_id: "provider-conversation".into(),
            agent_id: layer2_core::AgentId("agent".into()),
            worktree_id: layer2_core::WorktreeId("worktree".into()),
            source_event: layer2_core::sandbox::agent_io::ExternalEventContext {
                source_id: "channel".into(),
                conversation_id: "source-chat".into(),
                provider_conversation_id: Some("provider-conversation".into()),
                event_id: "event".into(),
                reference: "sealed-event-secret".into(),
                payload: serde_json::json!({"text":"source-text-secret"}),
            },
            reply_target_ref: "reply-target-secret".into(),
            reply_expires_at: 1000,
        }),
        wakeup: Some(ActorWakeup {
            notification_id: "d".repeat(64),
            state: ActorWakeupState::Pending { retry_at: 10 },
        }),
    }
}

#[tokio::test]
async fn activated_delegation_and_pending_notification_are_one_encrypted_document() {
    let store = super::super::tests::store().await;
    let row = record();
    let doc = store.actor_link_doc(&row).unwrap();
    assert_eq!(doc.wakeup_due_at, Some(10));
    let diagnostic = format!("{doc:?}");
    for secret in [
        "actor-secret",
        "sealed-event-secret",
        "source-text-secret",
        "reply-target-secret",
    ] {
        assert!(!diagnostic.contains(secret));
    }
    let restored = store.decode_actor_link(doc).unwrap();
    assert_eq!(
        serde_json::to_value(&restored).unwrap(),
        serde_json::to_value(&row).unwrap()
    );
    let mut corrupted = store.actor_link_doc(&row).unwrap();
    corrupted.wakeup_due_at = Some(11);
    assert!(store.decode_actor_link(corrupted).is_err());
}

#[test]
fn old_chat_proof_records_keep_their_activation_semantics() {
    let mut value = serde_json::to_value(record()).unwrap();
    let object = value.as_object_mut().unwrap();
    for field in ["activation", "browser_consent", "origin", "wakeup"] {
        object.remove(field);
    }
    object.insert(
        "confirmation_digest".into(),
        serde_json::json!("e".repeat(64)),
    );
    let row: ActorLinkRecord = serde_json::from_value(value).unwrap();
    row.validate().unwrap();
    assert_eq!(row.activation, ActorActivation::LegacyChatProof);
    assert!(row.wakeup.is_none() && row.browser_consent.is_none());
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_actor_consent_outbox_survives_reopen_and_fences_competing_claims() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").expect("disposable replica set required");
    let client = Client::with_uri_str(uri).await.unwrap();
    let name = format!("cma_actor_outbox_{}", uuid::Uuid::new_v4().simple());
    let key =
        AuthEncryptionKey::from_base64("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=").unwrap();
    let open = || MongoAuthStore::open(&client, &name, "actor-outbox-test", &key);
    let first = open().await.unwrap();
    let second = open().await.unwrap();
    let mut original = record();
    original.revision = 1;
    original.browser_consent.as_mut().unwrap().review_revision = 1;
    assert!(first
        .replace_actor_link(None, original.clone())
        .await
        .unwrap());
    let pending = second.due_actor_wakeups(10, 100).await.unwrap();
    assert_eq!(pending.len(), 1);
    assert!(
        pending[0].active
            && pending[0]
                .browser_consent
                .as_ref()
                .unwrap()
                .approved_at
                .is_some()
    );
    let claim = |holder: &str| {
        let mut row = original.clone();
        row.revision += 1;
        row.wakeup.as_mut().unwrap().state = ActorWakeupState::Claimed {
            holder: holder.into(),
            lease_until: 100,
        };
        row
    };
    let (a, b) = tokio::join!(
        first.replace_actor_link(Some(1), claim("one")),
        second.replace_actor_link(Some(1), claim("two"))
    );
    assert_ne!(a.unwrap(), b.unwrap());
    assert!(first.due_actor_wakeups(99, 100).await.unwrap().is_empty());
    drop(first);
    drop(second);
    let reopened = open().await.unwrap();
    let mut due = reopened.due_actor_wakeups(101, 100).await.unwrap();
    assert_eq!(due.len(), 1);
    let mut acknowledged = due.remove(0);
    assert_eq!(
        acknowledged.origin.as_ref().unwrap().reply_target_ref,
        "reply-target-secret"
    );
    assert_eq!(acknowledged.revision, 2);
    acknowledged.revision += 1;
    acknowledged.wakeup.as_mut().unwrap().state = ActorWakeupState::Acknowledged {
        receipt: "delivery-receipt".into(),
    };
    assert!(reopened
        .replace_actor_link(Some(2), acknowledged)
        .await
        .unwrap());
    assert!(reopened
        .due_actor_wakeups(1000, 100)
        .await
        .unwrap()
        .is_empty());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_actor_renewal_pages_follow_actor_key_without_changing_due_rows() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").expect("disposable replica set required");
    let client = Client::with_uri_str(uri).await.unwrap();
    let name = format!("cma_actor_renewal_{}", uuid::Uuid::new_v4().simple());
    let key =
        AuthEncryptionKey::from_base64("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=").unwrap();
    let store = MongoAuthStore::open(&client, &name, "actor-renewal-test", &key)
        .await
        .unwrap();
    let mut records = Vec::new();
    for index in 0..3 {
        let mut row = record();
        row.actor.actor_id = format!("renewal-sender-{index}");
        row.actor_key = SecretDigest::identify(
            "event-actor-link",
            &serde_json::to_string(&row.actor).unwrap(),
        )
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
        row.link_id = format!("{index:032x}");
        row.revision = 1;
        row.browser_consent.as_mut().unwrap().review_revision = 1;
        records.push(row);
    }
    records.sort_by(|left, right| left.actor_key.cmp(&right.actor_key));
    for (index, row) in records.iter_mut().enumerate() {
        row.renewed_at = (30 - index * 10) as u64;
        assert!(store.replace_actor_link(None, row.clone()).await.unwrap());
    }
    let page = store.due_actor_links(100, None, 2).await.unwrap();
    let next = store
        .due_actor_links(100, page.last().map(|row| row.actor_key.as_str()), 2)
        .await
        .unwrap();
    let empty = store
        .due_actor_links(100, next.last().map(|row| row.actor_key.as_str()), 2)
        .await
        .unwrap();
    let restarted = store.due_actor_links(100, None, 100).await.unwrap();
    let observed = page.iter().map(|row| &row.actor_key).collect::<Vec<_>>();
    let expected = records
        .iter()
        .take(2)
        .map(|row| &row.actor_key)
        .collect::<Vec<_>>();
    client.database(&name).drop().await.unwrap();
    assert_eq!(observed, expected);
    assert_eq!(
        next.iter().map(|row| &row.actor_key).collect::<Vec<_>>(),
        [&records[2].actor_key]
    );
    assert!(empty.is_empty());
    assert_eq!(
        serde_json::to_value(restarted).unwrap(),
        serde_json::to_value(records).unwrap()
    );
}
