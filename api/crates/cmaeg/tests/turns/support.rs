use super::fake_provider::{FakeProvider, FakeProviderConfig};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use cmaeg::{
    channels::resolver::ResolvedChannel,
    http::readiness::Readiness,
    ingress::launch::LaunchRequest,
    providers::{ProviderClient, ProviderRegistry},
    turns::{
        outcomes::{DeliveryCommand, TurnOutcome},
        TurnRunner, TurnTracker,
    },
    TurnConfig,
};
use cmaeg_core::{
    activity::{Activity, ActivityKind},
    channel::{ChannelDefinition, Secret},
    ids::{ConversationId, IdempotencyKey},
    seal::{ChannelPointer, EventRef, EventRefPayload, MasterKeyRing, ReplyAddress},
};
use serde_json::json;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::mpsc;

pub struct Fixture {
    pub fake: FakeProvider,
    pub runner: TurnRunner,
    pub tracker: Arc<TurnTracker>,
    pub readiness: Readiness,
    pub outcomes: mpsc::Receiver<DeliveryCommand>,
    pub channel: Arc<ResolvedChannel>,
    keys: MasterKeyRing,
}
impl Fixture {
    pub async fn new(fake_config: FakeProviderConfig, config: TurnConfig, capacity: usize) -> Self {
        let fake = FakeProvider::start(fake_config).await;
        // Two slugs on one fake, so a channel can be rebound to `nyxbot`.
        let entry = |slug: &str| json!({"slug":slug,"base_url":fake.base_url(),"kind":"responses_http","paths":{"agent_card":"/.well-known/agent-card.json","binding":"/api/v1/cmaeg/bindings/{binding_id}","conversation":"/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}","responses":"/api/v2/responses"},"max_inflight":capacity});
        let registry =
            ProviderRegistry::parse(&json!([entry("cma_codex"), entry("nyxbot")]).to_string())
                .unwrap();
        let client = Arc::new(
            ProviderClient::new(
                registry,
                Duration::from_secs(1),
                Duration::from_secs(2),
                capacity,
            )
            .unwrap(),
        );
        let readiness = Readiness::initialized();
        let tracker = TurnTracker::new(readiness.clone());
        let (delivery, outcomes) = mpsc::channel(64);
        let delivery = Arc::new(cmaeg::delivery::router::ShardedScheduler::new(delivery));
        let runner = TurnRunner::new(client, tracker.clone(), delivery, config);
        let keys = MasterKeyRing::parse(&format!("1:{}", STANDARD.encode([1; 32]))).unwrap();
        let mut definition: ChannelDefinition = serde_json::from_str(include_str!(
            "../../../cmaeg-core/tests/fixtures/channel.json"
        ))
        .unwrap();
        definition.profile_digest = definition.profile.digest();
        definition.secrets.provider_credential = Some(Secret::new("runtime-test-provider-bearer"));
        let pointer = ChannelPointer {
            v: 1,
            record_id: definition.record_id.clone(),
            record_url: "https://cma.example/api/v1/gateway/channel-records/record-1".into(),
        };
        let id = pointer.seal_id(&keys).unwrap();
        let channel = Arc::new(ResolvedChannel {
            id,
            pointer,
            definition,
            agent_key_hash: [0; 32],
            consumer_keys: Vec::new(),
            loaded_at: Instant::now(),
            version: 1,
        });
        Self {
            fake,
            runner,
            tracker,
            readiness,
            outcomes,
            channel,
            keys,
        }
    }
    pub fn request(&self, event: &str) -> LaunchRequest {
        self.request_with_kind(event, None)
    }
    pub fn request_with_kind(&self, event: &str, kind: Option<ActivityKind>) -> LaunchRequest {
        let mut activity: Activity = serde_json::from_str(include_str!(
            "../../../cmaeg-core/tests/fixtures/activity.json"
        ))
        .unwrap();
        activity.event_id = event.parse().unwrap();
        if let Some(kind) = kind {
            activity.kind = kind;
        }
        let partition = activity.partition_key(&self.channel.definition.partition);
        let conversation_id = ConversationId::derive(&self.keys, &self.channel.id, 1, &partition);
        let event_ref = EventRef::seal(
            &EventRefPayload {
                telegram_admission: None,
                turn_trace: None,
                v: 1,
                activity_sha256: Some(cmaeg_core::seal::event_ref::activity_digest(&activity)),
                channel_id: self.channel.id.clone(),
                record_id: self.channel.pointer.record_id.clone(),
                event_id: activity.event_id.clone(),
                partition_key: partition,
                conversation_id: conversation_id.clone(),
                expires_at: time::OffsetDateTime::now_utc().unix_timestamp() + 1800,
                reply: ReplyAddress::None,
                consumer: None,
                lineage: None,
            },
            &self.keys,
        )
        .unwrap();
        LaunchRequest {
            timing: None,
            consumer: None,
            trace: tracing::Span::current(),
            turn_trace: None,
            channel: self.channel.clone(),
            idempotency_key: IdempotencyKey::from_event(&activity.event_id),
            activity,
            conversation_id,
            event_ref,
            received_at: Instant::now(),
        }
    }
    pub async fn terminal(&mut self) -> Vec<TurnOutcome> {
        let mut outcomes = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let DeliveryCommand::StreamOutcome { outcome, .. } =
                    self.outcomes.recv().await.unwrap()
                else {
                    panic!("expected stream outcome");
                };
                let done = outcome.terminal();
                outcomes.push(outcome);
                if done {
                    break;
                }
            }
        })
        .await
        .unwrap();
        outcomes
    }
}
