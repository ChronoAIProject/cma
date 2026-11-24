use super::*;
use crate::sandbox::agent_scheduler::AgentScheduler;
use layer2_core::agent_ownership::{Membership, OwnershipError, OwnershipLease, OwnershipToken};
use layer2_core::sandbox::store::{AgentStopRequest, MAX_PAGE_SIZE};
use std::collections::BTreeSet;
use std::sync::atomic::AtomicBool;
use tokio::time::Instant;

struct AcquisitionFault {
    inner: MemoryOwnershipStore,
    targets: BTreeSet<AgentId>,
    stale: AtomicBool,
    attempts: Mutex<Vec<(AgentId, Instant)>>,
}

#[async_trait::async_trait]
impl OwnershipStore for AcquisitionFault {
    async fn register(&self, member: Member) -> Result<Membership, OwnershipError> {
        self.inner.register(member).await
    }

    async fn heartbeat(&self, id: &str, draining: bool) -> Result<Membership, OwnershipError> {
        self.inner.heartbeat(id, draining).await
    }

    async fn membership(&self) -> Result<Membership, OwnershipError> {
        self.inner.membership().await
    }

    async fn owner(&self, id: &AgentId) -> Result<Option<OwnershipLease>, OwnershipError> {
        self.inner.owner(id).await
    }

    async fn has_owner_history(&self, id: &AgentId) -> Result<bool, OwnershipError> {
        self.inner.has_owner_history(id).await
    }

    async fn acquire(
        &self,
        id: &AgentId,
        instance: &str,
    ) -> Result<Option<OwnershipLease>, OwnershipError> {
        if self.targets.contains(id) {
            self.attempts
                .lock()
                .unwrap()
                .push((id.clone(), Instant::now()));
            if self.stale.load(Ordering::SeqCst) {
                return Err(OwnershipError::Stale);
            }
        }
        self.inner.acquire(id, instance).await
    }

    async fn renew(&self, token: &OwnershipToken) -> Result<OwnershipLease, OwnershipError> {
        self.inner.renew(token).await
    }

    async fn release(&self, token: &OwnershipToken) -> Result<(), OwnershipError> {
        self.inner.release(token).await
    }

    async fn check(&self, token: &OwnershipToken) -> Result<(), OwnershipError> {
        self.inner.check(token).await
    }
}

async fn fault(
    h: &Harness,
    store: Arc<fixture::Store>,
    targets: BTreeSet<AgentId>,
) -> (
    Arc<AgentWorker>,
    Arc<OwnershipCoordinator>,
    Arc<AcquisitionFault>,
) {
    let ownership = Arc::new(AcquisitionFault {
        inner: MemoryOwnershipStore::at_fixed_time(1),
        targets,
        stale: AtomicBool::new(true),
        attempts: Mutex::default(),
    });
    let coordinator = OwnershipCoordinator::register(
        ownership.clone(),
        Member {
            instance_id: "stale-acquisition-fixture".into(),
            protocol: PROTOCOL_VERSION,
            relay_address: "http://owner.invalid".into(),
            expires_at_ms: 0,
            draining: false,
            owned_agents: 0,
            max_owned_agents: 16,
        },
    )
    .await
    .unwrap();
    let worker = runner(h, store, Some(coordinator.clone()));
    (worker, coordinator, ownership)
}

#[tokio::test(start_paused = true)]
async fn repeated_stale_acquisition_is_bounded_and_the_same_work_completes() {
    let h = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let pending = h.admit("repeated-stale-acquisition", vec![]).await;
    let healthy = h.admit("independent-agent", vec![]).await;
    let (worker, coordinator, ownership) = fault(
        &h,
        fixture::Store::new(h.store.clone()),
        BTreeSet::from([pending.id.clone()]),
    )
    .await;
    let start = Instant::now();
    let attempt = LifecycleAttempt::new(Phase::Execution, Some(&pending.id));
    let failed_at = [0, 1, 3, 7, 15, 31, 61, 91];
    for second in 0..=121 {
        if second != 0 {
            tokio::time::advance(Duration::from_secs(1)).await;
        }
        if second % 5 == 0 {
            coordinator.maintain().await.unwrap();
        }
        let now = h.now + Duration::from_secs(second);
        let due = worker.due_work(now, 100).await.unwrap();
        assert_eq!(
            due.contains(&pending.id),
            failed_at.contains(&second) || second == 121
        );
        if second == 2 {
            assert!(due.contains(&healthy.id));
            assert!(worker.run_scheduled(&healthy.id, now).await.unwrap());
            assert_eq!(h.stored(&healthy.id).await.status, AgentStatus::Ready);
        }
        if second == 92 {
            ownership.stale.store(false, Ordering::SeqCst);
        }
        let result = worker
            .run_scheduled_observed(&pending.id, now, &attempt)
            .await;
        if failed_at.contains(&second) {
            let refused = result.unwrap_err();
            assert_eq!(refused.observation.origin, Origin::OwnershipAcquire);
            assert_eq!(refused.observation.cause_class, CauseClass::OwnershipStale);
        } else {
            assert_eq!(result.unwrap(), second == 121);
        }
        if second < 121 {
            assert_eq!(h.stored(&pending.id).await, pending);
        }
    }
    assert_eq!(h.stored(&pending.id).await.status, AgentStatus::Ready);
    let timings: Vec<_> = ownership
        .attempts
        .lock()
        .unwrap()
        .iter()
        .map(|(_, time)| time.duration_since(start).as_millis())
        .collect();
    assert_eq!(
        timings,
        [0, 1000, 3000, 7000, 15000, 31000, 61000, 91000, 121000]
    );
}

