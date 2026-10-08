#[path = "tools/consumer_cards.rs"]
mod consumer_cards;
#[allow(dead_code)]
#[path = "tools/consumer_support.rs"]
mod consumer_support;
#[path = "fake_cma.rs"]
pub mod fake_cma;
#[path = "fake_provider.rs"]
pub mod fake_provider;
#[path = "relay/support.rs"]
pub mod relay_support;
/// The media helpers the shared tools app is built with.
pub mod media {
    #[path = "fakes.rs"]
    pub mod fakes;
    #[path = "image_fixtures.rs"]
    pub mod image_fixtures;
    #[path = "support.rs"]
    pub mod support;
}
#[path = "tools/media_support.rs"]
pub mod media_support;
#[path = "../../cmaeg-core/tests/routing_policy/fixture.rs"]
pub mod routing_fixture;
#[path = "relay/signer.rs"]
pub mod signer;
#[allow(dead_code)]
#[path = "tools/support.rs"]
mod support;
use base64::Engine as _;
use cmaeg::{
    interactions::{Destination, Error, InteractionStore},
    providers::{ProviderClient, ProviderRegistry},
    records::RecordClient,
};
use cmaeg_core::{
    ids::EventId,
    message::OutboundMessage,
    seal::{seal_consumer_interaction, seal_interaction, MasterKeyRing},
};
use serde_json::json;
use std::{sync::Arc, time::Duration};

/// A default-only admission policy for these legacy-card contracts.
fn default_only(consumer: Option<&cmaeg_core::ids::ConsumerId>) -> bool {
    consumer.is_none()
}

