use super::super::lifecycle_failure::{CauseClass, DetailCode, LifecycleAttempt, Origin, Phase};
use super::*;
use layer2_core::agent_ownership::{
    test_support::MemoryOwnershipStore, Member, OwnershipCoordinator, OwnershipStore,
    PROTOCOL_VERSION,
};
use layer2_core::sandbox::store::SandboxStoreError;
use layer3_adapter::owner_protocol::{ReceiverRevocation, TransportAuthority};
#[path = "acquisition_backoff_tests.rs"]
mod acquisition_backoff;
#[path = "lifecycle_failure_store.rs"]
pub(super) mod fixture;

fn runner(
    h: &Harness,
    store: Arc<fixture::Store>,
    ownership: Option<Arc<OwnershipCoordinator>>,
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
        AgentWorkerConfig::new("observed-worker", DEPLOYMENT_ID).with_ownership(ownership),
    )
}

async fn owner() -> (Arc<MemoryOwnershipStore>, Arc<OwnershipCoordinator>) {
    owner_with_pacing(false).await
}

async fn owner_with_pacing(
    enabled: bool,
) -> (Arc<MemoryOwnershipStore>, Arc<OwnershipCoordinator>) {
    let store = Arc::new(MemoryOwnershipStore::default());
    let coordinator = OwnershipCoordinator::register_with_cleanup_pacing(
        store.clone(),
        Member {
            instance_id: "fixture-owner".into(),
            protocol: PROTOCOL_VERSION,
            relay_address: "http://owner.invalid".into(),
            expires_at_ms: 0,
            draining: false,
            owned_agents: 0,
            max_owned_agents: 64,
        },
        enabled,
    )
    .await
    .unwrap();
    (store, coordinator)
}

#[tokio::test]
async fn acquisition_stale_and_unavailable_keep_conflict_without_claiming() {
    for stale in [true, false] {
        let h = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
        let agent = h.admit("observed-acquire", vec![]).await;
        let store = fixture::Store::new(h.store.clone());
        let (ownership_store, owner) = owner().await;
        if stale {
            owner.fail().await;
        } else {
            ownership_store.partition(true);
        }
        let worker = runner(&h, store.clone(), Some(owner));
        let attempt = LifecycleAttempt::new(Phase::Execution, Some(&agent.id));
        let failure = worker
            .run_scheduled_observed(&agent.id, h.now, &attempt)
            .await
            .unwrap_err();
        assert_eq!(failure.observation.origin, Origin::OwnershipAcquire);
        assert_eq!(
            failure.observation.cause_class,
            if stale {
                CauseClass::OwnershipStale
            } else {
                CauseClass::OwnershipUnavailable
            }
        );
        assert_eq!(failure.into_compatibility(), SandboxStoreError::Conflict);
        assert_eq!(
            worker.run_one(&agent.id, h.now).await,
            Err(SandboxStoreError::Conflict)
        );
        {
            let calls = store.calls.lock().unwrap();
            assert_eq!(
                calls.get("get_agent").copied().unwrap_or(0),
                if stale { 2 } else { 0 }
            );
            assert_eq!(calls.len(), usize::from(stale));
        }
        assert_eq!(h.stored(&agent.id).await, agent);
    }
}

#[tokio::test]
async fn authority_cancellation_retains_claim_without_inventing_execution_or_replay() {
    let h = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let agent = h.admit("observed-cancellation", vec![]).await;
    let store = fixture::Store::new(h.store.clone());
    let (_, owner) = owner().await;
    let authority = owner.acquire(&agent.id).await.unwrap().unwrap();
    *store.cancel_claim.lock().unwrap() = Some(authority);
    let worker = runner(&h, store.clone(), Some(owner));
    let attempt = LifecycleAttempt::new(Phase::Execution, Some(&agent.id));
    let before = h.backend.requests().await.len();
    let failure = worker
        .run_scheduled_observed(&agent.id, h.now, &attempt)
        .await
        .unwrap_err();
    assert_eq!(failure.observation.origin, Origin::AuthorityScope);
    assert_eq!(failure.observation.cause_class, CauseClass::OwnershipStale);
    assert_eq!(failure.into_compatibility(), SandboxStoreError::Conflict);
    let claimed = h.stored(&agent.id).await;
    assert!(claimed.work.lease.is_some());
    assert_eq!(claimed.work.step, agent.work.step);
    assert_eq!(h.backend.requests().await.len(), before);
    assert_eq!(
        store.calls.lock().unwrap().get("claim_agent_work"),
        Some(&1)
    );
}

