//! Actual authenticated HTTP ingress, encrypted records, provider HTTP, and routed execution.
use super::{adapter_test_server::HttpServer, executor_tests, scheduler::*, *};
use crate::{
    auth::webhook,
    channels::{resolver::ChannelResolver, routing_fixture},
    delivery::{
        custom::CustomDelivery,
        durable::DeliveryCoordinator,
        intents::DeliveryCommand,
        router::ShardedScheduler,
        sync::SyncRegistry,
        webhook_client::{WebhookClient, WebhookTransport},
    },
    http::{events::CustomIngressState, readiness::Readiness},
    ingress::{
        launch::{LaunchOutcome, LaunchRequest, TurnLauncher},
        routed::RoutedIngress,
        routing_plan::fixture,
        routing_store::{
            test_server::{Row, Server},
            AdmittedPlan, PlanStore,
        },
        IngressState,
    },
    media::service::MediaService,
    tools::ToolsState,
    turns::TurnTracker,
    DeliveryConfig, MediaToolProviders,
};
use cmaeg_core::{
    channel::Secret,
    seal::{seal_blob, ChannelPointer},
};
use futures_util::future::BoxFuture;
use serde_json::{json, Value};
use std::{collections::BTreeSet, sync::Arc, time::Duration};
use tokio::{
    sync::{mpsc, watch},
    task::JoinHandle,
};

pub(super) const AGENT_KEY: &str = "nyx_cmaeg_custom_http_default_fixture";

struct NoLegacy;
impl TurnLauncher for NoLegacy {
    fn launch(&self, _: LaunchRequest) -> BoxFuture<'_, LaunchOutcome> {
        panic!("selected routed HTTP ingress must never launch the legacy runner")
    }
}

