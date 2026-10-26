use super::{
    journal_fixture::*,
    journal_model::{Inbox, Lease},
    protocol::checkpoint,
    Checkpoint, Dispatch, GatewayError, NativeDiscordJournal,
};
use crate::{
    channels::{
        definition::build_update,
        write_gate::{check, WriteFlags},
        ChannelUpdateRequest,
    },
    connections::store::index_id,
};
use cmaeg_core::channel::{Secret, Source};
use serde_json::json;

fn ready(session: &str) -> (Dispatch, Checkpoint) {
    let event = Dispatch {
        name: "READY".into(),
        sequence: 0,
        data: json!({"session_id":session,"resume_gateway_url":"wss://gateway.discord.gg/","user":{"id":"77","bot":true}}),
    };
    let saved = checkpoint(None, &event, "77").unwrap();
    (event, saved)
}
fn message(f: &Fixture, sequence: u64, text: &str) -> Dispatch {
    Dispatch {
        name: "MESSAGE_CREATE".into(),
        sequence,
        data: json!({"id":f.message_id(),"channel_id":"99","author":{"id":"42"},"content":text}),
    }
}

#[test]
fn native_source_credentials_and_gate_preserve_legacy_bytes() {
    let native = definition();
    assert_eq!(
        check(None, &native, WriteFlags::default())
            .unwrap_err()
            .message,
        "native_discord_writes_disabled"
    );
    check(
        None,
        &native,
        WriteFlags {
            native_discord: true,
            ..Default::default()
        },
    )
    .unwrap();
    check(Some(&native), &native, WriteFlags::default()).unwrap();
    assert!(!native.to_public_json().to_string().contains(TOKEN));
    assert!(!format!("{native:?}").contains(TOKEN));
    assert!(native.to_blob_json().to_string().contains(TOKEN));
    let mut rotated = native.clone();
    rotated.secrets.discord_bot_token = Some(Secret::new("rotated-fixture"));
    assert!(check(Some(&native), &rotated, WriteFlags::default()).is_err());
    let mut invalid = native.clone();
    invalid.secrets.discord_bot_token = None;
    assert!(invalid.validate().is_err());
    let Source::Discord { intents, .. } = &mut invalid.sources[0] else {
        unreachable!()
    };
    *intents = 512;
    assert!(invalid.validate().is_err());
    let mut legacy = native;
    legacy.sources = vec![Source::Custom {
        auth: cmaeg_core::channel::CustomAuth::NyxidKey {
            key_hashes: vec![legacy.secrets.agent_key_hash.clone()],
        },
        prompt_template: None,
        schema: None,
        attachments: None,
    }];
    legacy.secrets.discord_bot_token = None;
    assert!(legacy.to_blob_json()["secrets"]
        .get("discord_bot_token")
        .is_none());
    let again: cmaeg_core::channel::ChannelDefinition =
        serde_json::from_value(legacy.to_blob_json()).unwrap();
    assert_eq!(again.to_blob_json(), legacy.to_blob_json());
    let mut body = native_update(&definition());
    body["credentials"] = json!({"discord_bot_token":"rotated-fixture"});
    let request: ChannelUpdateRequest = serde_json::from_value(body).unwrap();
    assert!(build_update(&request, &definition(), WriteFlags::default()).is_err());
    let mut removal = native_update(&definition());
    removal["sources"] = serde_json::to_value(&legacy.sources).unwrap();
    let removal: ChannelUpdateRequest = serde_json::from_value(removal).unwrap();
    let removed = build_update(&removal, &definition(), WriteFlags::default()).unwrap();
    assert!(removed.secrets.discord_bot_token.is_none());
    assert!(removed.to_blob_json()["secrets"]
        .get("discord_bot_token")
        .is_none());
}
fn native_update(definition: &cmaeg_core::channel::ChannelDefinition) -> serde_json::Value {
    let consumer = definition.consumer(None).unwrap();
    json!({"schema_version":1,"name":definition.name,"profile":consumer.profile,"provider":{"slug":consumer.provider.slug},"sources":definition.sources,"partition":definition.partition,"reply":definition.reply,"expected_version":1})
}

