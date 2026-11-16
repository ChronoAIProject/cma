//! Acceptance is evidence only while the captured execution fence is live.
use super::{test_support::*, *};
use crate::{
    channels::resolver::ResolvedChannel,
    ingress::routing_store::{AdmittedPlan, StoredTarget},
};
use cmaeg_core::channel::{ProviderBinding, Secret};
use futures_util::future::BoxFuture;
use std::sync::{
    atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering},
    Arc,
};

struct ExpiringAcceptance {
    inner: Arc<Port>,
    clock: Arc<AtomicI64>,
}
impl ExecutionPort for ExpiringAcceptance {
    fn support(&self, binding: &ProviderBinding) -> RecoverySupport {
        self.inner.support(binding)
    }
    fn prepare<'a>(
        &'a self,
        plan: &'a AdmittedPlan,
        target: &'a StoredTarget,
        current: &'a ResolvedChannel,
    ) -> BoxFuture<'a, Result<FrozenTurn, ExecutionError>> {
        ExecutionPort::prepare(self.inner.as_ref(), plan, target, current)
    }
    fn start<'a>(
        &'a self,
        body: &'a FrozenTurn,
        binding: &'a ProviderBinding,
        credential: &'a Secret,
    ) -> BoxFuture<'a, StartResult> {
        Box::pin(async move {
            let accepted = self.inner.start(body, binding, credential).await;
            // Simulate an acceptance arriving after a stopped executor's lease
            // elapsed. A heartbeat may not resurrect this captured fence.
            self.clock
                .fetch_add(model::LEASE_SECONDS + 1, Ordering::SeqCst);
            accepted
        })
    }
}

#[tokio::test]
async fn late_acceptance_after_lease_expiry_never_commits_or_reposts() {
    let setup = Setup::new(RecoverySupport {
        exact_body_idempotency: false,
        response_replay: true,
    })
    .await;
    let dispatcher = Dispatcher::new(
        setup.server.store(),
        setup.journal().clone(),
        setup.current.clone(),
        Arc::new(ExpiringAcceptance {
            inner: setup.port.clone(),
            clock: setup.server.memory.clock.clone(),
        }),
    );
    assert!(matches!(
        dispatcher
            .dispatch(&setup.plan.identity, &setup.target().id, "old")
            .await,
        Err(ExecutionError::LostLease)
    ));
    let (saved, _) = setup
        .journal()
        .read::<TargetState>(&setup.target().id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.phase, Phase::Starting);
    assert!(matches!(
        setup.dispatch().await.unwrap(),
        DispatchOutcome::Unknown
    ));
    assert_eq!(setup.port.starts.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn expired_event_is_refused_before_provider_start() {
    let setup = Setup::new(supported()).await;
    setup
        .server
        .memory
        .clock
        .store(setup.target().launch.event.expires_at, Ordering::SeqCst);
    assert!(matches!(
        setup.dispatch().await.unwrap(),
        DispatchOutcome::Terminal(Phase::Refused { .. })
    ));
    assert!(setup.port.starts.lock().unwrap().is_empty());
    let (saved, _) = setup
        .journal()
        .read::<TargetState>(&setup.target().id)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(saved.phase, Phase::Refused { .. }));
}

struct PendingAcceptance {
    inner: Arc<Port>,
    started: tokio::sync::Notify,
    cancelled: Arc<AtomicBool>,
}
struct CancelAttempt(Arc<AtomicBool>);
impl Drop for CancelAttempt {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}
impl ExecutionPort for PendingAcceptance {
    fn support(&self, binding: &ProviderBinding) -> RecoverySupport {
        self.inner.support(binding)
    }
    fn prepare<'a>(
        &'a self,
        plan: &'a AdmittedPlan,
        target: &'a StoredTarget,
        current: &'a ResolvedChannel,
    ) -> BoxFuture<'a, Result<FrozenTurn, ExecutionError>> {
        ExecutionPort::prepare(self.inner.as_ref(), plan, target, current)
    }
    fn start<'a>(
        &'a self,
        body: &'a FrozenTurn,
        binding: &'a ProviderBinding,
        credential: &'a Secret,
    ) -> BoxFuture<'a, StartResult> {
        Box::pin(async move {
            let _cancel = CancelAttempt(self.cancelled.clone());
            let _ = self.inner.start(body, binding, credential).await;
            self.started.notify_one();
            std::future::pending().await
        })
    }
}

