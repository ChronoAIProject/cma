//! Replacing an interactive card on a channel with several relays: the
//! card's own route decides whether it can be replaced, never the channel's
//! first chat source. Legacy channels keep the channel-level answer.
use super::{
    fake_cma::FakeCma,
    fake_http::FakeHttp,
    fake_provider::FakeProvider,
    recording_fixtures::{event, keys, relay_activity, relay_channel, relay_reply},
    recording_interactive::seed,
    recording_support::{Hooks, Recording},
};
use cmaeg::{
    channels::resolver::ResolvedChannel,
    delivery::intents::{DeliveryOutcome, ReplySource},
    interactions::InteractionStore,
    providers::{ProviderClient, ProviderRegistry},
    records::RecordClient,
    DeliveryConfig,
};
use cmaeg_core::{channel::PartitionKey, ids::EventId, seal::MessageRefPayload};
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};

/// The relay the fixture activity arrives on: Telegram, route `route-1`.
fn telegram_relay() -> Value {
    json!({"type":"nyxid_relay","issuer":"https://nyx.example","jwks_url":"https://nyx.example/jwks",
        "key_id":"key-1","route_ids":["route-1"],"platform":"telegram","admission":{"type":"all"}})
}

fn lark_relay() -> Value {
    json!({"type":"nyxid_relay","issuer":"https://nyx.example","jwks_url":"https://nyx.example/jwks",
        "key_id":"key-1","route_ids":["route-lark"],"platform":"lark","admission":{"type":"all"}})
}

/// The recording relay channel with `sources` instead of its single relay.
fn channel_with(sources: Value) -> Arc<ResolvedChannel> {
    let base = relay_channel(0);
    let mut definition = base.definition.clone();
    definition.sources = serde_json::from_value(sources).unwrap();
    // The recording fixture is never restored, so it leaves the digest and
    // the partition as the custom fixture had them; a reader checks both.
    definition.profile_digest = definition.profile.digest();
    definition.partition.key = PartitionKey::ConversationAndSender;
    definition.validate().unwrap();
    Arc::new(ResolvedChannel {
        id: base.id.clone(),
        pointer: base.pointer.clone(),
        definition,
        agent_key_hash: base.agent_key_hash,
        consumer_keys: base.consumer_keys.clone(),
        loaded_at: base.loaded_at,
        version: base.version,
    })
}

const CHOICE: &str = r#"{"text":"Choose","blocks":[{"type":"buttons","id":"choice","options":[{"id":"first","label":"First"}]}]}"#;
const CHOSEN: &str = r#"{"text":"Chosen: First"}"#;

fn providers(base_url: &str) -> Arc<ProviderClient> {
    let registry = ProviderRegistry::parse(&json!([{"slug":"cma_codex","base_url":base_url,"kind":"responses_http","paths":{"agent_card":"/.well-known/agent-card.json","binding":"/api/v1/cmaeg/bindings/{binding_id}","conversation":"/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}","responses":"/api/v2/responses"},"max_inflight":8}]).to_string()).unwrap();
    Arc::new(
        ProviderClient::new(registry, Duration::from_secs(1), Duration::from_secs(2), 8).unwrap(),
    )
}

