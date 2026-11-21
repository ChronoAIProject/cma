//! Real delivery engine and durable card store; only external NyxID sends are
//! recorded fakes. This establishes the wire contract, not platform acceptance.
use super::{
    fake_cma::FakeCma,
    fake_http::{Answer, FakeHttp},
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
use cmaeg_core::{
    activity::SourceKind,
    channel::{PartitionKey, Platform, Source},
    seal::{EventRefPayload, ReplyAddress},
};
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};

struct Setup {
    provider: FakeProvider,
    backend: FakeCma,
    store: Arc<InteractionStore>,
    channel: Arc<ResolvedChannel>,
    event: EventRefPayload,
    activity: cmaeg_core::activity::Activity,
}
impl Setup {
    async fn new() -> Self {
        let platform = Platform::Whatsapp;
        let provider = FakeProvider::start(Default::default()).await;
        let backend = FakeCma::start(false).await;
        let registry = ProviderRegistry::parse(&json!([{"slug":"cma_codex","base_url":provider.base_url(),"kind":"responses_http",
            "paths":{"agent_card":"/.well-known/agent-card.json","binding":"/api/v1/cmaeg/bindings/{binding_id}",
            "conversation":"/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}","responses":"/api/v2/responses"},"max_inflight":8}]).to_string()).unwrap();
        let providers = Arc::new(
            ProviderClient::new(registry, Duration::from_secs(1), Duration::from_secs(2), 8)
                .unwrap(),
        );
        let mut channel = relay_channel(0);
        let channel_mut = Arc::get_mut(&mut channel).unwrap();
        let Source::NyxidRelay {
            platform: current,
            verify_raw_event,
            ..
        } = &mut channel_mut.definition.sources[0]
        else {
            panic!()
        };
        *current = platform;
        *verify_raw_event = true;
        channel_mut.definition.profile_digest = channel_mut.definition.profile.digest();
        channel_mut.definition.partition.key = PartitionKey::ConversationAndSender;
        channel_mut.definition.validate().unwrap();
        let mut activity = relay_activity();
        activity.source = SourceKind::NyxidRelay {
            platform,
            route_id: "route-1".into(),
        };
        activity.actor.id = "7".into();
        activity.actor.kind = cmaeg_core::activity::ActorKind::Unknown;
        activity.conversation.id = "7".into();
        activity.conversation.kind = cmaeg_core::activity::ConversationKind::Private;
        activity.conversation.thread_id = None;
        let mut address = relay_reply();
        let ReplyAddress::NyxidRelay {
            platform: current, ..
        } = &mut address
        else {
            panic!()
        };
        *current = platform;
        let event = event(&channel, &activity, 1, address);
        seed(&providers, &keys(), &channel, &activity, &event).await;
        let store = Arc::new(InteractionStore::new(
            RecordClient::new(&backend.base_url, "cma-service").unwrap(),
            keys(),
            providers,
        ));
        Self {
            provider,
            backend,
            store,
            channel,
            event,
            activity,
        }
    }
    async fn named(&mut self) {
        use sha2::{Digest, Sha256};
        let id = cmaeg_core::ids::ConsumerId::parse("csm_00000000000a").unwrap();
        let channel = Arc::get_mut(&mut self.channel).unwrap();
        let mut value = channel.definition.to_blob_json();
        let mut provider = value["provider"].clone();
        provider["binding_id"] = "binding-reviewer".into();
        value["routing"] = json!({"version":1,"default_consumer":{"name":"lead"},"consumers":[{
            "id":id,"name":"reviewer","profile":value["profile"],"profile_digest":value["profile_digest"],
            "provider":provider,"agent_key_hash":format!("sha256:{:x}",Sha256::digest(b"named-fixture-key")),"created_at":"2026-10-01T00:00:00Z"}],"default_targets":["default"]});
        value["secrets"]["consumer_keys"] = json!([[id, "named-fixture-key"]]);
        channel.definition = serde_json::from_value(value).unwrap();
        channel.definition.validate().unwrap();
        self.event.conversation_id = cmaeg_core::ids::ConversationId::derive_for_consumer(
            &keys(),
            &channel.id,
            1,
            &id,
            &self.event.partition_key,
        );
        self.event.consumer = Some(id);
        // The fake provider keeps the seeded event under the named conversation.
        let entry = json!([{"slug":"cma_codex","base_url":self.provider.base_url(),"kind":"responses_http","paths":{"agent_card":"/.well-known/agent-card.json","binding":"/api/v1/cmaeg/bindings/{binding_id}","conversation":"/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}","responses":"/api/v2/responses"},"max_inflight":8}]);
        let providers = ProviderClient::new(
            ProviderRegistry::parse(&entry.to_string()).unwrap(),
            Duration::from_secs(1),
            Duration::from_secs(2),
            8,
        )
        .unwrap();
        seed(&providers, &keys(), channel, &self.activity, &self.event).await;
    }
    fn recording(&self, enabled: bool, mode: u8) -> Recording {
        self.recording_with_consumer(enabled, mode, false)
    }
    fn recording_with_consumer(&self, enabled: bool, mode: u8, consumer: bool) -> Recording {
        let fail_put = self.backend.fail_put.clone();
        let http = FakeHttp::new(move |_, count| {
            if mode == 1 {
                return Answer::json(200, json!({"message_id":"outbound-1"}));
            }
            if mode == 2 {
                return Answer::status(503);
            }
            if mode == 3 && count == 1 {
                return Answer::status(429).retry_after("0");
            }
            if mode == 4 {
                fail_put.store(true, std::sync::atomic::Ordering::SeqCst);
            }
            Answer::json(
                200,
                json!({"message_id":"outbound-1","platform_message_id":"wamid.card"}),
            )
        });
        Recording::start_with_relay(
            http,
            Hooks::answering([]),
            DeliveryConfig::default(),
            Some(self.store.clone()),
            false,
            |relay| {
                relay
                    .with_whatsapp_interactions(enabled)
                    .with_consumer_interactions(consumer)
            },
        )
    }
}
fn choice(list: bool) -> Value {
    let mut block =
        json!({"type":"buttons","id":"decision","options":[{"id":"yes","label":"Yes"}]});
    if list {
        block["type"] = "choice".into();
        block["prompt"] = "Approve?".into();
        block["multi"] = false.into();
    }
    json!({"text":"Approve?","blocks":[block]})
}
fn locator(body: &Value) -> String {
    let markup = &body["reply"]["metadata"]["interactive"];
    let value = if markup["type"] == "button" {
        &markup["action"]["buttons"][0]["reply"]["id"]
    } else {
        &markup["action"]["sections"][0]["rows"][0]["id"]
    };
    value.as_str().unwrap().into()
}

