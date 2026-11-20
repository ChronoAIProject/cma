use super::*;
use crate::sandbox::funding::{FundingError, SandboxFundingAdapter};
use layer2_core::sandbox::{SandboxHomeRestore, SandboxLifecycleOperation, SandboxRevivePhase};

struct FundingRecorder(ActionLog);

#[async_trait::async_trait]
impl SandboxFundingAdapter for FundingRecorder {
    async fn ensure_coverage(
        &self,
        _: &Sandbox,
        _: SystemTime,
    ) -> Result<Option<u64>, FundingError> {
        self.0.push("funding-coverage");
        Ok(None)
    }

    async fn settle(&self, _: &Sandbox, _: SystemTime) -> Result<(), FundingError> {
        self.0.push("funding-settle");
        Ok(())
    }

    async fn renew_due(&self, _: SystemTime) -> Result<usize, FundingError> {
        self.0.push("funding-renew");
        Ok(0)
    }
}

fn pin(sandbox: &Sandbox, now: SystemTime) -> SandboxHomeRestore {
    let operation = sandbox.lifecycle.operation.as_ref().unwrap().authority();
    serde_json::from_value(serde_json::json!({
        "restore_id": "restore-example", "backup_id": "hb-example-1",
        "operation_id": operation.operation_id, "requested_at": rfc3339_at(now),
        "requested_by": "operator-example", "key_sha256": "a".repeat(64),
        "evidence": {"kind": "gone", "observed_at": rfc3339_at(now)}
    }))
    .unwrap()
}

#[tokio::test]
async fn home_restore_pin_waits_before_effects_at_every_revive_phase() {
    for phase in layer2_core::sandbox::SANDBOX_REVIVE_PHASE_ORDER {
        for completed in [false, true] {
            assert_phase_waits(phase, completed).await;
        }
    }
}

async fn assert_phase_waits(phase: SandboxRevivePhase, completed: bool) {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let created = harness.create().await;
    let now = start_time(&created);
    harness.drive_active(&created.id, now).await;
    let at = now + Duration::from_secs(1);
    super::lifecycle_queue_tests::begin_queue_phase(&harness, &created.id, at).await;
    let current = harness.stored(&created.id).await;
    let mut pinned = current.clone();
    if let Some(SandboxLifecycleOperation::Revive { phase: p, .. }) =
        &mut pinned.lifecycle.operation
    {
        *p = phase;
    }
    if matches!(
        phase,
        SandboxRevivePhase::CheckpointVerification | SandboxRevivePhase::RuntimeProvisioning
    ) {
        pinned.runtime = None;
    }
    let mut restore = pin(&pinned, at);
    if completed {
        restore.receipt = Some(completion(at));
    }
    pinned.lifecycle.home_restore = Some(Box::new(restore));
    pinned.work.attempts = layer2_core::sandbox::work_retry::WORK_RETRY_BUDGET + 1;
    pinned.revision += 1;
    pinned.validate().unwrap();
    harness
        .store
        .save_sandbox(current.revision, pinned.clone())
        .await
        .unwrap();
    let requests = harness.backend.requests().await;
    let actions = harness.actions.snapshot();
    let mut probe_at = at;
    for _ in 0..2 {
        let worker = super::funding_tests::worker_with_funding(
            &harness,
            Arc::new(FundingRecorder(harness.actions.clone())),
        );
        assert!(worker.run_one(&created.id, probe_at).await.unwrap());
        let waited = harness.stored(&created.id).await;
        assert_eq!(waited.lifecycle, pinned.lifecycle, "{phase:?}");
        assert_eq!(waited.runtime, pinned.runtime);
        assert_eq!(waited.credential, pinned.credential);
        assert_eq!(waited.status, SandboxStatus::Reviving);
        assert!(waited.work.lease.is_none());
        assert!(
            seconds_from_rfc3339(&waited.work.ready_at).unwrap()
                > seconds_from_rfc3339(&rfc3339_at(probe_at)).unwrap()
        );
        assert_eq!(harness.backend.requests().await, requests);
        assert_eq!(harness.actions.snapshot(), actions);
        let events = harness
            .store
            .sandbox_events(&created.id, 0, 100)
            .await
            .unwrap();
        assert!(events.iter().any(|event| matches!(&event.kind,
            SandboxEventKind::WorkStepFailed { failure, .. }
                if failure.class == "home_restore_wait_for_compatible_worker")));
        probe_at = SystemTime::UNIX_EPOCH
            + Duration::from_secs(seconds_from_rfc3339(&waited.work.ready_at).unwrap());
    }
}

#[tokio::test]
async fn ordinary_revive_without_a_pin_continues() {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let created = harness.create().await;
    let now = start_time(&created);
    harness.drive_active(&created.id, now).await;
    let at = now + Duration::from_secs(1);
    super::lifecycle_queue_tests::begin_queue_phase(&harness, &created.id, at).await;
    let worker = super::funding_tests::worker_with_funding(
        &harness,
        Arc::new(FundingRecorder(harness.actions.clone())),
    );
    assert!(worker.run_one(&created.id, at).await.unwrap());
    assert!(worker.run_one(&created.id, at).await.unwrap());
    assert!(harness
        .actions
        .snapshot()
        .iter()
        .any(|action| action == "funding-coverage"));
    assert_eq!(
        harness.stored(&created.id).await.status,
        SandboxStatus::Active
    );
}

fn completion(now: SystemTime) -> layer2_core::sandbox::SandboxHomeRestoreReceipt {
    serde_json::from_value(serde_json::json!({
        "inventory_sha256": "b".repeat(64), "restored_to": rfc3339_at(now),
        "completed_at": rfc3339_at(now), "home_identity": "restored-home-example",
        "source_generation": 1, "covered_agents": [], "uncovered_agents": [],
        "missing_workspaces": [], "missing_worktrees": [],
        "removed_agent_homes": [], "removed_worktrees": []
    }))
    .unwrap()
}

#[tokio::test]
async fn receipt_waits_for_its_own_operation_but_does_not_block_a_later_ordinary_revive() {
    for original_operation in [true, false] {
        let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
        let created = harness.create().await;
        let now = start_time(&created);
        harness.drive_active(&created.id, now).await;
        let at = now + Duration::from_secs(1);
        super::lifecycle_queue_tests::begin_queue_phase(&harness, &created.id, at).await;
        let current = harness.stored(&created.id).await;
        let mut next = current.clone();
        let mut restore = pin(&next, at);
        restore.receipt = Some(completion(at));
        if !original_operation {
            restore.operation_id = "sandbox-lifecycle-v1:older:1".into();
        }
        next.lifecycle.home_restore = Some(Box::new(restore));
        next.revision += 1;
        harness
            .store
            .save_sandbox(current.revision, next.clone())
            .await
            .unwrap();
        harness.run(&created.id, at).await;
        let stored = harness.stored(&created.id).await;
        if original_operation {
            assert_eq!(stored.lifecycle, next.lifecycle);
            assert_eq!(stored.status, SandboxStatus::Reviving);
        } else {
            harness.run(&created.id, at).await;
            assert_eq!(
                harness.stored(&created.id).await.status,
                SandboxStatus::Active
            );
            assert_eq!(
                harness.stored(&created.id).await.lifecycle.home_restore,
                next.lifecycle.home_restore
            );
        }
    }
}