struct Receiver;
#[async_trait::async_trait]
impl ReceiverRevocation for Receiver {
    fn identity(&self) -> String {
        "private-runtime".into()
    }
    async fn revoke(&self) -> layer3_adapter::Result<()> {
        Err(AdapterError::Unavailable("private-receiver-detail".into()))
    }
}

#[tokio::test]
async fn handoff_failure_preserves_custody_and_registered_revocation_code() {
    let h = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let id = AgentId("absent-agent".into());
    let store = fixture::Store::new(h.store.clone());
    let (ownership_store, owner) = owner().await;
    let authority = owner.acquire(&id).await.unwrap().unwrap();
    authority.register_receiver(Arc::new(Receiver)).unwrap();
    let worker = runner(&h, store, Some(owner));
    let attempt = LifecycleAttempt::new(Phase::Execution, Some(&id));
    let failure = worker
        .run_scheduled_observed(&id, h.now, &attempt)
        .await
        .unwrap_err();
    assert_eq!(failure.observation.origin, Origin::Handoff);
    assert_eq!(
        failure.observation.cause_class,
        CauseClass::OwnershipUnavailable
    );
    assert_eq!(
        failure.observation.detail_code,
        DetailCode::ReceiverRevocationUnconfirmed
    );
    assert_eq!(failure.into_compatibility(), SandboxStoreError::Conflict);
    assert_eq!(
        ownership_store.owner(&id).await.unwrap().unwrap().token,
        authority.token
    );
}

#[tokio::test]
async fn claim_failure_keeps_original_error_and_has_no_effects() {
    for error in [
        SandboxStoreError::Conflict,
        SandboxStoreError::Unavailable("private-claim".into()),
    ] {
        let h = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
        let agent = h.admit("observed-claim", vec![]).await;
        let store = fixture::Store::new(h.store.clone());
        store.fail("claim_agent_work", 1, error.clone());
        store.fail("claim_agent_work", 2, error.clone());
        let worker = runner(&h, store, None);
        let attempt = LifecycleAttempt::new(Phase::Execution, Some(&agent.id));
        let failure = worker
            .run_scheduled_observed(&agent.id, h.now, &attempt)
            .await
            .unwrap_err();
        assert_eq!(failure.observation.origin, Origin::Claim);
        assert_eq!(
            failure.observation.cause_class,
            if error == SandboxStoreError::Conflict {
                CauseClass::StoreConflict
            } else {
                CauseClass::StoreUnavailable
            }
        );
        assert_eq!(failure.into_compatibility(), error);
        assert_eq!(worker.run_one(&agent.id, h.now).await, Err(error));
        assert_eq!(h.stored(&agent.id).await, agent);
    }
}

#[tokio::test]
async fn post_work_and_continuation_read_failures_keep_inner_origin() {
    for ownership in [None, Some(false), Some(true)] {
        let h = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
        let agent = h.admit("observed-read", vec![]).await;
        let store = fixture::Store::new(h.store.clone());
        let coordinator = if let Some(enabled) = ownership {
            Some(owner_with_pacing(enabled).await.1)
        } else {
            None
        };
        // AddWorktree reads the parent/workspace and commits; the orchestration
        // read then fails, after a real effect which must not be rolled back.
        store.fail(
            "get_agent",
            1,
            SandboxStoreError::Unavailable("private-read".into()),
        );
        let worker = runner(&h, store, coordinator);
        let attempt = LifecycleAttempt::new(Phase::Execution, Some(&agent.id));
        let failure = worker
            .run_scheduled_observed(&agent.id, h.now, &attempt)
            .await
            .unwrap_err();
        assert_eq!(
            failure.observation.origin,
            if ownership.is_some() {
                Origin::PostWorkRead
            } else {
                Origin::ContinuationRead
            }
        );
        assert_eq!(
            failure.observation.continuation_index,
            if ownership.is_some() { 0 } else { 1 }
        );
        assert_eq!(
            failure.into_compatibility(),
            SandboxStoreError::Unavailable("private-read".into())
        );
        let saved = h.stored(&agent.id).await;
        assert_eq!(saved.work.step, AgentWorkStep::MaterializeHome);
        assert_eq!(saved.work.receipts.len(), 1);
    }
}

