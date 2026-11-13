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
        let database = Fixture::new().await;
        let keys = database.keys();
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        let mut request = fixture::request(
            |v| {
                v["routing"]["default_targets"] = serde_json::json!(["default"]);
                v["secrets"]["telegram_bot_token"] = serde_json::json!("77:fixture-only");
            },
            fixture::message("replica tool fixture"),
        );
        let mut event = request
            .event_ref
            .open(&fixture::keys(), fixture::NOW)
            .unwrap()
            .payload;
        event.expires_at = now + 900;
        event.conversation_id =
            ConversationId::derive(&keys, &request.channel.id, 1, &event.partition_key);
        request.conversation_id = event.conversation_id.clone();
        request.event_ref = EventRef::seal(&event, &keys).unwrap();
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
        let candidate =
            Candidate::prepare(&request, &plan, b"replica-tool-event", &keys, now).unwrap();
        let nodes = vec![database.node(21).await, database.node(22).await];
        let plan = nodes[0].plans().admit(&candidate).await.unwrap();
        Arc::get_mut(&mut request.channel).unwrap().agent_key_hash = [9; 32];
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
        journal
            .freeze(&mut lease, &self.plan, self.target(), &body)
            .await
            .unwrap();
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
