use super::*;
use layer2_core::sandbox::store::{
    runtime_migration_now_ms, RuntimeMigrationAdmissionBasis, RuntimeMigrationClearance,
    SandboxCustomRuntimeMigrationAdmission, SandboxMaintenanceStore,
};
use layer2_core::sandbox::RuntimeMigrationIdentity;

fn paused_admission(sandbox: &Sandbox, target: char) -> SandboxCustomRuntimeMigrationAdmission {
    let now = runtime_migration_now_ms().unwrap();
    let migration = request(sandbox, target);
    SandboxCustomRuntimeMigrationAdmission {
        basis: RuntimeMigrationAdmissionBasis {
            authority_id: format!("fixture-authority-{target}"),
            policy_id: "fixture-qualified-custom-policy".into(),
            request_key: migration.request_key.clone(),
            identity: RuntimeMigrationIdentity::capture(sandbox).unwrap(),
            authority_issued_at_ms: now,
            authority_expires_at_ms: now + 30_000,
            approved_target: migration.target_image.clone(),
            source_runtime: sandbox.retained_runtime_binding().cloned(),
            clearance: RuntimeMigrationClearance::Paused,
            admitted_at: layer2_core::now_rfc3339(),
            previous_request_keys: vec![],
            earlier_admissions: vec![],
        },
        migration,
        expected_current_revision: sandbox.revision,
        restart_failed: false,
    }
}

async fn revive(
    f: &Fixture,
    sandbox: &Sandbox,
    now: &mut SystemTime,
    cause: SandboxReviveCause,
) -> Sandbox {
    *now = SystemTime::now();
    f.harness
        .store
        .schedule_sandbox_lifecycle(SandboxLifecycleCommand::revive(
            sandbox.id.clone(),
            sandbox.owner.clone(),
            rfc3339_at(*now),
            cause,
        ))
        .await
        .unwrap();
    // The deployment default is deliberately neither the original nor admitted pin.
    drive_current(f, &sandbox.id, now, SandboxStatus::Active).await
}

#[tokio::test]
async fn custom_runtime_supported_paused_migration_preserves_home_and_conversation_after_lost_ack()
{
    let (f, active, mut now) = active_current().await;
    let agent = lifecycle_recovery_tests::prepare_agent(&f.harness, &active, now, 1).await;
    let original = f.backend.snapshot().await;
    f.backend.lose_suspend.store(true, Ordering::SeqCst);
    let paused = pause_current(&f, &active, &mut now).await;
    let admission = paused_admission(&paused, 'b');
    let admitted = f
        .harness
        .store
        .admit_custom_runtime_migration(admission.clone())
        .await
        .unwrap();
    assert_eq!(admitted.status, SandboxStatus::Rotating);
    assert_eq!(admitted.profile, active.profile);
    assert_eq!(admitted.lifecycle.home, active.lifecycle.home);
    assert_eq!(
        admitted.effective_custom_runtime_image(),
        Some(image('a').as_str())
    );
    assert_eq!(
        sandbox_spec(&admitted, "test").unwrap().runtime_image,
        Some(image('a'))
    );
    assert_eq!(
        f.harness
            .store
            .admit_custom_runtime_migration(admission)
            .await
            .unwrap(),
        admitted
    );
    f.backend.lose_resume.store(true, Ordering::SeqCst);
    let mut bound_before_publication = false;
    let mut published_before_pause = false;
    let mut steps = 0;
    let updated = loop {
        steps += 1;
        assert!(steps < 32, "migration did not settle");
        now = SystemTime::now();
        f.step(&active.id, now, &image('c')).await;
        let current = f.harness.stored(&active.id).await;
        if current.status == SandboxStatus::Paused {
            break current;
        }
        let migration = current.lifecycle.custom_runtime.as_ref().unwrap();
        if current.status == SandboxStatus::Pausing {
            // Publication restores the original pause intent as a new operation.
            assert!(matches!(
                current.lifecycle.operation,
                Some(layer2_core::sandbox::SandboxLifecycleOperation::Pause { .. })
            ));
            assert!(migration.completed_at.is_some());
            assert!(current.failure.is_none(), "{:?}", current.failure);
            assert_eq!(
                current.effective_custom_runtime_image(),
                Some(image('b').as_str())
            );
            assert_eq!(
                current.retained_runtime_binding().unwrap().runtime_version,
                image('b')
            );
            published_before_pause = true;
            continue;
        }
        assert_eq!(current.status, SandboxStatus::Rotating);
        assert!(migration.completed_at.is_none());
        if current
            .runtime
            .as_ref()
            .is_some_and(|binding| binding.runtime_version == image('b'))
        {
            bound_before_publication = true;
            assert_eq!(
                current.effective_custom_runtime_image(),
                Some(image('a').as_str())
            );
            assert_eq!(
                sandbox_spec(&current, "test").unwrap().runtime_image,
                Some(image('b'))
            );
        }
    };
    assert!(bound_before_publication);
    assert!(published_before_pause);
    assert_eq!(
        updated.effective_custom_runtime_image(),
        Some(image('b').as_str())
    );
    assert_eq!(updated.profile, active.profile);
    assert_eq!(updated.lifecycle.home, active.lifecycle.home);
    assert_eq!(
        updated.retained_runtime_binding().unwrap().runtime_version,
        image('b')
    );
    assert!(updated
        .lifecycle
        .custom_runtime
        .as_ref()
        .unwrap()
        .completed_at
        .is_some());
    let resumed = revive(&f, &updated, &mut now, SandboxReviveCause::ExplicitRequest).await;
    assert_eq!(
        resumed.runtime.as_ref().unwrap().runtime_version,
        image('b')
    );
    assert_eq!(
        resumed.runtime.as_ref().unwrap().runtime_id,
        active.runtime.as_ref().unwrap().runtime_id
    );
    assert_eq!(resumed.profile, active.profile);
    assert_eq!(resumed.lifecycle.home, active.lifecycle.home);
    let restored = f.harness.store.get_agent(&agent.id).await.unwrap().unwrap();
    assert_eq!(restored.status, AgentStatus::Ready);
    assert_eq!(
        restored.conversation.conversation_id,
        agent.conversation.conversation_id
    );
    assert_eq!(restored.worktree_id, agent.worktree_id);
    assert_eq!(f.backend.snapshot().await.homes, original.homes);
    assert_eq!(f.backend.snapshot().await.workspaces, original.workspaces);
    assert_eq!(f.backend.snapshot().await.worktrees, original.worktrees);
    let calls = f.backend.calls.snapshot();
    assert!(calls.contains(&"upgrade".into()));
    assert!(calls.contains(&"resume-replay".into()));
    assert!(!calls.contains(&"delete-retained".into()));
}

