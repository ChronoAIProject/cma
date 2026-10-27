//! Real delivery engine and durable card store; only external NyxID sends are
//! recorded fakes. This establishes the wire contract, not platform acceptance.
#[path = "lark_forms.rs"]
mod forms;
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
    async fn new(platform: Platform) -> Self {
        Self::with_activity(platform, |_| {}).await
    }
    async fn with_activity(
        platform: Platform,
        adjust: impl FnOnce(&mut cmaeg_core::activity::Activity),
    ) -> Self {
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
            platform: current, ..
        } = &mut channel_mut.definition.sources[0]
        else {
            panic!()
        };
        *current = platform;
        channel_mut.definition.profile_digest = channel_mut.definition.profile.digest();
        channel_mut.definition.partition.key = PartitionKey::ConversationAndSender;
        channel_mut.definition.validate().unwrap();
        let mut activity = relay_activity();
        activity.source = SourceKind::NyxidRelay {
            platform,
            route_id: "route-1".into(),
        };
        activity.actor.id = "ou_sender".into();
        activity.conversation.id = "oc_chat".into();
        activity.conversation.thread_id = None;
        adjust(&mut activity);
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
    fn recording(&self, enabled: bool, bad_edit: bool) -> Recording {
        let http = FakeHttp::new(move |sent, _| {
            if sent.url.path().ends_with("reply/update") {
                Answer::json(
                    200,
                    json!({"upstream_message_id":if bad_edit {"om_other"} else {"om_card"},"edited_at":"2026-10-26T00:00:00Z"}),
                )
            } else {
                Answer::json(
                    200,
                    json!({"message_id":"outbound-1","platform_message_id":"om_card"}),
                )
            }
        });
        Recording::start_with_relay(
            http,
            Hooks::answering([]),
            DeliveryConfig::default(),
            Some(self.store.clone()),
            false,
            |relay| relay.with_lark_cards(enabled),
        )
    }
}

#[tokio::test]
async fn native_gate_keeps_legacy_lark_text_for_threaded_and_nonhuman_events() {
    for platform in [Platform::Lark, Platform::Feishu] {
        for case in 0..3 {
            let mut setup = Setup::with_activity(platform, |activity| {
                if case != 0 {
                    activity.conversation.thread_id = Some("thread-1".into());
                } else {
                    activity.actor.kind = cmaeg_core::activity::ActorKind::Bot;
                }
            })
            .await;
            if case == 2 {
                let ReplyAddress::NyxidRelay { thread_reply, .. } = &mut setup.event.reply else {
                    panic!()
                };
                *thread_reply = true;
            }
            let mut legacy = setup.recording(false, false);
            let before = legacy
                .reply(&setup.channel, &setup.event, choice(), ReplySource::Tool)
                .await;
            assert!(matches!(before.delivery, DeliveryOutcome::Confirmed { .. }));
            let expected = legacy.http.sent()[0].json();
            legacy.finish().await;
            let mut enabled = setup.recording(true, false);
            let after = enabled
                .reply(&setup.channel, &setup.event, choice(), ReplySource::Tool)
                .await;
            assert!(matches!(after.delivery, DeliveryOutcome::Confirmed { .. }));
            assert_eq!(enabled.http.sent().len(), 1);
            assert_eq!(enabled.http.sent()[0].json(), expected);
            assert!(setup.backend.records().is_empty());
            enabled.finish().await;
        }
    }
}

#[tokio::test]
async fn invalid_or_unavailable_native_context_never_falls_back_or_writes_a_card() {
    for unavailable in [false, true] {
        let mut setup = Setup::new(Platform::Lark).await;
        if unavailable {
            setup.event.event_id = "missing-context".parse().unwrap();
        } else {
            setup.event.activity_sha256 = Some("0".repeat(64));
        }
        let mut recording = setup.recording(true, false);
        let result = recording
            .reply(&setup.channel, &setup.event, choice(), ReplySource::Tool)
            .await;
        assert!(!matches!(
            result.delivery,
            DeliveryOutcome::Confirmed { .. }
        ));
        assert!(recording.http.sent().is_empty());
        assert!(setup.backend.records().is_empty());
        recording.finish().await;
    }
}
fn choice() -> Value {
    json!({"text":"Approve?","blocks":[{"type":"buttons","id":"decision","options":[{"id":"yes","label":"Yes"}]}]})
}
fn token(body: &Value) -> String {
    body["reply"]["metadata"]["card"]["body"]["elements"]
        .as_array()
        .unwrap()
        .iter()
        .find_map(|e| e.pointer("/behaviors/0/value/cma_action"))
        .unwrap()
        .as_str()
        .unwrap()
        .into()
}