#[tokio::test]
async fn callback_authority_survives_restart_and_fences_actor_chat_message_and_revision() {
    let app = support::App::new().await;
    let backend = fake_cma::FakeCma::start(false).await;
    let records = RecordClient::new(&backend.base_url, "cma-service").unwrap();
    let store = InteractionStore::new(
        records.clone(),
        app.state.keys.clone(),
        app.state.providers.clone(),
    );
    let event = app
        .event_ref
        .open(&app.state.keys, relay_support::now())
        .unwrap()
        .payload;
    let channel = app
        .state
        .resolver
        .resolve(event.channel_id.as_str())
        .await
        .unwrap();
    let message:OutboundMessage=serde_json::from_value(json!({"text":"Choose owner","blocks":[{"type":"choice","id":"owner","prompt":"Personal or org?","multi":false,"options":[{"id":"personal","label":"Personal","selected":true},{"id":"org","label":"Organization"}]},{"type":"link","url":"https://nyx.example/login","label":"Sign in"}]})).unwrap();
    let mut prepared = store.prepare(&channel, &event, &message).await.unwrap();
    let data = prepared.markup["inline_keyboard"][1][0]["callback_data"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(data.len() <= 64);
    assert_eq!(
        prepared.markup["inline_keyboard"][2][0]["url"],
        "https://nyx.example/login"
    );
    let reference = store
        .bind(
            &mut prepared,
            Destination {
                outbound_id: uuid::Uuid::new_v4().to_string(),
                platform_id: "42".into(),
            },
        )
        .await
        .unwrap();
    let snapshot = app
        .state
        .providers
        .event_context(
            &channel.definition.provider.slug,
            &channel.definition.provider.binding_id,
            &event.conversation_id,
            &event.event_id,
            channel
                .definition
                .secrets
                .provider_credential
                .as_ref()
                .unwrap(),
        )
        .await
        .unwrap();
    let mut activity = snapshot.activity;
    activity.event_id = EventId::new("click-1").unwrap();
    drop(store);
    let restarted =
        InteractionStore::new(records, app.state.keys.clone(), app.state.providers.clone());
    let mut impostor = activity.clone();
    impostor.actor.id = "another-user".into();
    assert!(restarted
        .consume(
            &channel,
            &impostor,
            "route-1",
            "query-1",
            &data,
            "42",
            &default_only
        )
        .await
        .is_err());
    impostor = activity.clone();
    impostor.conversation.id = "another-chat".into();
    assert!(restarted
        .consume(
            &channel,
            &impostor,
            "route-1",
            "query-1",
            &data,
            "42",
            &default_only
        )
        .await
        .is_err());
    assert!(restarted
        .consume(
            &channel,
            &activity,
            "route-1",
            "query-1",
            &data,
            "43",
            &default_only
        )
        .await
        .is_err());
    assert!(restarted
        .consume(
            &channel,
            &activity,
            "another-route",
            "query-1",
            &data,
            "42",
            &default_only
        )
        .await
        .is_err());
    let (a, b) = tokio::join!(
        restarted.consume(
            &channel,
            &activity,
            "route-1",
            "query-1",
            &data,
            "42",
            &default_only
        ),
        restarted.consume(
            &channel,
            &activity,
            "route-1",
            "query-1",
            &data,
            "42",
            &default_only
        )
    );
    let first = a.unwrap();
    assert_eq!(first.0, b.unwrap().0);
    assert_eq!(first.0.values["selected"], json!(["org"]));
    assert_eq!(first.0.values["message_ref"], reference.as_str());
    assert_eq!(first.0.block_id, "owner");
    assert_eq!(first.0.action_id, "org");
    assert!(restarted
        .consume(
            &channel,
            &activity,
            "route-1",
            "query-2",
            &data,
            "42",
            &default_only
        )
        .await
        .is_err());
    let payload = reference.open(&app.state.keys).unwrap().payload;
    let next = restarted
        .replacement(&channel, &payload, &message)
        .await
        .unwrap();
    assert_eq!(next.card.revision, 2);
    assert!(restarted
        .consume(
            &channel,
            &activity,
            "route-1",
            "query-1",
            &data,
            "42",
            &default_only
        )
        .await
        .is_err());
    // The opaque encrypted record is namespace- and ID-bound, and never stores plain UI/actor values.
    for record in backend.records().values() {
        assert!(record.blob.starts_with(b"cmaeg1.in."));
        assert!(!String::from_utf8_lossy(&record.blob).contains("Choose owner"));
    }
}

/// A card store over a fake CMA alone: loading a card needs no provider.
async fn card_store(keys: Arc<MasterKeyRing>) -> (fake_cma::FakeCma, InteractionStore) {
    let backend = fake_cma::FakeCma::start(false).await;
    let providers = ProviderClient::new(
        ProviderRegistry::default(),
        Duration::from_secs(1),
        Duration::from_secs(1),
        1,
    )
    .unwrap();
    let store = InteractionStore::new(
        RecordClient::new(&backend.base_url, "cma-service").unwrap(),
        keys,
        Arc::new(providers),
    );
    (backend, store)
}

fn card_json(id: &str, consumer: Option<&str>) -> Vec<u8> {
    let mut card = json!({
        "id": id, "owner": "nyx-user-1", "channel_id": "cmaeg1.ch.cards",
        "event_id": "message-1", "route_id": "route-1", "actor": "7", "chat": "42",
        "thread": null, "expires_at": 4_102_444_800_i64, "revision": 1,
        "message": {"text": "Choose", "blocks": []}, "destination": null,
        "reference": null, "consumed": null, "superseded": false
    });
    if let Some(consumer) = consumer {
        card["consumer"] = consumer.into();
    }
    serde_json::to_vec(&card).unwrap()
}

#[tokio::test]
async fn cards_load_only_when_their_sealed_kind_agrees_with_their_owner() {
    let keys = Arc::new(
        MasterKeyRing::parse(&format!(
            "1:{}",
            base64::engine::general_purpose::STANDARD.encode([2; 32])
        ))
        .unwrap(),
    );
    let (backend, store) = card_store(keys.clone()).await;
    let in_kind = |id: &str, bytes: &[u8]| seal_interaction(&keys, id, bytes).unwrap();
    let ic_kind = |id: &str, bytes: &[u8]| seal_consumer_interaction(&keys, id, bytes).unwrap();
    for (name, consumer, consumer_kind, loads) in [
        ("default card as in", None, false, true),
        ("consumer card as ic", Some("csm_00000000000a"), true, true),
        (
            "consumer card as in",
            Some("csm_00000000000a"),
            false,
            false,
        ),
        ("default card as ic", None, true, false),
        ("card naming default as ic", Some("default"), true, false),
        ("card naming default as in", Some("default"), false, false),
    ] {
        let id = uuid::Uuid::new_v4().to_string();
        let bytes = card_json(&id, consumer);
        let blob = if consumer_kind {
            ic_kind(&id, &bytes)
        } else {
            in_kind(&id, &bytes)
        };
        backend.seed(&id, "nyx-user-1", blob);
        let loaded = store.load(&id).await;
        assert_eq!(loaded.is_ok(), loads, "{name}");
        match loaded {
            Ok((card, _)) => assert_eq!(
                card.consumer.as_ref().map(|consumer| consumer.as_str()),
                consumer,
                "{name}"
            ),
            Err(error) => assert!(matches!(error, Error::Denied), "{name}"),
        }
    }
    backend.close().await;
}
