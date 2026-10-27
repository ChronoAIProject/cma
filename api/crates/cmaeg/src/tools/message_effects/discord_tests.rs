//! Exercise the real delivery engine, public effects routes and durable receipts.
use super::*;
use crate::channels::{definition::fixture as definition_fixture, routing_fixture};
use crate::tools::fake_cma;
use crate::{
    auth::AgentCaller,
    channels::resolver::ChannelResolver,
    delivery::{
        custom::CustomDelivery,
        engine::DeliveryEngine,
        intents::{DeliveryCommand, ReplySource},
        relay_client::RelayClient,
        router::ShardedScheduler,
    },
    discord::rest::effects_tests::Fixture as Platform,
    http,
    providers::{ProviderClient, ProviderRegistry},
    records::RecordClient,
    turns::TurnTracker,
    DeliveryConfig,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use cmaeg_core::{
    activity::{Activity, SourceKind},
    channel::{AdmissionPolicy, ChannelDefinition, Source},
    seal::{
        event_ref::activity_digest, seal_blob, ChannelPointer, EventRef, EventRefPayload,
        MasterKeyRing, ReplyAddress,
    },
};
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::time::Duration;
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
};

#[path = "discord_media_tests.rs"]
mod media;
#[path = "native_media_edit_tests.rs"]
mod media_edits;