#[tokio::test(start_paused = true)]
async fn a_fresh_stop_bypasses_background_backoff_and_retries_at_the_control_cap() {
    let h = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let pending = h.admit("stop-during-acquisition-backoff", vec![]).await;
    let (worker, _, ownership) = fault(
        &h,
        fixture::Store::new(h.store.clone()),
        BTreeSet::from([pending.id.clone()]),
    )
    .await;
    for _ in 0..2 {
        assert_eq!(
            worker.run_scheduled(&pending.id, h.now).await,
            Err(SandboxStoreError::Conflict)
        );
    }
    assert!(!worker.run_scheduled(&pending.id, h.now).await.unwrap());
    h.store
        .schedule_agent_stop(AgentStopRequest {
            target: AgentControlTarget::from(&pending),
            reason: AgentStopReason::Owner,
            requested_at: rfc3339_at(h.now),
        })
        .await
        .unwrap();
    let stopping = h.stored(&pending.id).await;
    assert_eq!(stopping.work.step, AgentWorkStep::FenceInteraction);
    assert_eq!(
        worker.run_scheduled(&pending.id, h.now).await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(ownership.attempts.lock().unwrap().len(), 3);

    let mut elapsed = 0;
    for (seconds, attempts) in [(1, 3), (2, 4), (5, 4), (6, 5), (9, 5), (10, 6)] {
        tokio::time::advance(Duration::from_secs(seconds - elapsed)).await;
        elapsed = seconds;
        let at = h.now + Duration::from_secs(seconds);
        let result = worker.run_scheduled(&pending.id, at).await;
        if matches!(seconds, 2 | 6 | 10) {
            assert_eq!(result, Err(SandboxStoreError::Conflict));
        } else {
            assert!(!result.unwrap());
        }
        assert_eq!(ownership.attempts.lock().unwrap().len(), attempts);
    }
    assert_eq!(h.stored(&pending.id).await, stopping);
}

#[tokio::test(start_paused = true)]
async fn scheduler_skips_a_full_cooled_page_and_runs_healthy_work_and_fresh_control() {
    let h = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let mut admitted = [
        h.admit("first-scheduler-agent", vec![]).await,
        h.admit("second-scheduler-agent", vec![]).await,
    ];
    admitted.sort_by(|left, right| left.id.cmp(&right.id));
    let [control, healthy] = admitted;
    let mut cooled = Vec::new();
    for index in 1..MAX_PAGE_SIZE {
        let mut agent = healthy.clone();
        let key = format!("000-cooled-{index:03}");
        agent.id = AgentId(key.clone());
        agent.creation_key = key;
        agent.worktree_id = WorktreeId(format!("fixture-tree-{index:03}"));
        h.store.create_agent(agent.clone()).await.unwrap();
        cooled.push(agent);
    }
    cooled.push(control.clone());
    let store = fixture::Store::new(h.store.clone());
    let (worker, _, ownership) = fault(
        &h,
        store.clone(),
        cooled.iter().map(|agent| agent.id.clone()).collect(),
    )
    .await;
    let full_page: Vec<_> = cooled.iter().map(|agent| agent.id.clone()).collect();
    assert_eq!(
        worker.due_work(h.now, MAX_PAGE_SIZE).await.unwrap(),
        full_page
    );
    for agent in &cooled {
        for _ in 0..2 {
            assert_eq!(
                worker.run_scheduled(&agent.id, h.now).await,
                Err(SandboxStoreError::Conflict)
            );
        }
    }
    store.agent_reads.lock().unwrap().clear();

    h.store
        .schedule_agent_stop(AgentStopRequest {
            target: AgentControlTarget::from(&control),
            reason: AgentStopReason::Owner,
            requested_at: rfc3339_at(h.now),
        })
        .await
        .unwrap();

    let (shutdown, signal) = tokio::sync::watch::channel(false);
    let started = Instant::now();
    let scheduler = AgentScheduler::new(worker, Arc::new(tokio::sync::Notify::new())).spawn(signal);
    for _ in 0..200 {
        if h.stored(&healthy.id).await.status == AgentStatus::Ready
            && ownership.attempts.lock().unwrap().len() > usize::from(MAX_PAGE_SIZE) * 2
        {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(Instant::now(), started);
    assert_eq!(h.stored(&healthy.id).await.status, AgentStatus::Ready);
    let unchanged: BTreeSet<_> = cooled
        .iter()
        .filter(|agent| agent.id != control.id)
        .map(|agent| &agent.id)
        .collect();
    assert!(store
        .agent_reads
        .lock()
        .unwrap()
        .iter()
        .all(|id| !unchanged.contains(id)));
    assert_eq!(
        ownership.attempts.lock().unwrap().len(),
        usize::from(MAX_PAGE_SIZE) * 2 + 1
    );
    assert_eq!(
        ownership.attempts.lock().unwrap().last().unwrap().0,
        control.id
    );

    shutdown.send(true).unwrap();
    scheduler.await.unwrap();
}
