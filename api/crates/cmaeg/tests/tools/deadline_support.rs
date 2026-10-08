//! Real handlers and delivery engine with no socket I/O, even during setup.
use super::{fake_http::FakeHttp, recording_fixtures as fixtures, routing_fixture};
use crate::{
    auth::AgentCaller,
    channels::resolver::{ChannelResolver, ResolvedChannel},
    delivery::{
        custom::CustomDelivery, engine::DeliveryEngine, intents::DeliveryCommand,
        relay_client::RelayClient, router::ShardedScheduler,
    },
    http::readiness::Readiness,
    providers::{ProviderClient, ProviderRegistry},
    records::RecordClient,
    tools::ToolsState,
    turns::TurnTracker,
    DeliveryConfig,
};
use axum::{
    body::{to_bytes, Body},
    extract::Request,
    Router,
};
use cmaeg_core::{
    activity::{Activity, ActivityRouting, ConsumerTarget, RouteResolution},
    channel::{ChannelDefinition, SourceId},
    ids::{ConsumerId, ConsumerName, MessageRef},
    seal::{ChannelPointer, EventRef, EventRefPayload, MessageRefPayload},
};
use hyper::service::Service;
use hyper_util::service::TowerToHyperService;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{convert::Infallible, sync::Arc, time::Duration};
use tokio::sync::mpsc;

pub struct App {
    pub state: ToolsState,
    pub channel: Arc<ResolvedChannel>,
    pub http: Arc<FakeHttp>,
    pub engine: Option<DeliveryEngine>,
}

pub fn consumer(index: usize) -> ConsumerId {
    if index == 0 {
        ConsumerId::default_consumer()
    } else {
        ConsumerId::parse(&format!("csm_{index:012x}")).unwrap()
    }
}
pub fn key(index: usize) -> String {
    format!("nyx_fixture_deadline_{}", consumer(index))
}
pub fn caller(index: usize) -> AgentCaller {
    AgentCaller {
        key_hash: Sha256::digest(key(index).as_bytes()).into(),
    }
}

