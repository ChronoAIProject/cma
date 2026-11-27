//! Shared real-Mongo delivery fixture; each node owns a separate runtime client.
use super::{
    test_support::{self, Current},
    *,
};
use crate::{
    auth::AgentCaller,
    delivery::{
        durable::{receipt_for, DeliveryCoordinator},
        intents::{DeliveryCommand, DeliveryOutcome, IntentReceipt},
        router::ShardedScheduler,
    },
    ingress::{
        routing_plan::{self, fixture, OrdinaryPlan, RoutingOptions},
        routing_store::{AdmittedPlan, Candidate},
    },
    replicas::mongo_fixture::{Fixture, Node},
};
use cmaeg_core::{
    ids::ConversationId,
    message::OutboundMessage,
    seal::{EventRef, EventRefPayload, ReplyTargetPayload},
};
use std::{
    collections::BTreeSet,
    sync::{atomic::AtomicUsize, Arc, Mutex},
    time::Duration,
};
use tokio::sync::mpsc;

pub(super) struct Harness {
    pub database: Fixture,
    pub nodes: Vec<Node>,
    pub plan: AdmittedPlan,
    pub current: Arc<Current>,
    pub tools: Vec<Arc<RoutedToolReplies>>,
    pub rx: mpsc::Receiver<DeliveryCommand>,
}
impl Harness {
    pub async fn new() -> Self {
        let request = fixture::request(
            |v| {
                v["routing"]["default_targets"] = serde_json::json!(["default"]);
                v["secrets"]["telegram_bot_token"] = serde_json::json!("77:fixture-only");
            },
            fixture::message("replica tool fixture"),
        );
        Self::with_request(request, false).await
    }
    pub async fn compatibility() -> Self {
        let request = fixture::request(
            |v| {
                v["routing"] = serde_json::Value::Null;
                v["secrets"]["consumer_keys"] = serde_json::json!([]);
                v["secrets"]["telegram_bot_token"] = serde_json::json!("77:fixture-only");
            },
            fixture::message("replica compatibility fixture"),
        );
        Self::with_request(request, false).await
    }
    pub async fn media() -> Self {
        let mut activity = fixture::message("replica media fixture");
        let cmaeg_core::activity::ActivityKind::Message { attachments, .. } = &mut activity.kind
        else {
            unreachable!()
        };
        attachments.push(serde_json::from_value(serde_json::json!({
            "reference":"file","kind":"file","filename":"file.txt",
            "source":{"type":"telegram","file_id":"fixture_file","file_unique_id":"fixture_unique"}
        })).unwrap());
        let request = fixture::request(
            |v| {
                v["routing"]["default_targets"] = serde_json::json!(["default"]);
                v["provider"]["target"] =
                    serde_json::json!({"type":"existing_agent","agent_id":"agt_existing"});
                v["secrets"]["telegram_bot_token"] = serde_json::json!("77:fixture-only");
            },
            activity,
        );
        Self::with_request(request, true).await
    }
    pub(super) async fn with_request(
        request: crate::ingress::launch::LaunchRequest,
        media: bool,
    ) -> Self {
        Self::with_http_request(request, media, None).await
    }
    pub(super) async fn with_http_request(
        mut request: crate::ingress::launch::LaunchRequest,
        media: bool,
        http: Option<(&crate::tools::fake_cma::FakeCma, AgentCaller)>,
    ) -> Self {
        let database = Fixture::new().await;
        if media {
            database.enable_media().await;
        }
        let keys = database.keys();
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        let mut event = request
            .event_ref
            .open(&fixture::keys(), fixture::NOW)
            .unwrap()
            .payload;
        if let Some((backend, caller)) = http {
            let records =
                crate::records::RecordClient::new(&backend.base_url, "cma-service").unwrap();
            let channel = Arc::get_mut(&mut request.channel).unwrap();
            channel.pointer.record_url = records
                .record_url(channel.pointer.record_id.as_str())
                .unwrap()
                .to_string();
            channel.id = channel.pointer.seal_id(&keys).unwrap();
            channel.agent_key_hash = caller.key_hash;
            event.channel_id = channel.id.clone();
            backend.seed(
                channel.pointer.record_id.as_str(),
                &channel.definition.owner.subject,
                cmaeg_core::seal::seal_blob(
                    &keys,
                    channel.definition.to_blob_json().to_string().as_bytes(),
                )
                .unwrap(),
            );
        }
        event.expires_at = now + 900;
        event.conversation_id =
            ConversationId::derive(&keys, &request.channel.id, 1, &event.partition_key);
        request.conversation_id = event.conversation_id.clone();
        request.event_ref = EventRef::seal(&event, &keys).unwrap();
        let candidate = if request.channel.definition.routing.is_none() {
            Candidate::prepare_compatibility(&request, b"replica-tool-event", &keys, now).unwrap()
        } else {
            let OrdinaryPlan::Routed(plan) = routing_plan::plan(
                &request,
                &keys,
                now,
                RoutingOptions {
                    enabled: true,
                    attributes_signed: true,
                    non_human_providers: &BTreeSet::from(["cma_codex".into()]),
                },
            )
            .unwrap() else {
                panic!("routed fixture required")
            };
            Candidate::prepare(&request, &plan, b"replica-tool-event", &keys, now).unwrap()
        };
        let nodes = vec![database.node(21).await, database.node(22).await];
        let plan = nodes[0].plans().admit(&candidate).await.unwrap();
        if http.is_none() {
            Arc::get_mut(&mut request.channel).unwrap().agent_key_hash = [9; 32];
        }
        let current = Arc::new(Current {
            channel: Mutex::new(request.channel),
            calls: AtomicUsize::new(0),
            revoke_at: AtomicUsize::new(usize::MAX),
        });
        let (tx, rx) = mpsc::channel(8);
        let tools = nodes
            .iter()
            .map(|node| {
                Arc::new(
                    RoutedToolReplies::new(
                        node.plans(),
                        node.journal(),
                        current.clone(),
                        DeliveryCoordinator::new(Arc::new(ShardedScheduler::new(tx.clone()))),
                    )
                    .with_replica_relay()
                    .unwrap(),
                )
            })
            .collect();
        Self {
            database,
            nodes,
            plan,
            current,
            tools,
            rx,
        }
    }
    pub fn target(&self) -> &crate::ingress::routing_store::StoredTarget {
        &self.plan.targets[0]
    }
    pub fn event(&self) -> EventRefPayload {
        model::delivery_event(&self.plan, self.target())
    }
    pub fn capability(&self) -> ReplyTargetPayload {
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        ReplyTargetPayload {
            v: 1,
            provider: self.target().launch.provider.slug.clone(),
            binding_id: self.target().launch.provider.binding_id.clone(),
            agent_key_hash: [9; 32],
            issued_at: now,
            expires_at: now + 3600,
            event: self.event(),
        }
    }
    pub async fn acquire(&self) -> (usize, Lease) {
        let until = tokio::time::Instant::now() + Duration::from_secs(25);
        loop {
            for (index, node) in self.nodes.iter().enumerate() {
                if node.worker.is_none() {
                    continue;
                }
                match node
                    .journal()
                    .claim(&self.plan, self.target(), "tool-fixture")
                    .await
                {
                    Ok(lease) => return (index, lease),
                    Err(ExecutionError::Busy | ExecutionError::LostLease) => {}
                    Err(error) => panic!("claim refused: {error:?}"),
                }
            }
            assert!(tokio::time::Instant::now() < until, "ownership unavailable");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    pub async fn accepted(&self) -> (usize, Lease) {
        let (owner, mut lease) = self.acquire().await;
        let journal = self.nodes[owner].journal();
        let channel = self.current.channel.lock().unwrap().clone();
        let mut body = test_support::frozen(&self.plan, self.target(), &channel, 32);
        body.event_context.event_ref =
            FrozenTurn::event_ref(&self.plan, self.target(), &self.database.keys()).unwrap();
        if !crate::turns::render_media::attachments(&self.plan.activity).is_empty() {
            body.metadata.insert(
                super::media_schedule::KEY.into(),
                super::media_schedule::MediaSchedule::capture(
                    &self.plan.activity,
                    &Default::default(),
                )
                .unwrap()
                .encode()
                .unwrap(),
            );
        }
        journal
            .freeze(&mut lease, &self.plan, self.target(), &body)
            .await
            .unwrap();
        if !crate::turns::render_media::attachments(&self.plan.activity).is_empty() {
            MediaWorkIndex::new(journal.clone(), self.nodes[owner].plans())
                .prepare(&self.plan, self.target(), &body)
                .await
                .unwrap();
        }
        journal
            .transition(
                &mut lease,
                Phase::Accepted {
                    response_id: "fixture-response".into(),
                    durable_cursor: None,
                },
            )
            .await
            .unwrap();
        (owner, lease)
    }
    pub async fn finish_parent(&self, owner: usize, lease: &mut Lease) {
        let delivery = RoutedDelivery::new(
            self.nodes[owner].plans(),
            self.nodes[owner].journal(),
            self.current.clone(),
            self.tools[owner].coordinator.clone(),
        );
        let frame = test_support::recovery_frame("fixture-response", 0, true);
        assert!(matches!(
            delivery
                .deliver_frame(
                    &self.plan.identity,
                    &self.target().id,
                    lease,
                    &frame,
                    ReplayEffect::NoEffect
                )
                .await
                .unwrap(),
            FrameDelivery::Settled { .. }
        ));
        self.nodes[owner]
            .journal()
            .complete(lease, "fixture-response")
            .await
            .unwrap();
        self.nodes[owner]
            .partitions
            .complete(lease.partition.as_ref().unwrap())
            .await
            .unwrap();
    }
    pub async fn cleanup(self) {
        drop(self.tools);
        drop(self.nodes);
        self.database.cleanup().await;
    }
}
pub(super) fn caller() -> AgentCaller {
    AgentCaller { key_hash: [9; 32] }
}
pub(super) fn message() -> OutboundMessage {
    let ReplayEffect::Reply(message) = super::delivery_tests::effect() else {
        unreachable!()
    };
    *message
}
pub(super) fn receipt() -> IntentReceipt {
    receipt_for(DeliveryOutcome::Confirmed {
        platform_message_id: Some("replica-fixture-message".into()),
    })
}