#[tokio::test]
async fn custom_runtime_second_migration_and_child_resume_use_effective_pin_with_changed_default() {
    let (f, active, mut now) = active_current().await;
    let agent = lifecycle_recovery_tests::prepare_agent(&f.harness, &active, now, 1).await;
    let paused = pause_current(&f, &active, &mut now).await;
    let first = paused_admission(&paused, 'b');
    f.harness
        .store
        .admit_custom_runtime_migration(first.clone())
        .await
        .unwrap();
    let first_complete = drive_current(&f, &active.id, &mut now, SandboxStatus::Paused).await;
    let second = paused_admission(&first_complete, 'd');
    assert_eq!(second.migration.expected_image, image('b'));
    assert_eq!(second.migration.origin, first.migration.origin);
    f.harness
        .store
        .admit_custom_runtime_migration(second)
        .await
        .unwrap();
    assert!(f
        .harness
        .store
        .admit_custom_runtime_migration(first)
        .await
        .is_err());
    let second_complete = drive_current(&f, &active.id, &mut now, SandboxStatus::Paused).await;
    assert_eq!(
        second_complete.effective_custom_runtime_image(),
        Some(image('d').as_str())
    );
    assert_eq!(second_complete.profile, active.profile);
    assert_eq!(second_complete.lifecycle.home, active.lifecycle.home);
    let resumed = revive(
        &f,
        &second_complete,
        &mut now,
        SandboxReviveCause::AgentMessage,
    )
    .await;
    assert_eq!(
        resumed.runtime.as_ref().unwrap().runtime_version,
        image('d')
    );
    assert_eq!(resumed.profile, active.profile);
    assert_eq!(resumed.lifecycle.home, active.lifecycle.home);
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
    let mut managed_request = resumed.clone();
    assert!(managed_request
        .request_runtime_update(image('c'), false, rfc3339_at(now))
        .is_err());
    assert_eq!(managed_request, resumed);
    let retained = crate::sandbox::runtime_attachment::retained_runtime_request(
        &resumed,
        resumed.runtime.as_ref().unwrap(),
        "test",
    )
    .unwrap();
    assert_eq!(retained.expected_image, Some(image('d')));
    assert_eq!(retained.spec.runtime_image, Some(image('d')));
}

fn observed_admission(
    f: &Fixture,
    sandbox: &Sandbox,
    target: char,
) -> SandboxCustomRuntimeMigrationAdmission {
    let mut admission = paused_admission(sandbox, target);
    admission.basis.clearance = RuntimeMigrationClearance::Observed {
        proof: Box::new(
            f.harness
                .store
                .runtime_activity_quiet_proof_fixture(
                    &sandbox.id,
                    runtime_migration_now_ms().unwrap(),
                )
                .unwrap(),
        ),
    };
    admission
}

