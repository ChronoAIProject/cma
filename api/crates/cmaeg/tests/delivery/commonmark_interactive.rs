//! CommonMark keyboard sends and edits retain durable callback authority.
use super::{
    commonmark::SOURCE,
    fake_cma::FakeCma,
    fake_http::{Answer, FakeHttp},
    fake_provider::FakeProvider,
    recording_fixtures::{event, keys, relay_activity, relay_channel, relay_reply},
    recording_interactive::seed,
    recording_support::{Hooks, Recording},
};
use cmaeg::{
    delivery::intents::{DeliveryOutcome, ReplySource},
    interactions::InteractionStore,
    providers::{ProviderClient, ProviderRegistry},
    records::RecordClient,
    DeliveryConfig,
};
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};

fn buttons(text: &str) -> Value {
    json!({"text":text,"blocks":[{"type":"buttons","id":"choice","options":[{"id":"first","label":"First"}]}]})
}

#[tokio::test]
async fn commonmark_keyboard_send_click_and_update_preserve_source_and_metadata() {
    exercise(200, true).await;
    exercise(200, false).await;
}

#[tokio::test]
async fn commonmark_keyboard_refusal_cannot_create_a_reference_or_retry() {
    for status in [400, 404, 502] {
        exercise(status, false).await;
    }
}

async fn exercise(status: u16, valid_edit: bool) {
    let provider = FakeProvider::start(Default::default()).await;
    let backend = FakeCma::start(false).await;
    let keys = keys();
    let registry = ProviderRegistry::parse(&json!([{"slug":"cma_codex","base_url":provider.base_url(),"kind":"responses_http","paths":{"agent_card":"/.well-known/agent-card.json","binding":"/api/v1/cmaeg/bindings/{binding_id}","conversation":"/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}","responses":"/api/v2/responses"},"max_inflight":8}]).to_string()).unwrap();
    let providers = Arc::new(
        ProviderClient::new(registry, Duration::from_secs(1), Duration::from_secs(2), 8).unwrap(),
    );
    let channel = relay_channel(0);
    let activity = relay_activity();
    let event = event(&channel, &activity, 1, relay_reply());
    seed(&providers, &keys, &channel, &activity, &event).await;
    let store = Arc::new(InteractionStore::new(
        RecordClient::new(&backend.base_url, "cma-service").unwrap(),
        keys.clone(),
        providers,
    ));
    let http = FakeHttp::new(move |sent, _| {
        if sent.url.path().contains("/update/") {
            Answer::json(
                200,
                json!({"upstream_message_id":if valid_edit { "platform-1" } else { "wrong" },"edited_at":"2026-11-13T00:00:00Z"}),
            )
        } else {
            Answer::json(
                status,
                json!({"message_id":"out-1","platform_message_id":"platform-1"}),
            )
        }
    });
    let mut rec = Recording::start_with_relay(
        http,
        Hooks::answering([]),
        DeliveryConfig::default(),
        Some(store.clone()),
        false,
        |relay| relay.with_commonmark_relay(true),
    );
    let receipt = rec
        .reply(&channel, &event, buttons(SOURCE), ReplySource::Tool)
        .await;
    let sent = rec.http.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].url.path(), "/api/v1/channel-relay/reply/commonmark");
    assert_eq!(sent[0].json()["reply"]["text"], SOURCE);
    let body = sent[0].json();
    let keyboard = &body["reply"]["metadata"]["reply_markup"]["inline_keyboard"];
    assert_eq!(keyboard[0][0]["text"], "First");
    assert!(body["reply"]["metadata"].get("entities").is_none());
    assert!(body["reply"]["metadata"].get("parse_mode").is_none());
    if status != 200 {
        assert!(matches!(receipt.delivery, DeliveryOutcome::Unknown { .. }));
        assert!(receipt.message_ref.is_none());
        rec.finish().await;
        return;
    }
    assert!(matches!(
        receipt.delivery,
        DeliveryOutcome::Confirmed { .. }
    ));
    let mut click = activity.clone();
    click.event_id = "commonmark-click".parse().unwrap();
    store
        .consume(
            &channel,
            &click,
            "route-1",
            "callback-1",
            keyboard[0][0]["callback_data"].as_str().unwrap(),
            "platform-1",
            &|consumer| consumer.is_none(),
        )
        .await
        .unwrap();
    let reference = receipt.message_ref.unwrap().open(&keys).unwrap().payload;
    let edited = rec.update(&channel, reference, buttons(SOURCE)).await;
    assert_eq!(
        matches!(edited.delivery, DeliveryOutcome::Confirmed { .. }),
        valid_edit
    );
    if !valid_edit {
        assert!(matches!(edited.delivery, DeliveryOutcome::Unknown { .. }));
        assert!(edited.message_ref.is_none());
    }
    let sent = rec.http.sent();
    assert_eq!(sent.len(), 2);
    assert_eq!(
        sent[1].url.path(),
        "/api/v1/channel-relay/reply/update/commonmark"
    );
    assert_eq!(sent[1].json()["reply"]["text"], SOURCE);
    assert_eq!(sent[1].json()["outbound_message_id"], "out-1");
    assert_eq!(sent[1].json()["message_id"], "platform-1");
    assert_eq!(
        sent[1].json()["reply"]["metadata"]["reply_markup"]["inline_keyboard"][0][0]["text"],
        "First"
    );
    rec.finish().await;
}