#[tokio::test]
async fn gated_lark_and_feishu_cards_send_native_metadata_and_edit_after_issuance_is_disabled() {
    for platform in [Platform::Lark, Platform::Feishu] {
        let setup = Setup::new(platform).await;
        let mut disabled = setup.recording(false, false);
        let reply = disabled
            .reply(&setup.channel, &setup.event, choice(), ReplySource::Tool)
            .await;
        assert!(matches!(reply.delivery, DeliveryOutcome::Confirmed { .. }));
        assert!(disabled.http.sent()[0].json()["reply"]["metadata"].is_null());
        assert!(setup.backend.records().is_empty());
        disabled.finish().await;
        let mut enabled = setup.recording(true, false);
        let reply = enabled
            .reply(&setup.channel, &setup.event, choice(), ReplySource::Tool)
            .await;
        assert!(
            matches!(reply.delivery, DeliveryOutcome::Confirmed { .. }),
            "{:?}",
            reply.delivery
        );
        let body = enabled.http.sent()[0].json();
        assert!(body["reply"]["text"].is_null());
        assert_eq!(body["reply"]["metadata"]["card"]["schema"], "2.0");
        assert!(body["reply"]["metadata"].get("reply_markup").is_none());
        assert_eq!(
            setup
                .provider
                .log()
                .iter()
                .filter(|request| request.method == "GET")
                .count(),
            1,
            "native preflight and preparation must share one context read"
        );
        let mut click = setup.activity.clone();
        click.event_id = "click-1".parse().unwrap();
        setup
            .store
            .consume(
                &setup.channel,
                &click,
                "route-1",
                "query-1",
                &token(&body),
                "om_card",
                &|c| c.is_none(),
            )
            .await
            .unwrap();
        let reference = reply.message_ref.unwrap().open(&keys()).unwrap().payload;
        enabled.finish().await;
        let mut restarted_disabled = setup.recording(false, false);
        let edited = restarted_disabled
            .update(
                &setup.channel,
                reference,
                serde_json::from_value(json!({"text":"Approved"})).unwrap(),
            )
            .await;
        assert!(
            matches!(edited.delivery, DeliveryOutcome::Confirmed { .. }),
            "{:?}",
            edited.delivery
        );
        assert!(edited.message_ref.is_some());
        let request = &restarted_disabled.http.sent()[0];
        assert!(request.url.path().ends_with("reply/update"));
        assert_eq!(request.json()["outbound_message_id"], "outbound-1");
        assert_eq!(request.json()["message_id"], "om_card");
        assert_eq!(
            request.json()["reply"]["metadata"]["card"]["body"]["elements"][0]["text"]["content"],
            "Approved"
        );
        restarted_disabled.finish().await;
        assert!(!setup.provider.requests().is_empty());
    }
}

#[tokio::test]
async fn wrong_edit_receipt_is_unknown_and_never_reactivates_previous_controls() {
    let setup = Setup::new(Platform::Lark).await;
    let mut recording = setup.recording(true, true);
    let reply = recording
        .reply(&setup.channel, &setup.event, choice(), ReplySource::Tool)
        .await;
    let data = token(&recording.http.sent()[0].json());
    let mut click = setup.activity.clone();
    click.event_id = "click-1".parse().unwrap();
    setup
        .store
        .consume(
            &setup.channel,
            &click,
            "route-1",
            "query-1",
            &data,
            "om_card",
            &|c| c.is_none(),
        )
        .await
        .unwrap();
    let reference = reply.message_ref.unwrap().open(&keys()).unwrap().payload;
    let edited = recording.update(&setup.channel, reference, choice()).await;
    assert_eq!(
        edited.delivery,
        DeliveryOutcome::Unknown {
            reason: "receipt_invalid".into()
        }
    );
    assert!(edited.message_ref.is_none());
    assert!(setup
        .store
        .consume(
            &setup.channel,
            &click,
            "route-1",
            "query-1",
            &data,
            "om_card",
            &|c| c.is_none()
        )
        .await
        .is_err());
    assert_eq!(recording.http.sent().len(), 2);
    recording.finish().await;
}

#[tokio::test]
async fn native_card_media_and_oversize_refuse_before_record_or_send() {
    let setup = Setup::new(Platform::Lark).await;
    let mut recording = setup.recording(true, false);
    for media in [false, true] {
        let mut message = serde_json::to_value(choice()).unwrap();
        if media {
            message["native_attachments"] =
                json!([{"kind":"file","source":{"type":"base64","data":"YQ=="}}]);
        } else {
            message["text"] = "x".repeat(4001).into();
        }
        let result = recording
            .reply(
                &setup.channel,
                &setup.event,
                serde_json::from_value(message).unwrap(),
                ReplySource::Tool,
            )
            .await;
        assert!(!matches!(
            result.delivery,
            DeliveryOutcome::Confirmed { .. }
        ));
        assert!(recording.http.sent().is_empty());
        assert!(setup.backend.records().is_empty());
    }
    recording.finish().await;
}
