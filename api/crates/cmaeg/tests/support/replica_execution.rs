//! Explicit provider/sink boundary fixtures; these do not model a real provider's
//! reconciliation capability and cannot qualify native external exactly-once.
use super::replica_delivery::{runner_keys, DeliveryOracle};
use cmaeg::{
    channels::resolver::ResolvedChannel,
    delivery::{
        durable::DeliveryCoordinator,
        intents::{DeliveryCommand, DeliveryOutcome, IntentReceipt},
        router::ShardedScheduler,
    },
    ingress::routing_store::{AdmittedPlan, EventIdentity, StoredTarget},
    providers::EventContextSnapshot,
    turns::routed::{
        Authority, DispatchOutcome, Dispatcher, ExecutionError, ExecutionPort, FrozenDelivery,
        FrozenTurn, Lease, RecoverySupport, ReplayEffect, RoutedDelivery, StartResult,
    },
};
use cmaeg_core::channel::{ProviderBinding, Secret};
use futures_util::future::BoxFuture;
use serde_json::json;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tokio::sync::mpsc;

pub struct Current(pub Arc<ResolvedChannel>);
impl Authority for Current {
    fn current<'a>(
        &'a self,
        _identity: &'a EventIdentity,
    ) -> BoxFuture<'a, Result<Arc<ResolvedChannel>, ExecutionError>> {
        Box::pin(async { Ok(self.0.clone()) })
    }
}
#[derive(Default)]
pub struct Port(pub AtomicUsize);
impl ExecutionPort for Port {
    fn support(&self, _binding: &ProviderBinding) -> RecoverySupport {
        RecoverySupport {
            exact_body_idempotency: true,
            response_replay: true,
        }
    }
    fn prepare<'a>(
        &'a self,
        plan: &'a AdmittedPlan,
        target: &'a StoredTarget,
        current: &'a ResolvedChannel,
    ) -> BoxFuture<'a, Result<FrozenTurn, ExecutionError>> {
        Box::pin(async move {
            let consumer = current
                .definition
                .consumer(Some(&target.launch.consumer))
                .unwrap();
            Ok(FrozenTurn {
                conversation_id: target.launch.event.conversation_id.clone(),
                binding_generation: None,
                idempotency_key: target.launch.idempotency_key.clone(),
                input: vec![json!({"role":"user","content":"fixture request"})],
                metadata: Default::default(),
                event_context: EventContextSnapshot {
                    schema_version: 1,
                    channel_id: plan.identity.channel_id.clone(),
                    conversation_id: target.launch.event.conversation_id.clone(),
                    activity: plan.activity.clone(),
                    event_ref: FrozenTurn::event_ref(plan, target, &runner_keys()).unwrap(),
                },
                delivery: FrozenDelivery {
                    consumer: target.launch.consumer.clone(),
                    label: consumer.label.into(),
                    auto_final: consumer.reply.auto_final,
                    destination: consumer.reply.destination,
                    default_destination: None,
                    policy: current.definition.reply.clone(),
                },
            })
        })
    }
    fn start<'a>(
        &'a self,
        _body: &'a FrozenTurn,
        _binding: &'a ProviderBinding,
        _credential: &'a Secret,
    ) -> BoxFuture<'a, StartResult> {
        Box::pin(async {
            self.0.fetch_add(1, Ordering::SeqCst);
            StartResult::Accepted {
                response_id: "response-fixture".into(),
            }
        })
    }
}
pub fn dispatcher(
    oracle: &DeliveryOracle,
    channel: Arc<ResolvedChannel>,
    port: Arc<Port>,
) -> Dispatcher {
    Dispatcher::new(
        oracle.plans.clone(),
        oracle.journal(),
        Arc::new(Current(channel)),
        port,
    )
}
pub async fn accepted(dispatcher: &Dispatcher, plan: &AdmittedPlan, worker: &str) -> Box<Lease> {
    let DispatchOutcome::Replay { lease, .. } = dispatcher
        .dispatch(&plan.identity, &plan.targets[0].id, worker)
        .await
        .unwrap()
    else {
        panic!("provider acceptance must be retained")
    };
    lease
}
pub fn lane(
    oracle: &DeliveryOracle,
    channel: Arc<ResolvedChannel>,
) -> (RoutedDelivery, mpsc::Receiver<DeliveryCommand>) {
    let (tx, rx) = mpsc::channel(4);
    (
        RoutedDelivery::new(
            oracle.plans.clone(),
            oracle.journal(),
            Arc::new(Current(channel)),
            DeliveryCoordinator::new(Arc::new(ShardedScheduler::new(tx))),
        ),
        rx,
    )
}
#[path = "replica_recovery_frame.rs"]
mod recovery_frame;
pub use recovery_frame::frame;
pub fn effect() -> ReplayEffect {
    ReplayEffect::Reply(Box::new(
        serde_json::from_value(json!({"text":"fixture reply"})).unwrap(),
    ))
}
pub fn confirmed() -> IntentReceipt {
    IntentReceipt {
        message_parts: vec![],
        message_ref: None,
        rendering: serde_json::from_value(json!({"outcome":"rendered","blocks":[]})).unwrap(),
        delivery: DeliveryOutcome::Confirmed {
            platform_message_id: Some("fixture-platform-message".into()),
        },
    }
}
