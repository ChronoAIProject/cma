//! Real shared storage and independently authenticated replicas for #2171.
use super::{
    test_support::{self, Current},
    *,
};
use crate::{
    ingress::{
        routing_plan::{self, fixture, OrdinaryPlan, RoutingOptions},
        routing_store::{AdmittedPlan, Candidate},
    },
    replicas::mongo_fixture::{Fixture, Node},
};
use cmaeg_core::{
    ids::{ConversationId, EventId},
    seal::EventRef,
};
use std::{
    collections::BTreeSet,
    sync::{atomic::AtomicUsize, Arc, Mutex},
    time::Duration,
};

pub(crate) struct ReleaseHarness {
    pub database: Fixture,
    pub nodes: Vec<Node>,
    pub plans: Vec<AdmittedPlan>,
    pub current: Arc<Current>,
    pub partition: String,
}

impl ReleaseHarness {
    pub async fn new() -> Self {
        Self::with_successors(true).await
    }

    pub async fn with_successors(enqueue_successors: bool) -> Self {
        let database = Fixture::new().await;
        let nodes = vec![database.node(51).await, database.node(52).await];
        let mut plans = Vec::new();
        let mut channel = None;
        for index in 0..3 {
            let mut activity = fixture::message("release regression fixture");
            activity.event_id = EventId::new(format!("eg2171-release-{index}")).unwrap();
            let mut request = fixture::request(
                |v| {
                    v["routing"]["default_targets"] = serde_json::json!(["default"]);
                    v["secrets"]["telegram_bot_token"] = serde_json::json!("77:fixture-only");
                },
                activity,
            );
            let now = time::OffsetDateTime::now_utc().unix_timestamp();
            let mut event = request
                .event_ref
                .open(&fixture::keys(), fixture::NOW)
                .unwrap()
                .payload;
            // Real server time expires only the first head. No database repair
            // or forged lease is needed to reach the operator-release boundary.
            event.expires_at = now + if index == 0 { 15 } else { 900 };
            event.conversation_id = ConversationId::derive(
                &database.keys(),
                &request.channel.id,
                1,
                &event.partition_key,
            );
            request.conversation_id = event.conversation_id.clone();
            request.event_ref = EventRef::seal(&event, &database.keys()).unwrap();
            let OrdinaryPlan::Routed(routed) = routing_plan::plan(
                &request,
                &database.keys(),
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
                Candidate::prepare(&request, &routed, b"release-fixture", &database.keys(), now)
                    .unwrap();
            let plan = crate::ingress::routing_store::PlanStore::new(
                nodes[0].journal().records.clone(),
                database.keys(),
            )
            .admit(&candidate)
            .await
            .unwrap();
            if index == 0 || enqueue_successors {
                nodes[0].partitions.enqueue(&plan).await.unwrap();
            }
            plans.push(plan);
            Arc::get_mut(&mut request.channel).unwrap().agent_key_hash = [9; 32];
            channel = Some(request.channel);
        }
        let current = Arc::new(Current {
            channel: Mutex::new(channel.unwrap()),
            calls: AtomicUsize::new(0),
            revoke_at: AtomicUsize::new(usize::MAX),
        });
        let mut h = Self {
            database,
            nodes,
            plans,
            current,
            partition: String::new(),
        };
        let (owner, mut lease) = h.claim(0).await;
        let journal = h.nodes[owner].journal();
        let plan = &h.plans[0];
        let target = &plan.targets[0];
        let mut body = test_support::frozen(plan, target, &h.current.channel.lock().unwrap(), 32);
        body.event_context.event_ref =
            FrozenTurn::event_ref(plan, target, &h.database.keys()).unwrap();
        journal
            .freeze(&mut lease, plan, target, &body)
            .await
            .unwrap();
        journal
            .transition(&mut lease, Phase::Starting)
            .await
            .unwrap();
        journal
            .transition(&mut lease, Phase::Unknown)
            .await
            .unwrap();
        h.partition = h.nodes[owner]
            .partitions
            .guard(lease.partition.as_ref().unwrap())
            .await
            .unwrap()
            .id;
        journal.release(&mut lease).await.unwrap();
        h
    }

    pub async fn claim(&self, index: usize) -> (usize, Lease) {
        let plan = &self.plans[index];
        let until = tokio::time::Instant::now() + Duration::from_secs(25);
        loop {
            for (owner, node) in self.nodes.iter().enumerate() {
                match node
                    .journal()
                    .claim(plan, &plan.targets[0], "release-fixture")
                    .await
                {
                    Ok(lease) => return (owner, lease),
                    Err(ExecutionError::Busy | ExecutionError::LostLease) => {}
                    Err(error) => panic!("claim failed: {error:?}"),
                }
            }
            assert!(
                tokio::time::Instant::now() < until,
                "release fixture ownership unavailable"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    pub async fn expired(&self) {
        let until = tokio::time::Instant::now() + Duration::from_secs(20);
        let expiry = self.plans[0].targets[0].launch.event.expires_at;
        while (self.nodes[0].journal().clock)() <= expiry {
            assert!(
                tokio::time::Instant::now() < until,
                "server clock did not reach event expiry"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    pub async fn state(&self, index: usize) -> TargetState {
        self.nodes[1]
            .journal()
            .read::<TargetState>(&self.plans[index].targets[0].id)
            .await
            .unwrap()
            .unwrap()
            .0
    }

    pub async fn assert_blocked(&self, index: usize) {
        let plan = &self.plans[index];
        for node in &self.nodes {
            assert!(
                matches!(
                    node.partitions.claim(plan, &plan.targets[0]).await,
                    Err(crate::replicas::Error::Busy | crate::replicas::Error::Lost)
                ),
                "unreleased FIFO successor gained authority"
            );
        }
    }

    pub fn service(&self, node: usize, enabled: bool) -> unresolved::UnresolvedRelease {
        unresolved::UnresolvedRelease::new(
            self.nodes[node].journal(),
            self.nodes[node].plans(),
            self.nodes[node].partitions.clone(),
            enabled,
        )
    }

    pub async fn request(&self) -> unresolved::ReleaseRequest {
        let plan = &self.plans[0];
        let preview = self
            .service(0, true)
            .preview(
                &plan.identity.channel_id,
                &plan.identity.owner,
                &self.partition,
            )
            .await
            .unwrap();
        unresolved::ReleaseRequest {
            partition_id: self.partition.clone(),
            expected_head: preview.head,
            expected_epoch: preview.epoch,
            expected_version: preview.version,
            operation_id: uuid::Uuid::new_v4().to_string(),
            provider_terminal_confirmed: true,
            suppress_delayed_replies: true,
        }
    }

    pub async fn release(
        &self,
        node: usize,
        request: &unresolved::ReleaseRequest,
    ) -> Result<unresolved::ReleaseResult, ExecutionError> {
        let plan = &self.plans[0];
        self.service(node, true)
            .release(
                &plan.identity.channel_id,
                &plan.identity.owner,
                &"a".repeat(64),
                request,
            )
            .await
    }

    pub async fn record(&self, id: &str, ephemeral: bool) -> (serde_json::Value, u64) {
        crate::replicas::store::Store::new(
            self.nodes[1].journal().records.clone(),
            self.database.keys(),
        )
        .unwrap()
        .read(id, ephemeral)
        .await
        .unwrap()
        .unwrap()
    }

    pub async fn queue(&self) -> Vec<String> {
        serde_json::from_value(self.record(&self.partition, false).await.0["queue"].clone())
            .unwrap()
    }

    pub async fn cleanup(self) {
        drop(self.nodes);
        self.database.cleanup().await;
    }
}
