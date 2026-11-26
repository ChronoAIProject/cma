use super::*;
pub(super) use crate::ingress::routing_plan::fixture;
use crate::{
    channels::resolver::ResolvedChannel,
    ingress::{
        launch::LaunchRequest,
        routing_store::{
            test_server::Server, AdmittedPlan, Candidate, EventIdentity, StoredTarget,
        },
    },
    providers::EventContextSnapshot,
};
use cmaeg_core::channel::{ProviderBinding, Secret};
use futures_util::future::BoxFuture;
use serde_json::json;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};

pub struct Current {
    pub channel: Mutex<Arc<ResolvedChannel>>,
    pub calls: AtomicUsize,
    pub revoke_at: AtomicUsize,
}
impl Authority for Current {
    fn current<'a>(
        &'a self,
        _identity: &'a EventIdentity,
    ) -> BoxFuture<'a, Result<Arc<ResolvedChannel>, ExecutionError>> {
        Box::pin(async move {
            let count = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            if count >= self.revoke_at.load(Ordering::SeqCst) {
                return Err(ExecutionError::AuthorityChanged);
            }
            Ok(self.channel.lock().unwrap().clone())
        })
    }
}
pub struct Port {
    pub starts: Mutex<Vec<(Vec<u8>, String)>>,
    pub prepares: AtomicUsize,
    pub support: RecoverySupport,
    pub next: Mutex<Option<StartResult>>,
    pub input_size: usize,
}
impl ExecutionPort for Port {
    fn support(&self, _binding: &ProviderBinding) -> RecoverySupport {
        self.support
    }
    fn prepare<'a>(
        &'a self,
        plan: &'a AdmittedPlan,
        target: &'a StoredTarget,
        current: &'a ResolvedChannel,
    ) -> BoxFuture<'a, Result<FrozenTurn, ExecutionError>> {
        Box::pin(async move {
            self.prepares.fetch_add(1, Ordering::SeqCst);
            Ok(frozen(plan, target, current, self.input_size))
        })
    }
    fn start<'a>(
        &'a self,
        body: &'a FrozenTurn,
        _binding: &'a ProviderBinding,
        credential: &'a Secret,
    ) -> BoxFuture<'a, StartResult> {
        Box::pin(async move {
            self.starts.lock().unwrap().push((
                super::storage::bytes(body).unwrap(),
                credential.as_str().into(),
            ));
            self.next
                .lock()
                .unwrap()
                .take()
                .unwrap_or(StartResult::Accepted {
                    response_id: "resp_1".into(),
                })
        })
    }
}
pub fn frozen(
    plan: &AdmittedPlan,
    target: &StoredTarget,
    current: &ResolvedChannel,
    size: usize,
) -> FrozenTurn {
    let consumer = current
        .definition
        .consumer(Some(&target.launch.consumer))
        .unwrap();
    FrozenTurn {
        conversation_id: target.launch.event.conversation_id.clone(),
        binding_generation: None,
        idempotency_key: target.launch.idempotency_key.clone(),
        input: vec![json!({"role":"user","content":"x".repeat(size)})],
        metadata: std::collections::BTreeMap::new(),
        event_context: EventContextSnapshot {
            schema_version: 1,
            channel_id: plan.identity.channel_id.clone(),
            conversation_id: target.launch.event.conversation_id.clone(),
            activity: plan.activity.clone(),
            event_ref: FrozenTurn::event_ref(plan, target, &fixture::keys()).unwrap(),
        },
        delivery: FrozenDelivery {
            consumer: target.launch.consumer.clone(),
            label: consumer.label.into(),
            auto_final: consumer.reply.auto_final,
            destination: consumer.reply.destination,
            default_destination: (consumer.reply.destination
                == cmaeg_core::channel::ReplyChoice::Default)
                .then(|| DefaultDestination::capture(current))
                .flatten(),
            policy: current.definition.reply.clone(),
        },
    }
}
pub struct Setup {
    pub server: Server,
    pub plan: AdmittedPlan,
    pub current: Arc<Current>,
    pub port: Arc<Port>,
    pub dispatcher: Dispatcher,
}
impl Setup {
    pub async fn new(support: RecoverySupport) -> Self {
        Self::with_request(
            support,
            fixture::request(fixture::targets, fixture::message("hello")),
        )
        .await
    }
    pub async fn with_request(support: RecoverySupport, request: LaunchRequest) -> Self {
        Self::with_source(support, request, b"raw-event").await
    }
    pub async fn with_source(
        support: RecoverySupport,
        request: LaunchRequest,
        source: &[u8],
    ) -> Self {
        Self::with_server(support, request, source, Server::start().await).await
    }
    pub async fn with_server(
        support: RecoverySupport,
        request: LaunchRequest,
        source: &[u8],
        server: Server,
    ) -> Self {
        let candidate = Candidate::prepare(
            &request,
            &fixture::routed(&request),
            source,
            &fixture::keys(),
            fixture::NOW,
        )
        .unwrap();
        let plan = server.store().admit(&candidate).await.unwrap();
        let mut journal = Journal::new(server.records.clone(), Arc::new(fixture::keys()));
        let clock = server.memory.clock.clone();
        journal.clock = Arc::new(move || clock.load(Ordering::SeqCst));
        let current = Arc::new(Current {
            channel: Mutex::new(request.channel),
            calls: AtomicUsize::new(0),
            revoke_at: AtomicUsize::new(usize::MAX),
        });
        let port = Arc::new(Port {
            starts: Mutex::new(Vec::new()),
            prepares: AtomicUsize::new(0),
            support,
            next: Mutex::new(None),
            input_size: 100 * 1024,
        });
        let dispatcher = Dispatcher::new(server.store(), journal, current.clone(), port.clone());
        Self {
            server,
            plan,
            current,
            port,
            dispatcher,
        }
    }
    pub fn journal(&self) -> &Journal {
        self.dispatcher.journal()
    }
    pub fn target(&self) -> &StoredTarget {
        &self.plan.targets[0]
    }
    pub async fn dispatch(&self) -> Result<DispatchOutcome, ExecutionError> {
        self.dispatcher
            .dispatch(&self.plan.identity, &self.target().id, "worker")
            .await
    }
    pub async fn prepared(&self) -> Lease {
        let mut lease = self
            .journal()
            .claim(&self.plan, self.target(), "prepare")
            .await
            .unwrap();
        let body = frozen(
            &self.plan,
            self.target(),
            &self.current.channel.lock().unwrap(),
            self.port.input_size,
        );
        self.journal()
            .freeze(&mut lease, &self.plan, self.target(), &body)
            .await
            .unwrap();
        lease
    }
}
pub fn supported() -> RecoverySupport {
    RecoverySupport {
        exact_body_idempotency: true,
        response_replay: true,
    }
}
pub fn event(body: &FrozenTurn) -> cmaeg_core::seal::EventRefPayload {
    body.event_context
        .event_ref
        .open(&fixture::keys(), fixture::NOW)
        .unwrap()
        .payload
}

