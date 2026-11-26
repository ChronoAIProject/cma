use super::super::agent_worker::WorkerFailure;
use super::*;
use layer2_core::sandbox::store::SandboxStoreError;

pub(super) fn worker<S: FixtureStore>(
    h: &Harness<S>,
    holder: &str,
    enabled: bool,
) -> Arc<AgentWorker> {
    AgentWorker::new(
        AgentWorkerDependencies {
            store: h.store.clone(),
            adapter: h.adapter.clone(),
            skills: Arc::new(EmptyAgentSkillResolver),
            registry: h.registry.clone(),
            updates: h.updates.clone(),
            operations: AgentOperationService::new(h.store.clone(), h.registry.clone()),
        },
        AgentWorkerConfig::new(holder, DEPLOYMENT_ID).with_work_retry_policy(enabled),
    )
}

fn failure() -> WorkerFailure {
    WorkerFailure::retryable("agent_runtime_unavailable", "dependency not yet available")
}

async fn claim(h: &Harness, agent: &Agent, holder: &str, now: SystemTime) -> Agent {
    h.store
        .claim_agent_work(
            &agent.id,
            holder,
            rfc3339_at(now),
            rfc3339_at(now + Duration::from_secs(60)),
        )
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn cleanup_retry_policy_survives_restart_and_parked_probe_advances() {
    for step in [AgentWorkStep::CloseProcess, AgentWorkStep::RemoveWorktree] {
        let h = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
        let initial = ledger_hold_tests::prepare(&h, step).await;
        let mut now = ledger_hold_tests::due(&initial);
        for (attempt, delay) in [
            30, 60, 120, 240, 300, 300, 300, 300, 300, 300, 300, 300, 1800, 1800,
        ]
        .into_iter()
        .enumerate()
        {
            let holder = format!("retry-holder-{attempt}");
            let worker = worker(&h, &holder, true);
            let current = claim(&h, &initial, &holder, now).await;
            let lease = current.work.lease.clone().unwrap();
            worker
                .handle_failure(current, failure(), now)
                .await
                .unwrap();
            let stored = h.stored(&initial.id).await;
            assert_eq!(stored.work.attempts, u32::try_from(attempt + 1).unwrap());
            assert_eq!(
                stored.work.ready_at,
                rfc3339_at(now + Duration::from_secs(delay))
            );
            assert_eq!(stored.status, initial.status);
            assert_eq!(stored.work.step, initial.work.step);
            assert_eq!(stored.work.receipts, initial.work.receipts);
            assert!(stored.work.lease.is_none());
            assert_eq!(stored.work.next_fence, lease.fence + 1);
            let decoded: Agent =
                serde_json::from_value(serde_json::to_value(&stored).unwrap()).unwrap();
            assert_eq!(decoded, stored);
            let before = now + Duration::from_secs(delay - 1);
            assert!(!worker.run_one(&initial.id, before).await.unwrap());
            now += Duration::from_secs(delay);
        }
        let probe = worker(&h, "healthy-probe", true);
        assert!(probe.run_one(&initial.id, now).await.unwrap());
        let progressed = h.stored(&initial.id).await;
        assert_eq!(progressed.work.attempts, 0);
        assert_ne!(progressed.work.step, initial.work.step);
    }
}

#[tokio::test]
async fn disabled_retry_policy_retains_legacy_cleanup_and_interactive_intervals() {
    for step in [
        AgentWorkStep::CloseProcess,
        AgentWorkStep::RemoveWorktree,
        AgentWorkStep::PublishConnection,
    ] {
        let h = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
        let initial = ledger_hold_tests::prepare(&h, step).await;
        let worker = worker(&h, "legacy-worker", false);
        let mut now = ledger_hold_tests::due(&initial);
        for attempt in 0..14_u32 {
            let current = claim(&h, &initial, "legacy-worker", now).await;
            worker
                .handle_failure(current, failure(), now)
                .await
                .unwrap();
            let expected = if step == AgentWorkStep::RemoveWorktree {
                (30 * (1_u64 << attempt.min(4))).min(300)
            } else {
                30
            };
            let stored = h.stored(&initial.id).await;
            assert_eq!(
                stored.work.ready_at,
                rfc3339_at(now + Duration::from_secs(expected))
            );
            now += Duration::from_secs(expected);
        }
    }
}

#[tokio::test]
async fn foreground_retries_keep_three_initial_intervals_and_never_park() {
    let h = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let initial = ledger_hold_tests::prepare(&h, AgentWorkStep::PublishConnection).await;
    let worker = worker(&h, "foreground", true);
    let mut now = ledger_hold_tests::due(&initial);
    for delay in [
        30, 30, 30, 60, 120, 240, 300, 300, 300, 300, 300, 300, 300, 300,
    ] {
        let current = claim(&h, &initial, "foreground", now).await;
        worker
            .handle_failure(current, failure(), now)
            .await
            .unwrap();
        let stored = h.stored(&initial.id).await;
        assert_eq!(
            stored.work.ready_at,
            rfc3339_at(now + Duration::from_secs(delay))
        );
        now += Duration::from_secs(delay);
    }
}

#[tokio::test]
async fn takeover_preserves_attempts_and_refuses_the_expired_holder() {
    let h = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let initial = ledger_hold_tests::prepare(&h, AgentWorkStep::CloseProcess).await;
    let start = ledger_hold_tests::due(&initial);
    let old = worker(&h, "old", true);
    let current = claim(&h, &initial, "old", start).await;
    old.handle_failure(current, failure(), start).await.unwrap();
    let due = start + Duration::from_secs(30);
    let expired = claim(&h, &initial, "old", due).await;
    let at = due + Duration::from_secs(60);
    let successor = claim(&h, &initial, "new", at).await;
    assert_eq!(successor.work.attempts, 1);
    assert_eq!(
        old.handle_failure(expired, failure(), at).await,
        Err(SandboxStoreError::Conflict)
    );
    worker(&h, "new", true)
        .handle_failure(successor, failure(), at)
        .await
        .unwrap();
    let stored = h.stored(&initial.id).await;
    assert_eq!(stored.work.attempts, 2);
    assert_eq!(
        stored.work.ready_at,
        rfc3339_at(at + Duration::from_secs(60))
    );
}

#[tokio::test]
async fn ledger_hold_keeps_legacy_floor_and_parks_only_maintenance() {
    for step in [
        AgentWorkStep::CloseProcess,
        AgentWorkStep::PublishConnection,
    ] {
        let h = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
        let initial = ledger_hold_tests::prepare(&h, step).await;
        let retry = worker(&h, "ledger", true);
        let mut now = ledger_hold_tests::due(&initial);
        for attempt in 0..14 {
            let current = claim(&h, &initial, "ledger", now).await;
            let failure = WorkerFailure::retryable(
                layer2_core::sandbox::store::AGENT_RUNTIME_LEDGER_HOLD,
                layer2_core::sandbox::store::AGENT_RUNTIME_LEDGER_HOLD_DETAIL,
            );
            retry.handle_failure(current, failure, now).await.unwrap();
            let delay = if attempt >= 12 && step == AgentWorkStep::CloseProcess {
                1800
            } else {
                (30 * (1_u64 << attempt.min(4))).min(300)
            };
            let stored = h.stored(&initial.id).await;
            assert_eq!(
                stored.work.ready_at,
                rfc3339_at(now + Duration::from_secs(delay))
            );
            assert_eq!(stored.work.step, step);
            assert_eq!(stored.work.receipts, initial.work.receipts);
            now += Duration::from_secs(delay);
        }
        assert!(retry.run_one(&initial.id, now).await.unwrap());
        assert_eq!(h.stored(&initial.id).await.work.attempts, 0);
    }
}

#[tokio::test(start_paused = true)]
async fn startup_retry_keeps_its_delay_after_slow_parent_reads() {
    for delay_seconds in [2, 61] {
        let h = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
        let initial = ledger_hold_tests::prepare(&h, AgentWorkStep::OpenConversation).await;
        let now = ledger_hold_tests::due(&initial);
        let claimed = claim(&h, &initial, "slow-retry", now).await;
        let store = lifecycle_failure_tests::fixture::Store::new(h.store.clone());
        store.delay_read("get_sandbox", 1, Duration::from_secs(delay_seconds));
        let mut config = AgentWorkerConfig::new("slow-retry", DEPLOYMENT_ID);
        config.retry_delay = Duration::from_secs(1);
        let worker = AgentWorker::new(
            AgentWorkerDependencies {
                store,
                adapter: h.adapter.clone(),
                skills: Arc::new(EmptyAgentSkillResolver),
                registry: h.registry.clone(),
                updates: h.updates.clone(),
                operations: AgentOperationService::new(h.store.clone(), h.registry.clone()),
            },
            config,
        );
        let before = h.backend.requests().await.len();
        let result = worker.handle_failure(claimed.clone(), failure(), now).await;
        let stored = h.stored(&initial.id).await;
        assert_eq!(h.backend.requests().await.len(), before);
        if delay_seconds == 61 {
            assert_eq!(result, Err(SandboxStoreError::Conflict));
            assert_eq!(
                stored, claimed,
                "expired authority must not publish a retry"
            );
            continue;
        }
        result.unwrap();
        assert_eq!(
            stored.work.ready_at,
            rfc3339_at(now + Duration::from_secs(3))
        );
        assert_eq!(stored.updated_at, rfc3339_at(now + Duration::from_secs(2)));
        assert_eq!(stored.work.attempts, claimed.work.attempts + 1);
        assert_eq!(stored.work.step, claimed.work.step);
        assert_eq!(stored.work.receipts, claimed.work.receipts);
        assert_eq!(stored.work.cleanup_debt, claimed.work.cleanup_debt);
        assert_eq!(
            stored.conversation.first_message,
            claimed.conversation.first_message
        );
        assert!(stored.work.lease.is_none());
        assert!(!worker
            .run_one(&initial.id, now + Duration::from_secs(2))
            .await
            .unwrap());
        assert!(worker
            .run_one(&initial.id, now + Duration::from_secs(3))
            .await
            .unwrap());
        assert_eq!(
            h.stored(&initial.id).await.work.step,
            AgentWorkStep::PublishReady
        );
    }
}
