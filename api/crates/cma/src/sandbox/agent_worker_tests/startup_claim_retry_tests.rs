//! Real worker scheduling after a definitively aborted startup publication.
use super::*;
use layer2_core::sandbox::store::SandboxStoreError;

async fn production_lease_harness() -> Harness {
    let mut harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let mut config = AgentWorkerConfig::new("agent-worker", DEPLOYMENT_ID);
    config.open_timeout = Duration::from_secs(2);
    harness.worker = AgentWorker::new(
        AgentWorkerDependencies {
            store: harness.store.clone(),
            adapter: harness.adapter.clone(),
            skills: Arc::new(EmptyAgentSkillResolver),
            registry: harness.registry.clone(),
            updates: harness.updates.clone(),
            operations: AgentOperationService::new(harness.store.clone(), harness.registry.clone()),
        },
        config,
    );
    harness
}

#[tokio::test(start_paused = true)]
async fn aborted_add_worktree_publication_reaches_ready_within_thirty_seconds() {
    for conflicts in [1, 8] {
        let harness = production_lease_harness().await;
        let agent = harness.admit("startup-conflict", vec![]).await;
        harness.store.inject_agent_work_publications(
            agent.id.clone(),
            (0..conflicts).map(|_| (AgentWorkStep::AddWorktree, SandboxStoreError::Conflict)),
        );
        let started = tokio::time::Instant::now();
        harness
            .worker
            .run_scheduled(&agent.id, harness.now)
            .await
            .unwrap();
        let pending = harness.stored(&agent.id).await;
        if conflicts == 8 {
            assert_eq!(pending.work.step, AgentWorkStep::AddWorktree);
            assert!(
                pending.work.lease.is_none(),
                "known-aborted intent must release the 18-minute lease"
            );
            assert_eq!(pending.work.attempts, 1);
            assert!(pending.work.cleanup_debt.is_empty());
            assert!(harness
                .backend
                .snapshot(&harness.runtime)
                .await
                .unwrap()
                .worktrees
                .is_empty());
            assert!(!harness
                .worker
                .run_scheduled(&agent.id, harness.now + started.elapsed())
                .await
                .unwrap());
            let due = SystemTime::UNIX_EPOCH
                + Duration::from_secs(
                    layer2_core::seconds_from_rfc3339(&pending.work.ready_at).unwrap(),
                );
            assert!(due.duration_since(harness.now).unwrap() < Duration::from_secs(30));
            tokio::time::sleep(
                due.duration_since(harness.now + started.elapsed())
                    .unwrap_or_default(),
            )
            .await;
            assert!(harness
                .worker
                .due_work(due, 10)
                .await
                .unwrap()
                .contains(&agent.id));
            harness.worker.run_scheduled(&agent.id, due).await.unwrap();
        }
        let ready = harness.stored(&agent.id).await;
        assert_eq!(ready.status, AgentStatus::Ready);
        assert_eq!(ready.work.step, AgentWorkStep::None);
        assert!(started.elapsed() < Duration::from_secs(30));
        assert_eq!(ready.work.receipts.len(), 3);
        let runtime = harness.backend.snapshot(&harness.runtime).await.unwrap();
        assert_eq!(runtime.worktrees.len(), 1);
        assert_eq!(runtime.homes.len(), 1);
    }
}

#[tokio::test(start_paused = true)]
async fn unknown_publication_and_completed_native_effect_do_not_release_or_replay() {
    for (next, failures) in [
        (
            AgentWorkStep::AddWorktree,
            vec![SandboxStoreError::Unavailable("unknown commit".into())],
        ),
        (
            AgentWorkStep::MaterializeHome,
            vec![SandboxStoreError::Conflict; 8],
        ),
    ] {
        let harness = production_lease_harness().await;
        let agent = harness.admit("uncertain-startup", vec![]).await;
        harness.store.inject_agent_work_publications(
            agent.id.clone(),
            failures.into_iter().map(|error| (next, error)),
        );
        assert!(harness
            .worker
            .run_scheduled(&agent.id, harness.now)
            .await
            .is_err());
        let retained = harness.stored(&agent.id).await;
        assert_eq!(
            retained.work.lease.as_ref().unwrap().expires_at,
            rfc3339_at(harness.now + Duration::from_secs(1080))
        );
        assert!(!harness
            .worker
            .run_scheduled(&agent.id, harness.now + Duration::from_secs(20))
            .await
            .unwrap());
        assert_eq!(harness.stored(&agent.id).await, retained);
        let runtime = harness.backend.snapshot(&harness.runtime).await.unwrap();
        assert_eq!(
            runtime.worktrees.len(),
            usize::from(next == AgentWorkStep::MaterializeHome)
        );
    }
}

