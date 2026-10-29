use super::{test_support::*, *};
use crate::{
    channels::resolver::ResolvedChannel,
    ingress::routing_store::{test_server::Server, AdmittedPlan, Candidate, StoredTarget},
};
use cmaeg_core::channel::{ProviderBinding, Secret};
use futures_util::future::BoxFuture;
use std::{
    collections::BTreeSet,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

pub(super) struct PreparingPort {
    pub(super) base: Port,
    pub(super) inspect_count: AtomicUsize,
    pub(super) reservations: Mutex<Vec<PreparationIntent>>,
    pub(super) on_inspect: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    lose_reply: AtomicBool,
    corrupt_receipt: bool,
    healthy: bool,
}
impl PreparingPort {
    pub(super) fn new(healthy: bool, lose_reply: bool, corrupt_receipt: bool) -> Arc<Self> {
        Arc::new(Self {
            base: Port {
                starts: Mutex::new(vec![]),
                prepares: AtomicUsize::new(0),
                support: supported(),
                next: Mutex::new(None),
                input_size: 100,
            },
            inspect_count: AtomicUsize::new(0),
            reservations: Mutex::new(vec![]),
            on_inspect: Mutex::new(None),
            lose_reply: AtomicBool::new(lose_reply),
            corrupt_receipt,
            healthy,
        })
    }
}
impl ExecutionPort for PreparingPort {
    fn support(&self, binding: &ProviderBinding) -> RecoverySupport {
        self.base.support(binding)
    }
    fn prepare<'a>(
        &'a self,
        plan: &'a AdmittedPlan,
        target: &'a StoredTarget,
        current: &'a ResolvedChannel,
    ) -> BoxFuture<'a, Result<FrozenTurn, ExecutionError>> {
        ExecutionPort::prepare(&self.base, plan, target, current)
    }
    fn prepare_at_generation<'a>(
        &'a self,
        plan: &'a AdmittedPlan,
        target: &'a StoredTarget,
        current: &'a ResolvedChannel,
        generation: Option<u64>,
    ) -> BoxFuture<'a, Result<FrozenTurn, ExecutionError>> {
        Box::pin(async move {
            let mut body = ExecutionPort::prepare(&self.base, plan, target, current).await?;
            body.binding_generation = generation;
            Ok(body)
        })
    }
    fn inspect_binding<'a>(
        &'a self,
        plan: &'a AdmittedPlan,
        target: &'a StoredTarget,
        _: &'a ResolvedChannel,
    ) -> BoxFuture<'a, Result<BindingPreparation, ExecutionError>> {
        Box::pin(async move {
            self.inspect_count.fetch_add(1, Ordering::SeqCst);
            if let Some(callback) = self.on_inspect.lock().unwrap().as_ref() {
                callback();
            }
            if self.healthy {
                return Ok(BindingPreparation::Pinned {
                    version: 1,
                    binding_id: target.launch.provider.binding_id.to_string(),
                    generation: 4,
                });
            }
            Ok(BindingPreparation::Pending {
                intent: PreparationIntent {
                    version: 1,
                    admission_id: plan.admission_id.clone(),
                    target_id: target.id.clone(),
                    request_hash: plan.request_hash.clone(),
                    binding_id: target.launch.provider.binding_id.to_string(),
                    owner: plan.identity.owner.clone(),
                    manager: "manager".into(),
                    generation: 3,
                    binding_revision: 14,
                    binding_digest: "a".repeat(64),
                    sandbox_digest: "b".repeat(64),
                    workspace_digest: "c".repeat(64),
                    admitted_at: plan.admitted_at,
                    expires_at: target.launch.event.expires_at.min(plan.retained_until),
                },
            })
        })
    }
    fn reserve_binding<'a>(
        &'a self,
        _: &'a StoredTarget,
        _: &'a ResolvedChannel,
        intent: &'a PreparationIntent,
    ) -> BoxFuture<'a, Result<PreparationReceipt, ExecutionError>> {
        Box::pin(async move {
            self.reservations.lock().unwrap().push(intent.clone());
            if self.lose_reply.swap(false, Ordering::SeqCst) {
                return Err(ExecutionError::Unavailable);
            }
            Ok(PreparationReceipt {
                version: 1,
                operation_id: intent.operation_id()?,
                intent_fingerprint: intent.fingerprint()?,
                generation: if self.corrupt_receipt { 5 } else { 4 },
                sandbox_id: "sbx_reserved".into(),
                workspace_id: "wks_reserved".into(),
            })
        })
    }
    fn start<'a>(
        &'a self,
        body: &'a FrozenTurn,
        binding: &'a ProviderBinding,
        credential: &'a Secret,
    ) -> BoxFuture<'a, StartResult> {
        self.base.start(body, binding, credential)
    }
}
struct PreparationSetup {
    server: Server,
    plan: AdmittedPlan,
    current: Arc<Current>,
    dispatcher: Dispatcher,
}
impl PreparationSetup {
    async fn new(port: Arc<PreparingPort>, replacement_allowed: bool) -> Self {
        let server = Server::start().await;
        let request = fixture::request(fixture::targets, fixture::message("fresh message"));
        let routed = fixture::routed(&request);
        let selection = if replacement_allowed {
            routed
                .targets
                .iter()
                .map(|target| target.provider.binding_id.to_string())
                .collect()
        } else {
            BTreeSet::new()
        };
        let candidate = Candidate::prepare_with_binding_preparation(
            &request,
            &routed,
            b"source",
            &fixture::keys(),
            fixture::NOW,
            None,
            &selection,
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
        let dispatcher = Dispatcher::new(server.store(), journal, current.clone(), port);
        Self {
            server,
            plan,
            current,
            dispatcher,
        }
    }
    async fn dispatch(&self) -> Result<DispatchOutcome, ExecutionError> {
        self.dispatcher
            .dispatch(&self.plan.identity, &self.plan.targets[0].id, "worker")
            .await
    }
}

#[tokio::test]
async fn lost_provider_receipt_reuses_one_persisted_intent_before_pinned_conversation_and_turn() {
    let port = PreparingPort::new(false, true, false);
    let setup = PreparationSetup::new(port.clone(), true).await;
    assert!(matches!(
        setup.dispatch().await,
        Err(ExecutionError::Unavailable)
    ));
    assert!(port.base.starts.lock().unwrap().is_empty());
    assert_eq!(port.base.prepares.load(Ordering::SeqCst), 0);
    // A different worker takes over from durable state and never re-inspects.
    setup.server.memory.clock.fetch_add(31, Ordering::SeqCst);
    let DispatchOutcome::Replay {
        mut lease, body, ..
    } = setup.dispatch().await.unwrap()
    else {
        panic!("expected accepted turn");
    };
    assert_eq!(body.binding_generation, Some(4));
    assert_eq!(port.inspect_count.load(Ordering::SeqCst), 1);
    {
        let intents = port.reservations.lock().unwrap();
        assert_eq!(intents.len(), 2);
        assert_eq!(intents[0], intents[1]);
    }
    setup
        .dispatcher
        .journal()
        .release(&mut lease)
        .await
        .unwrap();
    assert!(matches!(
        setup.dispatch().await.unwrap(),
        DispatchOutcome::Replay { .. }
    ));
    assert_eq!(port.inspect_count.load(Ordering::SeqCst), 1);
    assert_eq!(port.reservations.lock().unwrap().len(), 2);
    assert_eq!(port.base.starts.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn fresh_authority_is_checked_between_marker_and_reservation() {
    let port = PreparingPort::new(false, false, false);
    let setup = PreparationSetup::new(port.clone(), true).await;
    setup.current.revoke_at.store(3, Ordering::SeqCst);
    assert!(matches!(
        setup.dispatch().await,
        Err(ExecutionError::AuthorityChanged)
    ));
    assert_eq!(port.inspect_count.load(Ordering::SeqCst), 1);
    assert!(port.reservations.lock().unwrap().is_empty());
    setup.current.revoke_at.store(usize::MAX, Ordering::SeqCst);
    assert!(matches!(
        setup.dispatch().await.unwrap(),
        DispatchOutcome::Replay { .. }
    ));
    assert_eq!(port.inspect_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn replacement_opt_in_is_separate_from_generation_pins_and_corrupt_receipts_never_start() {
    let port = PreparingPort::new(false, false, false);
    let setup = PreparationSetup::new(port.clone(), false).await;
    assert!(matches!(
        setup.dispatch().await,
        Err(ExecutionError::Unsupported)
    ));
    assert!(port.reservations.lock().unwrap().is_empty());
    let port = PreparingPort::new(true, false, false);
    let setup = PreparationSetup::new(port.clone(), false).await;
    let DispatchOutcome::Replay { body, .. } = setup.dispatch().await.unwrap() else {
        panic!("expected pinned turn");
    };
    assert_eq!(body.binding_generation, Some(4));
    assert!(port.reservations.lock().unwrap().is_empty());
    let port = PreparingPort::new(false, false, true);
    let setup = PreparationSetup::new(port.clone(), true).await;
    assert!(matches!(
        setup.dispatch().await,
        Err(ExecutionError::Corrupt)
    ));
    assert!(port.base.starts.lock().unwrap().is_empty());
    assert_eq!(port.base.prepares.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn newly_admitted_message_changes_inherit_original_generation_without_current_inspection() {
    for original_generation in [None, Some(3)] {
        let port = PreparingPort::new(true, false, false); // Today's hierarchy would report generation4.
        let setup = Setup::new(supported()).await;
        let mut original = setup.plan.clone();
        if original_generation.is_some() {
            // Build a separate committed new-format original; do not rewrite
            // the original admission or silently enroll its historical event.
            let request = fixture::request(fixture::targets, {
                let mut activity = fixture::message("original prepared turn");
                activity.event_id = "prepared-original".parse().unwrap();
                activity
            });
            let upload = Candidate::prepare_with_binding_preparation(
                &request,
                &fixture::routed(&request),
                b"original",
                &fixture::keys(),
                fixture::NOW,
                None,
                &BTreeSet::new(),
            )
            .unwrap();
            original = setup.server.store().admit(&upload).await.unwrap();
        }
        let mut original_lease = setup
            .journal()
            .claim(&original, &original.targets[0], "original-worker")
            .await
            .unwrap();
        if let Some(generation) = original_generation {
            setup
                .journal()
                .pin_binding_preparation(
                    &mut original_lease,
                    &original,
                    &original.targets[0],
                    BindingPreparation::Pinned {
                        version: 1,
                        binding_id: original.targets[0].launch.provider.binding_id.to_string(),
                        generation,
                    },
                )
                .await
                .unwrap();
        }
        let mut activity = original.activity.clone();
        activity.routing = None;
        activity.event_id = "message-change".parse().unwrap();
        activity.actor.kind = cmaeg_core::activity::ActorKind::Unknown;
        activity.kind = serde_json::from_value(serde_json::json!({"type":"event","ce":{
            "specversion":"1.0","id":"message-change","source":"urn:cmaeg:telegram:77","type":"cma.message.deleted.v1","subject":"1",
            "data":{"message_id":"1","original_event_id":original.identity.event_id}}})).unwrap();
        let incoming = fixture::request(|_| {}, activity);
        let (request, routed) = crate::ingress::message_event::bind_for_generation_test(
            incoming,
            &original,
            &fixture::keys(),
            fixture::NOW,
        )
        .unwrap();
        let upload = Candidate::prepare_message_event_with_generation(
            &request,
            &routed,
            b"change",
            &fixture::keys(),
            fixture::NOW,
            true,
        )
        .unwrap();
        let derivative = setup.server.store().admit(&upload).await.unwrap();
        let dispatcher = Dispatcher::new(
            setup.server.store(),
            setup.journal().clone(),
            setup.current.clone(),
            port.clone(),
        );
        let DispatchOutcome::Replay { body, lease, .. } = dispatcher
            .dispatch(
                &derivative.identity,
                &derivative.targets[0].id,
                "callback-worker",
            )
            .await
            .unwrap()
        else {
            panic!("expected inherited turn");
        };
        assert_eq!(body.binding_generation, original_generation);
        assert!(
            matches!(&lease.state.binding_preparation,Some(BindingPreparation::Inherited {generation,..}) if *generation == original_generation)
        );
        assert_eq!(port.inspect_count.load(Ordering::SeqCst), 0);
        assert!(port.reservations.lock().unwrap().is_empty());
        setup.journal().guard(&original_lease).await.unwrap();
    }
}
