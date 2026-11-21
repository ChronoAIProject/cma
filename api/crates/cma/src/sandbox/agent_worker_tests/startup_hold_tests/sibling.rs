use super::*;
use crate::sandbox::agent_worker_tests::lifecycle_failure_tests::fixture;
use layer2_core::agent_ownership::{
    test_support::MemoryOwnershipStore, Member, OwnershipCoordinator, OwnershipStore,
    PROTOCOL_VERSION,
};
use layer2_core::sandbox::store::AgentStopRequest;
use layer3_adapter::{RuntimeStartupFailure, RuntimeStartupFailureClass, RuntimeStartupProbe};
use tokio::time::Instant;

const ADAPTER_INTERVAL: Duration = Duration::from_secs(15 * 60);

async fn coordinator() -> (Arc<MemoryOwnershipStore>, Arc<OwnershipCoordinator>) {
    let store = Arc::new(MemoryOwnershipStore::default());
    let coordinator = OwnershipCoordinator::register_with_cleanup_pacing(
        store.clone(),
        Member {
            instance_id: "startup-sibling-fixture".into(),
            protocol: PROTOCOL_VERSION,
            relay_address: "http://owner.invalid".into(),
            expires_at_ms: 0,
            draining: false,
            owned_agents: 0,
            max_owned_agents: 8,
        },
        false,
    )
    .await
    .unwrap();
    (store, coordinator)
}

fn runner(
    h: &Harness,
    store: Arc<fixture::Store>,
    ownership: Option<Arc<OwnershipCoordinator>>,
    enabled: bool,
) -> Arc<AgentWorker> {
    AgentWorker::new(
        AgentWorkerDependencies {
            store,
            adapter: h.adapter.clone(),
            skills: Arc::new(EmptyAgentSkillResolver),
            registry: h.registry.clone(),
            updates: h.updates.clone(),
            operations: AgentOperationService::new(h.store.clone(), h.registry.clone()),
        },
        AgentWorkerConfig::new("startup-sibling-worker", DEPLOYMENT_ID)
            .with_ownership(ownership)
            .with_startup_sibling_hold(enabled),
    )
}