pub(super) struct App {
    pub records: Server,
    pub provider: HttpServer,
    pub waiters: Arc<SyncRegistry>,
    pub url: String,
    pub journal: Journal,
    pub activity: Arc<crate::activity::ActivityStore>,
    store: PlanStore,
    stop: watch::Sender<bool>,
    worker: Option<JoinHandle<ScheduleReport>>,
    server: JoinHandle<()>,
    _legacy_delivery: mpsc::Receiver<DeliveryCommand>,
}
impl Drop for App {
    fn drop(&mut self) {
        self.server.abort();
        let _ = self.stop.send(true);
        if let Some(worker) = &self.worker {
            worker.abort();
        }
    }
}
impl App {
    pub async fn new(auto_final: bool) -> Self {
        Self::configured(auto_final, |_| {}, None).await
    }
    pub async fn configured(
        auto_final: bool,
        edit: impl FnOnce(&mut Value),
        media: Option<Arc<MediaService>>,
    ) -> Self {
        Self::with_delivery(auto_final, edit, media, None).await
    }
    pub async fn with_delivery(
        auto_final: bool,
        edit: impl FnOnce(&mut Value),
        media: Option<Arc<MediaService>>,
        webhook: Option<Arc<dyn WebhookTransport>>,
    ) -> Self {
        let records = Server::start_realtime().await;
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        let keys = Arc::new(fixture::keys());
        let record = uuid::Uuid::new_v4().to_string();
        let mut value = routing_fixture::routed_with_key(AGENT_KEY);
        value["record_id"] = record.clone().into();
        value["sources"] = json!([routing_fixture::custom()]);
        value["partition"]["key"] = "per_event".into();
        value["reply"]["sinks"] = json!([{"type":"sync"}]);
        value["reply"]["auto_final"] = auto_final.into();
        value["routing"]["consumers"] = json!([]);
        value["secrets"]["consumer_keys"] = json!([]);
        edit(&mut value);
        let definition = routing_fixture::definition(value);
        definition.validate().unwrap();
        let native_telegram = definition
            .sources
            .iter()
            .any(|source| matches!(source, cmaeg_core::channel::Source::Telegram { .. }));
        let pointer = ChannelPointer {
            v: 1,
            record_id: definition.record_id.clone(),
            record_url: records.records.record_url(&record).unwrap().to_string(),
        };
        let channel = pointer.seal_id(&keys).unwrap();
        records.memory.channels.lock().unwrap().insert(
            record.clone(),
            Row {
                owner: definition.owner.subject.clone(),
                blob: seal_blob(
                    &keys,
                    &serde_json::to_vec(&definition.to_blob_json()).unwrap(),
                )
                .unwrap(),
                expiry: now + 3600,
                version: 1,
            },
        );
        let resolver = ChannelResolver::new(
            Arc::new(records.records.clone()),
            keys.clone(),
            Default::default(),
        );
        let store = PlanStore::new(records.records.clone(), keys.clone());
        let activity = Arc::new(crate::activity::ActivityStore::new(
            records.records.clone(),
            keys.clone(),
        ));
        let observer = Arc::new(crate::activity::ObservedActivity::new(
            activity.clone(),
            Arc::new(crate::turns::observer::NoopObserver),
        ));
        let journal =
            Journal::new(records.records.clone(), keys.clone()).with_activity(activity.clone());
        let provider = HttpServer::start().await;
        {
            let mut memory = provider.memory.lock().unwrap();
            memory.config.replay_frames = Some(executor_tests::frames());
            memory.config.mirror_replay_metadata = true;
        }
        let providers = provider.client(true, Duration::from_secs(5));
        let builder = Arc::new(
            ProductionFrozenTurnBuilder::new(providers.clone(), keys.clone())
                .with_media(media.clone(), Default::default())
                .with_media_tool_providers(Arc::new(MediaToolProviders::from_iter(["cma_codex"
                    .parse()
                    .unwrap()]))),
        );
        let authority: Arc<dyn Authority> = Arc::new(resolver.clone());
        let port = Arc::new(HttpExecutionPort::new(
            providers.clone(),
            builder,
            store.clone(),
            journal.clone(),
            authority.clone(),
        ));
        let waiters = Arc::new(SyncRegistry::default());
        let mut custom = CustomDelivery::new("https://gateway.example".into(), waiters.clone())
            .with_activity(observer.clone());
        if let Some(webhook) = webhook {
            custom.webhooks = WebhookClient::with_transport(webhook);
        }
        let (tx, rx) = mpsc::channel(1);
        let delivery_sink = Arc::new(ShardedScheduler::new(tx));
        let delivery = RoutedDelivery::new(
            store.clone(),
            journal.clone(),
            authority,
            DeliveryCoordinator::new(delivery_sink.clone()).with_custom_stream(custom),
        );
        let tracker = TurnTracker::new(Readiness::initialized());
        let tools = ToolsState {
            discord_thread_history: None,
            telegram_topics: None,
            discord_threads: None,
            native_lark_conversation_history: None,
            discord_conversation_history: None,
            complete_replies_enabled: false,
            commonmark_relay: Default::default(),
            whatsapp_templates_enabled: false,
            discord_interactions_enabled: false,
            native_discord_media_controls_enabled: false,
            native_discord_media_card_edits_enabled: false,
            native_discord_media_card_replacements_enabled: false,
            native_discord_media_card_sets_enabled: false,
            whatsapp_interactions_enabled: false,
            native_whatsapp_interactions_enabled: false,
            native_whatsapp_media_controls_enabled: false,
            consumer_interactions_enabled: false,
            telegram_effects_issuance_enabled: false,
            native_telegram_media_controls_enabled: false,
            native_telegram_media_control_edits_enabled: false,
            native_media_edits: None,
            native_whatsapp_forms: None,
            native_typing: None,
            native_lark_effects: None,
            lark_effects_issuance_enabled: false,
            lark_effects: None,
            telegram_effects: None,
            native_media_enabled: false,
            native_lark_outbound_media_enabled: false,
            native_lark_sticker_replies_enabled: false,
            native_lark_media_controls_enabled: false,
            native_lark_multi_image_controls_enabled: false,
            native_lark_media_card_replacements_enabled: false,
            native_media_presentations_enabled: false,
            native_discord_voice_enabled: false,
            native_telegram_structured_replies_enabled: false,
            native_whatsapp_structured_replies_enabled: false,
            native_telegram_albums_enabled: false,
            native_lark_thread_start_enabled: false,
            native_whatsapp_outbound_media_enabled: false,
            whatsapp_reactions: None,
            native_whatsapp_templates_enabled: false,
            native_whatsapp_groups_enabled: false,
            native_discord_edits_enabled: false,
            discord_effects_issuance_enabled: false,
            discord_effects: None,
            relay_text_edits_enabled: false,
            lark_cards_enabled: false,
            resolver: resolver.clone(),
            keys: keys.clone(),
            providers,
            delivery: delivery_sink,
            routed_replies: Some(Arc::new(delivery.tool_replies())),
            tracker: tracker.clone(),
            public_origin: "https://gateway.example".into(),
            public_api_url: "https://gateway.example".into(),
            public_api_aliases: vec![],
            advertise_public_api_url: true,
            media: media.clone(),
            downloads: Default::default(),
        };
        let executor = Arc::new(RoutedTargetExecutor::new(
            port,
            delivery,
            tracker,
            &DeliveryConfig::default(),
        ));
        let (scheduler, schedule) = Scheduler::new(
            Arc::new(store.clone()),
            executor,
            ScheduleConfig {
                sweep_delay: Duration::from_millis(50),
                drain_timeout: Duration::from_secs(5),
                ..Default::default()
            },
        )
        .unwrap();
        let routed = RoutedIngress::new(
            store.clone(),
            schedule,
            keys.clone(),
            if native_telegram || definition.durable_custom_execution {
                BTreeSet::new()
            } else {
                BTreeSet::from([record])
            },
            BTreeSet::from(["cma_codex".into()]),
            BTreeSet::from(["cma_codex".into()]),
        )
        .with_sync_delivery(true);
        let ingress = IngressState::new(resolver, keys, Default::default(), Arc::new(NoLegacy))
            .unwrap()
            .with_activity_observer(observer)
            .with_media_enabled(media.is_some())
            .with_routed(Some(Arc::new(routed)));
        let router = if native_telegram {
            crate::http::telegram::router(ingress)
        } else {
            crate::http::events::router(CustomIngressState {
                ingress,
                waiters: waiters.clone(),
            })
        }
        .merge(crate::http::tools::router(tools));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let path = if native_telegram {
            format!("/v1/telegram/{channel}")
        } else {
            format!("/v1/channels/{channel}/events")
        };
        let url = format!("http://{}{path}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let (stop, stopped) = watch::channel(false);
        let worker = Some(tokio::spawn(scheduler.run(stopped)));
        Self {
            records,
            provider,
            waiters,
            url,
            journal,
            activity,
            store,
            stop,
            worker,
            server,
            _legacy_delivery: rx,
        }
    }
    pub fn event(&self, id: &str) -> Value {
        json!({"specversion":"1.0","id":id,"source":"https://producer.example/build",
            "type":"build.completed","subject":"custom-verification","data":{"result":"passed"}})
    }
    pub fn request(&self, body: String, stream: bool) -> reqwest::RequestBuilder {
        let timestamp = time::OffsetDateTime::now_utc().unix_timestamp().to_string();
        let signature = webhook::signature(
            &Secret::new("Zml4dHVyZQ=="),
            "custom-http-test",
            &timestamp,
            body.as_bytes(),
        );
        reqwest::Client::new()
            .post(format!(
                "{}{}",
                self.url,
                if stream { "?mode=stream" } else { "" }
            ))
            .header("content-type", "application/cloudevents+json")
            .header("webhook-id", "custom-http-test")
            .header("webhook-timestamp", timestamp)
            .header("webhook-signature", signature)
            .timeout(Duration::from_secs(10))
            .body(body)
    }
    pub async fn plans(&self) -> Vec<AdmittedPlan> {
        self.store.recover_all_page(None, 32).await.unwrap().plans
    }
    pub async fn drain(&mut self) {
        self.stop.send(true).unwrap();
        let report = tokio::time::timeout(Duration::from_secs(6), self.worker.take().unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(report.aborted, 0);
        assert!(!report.recovery_failed);
    }
}