#[tokio::test]
async fn execute_failure_keeps_claim_and_is_not_relabelled_by_outer_ownership() {
    let h = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let agent = h.admit("observed-execute", vec![]).await;
    let store = fixture::Store::new(h.store.clone());
    // A failed publication is uncertain; diagnostics must not replay or clear it.
    store.fail(
        "commit_agent_work",
        1,
        SandboxStoreError::Unavailable("unknown commit".into()),
    );
    let worker = runner(&h, store.clone(), Some(owner().await.1));
    let attempt = LifecycleAttempt::new(Phase::Execution, Some(&agent.id));
    let failure = worker
        .run_scheduled_observed(&agent.id, h.now, &attempt)
        .await
        .unwrap_err();
    assert_eq!(failure.observation.origin, Origin::Execute);
    assert_eq!(
        failure.observation.cause_class,
        CauseClass::StoreUnavailable
    );
    assert_eq!(
        failure.into_compatibility(),
        SandboxStoreError::Unavailable("unknown commit".into())
    );
    assert_eq!(
        store.calls.lock().unwrap().get("commit_agent_work"),
        Some(&1)
    );
    assert!(h.stored(&agent.id).await.work.lease.is_some());
}

#[tokio::test]
async fn sequential_claim_failure_retains_admission_context_and_prior_committed_progress() {
    let h = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let agent = h.admit("observed-continuation", vec![]).await;
    let store = fixture::Store::new(h.store.clone());
    store.fail("claim_agent_work", 2, SandboxStoreError::Conflict);
    let worker = runner(&h, store.clone(), None);
    let attempt = LifecycleAttempt::new(Phase::Execution, Some(&agent.id));
    let initial = attempt.terminated(false);
    let failure = worker
        .run_scheduled_observed(&agent.id, h.now, &attempt)
        .await
        .unwrap_err();
    assert_eq!(failure.observation.attempt_id, initial.attempt_id);
    assert_eq!(failure.observation.agent_hash, initial.agent_hash);
    assert_eq!(failure.observation.continuation_index, 1);
    assert_eq!(failure.observation.origin, Origin::Claim);
    assert_eq!(failure.into_compatibility(), SandboxStoreError::Conflict);
    assert_eq!(
        store.calls.lock().unwrap().get("claim_agent_work"),
        Some(&2)
    );
    let saved = h.stored(&agent.id).await;
    assert_eq!(saved.work.step, AgentWorkStep::MaterializeHome);
    assert_eq!(saved.work.receipts.len(), 1);
    assert!(saved.work.lease.is_none());
}

#[tokio::test]
async fn waiting_retry_publication_and_claim_loss_remain_successful_observed_results() {
    let skills = ScriptedSkills::new([
        Err(AgentSkillResolutionError::Unavailable(
            "private-transient".into(),
        )),
        Ok(vec![]),
    ]);
    let h = Harness::new(skills.clone()).await;
    let agent = h.admit("observed-retry", vec![]).await;
    let attempt = LifecycleAttempt::new(Phase::Execution, Some(&agent.id));
    assert!(h
        .worker
        .run_scheduled_observed(&agent.id, h.now, &attempt)
        .await
        .unwrap());
    let pending = h.stored(&agent.id).await;
    assert_eq!(pending.work.attempts, 1);
    assert_eq!(pending.work.step, AgentWorkStep::MaterializeHome);
    assert_eq!(pending.work.receipts.len(), 1);
    let lost_claim = LifecycleAttempt::new(Phase::Execution, Some(&agent.id));
    assert!(!h
        .worker
        .run_scheduled_observed(&agent.id, h.now, &lost_claim)
        .await
        .unwrap());
    let retry_at = SystemTime::UNIX_EPOCH
        + Duration::from_secs(layer2_core::seconds_from_rfc3339(&pending.work.ready_at).unwrap());
    let retry = LifecycleAttempt::new(Phase::Execution, Some(&agent.id));
    assert!(h
        .worker
        .run_scheduled_observed(&agent.id, retry_at, &retry)
        .await
        .unwrap());
    let ready = h.stored(&agent.id).await;
    assert_eq!(ready.status, AgentStatus::Ready);
    assert_eq!(ready.work.receipts[0], pending.work.receipts[0]);
    assert_eq!(ready.work.receipts.len(), 3);
    assert_eq!(skills.calls.load(Ordering::Relaxed), 2);
    assert_ne!(
        attempt.terminated(false).attempt_id,
        retry.terminated(false).attempt_id
    );
}