impl App {
    pub fn new(routed: bool, flag: bool, http: FakeHttp) -> Self {
        let keys = fixtures::keys();
        let mut value = routing_fixture::routed();
        value["record_id"] = "11111111-1111-4111-8111-111111111111".into();
        value["sources"] = json!([
            routing_fixture::relay("telegram", "route-1"),
            routing_fixture::telegram()
        ]);
        value["reply"]["sinks"] = json!([{"type":"source"}]);
        value["reply"]["ack_after_ms"] = 0.into();
        value["secrets"]["channel_key"] = fixtures::CHANNEL_KEY.into();
        value["secrets"]["agent_key_hash"] = routing_fixture::hash(&key(0)).into();
        value["secrets"]["provider_credential"] = key(0).into();
        let consumers: Vec<_> = (1..8)
            .map(|i| {
                let mut entry = routing_fixture::consumer(
                    consumer(i).as_str(),
                    &format!("worker{i}"),
                    "cma_codex",
                );
                entry["agent_key_hash"] = routing_fixture::hash(&key(i)).into();
                entry
            })
            .collect();
        value["routing"]["consumers"] = json!(consumers);
        value["routing"]["max_targets"] = 8.into();
        value["secrets"]["consumer_keys"] =
            json!((1..8).map(|i| (consumer(i), key(i))).collect::<Vec<_>>());
        if !routed {
            value.as_object_mut().unwrap().remove("routing");
            value["secrets"]
                .as_object_mut()
                .unwrap()
                .remove("consumer_keys");
        }
        let definition: ChannelDefinition = serde_json::from_value(value).unwrap();
        definition.validate().unwrap();
        let records = Arc::new(RecordClient::new("http://127.0.0.1:1", "fixture").unwrap());
        let pointer = ChannelPointer {
            v: 1,
            record_id: definition.record_id.clone(),
            record_url: records
                .record_url(definition.record_id.as_str())
                .unwrap()
                .to_string(),
        };
        let channel = Arc::new(ResolvedChannel {
            id: pointer.seal_id(&keys).unwrap(),
            pointer,
            definition,
            agent_key_hash: caller(0).key_hash,
            consumer_keys: (0..if routed { 8 } else { 1 })
                .map(|i| (consumer(i), caller(i).key_hash))
                .collect(),
            loaded_at: std::time::Instant::now(),
            version: 1,
        });
        let resolver = ChannelResolver::new(records, keys.clone(), Default::default());
        resolver.cache_for_test(channel.clone());
        let registry = ProviderRegistry::parse("[]").unwrap();
        let providers = Arc::new(
            ProviderClient::new(registry, Duration::from_secs(1), Duration::from_secs(1), 8)
                .unwrap(),
        );
        let (inbound, commands) = mpsc::channel(256);
        let state = ToolsState {
            resolver,
            keys,
            providers,
            delivery: Arc::new(ShardedScheduler::new(inbound)),
            tracker: TurnTracker::new(Readiness::initialized()),
            public_origin: "https://gateway.example".into(),
            media: None,
            downloads: Default::default(),
            multi_consumer_enabled: flag,
        };
        let http = Arc::new(http);
        let engine = start_engine(commands, &state, http.clone());
        Self {
            state,
            channel,
            http,
            engine: Some(engine),
        }
    }
    pub fn event(&self, index: usize) -> EventRefPayload {
        let mut event = fixtures::event(
            &self.channel,
            &self.activity(),
            index as u8,
            fixtures::relay_reply(),
        );
        event.consumer = (index != 0).then(|| consumer(index));
        event.expires_at = time::OffsetDateTime::now_utc().unix_timestamp() + 600;
        event
    }
    pub fn activity(&self) -> Activity {
        let mut activity = fixtures::relay_activity();
        if self.channel.definition.routing.is_some() && self.state.multi_consumer_enabled {
            activity.routing = Some(ActivityRouting {
                source_id: SourceId::parse("relay_telegram").unwrap(),
                resolution: RouteResolution::Default,
                rule_id: None,
                addressing_bytes: 0,
                causation: None,
                targets: (0..8)
                    .map(|i| ConsumerTarget {
                        id: consumer(i),
                        name: ConsumerName::parse(&if i == 0 {
                            "lead".into()
                        } else {
                            format!("worker{i}")
                        })
                        .unwrap(),
                    })
                    .collect(),
            });
        }
        activity.validate().unwrap();
        activity
    }
    pub fn reference(&self, index: usize) -> EventRef {
        EventRef::seal(&self.event(index), &self.state.keys).unwrap()
    }
    pub fn message_ref(&self, index: usize) -> MessageRef {
        MessageRef::seal(
            &MessageRefPayload {
                channel_id: self.channel.id.clone(),
                event_id: self.event(index).event_id,
                platform_message_id: "101".into(),
                route_id: "telegram:77:-1009954455667".into(),
                interaction_record_id: None,
                consumer: (index != 0).then(|| consumer(index)),
            },
            &self.state.keys,
        )
        .unwrap()
    }
    pub fn router(&self) -> Router {
        crate::http::tools::router(self.state.clone()).merge(crate::mcp::router(self.state.clone()))
    }
    pub async fn finish(mut self) {
        self.drain().await;
    }
    pub async fn drain(&mut self) {
        assert!(
            self.engine
                .take()
                .unwrap()
                .drain(std::time::Instant::now() + Duration::from_secs(3600))
                .await
        );
    }
}

pub fn start_engine(
    commands: mpsc::Receiver<DeliveryCommand>,
    state: &ToolsState,
    http: Arc<FakeHttp>,
) -> DeliveryEngine {
    let config = DeliveryConfig {
        multi_consumer_enabled: state.multi_consumer_enabled,
        ..Default::default()
    };
    let telegram = crate::telegram::TelegramClient::production(config.reply_timeout)
        .unwrap()
        .with_transport(http.clone());
    let relay = RelayClient::new("https://nyx.example", config.reply_timeout)
        .unwrap()
        .with_transport(http)
        .with_telegram(telegram);
    DeliveryEngine::start(
        commands,
        state.keys.clone(),
        relay,
        config,
        state.tracker.clone(),
        CustomDelivery::new(state.public_origin.clone(), Arc::default()),
    )
}

pub async fn call(
    router: Router,
    index: usize,
    path: &str,
    value: Value,
    delay: Duration,
) -> (u16, Value) {
    let body = Body::from_stream(futures_util::stream::once(async move {
        tokio::time::sleep(delay).await;
        Ok::<_, Infallible>(bytes::Bytes::from(serde_json::to_vec(&value).unwrap()))
    }));
    let request = Request::post(path)
        .header("authorization", format!("Bearer {}", key(index)))
        .header("content-type", "application/json")
        .body(body)
        .unwrap();
    let response = TowerToHyperService::new(router)
        .call(request)
        .await
        .unwrap();
    let status = response.status().as_u16();
    let body = to_bytes(response.into_body(), 65536).await.unwrap();
    (status, serde_json::from_slice(&body).unwrap())
}
