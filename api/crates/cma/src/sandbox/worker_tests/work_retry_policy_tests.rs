use super::*;

struct RetryStopper {
    fail: AtomicBool,
    external: bool,
}

#[async_trait::async_trait]
impl LiveAgentStopper for RetryStopper {
    async fn stop_live_agents(
        &self,
        _: &Sandbox,
        _: Option<&RuntimeHandle>,
        _: &str,
    ) -> Result<(), SandboxDependencyError> {
        if !self.fail.load(Ordering::SeqCst) {
            return Ok(());
        }
        Err(if self.external {
            SandboxDependencyError::Security("retained authority needs recovery".into())
        } else {
            SandboxDependencyError::Transient("sandbox agents are still stopping".into())
        })
    }
}

fn retry_worker(
    h: &Harness,
    holder: &str,
    enabled: bool,
    stopper: Arc<RetryStopper>,
) -> Arc<SandboxWorker> {
    SandboxWorker::new(
        SandboxWorkerDependencies {
            store: h.store.clone(),
            adapter: h.adapter.clone(),
            credentials: h.credentials.clone(),
            workspaces: Arc::new(WorkspaceRecorder {
                actions: h.actions.clone(),
            }),
            agents: stopper,
            registry: h.registry.clone(),
            updates: DurableAgentUpdateRecorder::new(h.store.clone()),
            operations: AgentOperationService::new(h.store.clone(), h.registry.clone()),
        },
        SandboxWorkerConfig::new(holder, "test", runtime_image(), h.policy.clone())
            .with_work_retry_policy(enabled),
    )
}

async fn retiring(h: &Harness) -> (Sandbox, SystemTime) {
    let created = h.create().await;
    let now = start_time(&created);
    let active = h.drive_active(&created.id, now).await;
    h.service
        .delete(&caller("owner", [Capability::DeleteOwnSandbox]), &active.id)
        .await
        .unwrap();
    let initial = h.stored(&active.id).await;
    let now = SystemTime::UNIX_EPOCH
        + Duration::from_secs(seconds_from_rfc3339(&initial.work.ready_at).unwrap());
    assert_eq!(initial.work.step, SandboxWorkStep::StopAgents);
    (initial, now)
}

#[tokio::test]
async fn retirement_retry_budget_survives_worker_replacement_and_success_resets_attempts() {
    for external in [false, true] {
        let h = Harness::new(vec![], 0, Duration::from_secs(60)).await;
        let (initial, mut now) = retiring(&h).await;
        let stopper = Arc::new(RetryStopper {
            fail: AtomicBool::new(true),
            external,
        });
        let delays: &[u64] = if external {
            &[30, 60, 120]
        } else {
            &[30, 60, 120, 240, 300, 300, 300, 300, 300, 300, 300, 300]
        };
        for (attempt, &delay) in delays.iter().enumerate() {
            let worker = retry_worker(&h, &format!("retry-{attempt}"), true, stopper.clone());
            assert!(worker.run_one(&initial.id, now).await.unwrap());
            let stored = h.stored(&initial.id).await;
            assert_eq!(stored.work.attempts, u32::try_from(attempt + 1).unwrap());
            assert_eq!(
                stored.work.ready_at,
                rfc3339_at(now + Duration::from_secs(delay))
            );
            assert_eq!(stored.status, SandboxStatus::Retiring);
            assert_eq!(stored.runtime, initial.runtime);
            assert_eq!(stored.work.step, SandboxWorkStep::StopAgents);
            assert!(stored.work.lease.is_none());
            let decoded: Sandbox =
                serde_json::from_value(serde_json::to_value(&stored).unwrap()).unwrap();
            assert_eq!(decoded, stored);
            assert!(!worker
                .run_one(&initial.id, now + Duration::from_secs(delay - 1))
                .await
                .unwrap());
            now += Duration::from_secs(delay);
        }
        stopper.fail.store(false, Ordering::SeqCst);
        let successor = retry_worker(&h, "successful-successor", true, stopper);
        assert!(successor.run_one(&initial.id, now).await.unwrap());
        let advanced = h.stored(&initial.id).await;
        assert_eq!(advanced.work.attempts, 0);
        assert_eq!(
            advanced.work.step,
            SandboxWorkStep::RevokeRetirementCredentials
        );
        assert_eq!(advanced.work.ready_at, rfc3339_at(now));
    }
}