#[tokio::test(start_paused = true)]
async fn slow_observations_do_not_consume_the_healthy_discovery_budget() {
    let h = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let (_, owner) = owner_with_pacing(true).await;
    let store = fixture::Store::new(h.store.clone());
    let worker = runner(&h, store.clone(), Some(owner.clone()));
    let healthy = h.admit("healthy-control", vec![]).await;
    let mut owned = Vec::new();
    for key in ["failed-first", "failed-second", "failed-third"] {
        let a = h.admit(key, vec![]).await;
        let auth = owner.acquire(&a.id).await.unwrap().unwrap();
        auth.register_receiver(Arc::new(Receiver)).unwrap();
        assert!(owner.handoff_background(&auth).await.is_err());
        store.delayed_observations.lock().unwrap().push(a.id);
        owned.push(auth);
    }
    tokio::time::advance(std::time::Duration::from_millis(3301)).await;
    for auth in &owned {
        assert!(owner.handoff_background(auth).await.is_err());
    }
    let due = crate::worker_supervision::discover(
        None,
        worker.due_work(h.now, 200),
        SandboxStoreError::Unavailable("discovery deadline".into()),
    )
    .await;
    assert!(
        due.as_ref().is_ok_and(|ids| ids.contains(&healthy.id)),
        "healthy work was lost: {due:?}"
    );
    assert!(!store.calls.lock().unwrap().contains_key("get_agent"));
}

#[tokio::test]
async fn paced_prefix_does_not_consume_the_discovery_limit() {
    let h = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    h.admit("first-candidate", vec![]).await;
    h.admit("second-candidate", vec![]).await;
    let (_, owner) = owner_with_pacing(true).await;
    let worker = runner(
        &h,
        fixture::Store::new(h.store.clone()),
        Some(owner.clone()),
    );
    let ids = worker.due_work(h.now, 2).await.unwrap();
    assert_eq!(ids.len(), 2);
    assert_eq!(
        worker.due_work(h.now, 1).await.unwrap(),
        vec![ids[0].clone()]
    );
    let auth = owner.acquire(&ids[0]).await.unwrap().unwrap();
    auth.register_receiver(Arc::new(Receiver)).unwrap();
    assert!(owner.handoff_background(&auth).await.is_err());
    assert_eq!(
        worker.due_work(h.now, 1).await.unwrap(),
        vec![ids[1].clone()]
    );
}

#[tokio::test]
async fn single_attempt_diagnostics_preserve_claim_and_exhausted_publication_conflicts() {
    for (method, origin, failures) in [
        ("claim_agent_work", Origin::Claim, 1),
        ("commit_agent_work", Origin::Execute, 8),
    ] {
        let h = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
        let agent = h.admit("diagnostic-attempt", vec![]).await;
        let store = fixture::Store::new(h.store.clone());
        // Exhaust the existing publication budget; diagnostics must not restart the worker.
        for call in 1..=failures {
            store.fail(method, call, SandboxStoreError::Conflict);
        }
        let requests = h.backend.requests().await;
        let worker = runner(&h, store.clone(), None);
        let failure = worker
            .run_one_diagnostic(&agent.id, h.now)
            .await
            .unwrap_err();
        assert_eq!(failure.observation.origin, origin);
        assert_eq!(failure.observation.cause_class, CauseClass::StoreConflict);
        assert_eq!(failure.into_compatibility(), SandboxStoreError::Conflict);
        assert_eq!(store.calls.lock().unwrap().get(method), Some(&failures));
        assert_eq!(
            store.calls.lock().unwrap().get("claim_agent_work"),
            Some(&1)
        );
        assert_eq!(h.backend.requests().await, requests);
        assert_eq!(
            h.stored(&agent.id).await.work.lease.is_some(),
            origin == Origin::Execute
        );
    }
}
