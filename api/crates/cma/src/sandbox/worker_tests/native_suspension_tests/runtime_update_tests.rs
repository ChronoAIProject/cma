use super::*;
use layer2_core::sandbox::store::{
    SandboxAdmission, SandboxAdmissionName, SandboxAdmissionResult, SandboxAdmissionStore,
    SandboxImageRotationRequest, SandboxMaintenanceStore,
};

#[path = "runtime_update_reopen_refusal_tests.rs"]
mod reopen_refusal;
pub(crate) use reopen_refusal::exercise_runtime_update_reopen_http;

async fn active() -> (Fixture, Sandbox, SystemTime) {
    active_with_image_origin(false).await
}

async fn active_with_image_origin(managed_desktop: bool) -> (Fixture, Sandbox, SystemTime) {
    let mut f = Fixture::native().await;
    let created = if managed_desktop {
        let seed = Harness::new(vec![], 0, Duration::from_secs(60)).await;
        let mut created = seed.create().await;
        created.origin = layer2_core::sandbox::SandboxOrigin::Lifecycle {
            operation_id: "desktop-fixture".into(),
        };
        created.profile.content.runtime_image = Some(format!(
            "registry.example/runtime@sha256:{}",
            "a".repeat(64)
        ));
        created.profile.digest =
            layer2_core::profiles::content_digest(&created.profile.content).unwrap();
        created.lifecycle.runtime_image_origin =
            Some(layer2_core::sandbox::SandboxRuntimeImageOrigin::ManagedDesktop);
        let admission = SandboxAdmission::new(
            created.clone(),
            SandboxAdmissionName::Allocate(created.name),
            10,
            created.created_at,
        )
        .unwrap();
        let SandboxAdmissionResult::Created(created) =
            f.harness.store.admit_sandbox(admission).await.unwrap()
        else {
            panic!("expected a newly admitted Desktop Sandbox");
        };
        created
    } else {
        f.harness.create().await
    };
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
async fn retained_transition_worker_never_suspends_a_restarted_predecessor() {
    use layer3_adapter::hosted::HostedBackend;

    let (f, active, mut now) = active().await;
    let paused = f.pause(&active, &mut now).await;
    let binding = paused.lifecycle.suspended_runtime.as_deref().unwrap();
    let request =
        crate::sandbox::runtime_attachment::retained_runtime_request(&paused, binding, "test")
            .unwrap();
    let predecessor = f
        .adapter(&runtime_image())
        .retained_startup_policy()
        .unwrap()
        .inspect_retained_startup_policy(&request)
        .await
        .unwrap();
    let mut admitted = paused.clone();
    let receipt = admitted
        .begin_retained_runtime_transition(
            layer2_core::sandbox::RetainedRuntimeTransitionRequest {
                sandbox_id: paused.id.clone(),
                owner: paused.owner.clone(),
                expected_revision: paused.revision,
                grant_id: "retained-restarted-predecessor".into(),
                policy_revision: 1,
                target_image: predecessor.image.clone(),
                target_config_digest: format!("sha256:{}", "c".repeat(64)),
                journal_recovery: true,
                predecessor,
            },
            &[],
            rfc3339_at(now),
        )
        .unwrap();
    f.harness
        .store
        .save_sandbox(paused.revision, admitted)
        .await
        .unwrap();

    let previous = f.backend.request().await;
    f.backend
        .resume(&request.handle, &previous, &previous)
        .await
        .unwrap();
    let before = f.backend.calls.snapshot().len();
    now += Duration::from_secs(6);
    f.step(&paused.id, now, &runtime_image()).await;

    let calls = f.backend.calls.snapshot();
    assert!(calls[before..]
        .iter()
        .any(|call| call == "inspect-retained-predecessor"));
    assert!(!calls[before..]
        .iter()
        .any(|call| matches!(call.as_str(), "suspend" | "upgrade" | "resume")));
    assert_eq!(f.backend.snapshot().await.request, previous);
    let pending = f.harness.stored(&paused.id).await;
    assert_eq!(
        pending.admitted_retained_runtime_transition(),
        Some(&receipt)
    );
    assert!(pending.lifecycle.runtime_provision.is_none());
    assert_eq!(pending.lifecycle.home, paused.lifecycle.home);
    assert_eq!(
        pending.lifecycle.suspended_runtime,
        paused.lifecycle.suspended_runtime
    );
}

#[tokio::test]
async fn runtime_update_worker_reopens_original_conversation_on_same_pvc_after_lost_ack() {
    exercise_retained_update(false).await;
}

#[tokio::test]
async fn managed_desktop_update_reopens_original_conversation_on_same_pvc_after_lost_ack() {
    exercise_retained_update(true).await;
}

async fn exercise_retained_update(managed_desktop: bool) {
    let (f, active, mut now) = active_with_image_origin(managed_desktop).await;
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
    assert_eq!(updated.profile, active.profile);
    assert_eq!(
        updated.lifecycle.runtime_image_origin,
        active.lifecycle.runtime_image_origin
    );
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
    assert_eq!(failed.work.step, SandboxWorkStep::None);
    assert!(failed.failure.is_none());
    let events = f
        .harness
        .store
        .sandbox_events(&failed.id, 0, 200)
        .await
        .unwrap();
    assert_eq!(
        events.last().unwrap().kind,
        SandboxEventKind::WorkStepFailed {
            step: SandboxWorkStep::RunLifecycleOperation,
            failure: failed
                .runtime_update_request()
                .unwrap()
                .failure
                .clone()
                .unwrap(),
        }
    );
    assert!(!events
        .iter()
        .any(|event| matches!(event.kind, SandboxEventKind::Failed { .. })));
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

#[tokio::test]
async fn rotation_recovery_parks_exhausted_credential_delivery_with_flag_enabled() {
    use layer2_core::sandbox::work_retry::WORK_RETRY_BUDGET;
    const TARGET: &str = "registry.example/runtime@sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    for enabled in [false, true] {
        let (mut fixture, active, mut now) = active().await;
        fixture.harness.store = Arc::new(
            (*fixture.harness.store)
                .clone()
                .with_runtime_rotation_recovery(enabled),
        );
        update(&fixture, &active, TARGET, now).await;
        fixture
            .backend
            .fail_credential_transiently
            .store(true, Ordering::SeqCst);
        let mut before = None;
        for _ in 0..16 {
            now += Duration::from_secs(6);
            let current = fixture.harness.stored(&active.id).await;
            if matches!(
                current.lifecycle.operation,
                Some(SandboxLifecycleOperation::ImageRotation {
                    phase: layer2_core::sandbox::SandboxImageRotationPhase::CredentialDelivery,
                    ..
                })
            ) {
                before = Some(current);
                break;
            }
            fixture.step(&active.id, now, TARGET).await;
        }
        let before = before.expect("rotation must reach credential delivery");
        for attempt in 0..=WORK_RETRY_BUDGET {
            fixture.step(&active.id, now, TARGET).await;
            let row = fixture.harness.stored(&active.id).await;
            assert_eq!(row.lifecycle.operation, before.lifecycle.operation);
            assert_eq!(row.lifecycle.home, before.lifecycle.home);
            assert!(row.work.lease.is_none());
            if enabled && attempt == WORK_RETRY_BUDGET {
                assert_eq!(row.work.step, SandboxWorkStep::None);
                assert!(row.runtime_update_request().unwrap().failure.is_some());
            } else {
                assert_eq!(row.work.step, SandboxWorkStep::RunLifecycleOperation);
                assert!(row.runtime_update_request().unwrap().failure.is_none());
            }
            now = stamp_time(&row.work.ready_at);
        }
    }
}