#[tokio::test]
async fn verified_bot_lease_is_exclusive_and_stale_workers_are_fenced() {
    let f = Fixture::new().await;
    f.memory.user.lock().unwrap()["bot"] = false.into();
    assert!(matches!(f.acquire().await, Err(GatewayError::Authority)));
    assert!(f.memory.rows.lock().unwrap().is_empty());
    f.memory.user.lock().unwrap()["bot"] = true.into();
    let mut unready = definition();
    unready.creation = Some(cmaeg_core::channel::ChannelCreation {
        channel_id: cmaeg_core::ids::ChannelId::new("cmaeg1.ch.discord-fixture").unwrap(),
        fingerprint: "fixture".into(),
        ready: false,
    });
    f.replace(&unready);
    assert!(matches!(f.acquire().await, Err(GatewayError::Authority)));
    f.replace(&definition());
    let first = f.acquire().await.unwrap();
    assert!(matches!(f.acquire().await, Err(GatewayError::Authority)));
    f.advance(30_001);
    let second = f.acquire().await.unwrap();
    assert!(first.renew().await.is_err());
    assert!(first.release().await.is_err());
    second.renew().await.unwrap();
    let (lease, _) = f
        .store
        .load::<Lease>(&second.id, false)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(lease.epoch, 2);
    let all = f
        .memory
        .rows
        .lock()
        .unwrap()
        .values()
        .flat_map(|r| r.blob.clone())
        .collect::<Vec<_>>();
    assert!(!String::from_utf8_lossy(&all).contains(TOKEN));
}

#[tokio::test]
async fn identify_quota_survives_restarts_and_resume_does_not_spend_it() {
    let f = Fixture::new().await;
    let first = f.acquire().await.unwrap();
    first.authorize_connection(None).await.unwrap();
    assert!(first.authorize_connection(None).await.is_err());
    let (event, saved) = ready("retained");
    first.admit(&saved, &event).await.unwrap();
    first.release().await.unwrap();
    let second = f.acquire().await.unwrap();
    second.authorize_connection(Some(&saved)).await.unwrap();
    assert_eq!(
        f.store
            .load::<Lease>(&second.id, false)
            .await
            .unwrap()
            .unwrap()
            .0
            .remaining,
        9
    );
    second.release().await.unwrap();
    let third = f.acquire().await.unwrap();
    assert_eq!(
        third.authorize_connection(None).await,
        Err(GatewayError::RateLimited)
    );
    f.advance(5_001);
    third.authorize_connection(None).await.unwrap();
    assert_eq!(
        f.store
            .load::<Lease>(&third.id, false)
            .await
            .unwrap()
            .unwrap()
            .0
            .remaining,
        8
    );
}

#[tokio::test]
async fn raw_message_is_durable_before_checkpoint_and_replay_preserves_original_snapshot() {
    let f = Fixture::new().await;
    let journal = f.acquire().await.unwrap();
    journal.authorize_connection(None).await.unwrap();
    let (event, mut saved) = ready("original");
    journal.admit(&saved, &event).await.unwrap();
    let message = message(&f, 1, &"x".repeat(100_000));
    saved.sequence = 1;
    let id = index_id(
        "discord-native-message-v1",
        &[&f.record, "77", message.message_id().unwrap()],
    );
    f.fail(&id, false);
    assert_eq!(
        journal.admit(&saved, &message).await,
        Err(GatewayError::Unavailable)
    );
    assert_eq!(journal.checkpoint().await.unwrap().unwrap().sequence, 0);
    f.fail(&id, true);
    journal.admit(&saved, &message).await.unwrap();
    assert_eq!(journal.checkpoint().await.unwrap().unwrap().sequence, 1);
    let (original, _) = f.store.load::<Inbox>(&id, true).await.unwrap().unwrap();
    assert!(original.parts.len() > 1);
    let mut changed = definition();
    changed.name = "renamed after receipt".into();
    f.replace(&changed);
    let replay = Dispatch {
        sequence: 2,
        data: json!({"id":message.message_id(),"content":"different redelivery"}),
        ..message
    };
    saved.sequence = 2;
    journal.admit(&saved, &replay).await.unwrap();
    let (retained, _) = f.store.load::<Inbox>(&id, true).await.unwrap().unwrap();
    assert_eq!(retained.payload_hash, original.payload_hash);
    assert_eq!(retained.parts, original.parts);
    assert_eq!(retained.received_at, original.received_at);
}

