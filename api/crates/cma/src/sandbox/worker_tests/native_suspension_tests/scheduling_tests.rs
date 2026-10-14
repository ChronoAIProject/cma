use super::*;

async fn active_fixture() -> (Fixture, Sandbox, SystemTime, Arc<SandboxWorker>) {
    let mut f = Fixture::new().await;
    let created = f.harness.create().await;
    let mut now = start_time(&created);
    let active = f
        .drive(
            &created.id,
            &mut now,
            SandboxStatus::Active,
            &runtime_image(),
        )
        .await;
    f.harness.adapter = f.adapter(&runtime_image());
    crate::sandbox::runtime_attachment::adopt_bound_runtime(
        f.harness.adapter.as_ref(),
        &active,
        "test",
    )
    .await
    .unwrap();
    let worker = f
        .harness
        .worker_with_adapter("chained", f.harness.adapter.clone());
    (f, active, now, worker)
}

async fn schedule_pause(f: &Fixture, active: &Sandbox, now: SystemTime) {
    f.harness
        .store
        .schedule_sandbox_lifecycle(SandboxLifecycleCommand::pause(
            active.id.clone(),
            active.owner.clone(),
            rfc3339_at(now),
            None,
        ))
        .await
        .unwrap();
}

#[tokio::test]
async fn native_lifecycle_sweep_chains_pause_and_resume_with_fresh_fences_and_same_home() {
    let (f, active, now, worker) = active_fixture().await;
    let first = lifecycle_recovery_tests::prepare_agent(&f.harness, &active, now, 1).await;
    let second = lifecycle_recovery_tests::prepare_agent(&f.harness, &active, now, 2).await;
    let original = f.backend.snapshot().await;
    schedule_pause(&f, &active, now).await;
    let admitted = f.harness.stored(&active.id).await;
    assert_eq!(worker.run_due(now).await.unwrap(), 1);
    let paused = f.harness.stored(&active.id).await;
    assert_eq!(paused.status, SandboxStatus::Paused);
    assert_eq!(paused.work.next_fence, admitted.work.next_fence + 4);
    assert!(paused.work.lease.is_none());
    assert_eq!(paused.lifecycle.home, active.lifecycle.home);
    assert_eq!(
        paused.lifecycle.suspended_runtime.as_deref(),
        active.runtime.as_ref()
    );
    f.harness
        .store
        .schedule_sandbox_lifecycle(SandboxLifecycleCommand::revive(
            paused.id.clone(),
            paused.owner.clone(),
            rfc3339_at(now),
            SandboxReviveCause::ExplicitRequest,
        ))
        .await
        .unwrap();
    assert_eq!(worker.run_due(now).await.unwrap(), 1);
    let resumed = f.harness.stored(&active.id).await;
    assert_eq!(
        resumed.status,
        SandboxStatus::Active,
        "{:?}",
        resumed.failure
    );
    assert_eq!(resumed.work.next_fence, paused.work.next_fence + 8);
    assert!(resumed.work.lease.is_none());
    assert_eq!(resumed.lifecycle.home, active.lifecycle.home);
    assert_eq!(
        resumed.runtime.as_ref().unwrap().runtime_id,
        active.runtime.as_ref().unwrap().runtime_id
    );
    let restored = f.backend.snapshot().await;
    assert_eq!(restored.workspaces, original.workspaces);
    assert_eq!(restored.worktrees, original.worktrees);
    assert_eq!(restored.homes, original.homes);
    for previous in [first, second] {
        let after = f
            .harness
            .store
            .get_agent(&previous.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(after.status, AgentStatus::Ready);
        assert_eq!(
            after.conversation.conversation_id,
            previous.conversation.conversation_id
        );
        assert!(after.conversation.runtime_generation > previous.conversation.runtime_generation);
    }
    let events = f
        .harness
        .store
        .sandbox_events(&active.id, 0, 200)
        .await
        .unwrap();
    assert!(events.windows(2).all(|pair| pair[1].seq == pair[0].seq + 1));
    assert_eq!(worker.run_due(now).await.unwrap(), 0);
}

#[tokio::test]
async fn native_lifecycle_sweep_yields_after_retry_without_replaying_effects_before_due() {
    let (f, active, now, worker) = active_fixture().await;
    schedule_pause(&f, &active, now).await;
    f.backend.lose_suspend.store(true, Ordering::SeqCst);
    assert_eq!(worker.run_due(now).await.unwrap(), 1);
    let retry = f.harness.stored(&active.id).await;
    assert_eq!(retry.status, SandboxStatus::Pausing);
    assert_eq!(retry.work.attempts, 1);
    assert!(stamp_time(&retry.work.ready_at) > now);
    assert!(retry.work.lease.is_none());
    let calls = f.backend.calls.snapshot();
    assert_eq!(worker.run_due(now).await.unwrap(), 0);
    assert_eq!(f.harness.stored(&active.id).await, retry);
    assert_eq!(f.backend.calls.snapshot(), calls);
    assert_eq!(
        worker
            .run_due(stamp_time(&retry.work.ready_at))
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        f.harness.stored(&active.id).await.status,
        SandboxStatus::Paused
    );
    assert_eq!(
        f.backend
            .calls
            .snapshot()
            .iter()
            .filter(|s| s.as_str() == "suspend")
            .count(),
        1
    );
}

#[tokio::test]
async fn native_lifecycle_sweep_respects_active_lease_and_rejects_stale_completion() {
    let (f, active, now, worker) = active_fixture().await;
    schedule_pause(&f, &active, now).await;
    worker.run_one(&active.id, now).await.unwrap();
    worker.run_one(&active.id, now).await.unwrap();
    let expires = now + Duration::from_secs(2);
    let stale = f
        .harness
        .store
        .claim_sandbox_work(&active.id, "chained", rfc3339_at(now), rfc3339_at(expires))
        .await
        .unwrap()
        .unwrap();
    let before = f.backend.calls.snapshot();
    assert_eq!(worker.run_due(now).await.unwrap(), 0);
    assert_eq!(f.backend.calls.snapshot(), before);
    assert_eq!(worker.run_due(expires).await.unwrap(), 1);
    let paused = f.harness.stored(&active.id).await;
    assert_eq!(paused.status, SandboxStatus::Paused);
    let completed = f.backend.calls.snapshot();
    assert!(worker
        .execute_claimed_for_test(stale, expires)
        .await
        .is_err());
    assert_eq!(f.harness.stored(&active.id).await, paused);
    assert_eq!(f.backend.calls.snapshot(), completed);
}

#[tokio::test]
async fn native_retirement_sweep_chains_steps_and_stops_before_purge_wait() {
    let (f, active, now, worker) = active_fixture().await;
    schedule_pause(&f, &active, now).await;
    worker.run_due(now).await.unwrap();
    let paused = f.harness.stored(&active.id).await;
    let mut retiring = paused.clone();
    retiring.begin_retirement(rfc3339_at(now)).unwrap();
    f.harness
        .store
        .save_sandbox(paused.revision, retiring)
        .await
        .unwrap();
    assert_eq!(worker.run_due(now).await.unwrap(), 1);
    let retired = f.harness.stored(&active.id).await;
    assert_eq!(
        retired.status,
        SandboxStatus::Retired,
        "{:?}",
        retired.failure
    );
    assert!(retired.lifecycle.home.as_ref().unwrap().released);
    assert!(retired.lifecycle.suspended_runtime.is_none());
    assert_eq!(retired.work.step, SandboxWorkStep::WaitForPurge);
    assert_eq!(worker.run_due(now).await.unwrap(), 0);
}