#[tokio::test(start_paused = true)]
async fn startup_claim_retry_is_capped() {
    let harness = production_lease_harness().await;
    let agent = harness.admit("bounded-startup", vec![]).await;
    let mut now = harness.now;
    for attempt in 0..4 {
        harness.store.inject_agent_work_publications(
            agent.id.clone(),
            (0..8).map(|_| (AgentWorkStep::AddWorktree, SandboxStoreError::Conflict)),
        );
        let result = harness.worker.run_scheduled(&agent.id, now).await;
        let current = harness.stored(&agent.id).await;
        assert_eq!(current.work.attempts, (attempt + 1).min(3));
        if attempt == 3 {
            assert!(result.is_err());
            assert!(current.work.lease.is_some());
        } else {
            result.unwrap();
            assert!(current.work.lease.is_none());
            now = SystemTime::UNIX_EPOCH
                + Duration::from_secs(
                    layer2_core::seconds_from_rfc3339(&current.work.ready_at).unwrap(),
                );
        }
    }
}

#[tokio::test(start_paused = true)]
async fn completion_conflict_waits_inside_publication_without_a_new_work_claim() {
    let harness = production_lease_harness().await;
    let agent = harness.admit("completed-effect-conflict", vec![]).await;
    harness.store.inject_agent_work_publications(
        agent.id.clone(),
        (0..5).map(|_| (AgentWorkStep::MaterializeHome, SandboxStoreError::Conflict)),
    );
    let started = tokio::time::Instant::now();
    harness
        .worker
        .run_scheduled(&agent.id, harness.now)
        .await
        .unwrap();
    let ready = harness.stored(&agent.id).await;
    assert_eq!(ready.status, AgentStatus::Ready);
    assert_eq!(ready.work.step, AgentWorkStep::None);
    assert_eq!(
        ready.work.next_fence, 6,
        "completion retries must retain the original work claim"
    );
    assert_eq!(ready.work.receipts.len(), 3);
    assert!(started.elapsed() >= Duration::from_secs(3));
    assert!(started.elapsed() < Duration::from_secs(30));
}

#[tokio::test]
async fn retry_refuses_stop_changed_claim_expiry_and_parent_retirement() {
    use layer2_core::sandbox::store::AgentStopRequest;
    for change in ["stop", "holder", "fence", "expired", "parent"] {
        let harness = production_lease_harness().await;
        let agent = harness.admit("superseding-startup", vec![]).await;
        let claimed = harness
            .store
            .claim_agent_work(
                &agent.id,
                "agent-worker",
                rfc3339_at(harness.now),
                rfc3339_at(harness.now + Duration::from_secs(1080)),
            )
            .await
            .unwrap()
            .unwrap();
        let mut changed = claimed.clone();
        match change {
            "stop" => {
                harness
                    .store
                    .schedule_agent_stop(AgentStopRequest {
                        target: AgentControlTarget::from(&claimed),
                        reason: AgentStopReason::Owner,
                        requested_at: rfc3339_at(harness.now),
                    })
                    .await
                    .unwrap();
            }
            "parent" => {
                let mut parent = harness
                    .store
                    .get_sandbox(&agent.sandbox_id)
                    .await
                    .unwrap()
                    .unwrap();
                let revision = parent.revision;
                parent.begin_retirement(rfc3339_at(harness.now)).unwrap();
                harness.store.save_sandbox(revision, parent).await.unwrap();
            }
            _ => {
                match change {
                    "holder" => changed.work.lease.as_mut().unwrap().holder = "successor".into(),
                    "fence" => {
                        changed.work.lease.as_mut().unwrap().fence += 1;
                        changed.work.next_fence += 1;
                    }
                    _ => changed.work.lease.as_mut().unwrap().expires_at = rfc3339_at(harness.now),
                }
                changed.revision += 1;
                harness
                    .store
                    .save_agent(claimed.revision, changed)
                    .await
                    .unwrap();
            }
        }
        let retained = harness.stored(&agent.id).await;
        assert_eq!(
            harness
                .worker
                .aborted_intent_for_test(&claimed, harness.now)
                .await,
            Err(SandboxStoreError::Conflict),
            "{change}"
        );
        assert_eq!(harness.stored(&agent.id).await, retained, "{change}");
    }
}