async fn record_hold(h: &Harness, worker: &AgentWorker, due_at: Instant) -> Sandbox {
    let sandbox = h
        .store
        .get_sandbox(&h.workspace.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let runtime = sandbox.runtime.as_ref().unwrap();
    assert!(runtime.boot_id.is_some());
    let result: Result<(), _> = Err(AdapterError::RuntimeStartupHold(
        RuntimeStartupFailure::new(RuntimeStartupFailureClass::ExitedBeforeServing, 3).with_probe(
            RuntimeStartupProbe {
                runtime: h.runtime.clone(),
                sandbox_id: sandbox.id.to_string(),
                generation: runtime.provision_generation.unwrap(),
                next_probe_at: due_at,
            },
        ),
    ));
    worker.observe_runtime_startup(&sandbox, Instant::now(), &result);
    sandbox
}

#[tokio::test(start_paused = true)]
async fn startup_sibling_hold_suppresses_every_closed_attempt_and_wakes_one_due_probe() {
    for owned in [false, true] {
        let h = Harness::with_upgrade_support(Arc::new(EmptyAgentSkillResolver)).await;
        let other =
            Harness::with_store(h.store.clone(), Arc::new(EmptyAgentSkillResolver), 1).await;
        let one = h.admit("held-one", vec![]).await;
        let two = h.admit("held-two", vec![]).await;
        let healthy = other.admit("healthy", vec![]).await;
        let store = fixture::Store::new(h.store.clone());
        let (owners, owner) = coordinator().await;
        let worker = runner(&h, store.clone(), owned.then(|| owner.clone()), true);
        let queued = worker.due_work(h.now, 3).await.unwrap();
        assert!(queued.contains(&one.id) && queued.contains(&two.id));
        let started = Instant::now();
        let due_at = started + ADAPTER_INTERVAL;
        let _ = record_hold(&h, &worker, due_at).await;
        let before = h.backend.requests().await.len();

        assert_eq!(
            worker.due_work(h.now, 1).await.unwrap().as_slice(),
            std::slice::from_ref(&healthy.id)
        );
        assert!(other
            .worker
            .run_scheduled(&healthy.id, h.now)
            .await
            .unwrap());
        assert_eq!(other.stored(&healthy.id).await.status, AgentStatus::Ready);
        // A held sibling must return before asking the coordinator, even from
        // work that was queued before the negative inspection was observed.
        for _ in 0..89 {
            tokio::time::advance(Duration::from_secs(10)).await;
            if owned {
                owner.maintain().await.unwrap();
            }
            owners.partition(true);
            let now = h.now + started.elapsed();
            assert!(worker.due_work(now, 1).await.unwrap().is_empty());
            for held in [&one, &two] {
                assert!(!worker.run_one(&held.id, now).await.unwrap());
            }
            owners.partition(false);
        }
        tokio::time::advance(Duration::from_secs(9)).await;
        owners.partition(true);
        assert!(!worker
            .run_one(&one.id, h.now + started.elapsed())
            .await
            .unwrap());
        assert_eq!(worker.next_startup_probe(), Some(due_at));
        {
            let calls = store.calls.lock().unwrap();
            assert!(!calls.contains_key("claim_agent_work"));
            assert!(!calls.contains_key("commit_agent_work"));
        }
        assert_eq!(h.backend.requests().await.len(), before);
        assert_eq!(h.stored(&one.id).await, one);
        assert_eq!(h.stored(&two.id).await, two);
        owners.partition(false);
        for held in [&one, &two] {
            assert!(owners.owner(&held.id).await.unwrap().is_none());
        }

        tokio::time::advance(Duration::from_secs(1)).await;
        let now = h.now + started.elapsed();
        let probe = worker.due_work(now, 1).await.unwrap();
        assert_eq!(probe.len(), 1);
        assert!(probe[0] == one.id || probe[0] == two.id);
        assert!(worker.run_one(&probe[0], now).await.unwrap());
        assert_eq!(
            store.calls.lock().unwrap().get("claim_agent_work"),
            Some(&1)
        );
    }
}

#[tokio::test(start_paused = true)]
async fn startup_sibling_hold_never_filters_a_new_control_or_changes_the_disabled_path() {
    let h = Harness::with_upgrade_support(Arc::new(EmptyAgentSkillResolver)).await;
    let agent = h.admit("held-control", vec![]).await;
    let store = fixture::Store::new(h.store.clone());
    let disabled = runner(&h, store.clone(), None, false);
    let started = Instant::now();
    let _ = record_hold(&h, &disabled, started + ADAPTER_INTERVAL).await;
    assert!(disabled.next_startup_probe().is_none());
    assert_eq!(
        disabled.due_work(h.now, 1).await.unwrap().as_slice(),
        std::slice::from_ref(&agent.id)
    );

    let enabled = runner(&h, store.clone(), None, true);
    let _ = record_hold(&h, &enabled, started + ADAPTER_INTERVAL).await;
    assert!(enabled.due_work(h.now, 1).await.unwrap().is_empty());
    h.store
        .schedule_agent_stop(AgentStopRequest {
            target: AgentControlTarget::from(&agent),
            reason: AgentStopReason::Owner,
            requested_at: rfc3339_at(h.now),
        })
        .await
        .unwrap();
    let control = h.stored(&agent.id).await;
    assert_eq!(control.work.step, AgentWorkStep::FenceInteraction);
    assert_eq!(
        enabled.due_work(h.now, 1).await.unwrap().as_slice(),
        std::slice::from_ref(&agent.id)
    );
    assert!(enabled.run_one(&agent.id, h.now).await.unwrap());
    assert_ne!(
        h.stored(&agent.id).await.work.step,
        AgentWorkStep::FenceInteraction
    );
    assert_eq!(
        store.calls.lock().unwrap().get("claim_agent_work"),
        Some(&1)
    );
}

#[tokio::test(start_paused = true)]
async fn startup_sibling_hold_times_out_advisory_reads_before_acquisition_and_claim() {
    let h = Harness::with_upgrade_support(Arc::new(EmptyAgentSkillResolver)).await;
    let control = h.admit("timed-control", vec![]).await;
    let sibling = h.admit("verified-closed", vec![]).await;
    h.store
        .schedule_agent_stop(AgentStopRequest {
            target: AgentControlTarget::from(&control),
            reason: AgentStopReason::Owner,
            requested_at: rfc3339_at(h.now),
        })
        .await
        .unwrap();
    let store = fixture::Store::new(h.store.clone());
    let (owners, owner) = coordinator().await;
    let worker = runner(&h, store.clone(), Some(owner), true);
    let _ = record_hold(&h, &worker, Instant::now() + ADAPTER_INTERVAL).await;
    // Only the advisory reads are delayed. The claim and post-work read use
    // the healthy store, and the held sibling can still be verified.
    store.delay_read("get_agent", 1, Duration::from_secs(2));
    store.delay_read("get_agent", 2, Duration::from_secs(2));
    let started = Instant::now();
    assert!(tokio::time::timeout(
        Duration::from_millis(350),
        worker.run_one(&control.id, h.now)
    )
    .await
    .expect("both advisory reads must time out before the control loses its slot")
    .unwrap());
    assert!(started.elapsed() >= Duration::from_millis(300));
    assert_ne!(
        h.stored(&control.id).await.work.step,
        AgentWorkStep::FenceInteraction
    );
    assert_eq!(
        store.calls.lock().unwrap().get("claim_agent_work"),
        Some(&1)
    );
    assert_eq!(store.calls.lock().unwrap().get("get_agent"), Some(&3));
    owners.partition(true);
    assert!(!worker.run_one(&sibling.id, h.now).await.unwrap());
    assert_eq!(
        store.calls.lock().unwrap().get("claim_agent_work"),
        Some(&1)
    );
}

#[tokio::test(start_paused = true)]
async fn startup_sibling_hold_shares_one_deadline_across_agent_and_binding_reads() {
    let h = Harness::with_upgrade_support(Arc::new(EmptyAgentSkillResolver)).await;
    let agent = h.admit("shared-advisory-budget", vec![]).await;
    let store = fixture::Store::new(h.store.clone());
    let worker = runner(&h, store.clone(), None, true);
    let _ = record_hold(&h, &worker, Instant::now() + ADAPTER_INTERVAL).await;
    store.delay_read("get_agent", 1, Duration::from_millis(100));
    store.delay_read("get_sandbox", 1, Duration::from_millis(100));
    let started = Instant::now();
    assert!(
        tokio::time::timeout(Duration::from_millis(180), worker.run_one(&agent.id, h.now))
            .await
            .expect("the first read must consume part of the binding check budget")
            .unwrap()
    );
    assert!(started.elapsed() >= Duration::from_millis(150));
    assert_eq!(
        store.calls.lock().unwrap().get("claim_agent_work"),
        Some(&1)
    );
}