#[tokio::test]
async fn disabled_retry_pacing_still_gives_retirement_a_terminal_result() {
    let h = Harness::new(vec![], 0, Duration::from_secs(60)).await;
    let (initial, mut now) = retiring(&h).await;
    let stopper = Arc::new(RetryStopper {
        fail: AtomicBool::new(true),
        external: true,
    });
    let worker = retry_worker(&h, "legacy", false, stopper);
    for attempt in 1..=3 {
        assert!(worker.run_one(&initial.id, now).await.unwrap());
        let stored = h.stored(&initial.id).await;
        assert_eq!(stored.work.attempts, attempt);
        assert_eq!(
            stored.work.ready_at,
            rfc3339_at(now + Duration::from_secs(30))
        );
        assert_eq!(stored.status, SandboxStatus::Retiring);
        now += Duration::from_secs(30);
    }
    assert!(worker.run_one(&initial.id, now).await.unwrap());
    let stored = h.stored(&initial.id).await;
    assert!(stored.retirement_retry_exhausted());
    assert_eq!(stored.runtime, initial.runtime);
    assert!(!worker
        .run_one(&initial.id, now + Duration::from_secs(3600))
        .await
        .unwrap());
}

#[tokio::test]
async fn sandbox_takeover_continues_the_budget_and_fences_the_old_holder() {
    let h = Harness::new(vec![], 0, Duration::from_secs(60)).await;
    let (initial, now) = retiring(&h).await;
    let stopper = Arc::new(RetryStopper {
        fail: AtomicBool::new(true),
        external: false,
    });
    let old = retry_worker(&h, "old", true, stopper.clone());
    assert!(old.run_one(&initial.id, now).await.unwrap());
    let due = now + Duration::from_secs(30);
    let expired = h
        .store
        .claim_sandbox_work(
            &initial.id,
            "old",
            rfc3339_at(due),
            rfc3339_at(due + Duration::from_secs(60)),
        )
        .await
        .unwrap()
        .unwrap();
    let at = due + Duration::from_secs(60);
    let next = h
        .store
        .claim_sandbox_work(
            &initial.id,
            "new",
            rfc3339_at(at),
            rfc3339_at(at + Duration::from_secs(60)),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(next.work.attempts, 1);
    assert_eq!(
        old.execute_claimed_for_test(expired, at).await,
        Err(SandboxStoreError::Conflict)
    );
    retry_worker(&h, "new", true, stopper)
        .execute_claimed_for_test(next, at)
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
async fn interactive_revive_queue_keeps_short_initial_retries_and_never_parks() {
    let h = Harness::new(vec![], 0, Duration::from_secs(60)).await;
    let created = h.create().await;
    let start = start_time(&created);
    let active = h.drive_active(&created.id, start).await;
    let mut agent = lifecycle_queue_tests::ready_agent(&active, start);
    agent.work.step = layer2_core::sandbox::AgentWorkStep::PublishConnection;
    h.store.create_agent(agent.clone()).await.unwrap();
    let mut now = start + Duration::from_secs(1);
    lifecycle_queue_tests::begin_queue_phase(&h, &active.id, now).await;
    let worker = retry_worker(
        &h,
        "queue",
        true,
        Arc::new(RetryStopper {
            fail: AtomicBool::new(false),
            external: false,
        }),
    );
    let requests = h.backend.requests().await;
    for (attempt, delay) in [5, 5, 5, 10, 20, 40, 80, 80, 80, 80, 80, 80, 80, 80]
        .into_iter()
        .enumerate()
    {
        assert!(worker.run_one(&active.id, now).await.unwrap());
        let stored = h.stored(&active.id).await;
        assert_eq!(stored.work.attempts, u32::try_from(attempt + 1).unwrap());
        assert_eq!(
            stored.work.ready_at,
            rfc3339_at(now + Duration::from_secs(delay))
        );
        now += Duration::from_secs(delay);
    }
    assert_eq!(h.backend.requests().await, requests);
    let expected = agent.revision;
    agent.revision += 1;
    agent
        .work
        .schedule(layer2_core::sandbox::AgentWorkStep::None, rfc3339_at(now));
    h.store.save_agent(expected, agent).await.unwrap();
    for _ in 0..2 {
        assert!(worker.run_one(&active.id, now).await.unwrap());
    }
    let ready = h.stored(&active.id).await;
    assert_eq!(ready.status, SandboxStatus::Active);
    assert_eq!(ready.work.attempts, 0);
}