/// Exercise the actual receipt checkpoint for frames whose projection has no send.
pub async fn acknowledge_no_effect(
    setup: &Setup,
    lease: &mut Lease,
    response: &str,
    cursor: u64,
    terminal: bool,
) {
    use crate::delivery::{durable::DeliveryCoordinator, router::ShardedScheduler};
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    let delivery = RoutedDelivery::new(
        setup.server.store(),
        setup.journal().clone(),
        setup.current.clone(),
        DeliveryCoordinator::new(Arc::new(ShardedScheduler::new(tx))),
    );
    let start = match lease.state().phase {
        Phase::Accepted { durable_cursor, .. } => durable_cursor.map_or(0, |v| v + 1),
        _ => panic!("accepted"),
    };
    for sequence in start..=cursor {
        let frame = recovery_frame(response, sequence, terminal && sequence == cursor);
        assert!(matches!(
            delivery
                .deliver_frame(
                    &setup.plan.identity,
                    &setup.target().id,
                    lease,
                    &frame,
                    ReplayEffect::NoEffect
                )
                .await
                .unwrap(),
            FrameDelivery::Settled { .. }
        ));
    }
}
pub fn recovery_frame(
    response: &str,
    sequence: u64,
    terminal: bool,
) -> crate::providers::RecoveryEvent {
    use crate::providers::{RecoveryEvent, ResponsesEvent};
    RecoveryEvent {
        trace_contract: false,
        trace: None,
        response_id: response.into(),
        sequence_number: sequence,
        event: if terminal {
            ResponsesEvent::Completed {
                response: json!({"id":response}),
                matching_response_identity: true,
            }
        } else {
            ResponsesEvent::InProgress {
                native_agent_id: None,
            }
        },
        raw: if terminal {
            json!({"type":"response.completed", "response_id":response, "sequence_number":sequence, "response":{"id":response}})
        } else {
            json!({"type":"response.in_progress", "response_id":response, "sequence_number":sequence})
        },
    }
}