#[tokio::test]
async fn gated_buttons_and_lists_send_metadata_and_bind_only_exact_receipts() {
    for list in [false, true] {
        let setup = Setup::new().await;
        let mut disabled = setup.recording(false, 0);
        let result = disabled
            .reply(
                &setup.channel,
                &setup.event,
                choice(list),
                ReplySource::Tool,
            )
            .await;
        assert!(matches!(result.delivery, DeliveryOutcome::Confirmed { .. }));
        assert!(disabled.http.sent()[0].json()["reply"]["metadata"].is_null());
        assert!(setup.backend.records().is_empty());
        disabled.finish().await;
        let mut enabled = setup.recording(true, 0);
        let result = enabled
            .reply(
                &setup.channel,
                &setup.event,
                choice(list),
                ReplySource::Tool,
            )
            .await;
        assert!(
            matches!(result.delivery, DeliveryOutcome::Confirmed { .. }),
            "{:?}",
            result.delivery
        );
        let sent = enabled.http.sent();
        assert_eq!(sent.len(), 1);
        assert!(sent[0].url.path().ends_with("channel-relay/reply"));
        let body = sent[0].json();
        assert!(body["reply"]["text"].is_null());
        assert!(body.get("to").is_none());
        assert_eq!(body["reply"]["metadata"].as_object().unwrap().len(), 1);
        assert_eq!(
            body["reply"]["metadata"]["interactive"]["type"],
            if list { "list" } else { "button" }
        );
        let mut click = setup.activity.clone();
        click.event_id = "click-1".parse().unwrap();
        let (interaction, _) = setup
            .store
            .consume(
                &setup.channel,
                &click,
                "route-1",
                "wamid.click",
                &locator(&body),
                "wamid.card",
                &|consumer| consumer.is_none(),
            )
            .await
            .unwrap();
        assert_eq!(interaction.values["selected"], json!(["yes"]));
        let reference = result.message_ref.unwrap().open(&keys()).unwrap().payload;
        let edited = enabled
            .update(&setup.channel, reference, choice(list))
            .await;
        assert!(matches!(edited.delivery, DeliveryOutcome::Unsupported));
        assert_eq!(enabled.http.sent().len(), 1);
        enabled.finish().await;
        assert!(!setup.provider.requests().is_empty());
    }
}

#[tokio::test]
async fn uncertain_receipts_and_failed_binding_never_grant_callback_authority_or_resend() {
    for mode in [1, 2, 4] {
        let setup = Setup::new().await;
        let mut recording = setup.recording(true, mode);
        let result = recording
            .reply(
                &setup.channel,
                &setup.event,
                choice(false),
                ReplySource::Tool,
            )
            .await;
        assert!(
            matches!(result.delivery, DeliveryOutcome::Unknown { .. }),
            "{:?}",
            result.delivery
        );
        assert!(result.message_ref.is_none());
        assert_eq!(recording.http.sent().len(), 1);
        let id = setup.backend.records().keys().next().unwrap().clone();
        let card = setup.store.load(&id).await.unwrap().0;
        assert!(card.reference.is_none());
        assert!(card.destination.is_none());
        recording.finish().await;
    }
}

