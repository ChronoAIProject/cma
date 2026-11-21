//! Concurrent receipt acceptance and application preserve the live hierarchy.
use super::*;

#[tokio::test]
async fn sandbox_controls_concurrent_acceptance_and_application_preserve_live_tree() {
    let f = Fixture::new().await;
    f.store.admit_agent(admission("keep-agent")).await.unwrap();
    let before = f.store.lock().clone();
    let revision = before.sandboxes[&sandbox(4).id].revision;
    let (left, right) = tokio::join!(
        f.admit("owner", SandboxControl::Pause, "retry", revision),
        f.admit("owner", SandboxControl::Pause, "retry", revision)
    );
    let record = left.unwrap();
    assert_eq!(record, right.unwrap());
    assert!(matches!(
        f.admit("owner", SandboxControl::Revive, "other", revision)
            .await,
        Err(SandboxControlServiceError::Store(SandboxControlError::Busy))
    ));
    let proof = f.proof("owner", &record).await;
    let (left, right) = tokio::join!(
        f.store.apply_sandbox_control(&record, proof.clone()),
        f.store.apply_sandbox_control(&record, proof)
    );
    let applied = left.unwrap();
    assert_eq!(right, Err(SandboxControlError::Conflict));
    assert!(matches!(
        applied.status,
        SandboxControlStatus::Applied {
            disposition: SandboxControlDisposition::Scheduled,
            ..
        }
    ));
    assert_eq!(
        f.admit("owner", SandboxControl::Pause, "retry", revision)
            .await
            .unwrap(),
        applied
    );
    assert!(matches!(
        f.admit("owner", SandboxControl::Pause, "retry", revision + 1)
            .await,
        Err(SandboxControlServiceError::Store(
            SandboxControlError::IdempotencyConflict
        ))
    ));
    let after = f.store.lock();
    let root = &after.sandboxes[&sandbox(4).id];
    assert_eq!(root.status, SandboxStatus::Pausing);
    assert_eq!(root.runtime, before.sandboxes[&root.id].runtime);
    assert_eq!(root.profile, before.sandboxes[&root.id].profile);
    assert_eq!(root.credential, before.sandboxes[&root.id].credential);
    assert_eq!(root.owner, before.sandboxes[&root.id].owner);
    assert_eq!(after.agents, before.agents);
    assert_eq!(after.agent_logs, before.agent_logs);
    assert_eq!(after.worktrees, before.worktrees);
    assert_eq!(after.workspaces, before.workspaces);
    assert_eq!(
        after.agent_creation_provenance,
        before.agent_creation_provenance
    );
    assert_eq!(
        after.sandbox_events[&root.id].len(),
        before.sandbox_events.get(&root.id).map_or(0, Vec::len) + 1
    );
    assert_eq!(after.sandbox_controls.len(), 1);
}

#[tokio::test]
async fn queued_control_keeps_original_result_deadline_when_applied_later() {
    use crate::sandbox::{SandboxLifecycleResult, SandboxLifecycleResultState};
    let f = Fixture::new().await;
    let revision = f.store.lock().sandboxes[&sandbox(4).id].revision;
    let mut record = f
        .admit("owner", SandboxControl::Pause, "bounded", revision)
        .await
        .unwrap();
    let now = SystemTime::now();
    record.admitted_at = crate::rfc3339_at(now - Duration::from_secs(100));
    record.queue_deadline = crate::rfc3339_at(now + Duration::from_secs(200));
    record.validate().unwrap();
    let original = SandboxLifecycleResult::queued_control(&record, now).unwrap();
    assert_eq!(
        original.state,
        SandboxLifecycleResultState::RecoverablePending
    );
    let replica: SandboxControlRecord =
        serde_json::from_str(&serde_json::to_string(&record).unwrap()).unwrap();
    assert_eq!(
        SandboxLifecycleResult::queued_control(&replica, now),
        Some(original.clone())
    );
    let parent = f.store.get_sandbox(&sandbox(4).id).await.unwrap().unwrap();
    let SandboxControlPlan::Transition {
        sandbox: applied, ..
    } = plan_sandbox_control(&record, &parent, &crate::rfc3339_at(now), None).unwrap()
    else {
        panic!("expected pause")
    };
    let result = SandboxLifecycleResult::observe(&applied, None, now, true).unwrap();
    assert_eq!(result.started_at, original.started_at);
    assert_eq!(result.result_due_at, original.result_due_at);
    assert_eq!(
        result.state,
        SandboxLifecycleResultState::RecoverablePending
    );
    assert!(matches!(
        plan_sandbox_control(&record, &parent, &record.queue_deadline, None).unwrap(),
        SandboxControlPlan::Reject(SandboxControlFailure::QueueExpired)
    ));
    assert!(SandboxLifecycleResult::queued_control(
        &record
            .rejected(SandboxControlFailure::QueueExpired, &record.queue_deadline)
            .unwrap(),
        now
    )
    .is_none());
}
