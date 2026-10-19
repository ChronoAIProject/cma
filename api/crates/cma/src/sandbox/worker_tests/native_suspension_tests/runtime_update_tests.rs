use super::*;
use layer2_core::sandbox::store::{SandboxImageRotationRequest, SandboxMaintenanceStore};

async fn active() -> (Fixture, Sandbox, SystemTime) {
    let mut f = Fixture::native().await;
    let created = f.harness.create().await;
    let mut now = start_time(&created);
    let value = f
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
        &value,
        "test",
    )
    .await
    .unwrap();
    (f, value, now)
}
async fn update(f: &Fixture, sandbox: &Sandbox, target: &str, now: SystemTime) {
    let mut requested = sandbox.clone();
    requested
        .request_runtime_update(target.into(), false, rfc3339_at(now))
        .unwrap();
    f.harness
        .store
        .save_sandbox(sandbox.revision, requested)
        .await
        .unwrap();
    assert_eq!(
        f.harness
            .store
            .schedule_image_rotation(SandboxImageRotationRequest {
                sandbox_id: sandbox.id.clone(),
                requested_at: rfc3339_at(now),
                expected_runtime_version: target.into()
            })
            .await
            .unwrap(),
        layer2_core::sandbox::store::SandboxMaintenanceOutcome::Scheduled
    );
}

#[tokio::test]
async fn runtime_update_worker_reopens_original_conversation_on_same_pvc_after_lost_ack() {
    let (f, active, mut now) = active().await;
    let agent = lifecycle_recovery_tests::prepare_agent(&f.harness, &active, now, 1).await;
    let original = f.backend.snapshot().await;
    let target = "registry.example/runtime@sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    update(&f, &active, target, now).await;
    f.backend.lose_suspend.store(true, Ordering::SeqCst);
    f.backend.lose_resume.store(true, Ordering::SeqCst);
    let updated = f
        .drive(&active.id, &mut now, SandboxStatus::Active, target)
        .await;
    assert_eq!(updated.id, active.id);
    assert_eq!(updated.lifecycle.home, active.lifecycle.home);
    assert_eq!(
        updated.runtime.as_ref().unwrap().runtime_id,
        active.runtime.as_ref().unwrap().runtime_id
    );
    assert_eq!(updated.runtime.as_ref().unwrap().runtime_version, target);
    assert!(
        updated.runtime.as_ref().unwrap().provision_generation
            > active.runtime.as_ref().unwrap().provision_generation
    );
    assert_ne!(
        updated.runtime.as_ref().unwrap().boot_id,
        active.runtime.as_ref().unwrap().boot_id
    );
    let restored = f.harness.store.get_agent(&agent.id).await.unwrap().unwrap();
    assert_eq!(
        restored.conversation.conversation_id,
        agent.conversation.conversation_id
    );
    assert_eq!(restored.worktree_id, agent.worktree_id);
    assert_eq!(restored.status, AgentStatus::Ready);
    assert_eq!(f.backend.snapshot().await.homes, original.homes);
    assert_eq!(f.backend.snapshot().await.workspaces, original.workspaces);
    assert_eq!(f.backend.snapshot().await.worktrees, original.worktrees);
    assert!(f.backend.calls.snapshot().contains(&"upgrade".into()));
    assert!(!f
        .backend
        .calls
        .snapshot()
        .contains(&"delete-retained".into()));
    let paused = f.pause(&updated, &mut now).await;
    assert_eq!(paused.lifecycle.home, active.lifecycle.home);
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
    let resumed = f
        .drive(&paused.id, &mut now, SandboxStatus::Active, target)
        .await;
    assert_eq!(resumed.runtime.as_ref().unwrap().runtime_version, target);
    assert_eq!(resumed.lifecycle.home, active.lifecycle.home);
}

#[tokio::test]
async fn runtime_update_worker_retains_paused_intent_and_allows_exact_failed_retry() {
    let (f, active, mut now) = active().await;
    let paused = f.pause(&active, &mut now).await;
    let target="registry.example/runtime@sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    update(&f, &paused, target, now).await;
    f.backend.reject_credential.store(true, Ordering::SeqCst);
    let mut failed = None;
    for _ in 0..12 {
        now += Duration::from_secs(6);
        f.step(&paused.id, now, target).await;
        let current = f.harness.stored(&paused.id).await;
        if current
            .runtime_update_request()
            .is_some_and(|request| request.failure.is_some())
        {
            failed = Some(current);
            break;
        }
    }
    let mut failed = failed.expect("terminal failure must park precise update phase");
    let previous = failed.clone();
    assert_eq!(failed.status, SandboxStatus::Rotating);
    assert_eq!(failed.lifecycle.home, active.lifecycle.home);
    f.backend.reject_credential.store(false, Ordering::SeqCst);
    failed
        .request_runtime_update(target.into(), false, rfc3339_at(now))
        .unwrap();
    f.harness
        .store
        .save_sandbox(previous.revision, failed)
        .await
        .unwrap();
    let updated = f
        .drive(&paused.id, &mut now, SandboxStatus::Paused, target)
        .await;
    assert_eq!(
        updated.retained_runtime_binding().unwrap().runtime_version,
        target
    );
    assert_eq!(updated.lifecycle.home, active.lifecycle.home);
    assert!(updated
        .runtime_update_request()
        .unwrap()
        .completed_at
        .is_some());
}

#[tokio::test]
async fn runtime_update_worker_resume_uses_admitted_target_on_original_home() {
    let (f, active, mut now) = active().await;
    let agent = lifecycle_recovery_tests::prepare_agent(&f.harness, &active, now, 1).await;
    let paused = f.pause(&active, &mut now).await;
    let target = "registry.example/runtime@sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    let mut admitted = paused.clone();
    admitted
        .begin_revive_with_managed_runtime(
            SandboxReviveCause::ExplicitRequest,
            Some(target),
            rfc3339_at(now),
        )
        .unwrap();
    f.harness
        .store
        .save_sandbox(paused.revision, admitted)
        .await
        .unwrap();
    // A newer deployment cannot replace the exact target already admitted.
    let updated = f.drive(&paused.id, &mut now, SandboxStatus::Active, "registry.example/runtime@sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc").await;
    assert_eq!(updated.runtime.as_ref().unwrap().runtime_version, target);
    assert_eq!(
        updated.runtime.as_ref().unwrap().runtime_id,
        active.runtime.unwrap().runtime_id
    );
    assert_eq!(updated.lifecycle.home, active.lifecycle.home);
    assert!(updated
        .runtime_update_request()
        .unwrap()
        .completed_at
        .is_some());
    assert_eq!(
        f.harness
            .store
            .get_agent(&agent.id)
            .await
            .unwrap()
            .unwrap()
            .conversation
            .conversation_id,
        agent.conversation.conversation_id
    );
    assert!(f.backend.calls.snapshot().contains(&"upgrade".into()));
}