#[tokio::test]
async fn definite_rate_limit_retries_the_same_locator_once() {
    let setup = Setup::new().await;
    let mut recording = setup.recording(true, 3);
    let result = recording
        .reply(
            &setup.channel,
            &setup.event,
            choice(false),
            ReplySource::Tool,
        )
        .await;
    assert!(
        matches!(result.delivery, DeliveryOutcome::Confirmed { .. }),
        "{:?}",
        result.delivery
    );
    let sent = recording.http.sent();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0].json(), sent[1].json());
    assert_eq!(setup.backend.records().len(), 1);
    assert!(result.message_ref.is_some());
    recording.finish().await;
}

#[tokio::test]
async fn unsupported_controls_fail_before_any_durable_write_or_send() {
    let setup = Setup::new().await;
    let mut recording = setup.recording(true, 0);
    for case in 0..4 {
        let mut message = choice(false);
        match case {
            0 => message["text"] = "x".repeat(1025).into(),
            1 => {
                message = choice(true);
                message["blocks"][0]["multi"] = true.into();
            }
            2 => {
                message["native_attachments"] =
                    json!([{"kind":"file","source":{"type":"base64","data":"YQ=="}}])
            }
            _ => message["thread_reply"] = true.into(),
        }
        let result = recording
            .reply(&setup.channel, &setup.event, message, ReplySource::Tool)
            .await;
        assert!(!matches!(
            result.delivery,
            DeliveryOutcome::Confirmed { .. }
        ));
        assert!(setup.backend.records().is_empty());
        assert!(recording.http.sent().is_empty());
    }
    recording.finish().await;
}

#[tokio::test]
async fn named_whatsapp_issuance_requires_gate_and_retained_callback_keeps_owner() {
    let mut setup = Setup::new().await;
    setup.named().await;
    let mut legacy = setup.recording(false, 0);
    let before = legacy
        .reply(
            &setup.channel,
            &setup.event,
            choice(false),
            ReplySource::Tool,
        )
        .await;
    assert!(matches!(before.delivery, DeliveryOutcome::Confirmed { .. }));
    let expected = legacy.http.sent()[0].json();
    legacy.finish().await;
    let mut disabled = setup.recording(true, 0);
    let fallback = disabled
        .reply(
            &setup.channel,
            &setup.event,
            choice(false),
            ReplySource::Tool,
        )
        .await;
    assert!(matches!(
        fallback.delivery,
        DeliveryOutcome::Confirmed { .. }
    ));
    assert_eq!(disabled.http.sent().len(), 1);
    assert_eq!(disabled.http.sent()[0].json(), expected);
    assert!(setup.backend.records().is_empty());
    disabled.finish().await;
    let mut enabled = setup.recording_with_consumer(true, 0, true);
    let sent = enabled
        .reply(
            &setup.channel,
            &setup.event,
            choice(false),
            ReplySource::Tool,
        )
        .await;
    assert!(
        matches!(sent.delivery, DeliveryOutcome::Confirmed { .. }),
        "{:?}",
        sent.delivery
    );
    let reference = sent.message_ref.unwrap().open(&keys()).unwrap().payload;
    assert_eq!(reference.consumer, setup.event.consumer);
    let id = reference.interaction_record_id.unwrap();
    assert!(setup
        .store
        .load(&id)
        .await
        .unwrap()
        .0
        .callback_owner
        .is_some());
    let mut click = setup.activity.clone();
    click.event_id = "click-named".parse().unwrap();
    assert!(setup
        .store
        .consume(
            &setup.channel,
            &click,
            "route-1",
            "click-named",
            &locator(&enabled.http.sent()[0].json()),
            "wamid.card",
            &|_| true
        )
        .await
        .is_ok());
    enabled.finish().await;
}

#[tokio::test]
async fn native_gate_keeps_legacy_whatsapp_text_for_trusted_normalized_sources() {
    let mut setup = Setup::new().await;
    let channel = Arc::get_mut(&mut setup.channel).unwrap();
    let Source::NyxidRelay {
        verify_raw_event,
        trust_normalized,
        ..
    } = &mut channel.definition.sources[0]
    else {
        panic!()
    };
    *verify_raw_event = false;
    *trust_normalized = true;
    channel.definition.validate().unwrap();
    for list in [false, true] {
        let mut legacy = setup.recording(false, 0);
        let before = legacy
            .reply(
                &setup.channel,
                &setup.event,
                choice(list),
                ReplySource::Tool,
            )
            .await;
        assert!(matches!(before.delivery, DeliveryOutcome::Confirmed { .. }));
        let expected = legacy.http.sent()[0].json();
        legacy.finish().await;
        let mut enabled = setup.recording(true, 0);
        let after = enabled
            .reply(
                &setup.channel,
                &setup.event,
                choice(list),
                ReplySource::Tool,
            )
            .await;
        assert!(matches!(after.delivery, DeliveryOutcome::Confirmed { .. }));
        assert_eq!(enabled.http.sent().len(), 1);
        assert_eq!(enabled.http.sent()[0].json(), expected);
        assert!(setup.backend.records().is_empty());
        assert!(setup
            .provider
            .log()
            .iter()
            .all(|request| request.method != "GET"));
        enabled.finish().await;
    }
}