#[tokio::test]
async fn renewal_failure_cancels_wait_and_cannot_adopt_successor_fence() {
    let setup = Setup::new(RecoverySupport {
        exact_body_idempotency: false,
        response_replay: true,
    })
    .await;
    let cancelled = Arc::new(AtomicBool::new(false));
    let port = Arc::new(PendingAcceptance {
        inner: setup.port.clone(),
        started: tokio::sync::Notify::new(),
        cancelled: cancelled.clone(),
    });
    let dispatcher = Dispatcher::new(
        setup.server.store(),
        setup.journal().clone(),
        setup.current.clone(),
        port.clone(),
    );
    let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(6), async {
        tokio::join!(
            dispatcher.dispatch(
                &setup.plan.identity,
                &setup.target().id,
                "pending-old-owner"
            ),
            async {
                port.started.notified().await;
                setup
                    .server
                    .memory
                    .clock
                    .fetch_add(model::LEASE_SECONDS + 1, Ordering::SeqCst);
                // Another owner durably settles the uncertain start. Renewal
                // of the original capture must not adopt this new generation.
                assert!(matches!(
                    setup.dispatch().await.unwrap(),
                    DispatchOutcome::Unknown
                ));
            }
        )
    })
    .await
    .expect("lost renewal must cancel before the provider acceptance deadline");
    assert!(matches!(result, Err(ExecutionError::LostLease)));
    assert!(cancelled.load(Ordering::SeqCst));
    let (saved, _) = setup
        .journal()
        .read::<TargetState>(&setup.target().id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.phase, Phase::Unknown);
    assert_eq!(setup.port.starts.lock().unwrap().len(), 1);
}

struct SlowAcceptance {
    started: tokio::sync::Notify,
    starts: AtomicUsize,
}
impl ExecutionPort for SlowAcceptance {
    fn support(&self, _binding: &ProviderBinding) -> RecoverySupport {
        RecoverySupport {
            exact_body_idempotency: false,
            response_replay: true,
        }
    }
    fn prepare<'a>(
        &'a self,
        _plan: &'a AdmittedPlan,
        _target: &'a StoredTarget,
        _current: &'a ResolvedChannel,
    ) -> BoxFuture<'a, Result<FrozenTurn, ExecutionError>> {
        Box::pin(async { panic!("retained body must not be prepared again") })
    }
    fn start<'a>(
        &'a self,
        _body: &'a FrozenTurn,
        _binding: &'a ProviderBinding,
        _credential: &'a Secret,
    ) -> BoxFuture<'a, StartResult> {
        Box::pin(async move {
            self.starts.fetch_add(1, Ordering::SeqCst);
            self.started.notify_one();
            // Native partition leases last 15 s; provider acceptance may take
            // up to 20 s. This wait must cross a real server-side lease bound.
            tokio::time::sleep(std::time::Duration::from_secs(17)).await;
            StartResult::Accepted {
                response_id: "eg2171-slow-accepted".into(),
            }
        })
    }
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn native_acceptance_wait_renews_captured_fence_without_reposting() {
    use futures_util::FutureExt;
    use std::{panic::AssertUnwindSafe, time::Duration};
    let h = replica_test_fixture::Harness::new().await;
    let result = AssertUnwindSafe(async {
        let (owner, mut prepared) = h.acquire().await;
        let journal = h.nodes[owner].journal();
        let channel = h.current.channel.lock().unwrap().clone();
        let mut body = frozen(&h.plan, h.target(), &channel, 32);
        body.event_context.event_ref =
            FrozenTurn::event_ref(&h.plan, h.target(), &h.database.keys()).unwrap();
        journal
            .freeze(&mut prepared, &h.plan, h.target(), &body)
            .await
            .unwrap();
        journal.release(&mut prepared).await.unwrap();
        let port = Arc::new(SlowAcceptance {
            started: tokio::sync::Notify::new(),
            starts: AtomicUsize::new(0),
        });
        let dispatcher = Dispatcher::new(
            h.nodes[owner].plans(),
            journal.clone(),
            h.current.clone(),
            port.clone(),
        );
        let other = Dispatcher::new(
            h.nodes[1 - owner].plans(),
            h.nodes[1 - owner].journal(),
            h.current.clone(),
            port.clone(),
        );
        let (accepted, ()) = tokio::time::timeout(Duration::from_secs(25), async {
            tokio::join!(
                dispatcher.dispatch(&h.plan.identity, &h.target().id, "slow-owner"),
                async {
                    port.started.notified().await;
                    // A second replica cannot enter the pending original POST.
                    assert!(matches!(
                        other
                            .dispatch(&h.plan.identity, &h.target().id, "other")
                            .await,
                        Err(ExecutionError::Busy | ExecutionError::LostLease)
                    ));
                }
            )
        })
        .await
        .expect("acceptance wait exceeded its bounded fixture deadline");
        let DispatchOutcome::Replay {
            mut lease,
            response_id,
            ..
        } = accepted.unwrap()
        else {
            panic!("live captured lease must retain a valid slow acceptance")
        };
        assert_eq!(response_id, "eg2171-slow-accepted");
        journal.guard(&lease).await.unwrap();
        assert_eq!(port.starts.load(Ordering::SeqCst), 1);
        journal.release(&mut lease).await.unwrap();
        let (saved, _) = h.nodes[1 - owner]
            .journal()
            .read::<TargetState>(&h.target().id)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(saved.phase, Phase::Accepted { .. }));
    })
    .catch_unwind()
    .await;
    h.cleanup().await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}