struct App {
    backend: fake_cma::FakeCma,
    platform: Platform,
    state: ToolsState,
    definition: ChannelDefinition,
    event: EventRefPayload,
    activity: Activity,
    key: String,
    base: String,
    engine: Option<DeliveryEngine>,
    server: Option<JoinHandle<()>>,
    media_effects: bool,
}
impl Drop for App {
    fn drop(&mut self) {
        if let Some(server) = self.server.take() {
            server.abort();
        }
    }
}
impl App {
    async fn new() -> Self {
        let backend = fake_cma::FakeCma::start(false).await;
        let platform = Platform::new().await;
        let records = RecordClient::new(&backend.base_url, "cma-service").unwrap();
        let keys =
            Arc::new(MasterKeyRing::parse(&format!("1:{}", STANDARD.encode([3; 32]))).unwrap());
        let key = format!("nyxid_ag_{}", uuid::Uuid::new_v4().simple());
        let mut definition = definition_fixture::with_agent_key(&key);
        definition.record_id = uuid::Uuid::new_v4().to_string().parse().unwrap();
        definition.sources = vec![Source::Discord {
            thread_history: false,
            thread_lifecycle: false,
            bot_id: "77".into(),
            intents: 37377,
            admission: AdmissionPolicy::All {},
        }];
        definition.secrets.discord_bot_token = Some(platform.token.clone());
        definition.partition.key = cmaeg_core::channel::PartitionKey::ConversationAndSender;
        definition.validate().unwrap();
        let pointer = ChannelPointer {
            v: 1,
            record_id: definition.record_id.clone(),
            record_url: records
                .record_url(definition.record_id.as_str())
                .unwrap()
                .to_string(),
        };
        let id = pointer.seal_id(&keys).unwrap();
        backend.seed(
            definition.record_id.as_str(),
            &definition.owner.subject,
            seal_blob(
                &keys,
                &serde_json::to_vec(&definition.to_blob_json()).unwrap(),
            )
            .unwrap(),
        );
        let mut activity: Activity = serde_json::from_str(include_str!(
            "../../../../cmaeg-core/tests/fixtures/activity.json"
        ))
        .unwrap();
        activity.source = SourceKind::Discord {
            bot_id: "77".into(),
        };
        activity.conversation.id = "99".into();
        activity.event_id = "discord:77:700".parse().unwrap();
        let event = EventRefPayload {
            v: 1,
            telegram_admission: None,
            activity_sha256: Some(activity_digest(&activity)),
            channel_id: id,
            record_id: definition.record_id.clone(),
            event_id: activity.event_id.clone(),
            partition_key: "discord:99".into(),
            conversation_id: "conversation-1".parse().unwrap(),
            expires_at: now() + 600,
            reply: ReplyAddress::Discord {
                thread_id: None,
                bot_id: "77".into(),
                channel_id: "99".into(),
                message_id: "700".into(),
            },
            consumer: None,
            lineage: None,
        };
        let (sender, _) = mpsc::channel(16);
        let state = ToolsState {
            discord_thread_history: None,
            discord_threads: None,
            complete_replies_enabled: false,
            discord_interactions_enabled: false,
            telegram_effects_issuance_enabled: false,
            native_media_edits: None,
            native_whatsapp_forms: None,
            native_lark_effects: None,
            lark_effects_issuance_enabled: false,
            lark_effects: None,
            telegram_effects: None,
            whatsapp_templates_enabled: false,
            whatsapp_interactions_enabled: false,
            native_whatsapp_interactions_enabled: false,
            consumer_interactions_enabled: false,
            native_media_enabled: false,
            native_lark_outbound_media_enabled: false,
            native_whatsapp_outbound_media_enabled: false,
            whatsapp_reactions: None,
            native_whatsapp_templates_enabled: false,
            native_whatsapp_groups_enabled: false,
            relay_text_edits_enabled: false,
            native_discord_edits_enabled: true,
            discord_effects_issuance_enabled: true,
            discord_effects: Some(Arc::new(discord::DiscordEffectsClient::new(
                platform.api.clone(),
            ))),
            lark_cards_enabled: false,
            resolver: ChannelResolver::new(Arc::new(records), keys.clone(), Default::default()),
            keys,
            providers: Arc::new(
                ProviderClient::new(
                    ProviderRegistry::parse("[]").unwrap(),
                    Duration::from_secs(1),
                    Duration::from_secs(1),
                    1,
                )
                .unwrap(),
            ),
            delivery: Arc::new(ShardedScheduler::new(sender)),
            routed_replies: None,
            tracker: TurnTracker::new(http::readiness::Readiness::initialized()),
            public_origin: "https://gateway.example".into(),
            public_api_url: "https://gateway.example".into(),
            advertise_public_api_url: true,
            media: None,
            downloads: Default::default(),
        };
        let mut app = Self {
            backend,
            platform,
            state,
            definition,
            event,
            activity,
            key,
            base: String::new(),
            engine: None,
            server: None,
            media_effects: false,
        };
        app.restart().await;
        app
    }
    async fn restart(&mut self) {
        if let Some(server) = self.server.take() {
            server.abort();
        }
        self.engine.take();
        self.state.resolver = ChannelResolver::new(
            Arc::new(RecordClient::new(&self.backend.base_url, "cma-service").unwrap()),
            self.state.keys.clone(),
            Default::default(),
        );
        let (sender, receiver) = mpsc::channel(16);
        let mut transport = RelayClient::new("http://127.0.0.1:1", Duration::from_secs(1))
            .unwrap()
            .with_native_discord_edits(self.state.native_discord_edits_enabled)
            .with_native_discord_effects(self.state.discord_effects_issuance_enabled)
            .with_native_discord_media_effects(self.media_effects);
        transport.discord = self.platform.api.clone();
        if let Some(media) = &self.state.native_media_edits {
            transport = transport
                .with_telegram((*media.telegram).clone())
                .with_telegram_effects(true)
                .with_telegram_media_effects(true)
                .with_interactions(Arc::new(crate::interactions::InteractionStore::new(
                    RecordClient::new(&self.backend.base_url, "cma-service").unwrap(),
                    self.state.keys.clone(),
                    self.state.providers.clone(),
                )))
                .with_native_media_edits(
                    media.telegram_issuance,
                    media.discord_issuance,
                    Arc::new(crate::native_media::store::Store::new(
                        self.state.resolver.record_client(),
                        self.state.keys.clone(),
                    )),
                );
        }
        self.state.delivery = Arc::new(ShardedScheduler::new(sender));
        self.engine = Some(DeliveryEngine::start(
            receiver,
            self.state.keys.clone(),
            transport,
            DeliveryConfig::default(),
            self.state.tracker.clone(),
            CustomDelivery::new(self.state.public_origin.clone(), Arc::default()),
        ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        self.base = format!("http://{}", listener.local_addr().unwrap());
        let router =
            http::tools::router(self.state.clone()).merge(crate::mcp::router(self.state.clone()));
        self.server = Some(tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap()
        }));
    }
    async fn send(&self) -> crate::delivery::intents::IntentReceipt {
        self.send_message(serde_json::from_value(json!({"text":"hello"})).unwrap())
            .await
    }
    async fn send_message(
        &self,
        message: cmaeg_core::message::OutboundMessage,
    ) -> crate::delivery::intents::IntentReceipt {
        let (receipt, received) = oneshot::channel();
        let channel = self
            .state
            .resolver
            .resolve_current(self.event.channel_id.as_str())
            .await
            .unwrap();
        self.state
            .delivery
            .submit(DeliveryCommand::Reply {
                trace: tracing::Span::none(),
                channel,
                event: self.event.clone(),
                message,
                source: ReplySource::Tool,
                receipt,
            })
            .await
            .unwrap();
        received.await.unwrap()
    }
    async fn reference(&self) -> MessageRef {
        let sent = self.send().await;
        assert_eq!(sent.delivery.kind(), "confirmed");
        sent.message_ref.unwrap()
    }
    fn effect(
        &self,
        reference: &MessageRef,
        op: &str,
        key: &str,
        body: Value,
    ) -> reqwest::RequestBuilder {
        reqwest::Client::new()
            .post(format!(
                "{}/v1/messages/{}/{op}",
                self.base,
                reference.as_str()
            ))
            .bearer_auth(&self.key)
            .header("idempotency-key", key)
            .json(&body)
    }
    async fn caps(&self, reference: &MessageRef) -> Value {
        value(
            reqwest::Client::new()
                .get(format!(
                    "{}/v1/messages/{}/capabilities",
                    self.base,
                    reference.as_str()
                ))
                .bearer_auth(&self.key)
                .send()
                .await
                .unwrap(),
        )
        .await
    }
    async fn update(&mut self) {
        let previous = self.definition.version;
        self.definition.version += 1;
        self.state
            .resolver
            .record_client()
            .put(
                &self.definition.record_id,
                &self.definition.owner.subject,
                &seal_blob(
                    &self.state.keys,
                    &serde_json::to_vec(&self.definition.to_blob_json()).unwrap(),
                )
                .unwrap(),
                Some(previous),
            )
            .await
            .unwrap();
    }
}
async fn value(response: reqwest::Response) -> Value {
    let status = response.status();
    let body: Value = response.json().await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

#[tokio::test]
async fn actual_native_issuance_and_public_effects_survive_restart_without_duplicate_writes() {
    let mut app = App::new().await;
    let reference = app.reference().await;
    assert_eq!(app.caps(&reference).await["delete"], true);
    assert_eq!(app.caps(&reference).await["reaction"], true);
    for (op, key, body) in [
        (
            "reaction",
            "react",
            json!({"reaction":[{"type":"emoji","emoji":"👍"}]}),
        ),
        ("reaction", "clear", json!({"reaction":[]})),
        ("delete", "delete", json!({})),
    ] {
        if op == "reaction" && key == "clear" {
            app.platform.memory.lock().unwrap().message["reactions"] =
                json!([{"me":true,"emoji":{"name":"👍","id":null}}]);
        }
        let first = value(
            app.effect(&reference, op, key, body.clone())
                .send()
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(first["delivery"]["outcome"], "confirmed");
        let before = app.platform.memory.lock().unwrap().writes.len();
        app.restart().await;
        assert_eq!(
            value(app.effect(&reference, op, key, body).send().await.unwrap()).await,
            first
        );
        assert_eq!(app.platform.memory.lock().unwrap().writes.len(), before);
    }
    let memory = app.platform.memory.lock().unwrap();
    assert_eq!(memory.writes.len(), 4);
    assert_eq!(memory.writes[1].0, Method::PUT);
    assert!(memory.writes[1].1.ends_with("/@me"));
    assert_eq!(
        memory.writes[3],
        (Method::DELETE, "/api/v10/channels/99/messages/800".into())
    );
}

#[tokio::test]
async fn uncertainty_and_rejected_or_rate_limited_effects_have_durable_receipts() {
    for (status, expected) in [
        (StatusCode::BAD_GATEWAY, "unknown"),
        (StatusCode::FORBIDDEN, "rejected"),
        (StatusCode::TOO_MANY_REQUESTS, "rate_limited"),
    ] {
        let mut app = App::new().await;
        let reference = app.reference().await;
        app.platform.memory.lock().unwrap().reject_write = Some((2, status));
        let first = value(
            app.effect(&reference, "delete", "one", json!({}))
                .send()
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(first["delivery"]["outcome"], expected);
        app.restart().await;
        assert_eq!(
            value(
                app.effect(&reference, "delete", "one", json!({}))
                    .send()
                    .await
                    .unwrap()
            )
            .await,
            first
        );
        assert_eq!(app.platform.memory.lock().unwrap().writes.len(), 2);
    }
}

#[tokio::test]
async fn concurrent_effects_share_one_claim_and_conflicting_action_is_rejected() {
    let app = App::new().await;
    let reference = app.reference().await;
    let (a, b) = tokio::join!(
        app.effect(&reference, "delete", "same", json!({})).send(),
        app.effect(&reference, "delete", "same", json!({})).send()
    );
    let a = value(a.unwrap()).await;
    let b = value(b.unwrap()).await;
    assert!(a["delivery"]["outcome"] == "confirmed" || b["delivery"]["outcome"] == "confirmed");
    assert_eq!(app.platform.memory.lock().unwrap().writes.len(), 2);
    assert_eq!(
        app.effect(&reference, "reaction", "same", json!({"reaction":[]}))
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    assert_eq!(app.platform.memory.lock().unwrap().writes.len(), 2);
}

#[tokio::test]
async fn cached_authority_cannot_survive_source_binding_owner_or_key_changes() {
    for changed in [
        "source",
        "policy",
        "provider",
        "owner",
        "key",
        "token_removed",
    ] {
        let mut app = App::new().await;
        let reference = app.reference().await;
        app.caps(&reference).await;
        match changed {
            "source" => app.definition.sources.clear(),
            "policy" => {
                if let Source::Discord { admission, .. } = &mut app.definition.sources[0] {
                    *admission = AdmissionPolicy::Allowlist {
                        sender_ids: vec!["90".into()],
                    };
                }
            }
            "provider" => definition_fixture::replace_default_binding(
                &mut app.definition,
                "bnd_changed".parse().unwrap(),
            ),
            "owner" => app.definition.owner.subject = "another-owner".into(),
            "key" => app.definition.secrets.agent_key_hash = format!("sha256:{}", "1".repeat(64)),
            "token_removed" => app.definition.secrets.discord_bot_token = None,
            _ => unreachable!(),
        }
        app.update().await;
        assert!(
            !app.effect(&reference, "delete", "revoked", json!({}))
                .send()
                .await
                .unwrap()
                .status()
                .is_success(),
            "{changed}"
        );
        assert_eq!(app.platform.memory.lock().unwrap().writes.len(), 1);
    }
}

#[tokio::test]
async fn effects_gate_does_not_revoke_retained_references_or_renew_expiry_on_edit() {
    let mut app = App::new().await;
    let reference = app.reference().await;
    app.state.discord_effects_issuance_enabled = false;
    app.restart().await;
    let edit_only = app.reference().await;
    assert_eq!(app.caps(&edit_only).await["delete"], false);
    assert_eq!(app.caps(&reference).await["delete"], true);
    let mut payload = reference.open(&app.state.keys).unwrap().payload;
    let window = payload
        .discord_message
        .as_mut()
        .unwrap()
        .effects
        .as_mut()
        .unwrap();
    window.issued_at = now() - 86401;
    window.expires_at = now() - 1;
    let expired = MessageRef::seal(&payload, &app.state.keys).unwrap();
    assert_eq!(
        app.effect(&expired, "delete", "expired", json!({}))
            .send()
            .await
            .unwrap()
            .status(),
        410
    );
    let edited = value(
        app.effect(&expired, "update", "edit", json!({"text":"updated"}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(edited["delivery"]["outcome"], "confirmed");
    let edited = MessageRef::new(edited["message_ref"].as_str().unwrap())
        .unwrap()
        .open(&app.state.keys)
        .unwrap()
        .payload;
    assert_eq!(
        edited.discord_message.unwrap().effects,
        payload.discord_message.unwrap().effects
    );
}

#[tokio::test]
async fn context_capabilities_and_new_references_require_both_issuance_flags() {
    for (edits, effects) in [(false, false), (true, false), (false, true), (true, true)] {
        let mut app = App::new().await;
        app.state.native_discord_edits_enabled = edits;
        app.state.discord_effects_issuance_enabled = effects && edits;
        app.restart().await;
        let event = EventRef::seal(&app.event, &app.state.keys).unwrap();
        let view = app
            .state
            .verify_context(
                AgentCaller {
                    key_hash: Sha256::digest(app.key.as_bytes()).into(),
                },
                &event,
                app.activity.clone(),
            )
            .await
            .unwrap();
        assert_eq!(
            view.capabilities
                .message_effects
                .as_ref()
                .is_some_and(|caps| caps.delete),
            effects && edits
        );
        let sent = app.send().await;
        assert_eq!(sent.message_ref.is_some(), edits);
        if let Some(reference) = sent.message_ref {
            assert_eq!(
                reference
                    .open(&app.state.keys)
                    .unwrap()
                    .payload
                    .discord_message
                    .unwrap()
                    .effects
                    .is_some(),
                effects
            );
        }
    }
}

#[tokio::test]
async fn loss_of_final_receipt_cannot_repeat_an_already_applied_discord_effect() {
    use std::sync::atomic::Ordering;
    let mut app = App::new().await;
    let reference = app.reference().await;
    app.platform.memory.lock().unwrap().fail_receipt = Some(app.backend.fail_put.clone());
    let first = value(
        app.effect(&reference, "delete", "uncertain", json!({}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(first["delivery"]["outcome"], "unknown");
    app.platform.memory.lock().unwrap().fail_receipt = None;
    app.backend.fail_put.store(false, Ordering::SeqCst);
    app.restart().await;
    let replay = value(
        app.effect(&reference, "delete", "uncertain", json!({}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(replay["delivery"]["outcome"], "unknown");
    assert_eq!(app.platform.memory.lock().unwrap().writes.len(), 2);
}

#[tokio::test]
async fn named_consumer_reference_requires_its_own_live_key_and_binding() {
    let mut app = App::new().await;
    let id = routing_fixture::REVIEWER;
    let key = routing_fixture::agent_key(id);
    let mut definition = app.definition.to_blob_json();
    definition["routing"] = json!({"version":1,"default_consumer":{"name":"lead"},"consumers":[routing_fixture::consumer(id,"reviewer","cma_codex")],"default_targets":["default"]});
    definition["secrets"]["consumer_keys"] = json!([[id, key]]);
    app.definition = serde_json::from_value(definition).unwrap();
    app.update().await;
    app.event.consumer = Some(cmaeg_core::ids::ConsumerId::parse(id).unwrap());
    let reference = app.reference().await;
    assert_eq!(
        app.effect(&reference, "delete", "borrowed", json!({}))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    app.key = key;
    assert_eq!(
        value(
            app.effect(&reference, "delete", "owner", json!({}))
                .send()
                .await
                .unwrap()
        )
        .await["delivery"]["outcome"],
        "confirmed"
    );
    app.definition.routing = None;
    app.definition.secrets.consumer_keys.clear();
    app.update().await;
    assert_eq!(
        app.effect(&reference, "delete", "removed", json!({}))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(app.platform.memory.lock().unwrap().writes.len(), 2);
}

#[tokio::test]
async fn mcp_lists_discord_effects_and_shares_the_rest_idempotency_receipt() {
    let app = App::new().await;
    let reference = app.reference().await;
    let call = |method: &str, params: Value| {
        reqwest::Client::new()
            .post(format!("{}/mcp", app.base))
            .bearer_auth(&app.key)
            .header("accept", "application/json, text/event-stream")
            .json(&json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}))
    };
    let listed = value(call("tools/list", json!({})).send().await.unwrap()).await;
    assert!(listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool["name"] == "deleteMessage"));
    let result = value(call("tools/call",json!({"name":"deleteMessage","arguments":{"message_ref":reference.as_str(),"idempotency_key":"across-surfaces"}})).send().await.unwrap()).await;
    assert!(result.get("error").is_none(), "{result}");
    assert_ne!(result["result"]["isError"], true, "{result}");
    let replay = value(
        app.effect(&reference, "delete", "across-surfaces", json!({}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(replay["delivery"]["outcome"], "confirmed");
    assert_eq!(app.platform.memory.lock().unwrap().writes.len(), 2);
}

#[tokio::test]
async fn different_keys_on_separate_http_instances_cannot_replace_one_reaction_concurrently() {
    let app = App::new().await;
    let reference = app.reference().await;
    let entered = Arc::new(tokio::sync::Semaphore::new(0));
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    app.platform.memory.lock().unwrap().message_gate = Some((entered.clone(), release.clone()));
    let first = app.effect(
        &reference,
        "reaction",
        "first",
        json!({"reaction":[{"type":"emoji","emoji":"👍"}]}),
    );
    let first = tokio::spawn(async move { first.send().await.unwrap() });
    tokio::time::timeout(Duration::from_secs(2), entered.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    // A separate HTTP composition has its own client/capacity. Only the record
    // service can serialize these different idempotency keys.
    let mut second_state = app.state.clone();
    second_state.discord_effects = Some(Arc::new(discord::DiscordEffectsClient::new(
        app.platform.api.clone(),
    )));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "http://{}/v1/messages/{}/reaction",
        listener.local_addr().unwrap(),
        reference.as_str()
    );
    let router = http::tools::router(second_state);
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let second = tokio::time::timeout(
        Duration::from_secs(2),
        reqwest::Client::new()
            .post(url)
            .bearer_auth(&app.key)
            .header("idempotency-key", "second")
            .json(&json!({"reaction":[{"type":"emoji","emoji":"🎉"}]}))
            .send(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        value(second).await["delivery"],
        json!({"outcome":"rejected","reason":"native_message_effect_busy"})
    );
    release.add_permits(1);
    assert_eq!(
        value(first.await.unwrap()).await["delivery"]["outcome"],
        "confirmed"
    );
    server.abort();
    assert_eq!(app.platform.memory.lock().unwrap().writes.len(), 2);
    assert_eq!(
        entered.available_permits(),
        0,
        "the conflicting request cannot read or mutate the platform"
    );
    app.platform.memory.lock().unwrap().message_gate = None;
    assert_eq!(
        value(
            app.effect(
                &reference,
                "reaction",
                "after-completion",
                json!({"reaction":[]})
            )
            .send()
            .await
            .unwrap()
        )
        .await["delivery"]["outcome"],
        "confirmed"
    );
}

#[tokio::test]
async fn new_keys_cannot_bypass_an_ambiguous_message_fence_after_restart() {
    let mut app = App::new().await;
    let reference = app.reference().await;
    app.platform.memory.lock().unwrap().reject_write = Some((2, StatusCode::BAD_GATEWAY));
    assert_eq!(
        value(
            app.effect(&reference, "delete", "uncertain", json!({}))
                .send()
                .await
                .unwrap()
        )
        .await["delivery"]["outcome"],
        "unknown"
    );
    app.restart().await;
    assert_eq!(
        value(
            app.effect(
                &reference,
                "reaction",
                "new-key",
                json!({"reaction":[{"type":"emoji","emoji":"👍"}]})
            )
            .send()
            .await
            .unwrap()
        )
        .await["delivery"],
        json!({"outcome":"rejected","reason":"native_message_effect_busy"})
    );
    assert_eq!(app.platform.memory.lock().unwrap().writes.len(), 2);
}
