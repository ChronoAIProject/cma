//! Sequence (h) of the legacy delivery recording: an interactive relay reply,
//! a click on it and its `updateMessage` replacement, then a native Telegram
//! reply with controls.
//!
//! The interaction store reads the provider's event context and writes its
//! cards over sockets, and under the paused clock pending socket I/O lets
//! tokio advance time arbitrarily. This sequence therefore runs in real time
//! and records no offsets; its calls, their order, bodies and token classes
//! are pinned like every other sequence's.
use super::{
    fake_cma::FakeCma,
    fake_http::FakeHttp,
    fake_provider::FakeProvider,
    recording_fixtures::{
        activity, event, keys, native_channel, relay_activity, relay_channel, relay_reply,
    },
    recording_support::{Hooks, Recording},
};
use cmaeg::{
    channels::resolver::ResolvedChannel,
    delivery::intents::{DeliveryOutcome, ReplySource},
    interactions::InteractionStore,
    providers::{EventContextSnapshot, ProviderClient, ProviderRegistry, TurnRequest},
    records::RecordClient,
    DeliveryConfig,
};
use cmaeg_core::{
    activity::{Activity, SourceKind},
    channel::Platform,
    ids::IdempotencyKey,
    seal::{EventRef, EventRefPayload, MasterKeyRing, ReplyAddress},
};
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};

pub async fn record() -> Value {
    record_with(false).await
}

async fn record_with(complete_replies: bool) -> Value {
    let provider = FakeProvider::start(Default::default()).await;
    let backend = FakeCma::start(false).await;
    let keys = keys();
    let registry = ProviderRegistry::parse(&json!([{"slug":"cma_codex","base_url":provider.base_url(),"kind":"responses_http","paths":{"agent_card":"/.well-known/agent-card.json","binding":"/api/v1/cmaeg/bindings/{binding_id}","conversation":"/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}","responses":"/api/v2/responses"},"max_inflight":8}]).to_string()).unwrap();
    let providers = Arc::new(
        ProviderClient::new(registry, Duration::from_secs(1), Duration::from_secs(2), 8).unwrap(),
    );
    let relay = relay_channel(0);
    let relay_activity = relay_activity();
    let relay_event = event(&relay, &relay_activity, 1, relay_reply());
    let native = native_channel();
    let telegram = SourceKind::Telegram {
        platform: Platform::Telegram,
        bot_id: "77".into(),
    };
    let native_activity = activity(telegram, "msg-2");
    let native_event = event(
        &native,
        &native_activity,
        2,
        ReplyAddress::Telegram {
            bot_id: "77".into(),
            chat_id: "42".into(),
            message_id: "5".into(),
            thread_id: None,
        },
    );
    seed(&providers, &keys, &relay, &relay_activity, &relay_event).await;
    seed(&providers, &keys, &native, &native_activity, &native_event).await;
    let store = Arc::new(InteractionStore::new(
        RecordClient::new(&backend.base_url, "cma-service").unwrap(),
        keys.clone(),
        providers,
    ));
    let mut recording = Recording::start_with_relay(
        FakeHttp::platforms(),
        Hooks::answering([]),
        DeliveryConfig::default(),
        Some(store.clone()),
        false,
        |relay| relay.with_complete_replies(complete_replies),
    );
    let buttons = json!({"text":"Choose","blocks":[{"type":"buttons","id":"choice","options":[{"id":"first","label":"First"},{"id":"second","label":"Second"}]}]});
    let card = recording
        .reply(&relay, &relay_event, buttons.clone(), ReplySource::Tool)
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
    let mut click = relay_activity.clone();
    click.event_id = "click-1".parse().unwrap();
    store
        .consume(
            &relay,
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
    if complete_replies {
        assert_long_refusals(&recording, &relay, &relay_event, &reference).await;
    }
    recording
        .update(
            &relay,
            reference,
            json!({"text":"Chosen: First","blocks":[{"type":"buttons","id":"choice","options":[{"id":"again","label":"Again"}]}]}),
        )
        .await;
    recording
        .reply(&native, &native_event, buttons, ReplySource::Tool)
        .await;
    recording.finish().await
}

/// Start one provider turn carrying `event`'s context, so the provider can
/// answer the interaction store's context lookup for it.
pub async fn seed(
    providers: &ProviderClient,
    keys: &MasterKeyRing,
    channel: &ResolvedChannel,
    activity: &Activity,
    event: &EventRefPayload,
) {
    let consumer = channel.consumer(event.consumer.as_ref()).unwrap();
    let mut stream = providers
        .start_turn(
            &consumer.provider.slug,
            TurnRequest {
                nyxid_trace: false,
                timing: None,
                conversation_id: event.conversation_id.clone(),
                input: vec![],
                metadata: Default::default(),
                idempotency_key: IdempotencyKey::from_event(&activity.event_id),
                agent_key: consumer.credential.unwrap().clone(),
                event_context: Some(EventContextSnapshot {
                    schema_version: 1,
                    channel_id: channel.id.clone(),
                    conversation_id: event.conversation_id.clone(),
                    activity: activity.clone(),
                    event_ref: EventRef::seal(event, keys).unwrap(),
                }),
            },
        )
        .await
        .unwrap();
    (&mut stream.accepted).await.unwrap();
    while stream.events.recv().await.is_some() {}
}

#[tokio::test]
async fn complete_mode_preserves_interactive_replies_and_edits() {
    let legacy = record().await;
    let complete = record_with(true).await;
    assert_eq!(complete, legacy);
}

async fn assert_long_refusals(
    recording: &Recording,
    channel: &Arc<ResolvedChannel>,
    event: &EventRefPayload,
    reference: &cmaeg_core::seal::MessageRefPayload,
) {
    use cmaeg::delivery::intents::DeliveryCommand;
    use tokio::sync::oneshot;
    let message: cmaeg_core::message::OutboundMessage = serde_json::from_value(json!({"text":"🦀".repeat(2001),"blocks":[{"type":"buttons","id":"large","options":[{"id":"one","label":"One"}]}]})).unwrap();
    let calls = recording.http.sent().len();
    for update in [false, true] {
        let (receipt, received) = oneshot::channel();
        let command = if update {
            DeliveryCommand::Update {
                trace: tracing::Span::none(),
                channel: channel.clone(),
                message_ref: reference.clone(),
                message: message.clone(),
                receipt,
            }
        } else {
            DeliveryCommand::Reply {
                trace: tracing::Span::none(),
                channel: channel.clone(),
                event: event.clone(),
                message: message.clone(),
                source: ReplySource::Tool,
                receipt,
            }
        };
        recording.commands.send(command).await.unwrap();
        let receipt = received.await.unwrap();
        assert_eq!(
            receipt.delivery,
            DeliveryOutcome::Rejected {
                reason: "interactive_message_must_fit_one_part".into()
            }
        );
        assert!(receipt.message_ref.is_none());
        assert_eq!(recording.http.sent().len(), calls);
    }
}