#[tokio::test]
async fn checkpoint_failure_and_credential_revocation_do_not_lose_or_repeat_inbox() {
    let f = Fixture::new().await;
    let journal = f.acquire().await.unwrap();
    journal.authorize_connection(None).await.unwrap();
    let (event, mut saved) = ready("failed-cas");
    journal.admit(&saved, &event).await.unwrap();
    let message = message(&f, 1, "hello");
    saved.sequence = 1;
    f.fail(&journal.id, false);
    assert_eq!(
        journal.admit(&saved, &message).await,
        Err(GatewayError::Unavailable)
    );
    assert_eq!(journal.checkpoint().await.unwrap().unwrap().sequence, 0);
    journal.admit(&saved, &message).await.unwrap();
    assert_eq!(journal.checkpoint().await.unwrap().unwrap().sequence, 1);
    let mut changed = definition();
    changed.secrets.discord_bot_token = Some(Secret::new("rotated-fixture"));
    f.replace(&changed);
    assert_eq!(journal.renew().await, Err(GatewayError::Authority));
    assert_eq!(
        journal.admit(&saved, &message).await,
        Err(GatewayError::Authority)
    );
}

#[tokio::test]
async fn discovery_refuses_untrusted_gateway_and_shards_without_claiming() {
    let f = Fixture::new().await;
    f.memory.gateway.lock().unwrap()["url"] = "wss://attacker.example/".into();
    assert!(matches!(f.acquire().await, Err(GatewayError::Protocol)));
    f.memory.gateway.lock().unwrap()["url"] = "wss://gateway.discord.gg/".into();
    f.memory.gateway.lock().unwrap()["shards"] = 2.into();
    assert!(matches!(
        f.acquire().await,
        Err(GatewayError::InvalidConfiguration)
    ));
    assert!(f.memory.rows.lock().unwrap().is_empty());
}

#[tokio::test]
async fn expired_native_ids_and_missing_chunks_never_advance_checkpoint() {
    let f = Fixture::new().await;
    let journal = f.acquire().await.unwrap();
    journal.authorize_connection(None).await.unwrap();
    let (event, mut saved) = ready("bounded-replay");
    journal.admit(&saved, &event).await.unwrap();
    let mut message = message(&f, 1, "hello");
    message.data["id"] = "77".into();
    saved.sequence = 1;
    assert_eq!(
        journal.admit(&saved, &message).await,
        Err(GatewayError::Protocol)
    );
    assert_eq!(journal.checkpoint().await.unwrap().unwrap().sequence, 0);
    let future = f.message_id().parse::<u64>().unwrap() + (2000_u64 << 22);
    message.data["id"] = future.to_string().into();
    assert_eq!(
        journal.admit(&saved, &message).await,
        Err(GatewayError::Unavailable)
    );
    message.data["id"] = f.message_id().into();
    journal.admit(&saved, &message).await.unwrap();
    let id = index_id(
        "discord-native-message-v1",
        &[&f.record, "77", message.message_id().unwrap()],
    );
    let (inbox, _) = f.store.load::<Inbox>(&id, true).await.unwrap().unwrap();
    f.memory.rows.lock().unwrap().remove(&inbox.parts[0]);
    saved.sequence = 2;
    message.sequence = 2;
    assert_eq!(
        journal.admit(&saved, &message).await,
        Err(GatewayError::Unavailable)
    );
    assert_eq!(journal.checkpoint().await.unwrap().unwrap().sequence, 1);
}

#[tokio::test]
async fn observed_quota_cannot_refill_a_reserved_budget_until_reset() {
    let f = Fixture::new().await;
    f.memory.gateway.lock().unwrap()["session_start_limit"]["remaining"] = 1.into();
    let first = f.acquire().await.unwrap();
    first.authorize_connection(None).await.unwrap();
    first.release().await.unwrap();
    f.advance(5001);
    f.memory.gateway.lock().unwrap()["session_start_limit"]["remaining"] = 10.into();
    let second = f.acquire().await.unwrap();
    assert_eq!(
        second.authorize_connection(None).await,
        Err(GatewayError::RateLimited)
    );
    second.release().await.unwrap();
    f.advance(86_400_001);
    let third = f.acquire().await.unwrap();
    third.authorize_connection(None).await.unwrap();
    assert_eq!(
        f.store
            .load::<Lease>(&third.id, false)
            .await
            .unwrap()
            .unwrap()
            .0
            .remaining,
        9
    );
}