/// Send a card through the Telegram relay, click it and replace it,
/// returning the replacement's delivery outcome.
async fn replace_card(sources: Value) -> DeliveryOutcome {
    let provider = FakeProvider::start(Default::default()).await;
    let backend = FakeCma::start(false).await;
    let keys = keys();
    let providers = providers(provider.base_url());
    let channel = channel_with(sources);
    let activity = relay_activity();
    let event = event(&channel, &activity, 1, relay_reply());
    seed(&providers, &keys, &channel, &activity, &event).await;
    let store = Arc::new(InteractionStore::new(
        RecordClient::new(&backend.base_url, "cma-service").unwrap(),
        keys.clone(),
        providers,
    ));
    let mut recording = Recording::start_with(
        FakeHttp::platforms(),
        Hooks::answering([]),
        DeliveryConfig::default(),
        Some(store.clone()),
        false,
    );
    let card = recording
        .reply(
            &channel,
            &event,
            serde_json::from_str(CHOICE).unwrap(),
            ReplySource::Tool,
        )
        .await;
    let DeliveryOutcome::Confirmed {
        platform_message_id: Some(platform_id),
    } = &card.delivery
    else {
        panic!("the interactive relay reply failed: {:?}", card.delivery);
    };
    let markup = &recording.http.sent()[0].json()["reply"]["metadata"]["reply_markup"];
    let data = markup["inline_keyboard"][0][0]["callback_data"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut click = activity.clone();
    click.event_id = "click-1".parse().unwrap();
    store
        .consume(
            &channel,
            &click,
            "route-1",
            "callback-1",
            &data,
            platform_id,
            &|consumer| consumer.is_none(),
        )
        .await
        .unwrap();
    let reference = card.message_ref.unwrap().open(&keys).unwrap().payload;
    let receipt = recording
        .update(&channel, reference, serde_json::from_str(CHOSEN).unwrap())
        .await;
    recording.finish().await;
    receipt.delivery
}

#[tokio::test]
async fn a_telegram_relay_card_is_replaced_on_a_channel_whose_first_relay_is_lark() {
    let delivery = replace_card(json!([lark_relay(), telegram_relay()])).await;
    assert!(
        matches!(delivery, DeliveryOutcome::Confirmed { .. }),
        "{delivery:?}"
    );
}

#[tokio::test]
async fn a_single_telegram_relay_still_replaces_its_cards() {
    let delivery = replace_card(json!([telegram_relay()])).await;
    assert!(
        matches!(delivery, DeliveryOutcome::Confirmed { .. }),
        "{delivery:?}"
    );
}

/// A replacement of a card reference naming `route`. The store holds no
/// card, so a replacement that passes the route check is rejected by the
/// store, while one the check refuses is unsupported before any store access.
async fn replace_unknown(sources: Value, route: &str) -> DeliveryOutcome {
    let backend = FakeCma::start(false).await;
    let channel = channel_with(sources);
    let mut recording = Recording::start_with(
        FakeHttp::platforms(),
        Hooks::answering([]),
        DeliveryConfig::default(),
        Some(Arc::new(InteractionStore::new(
            RecordClient::new(&backend.base_url, "cma-service").unwrap(),
            keys(),
            providers("http://127.0.0.1:9"),
        ))),
        false,
    );
    let reference = MessageRefPayload {
        discord_message: None,
        native_lark_effect: None,
        whatsapp_reaction: None,
        lark_effect: None,
        telegram_effect: None,
        channel_id: channel.id.clone(),
        event_id: EventId::new("msg-1").unwrap(),
        platform_message_id: "100".into(),
        route_id: route.into(),
        interaction_record_id: Some("card-1".into()),
        relay_message: None,
        consumer: None,
    };
    let receipt = recording
        .update(&channel, reference, serde_json::from_str(CHOSEN).unwrap())
        .await;
    recording.finish().await;
    receipt.delivery
}

#[tokio::test]
async fn only_a_route_owned_by_a_telegram_relay_can_replace_a_card() {
    let several = json!([telegram_relay(), lark_relay()]);
    // With several relays, a Lark route is refused before any store access,
    // even though the channel's first chat source is a Telegram relay.
    let delivery = replace_unknown(several.clone(), "route-lark").await;
    assert_eq!(delivery, DeliveryOutcome::Unsupported);
    // The Telegram relay's own route reaches the store, which has no card.
    let delivery = replace_unknown(several, "route-1").await;
    assert_eq!(
        delivery,
        DeliveryOutcome::Rejected {
            reason: "interaction_expired_or_invalid".into()
        }
    );
    // A legacy Lark channel keeps its channel-level refusal.
    let delivery = replace_unknown(json!([lark_relay()]), "route-lark").await;
    assert_eq!(delivery, DeliveryOutcome::Unsupported);
}