#[tokio::test]
async fn custom_runtime_active_migration_failed_restart_keeps_exact_target_phase_and_conversation()
{
    let (f, active, mut now) = active_current().await;
    let agent = lifecycle_recovery_tests::prepare_agent(&f.harness, &active, now, 1).await;
    f.harness
        .store
        .create_workspace(lifecycle_recovery_tests::workspace_for(&agent, now))
        .await
        .unwrap();
    let original = f.backend.snapshot().await;
    let original_request = request(&active, 'b');
    let admission = observed_admission(&f, &active, 'b');
    f.harness
        .store
        .admit_custom_runtime_migration(admission)
        .await
        .unwrap();
    f.backend.reject_credential.store(true, Ordering::SeqCst);
    let mut failed = None;
    for _ in 0..16 {
        now = SystemTime::now();
        f.step(&active.id, now, &image('c')).await;
        let current = f.harness.stored(&active.id).await;
        if current
            .runtime_update_request()
            .is_some_and(|request| request.failure.is_some())
        {
            failed = Some(current);
            break;
        }
    }
    let failed = failed.expect("credential refusal parks the admitted target and phase");
    assert_eq!(
        failed.effective_custom_runtime_image(),
        Some(image('a').as_str())
    );
    assert_eq!(failed.profile, active.profile);
    assert_eq!(failed.lifecycle.home, active.lifecycle.home);
    let mut retry = observed_admission(&f, &failed, 'b');
    retry.migration = original_request;
    retry.restart_failed = true;
    retry.basis.authority_id = "fixture-restart-authority-b".into();
    let mut retarget = retry.clone();
    retarget.migration.target_image = image('d');
    retarget.basis.approved_target = image('d');
    assert!(f
        .harness
        .store
        .admit_custom_runtime_migration(retarget)
        .await
        .is_err());
    assert_eq!(f.harness.stored(&active.id).await, failed);
    f.backend.reject_credential.store(false, Ordering::SeqCst);
    let restarted = f
        .harness
        .store
        .admit_custom_runtime_migration(retry)
        .await
        .unwrap();
    assert_eq!(restarted.lifecycle.operation, failed.lifecycle.operation);
    assert_eq!(restarted.profile, active.profile);
    let restored = drive_current(&f, &active.id, &mut now, SandboxStatus::Active).await;
    assert_eq!(
        restored.effective_custom_runtime_image(),
        Some(image('b').as_str())
    );
    assert_eq!(
        restored.runtime.as_ref().unwrap().runtime_version,
        image('b')
    );
    assert_eq!(restored.lifecycle.home, active.lifecycle.home);
    assert_eq!(restored.profile, active.profile);
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
    assert_eq!(f.backend.snapshot().await.homes, original.homes);
    assert_eq!(f.backend.snapshot().await.workspaces, original.workspaces);
    assert_eq!(f.backend.snapshot().await.worktrees, original.worktrees);
}

// Admission uses authoritative wall time. Keep worker phases on that same clock
// instead of moving fixture timestamps ahead of the store's freshness checks.
async fn active_current() -> (Fixture, Sandbox, SystemTime) {
    let mut f = Fixture::native().await;
    f.retry_delay = Duration::ZERO;
    let mut created = f.harness.create().await;
    created.id = SandboxId(format!("{}_custom", created.id));
    created.name = "Custom runtime admission fixture".into();
    created.profile.content.runtime_image = Some(image('a'));
    created.profile.digest =
        layer2_core::profiles::content_digest(&created.profile.content).unwrap();
    f.harness
        .store
        .create_sandbox(created.clone())
        .await
        .unwrap();
    let mut now = SystemTime::now();
    let active = drive_current(&f, &created.id, &mut now, SandboxStatus::Active).await;
    f.harness.adapter = f.adapter(&image('c'));
    crate::sandbox::runtime_attachment::adopt_bound_runtime(
        f.harness.adapter.as_ref(),
        &active,
        "test",
    )
    .await
    .unwrap();
    assert_eq!(active.runtime.as_ref().unwrap().runtime_version, image('a'));
    (f, active, now)
}

async fn drive_current(
    f: &Fixture,
    id: &SandboxId,
    now: &mut SystemTime,
    status: SandboxStatus,
) -> Sandbox {
    for _ in 0..32 {
        let current = f.harness.stored(id).await;
        if current.status == status {
            return current;
        }
        assert_ne!(
            current.status,
            SandboxStatus::Failed,
            "{:?}",
            current.failure
        );
        *now = SystemTime::now();
        f.step(id, *now, &image('c')).await;
    }
    panic!(
        "custom lifecycle did not reach {status:?}: {:?}",
        f.harness.stored(id).await
    );
}

async fn pause_current(f: &Fixture, sandbox: &Sandbox, now: &mut SystemTime) -> Sandbox {
    *now = SystemTime::now();
    f.harness
        .store
        .schedule_sandbox_lifecycle(SandboxLifecycleCommand::pause(
            sandbox.id.clone(),
            sandbox.owner.clone(),
            rfc3339_at(*now),
            None,
        ))
        .await
        .unwrap();
    drive_current(f, &sandbox.id, now, SandboxStatus::Paused).await
}
