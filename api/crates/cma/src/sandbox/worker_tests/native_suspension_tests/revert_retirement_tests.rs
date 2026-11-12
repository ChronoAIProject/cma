use super::*;

#[tokio::test]
async fn retirement_after_lost_suspend_response_deletes_the_stopped_crd_and_settles_agents() {
    let mut fixture = Fixture::new().await;
    let created = fixture.harness.create().await;
    let mut now = start_time(&created);
    let active = fixture
        .drive(
            &created.id,
            &mut now,
            SandboxStatus::Active,
            &runtime_image(),
        )
        .await;
    fixture.harness.adapter = fixture.adapter(&runtime_image());
    crate::sandbox::runtime_attachment::adopt_bound_runtime(
        fixture.harness.adapter.as_ref(),
        &active,
        "test",
    )
    .await
    .unwrap();
    let agent = lifecycle_recovery_tests::prepare_agent(&fixture.harness, &active, now, 1).await;
    fixture
        .harness
        .store
        .create_workspace(lifecycle_recovery_tests::workspace_for(&agent, now))
        .await
        .unwrap();
    fixture.backend.lose_suspend.store(true, Ordering::SeqCst);
    fixture
        .harness
        .store
        .schedule_sandbox_lifecycle(SandboxLifecycleCommand::pause(
            active.id.clone(),
            active.owner.clone(),
            rfc3339_at(now),
            None,
        ))
        .await
        .unwrap();
    for _ in 0..4 {
        if fixture.backend.calls.snapshot().contains(&"suspend".into()) {
            break;
        }
        now += Duration::from_secs(6);
        fixture.step(&active.id, now, &runtime_image()).await;
    }
    assert!(fixture.backend.calls.snapshot().contains(&"suspend".into()));
    let pending = fixture.harness.stored(&active.id).await;
    assert_eq!(pending.runtime, active.runtime);
    assert!(pending.lifecycle.suspended_runtime.is_none());
    fixture.backend.lose_delete.store(true, Ordering::SeqCst);
    let mut retiring = pending.clone();
    retiring.begin_retirement(rfc3339_at(now)).unwrap();
    fixture
        .harness
        .store
        .save_sandbox(pending.revision, retiring)
        .await
        .unwrap();
    let retired = fixture
        .drive(
            &active.id,
            &mut now,
            SandboxStatus::Retired,
            &runtime_image(),
        )
        .await;
    assert!(retired.lifecycle.home.as_ref().unwrap().released);
    assert_eq!(
        fixture
            .harness
            .store
            .get_agent(&agent.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        AgentStatus::Stopped
    );
    assert!(fixture
        .backend
        .calls
        .snapshot()
        .contains(&"delete-retained".into()));
}

#[tokio::test]
async fn retirement_after_lost_resume_response_deletes_the_same_crd_without_adopting_a_new_process()
{
    let fixture = Fixture::new().await;
    let created = fixture.harness.create().await;
    let mut now = start_time(&created);
    let active = fixture
        .drive(
            &created.id,
            &mut now,
            SandboxStatus::Active,
            &runtime_image(),
        )
        .await;
    let paused = fixture.pause(&active, &mut now).await;
    fixture
        .harness
        .store
        .schedule_sandbox_lifecycle(SandboxLifecycleCommand::revive(
            paused.id.clone(),
            paused.owner.clone(),
            rfc3339_at(now),
            SandboxReviveCause::ExplicitRequest,
        ))
        .await
        .unwrap();
    fixture.backend.lose_resume.store(true, Ordering::SeqCst);
    for _ in 0..3 {
        now += Duration::from_secs(6);
        fixture.step(&paused.id, now, &runtime_image()).await;
    }
    let pending = fixture.harness.stored(&paused.id).await;
    assert!(matches!(
        pending.lifecycle.operation,
        Some(SandboxLifecycleOperation::Revive {
            phase: SandboxRevivePhase::RuntimeProvisioning,
            ..
        })
    ));
    assert!(fixture.backend.calls.snapshot().contains(&"resume".into()));
    let mut retiring = pending.clone();
    retiring.begin_retirement(rfc3339_at(now)).unwrap();
    fixture
        .harness
        .store
        .save_sandbox(pending.revision, retiring)
        .await
        .unwrap();
    let retired = fixture
        .drive(
            &paused.id,
            &mut now,
            SandboxStatus::Retired,
            &runtime_image(),
        )
        .await;
    assert!(retired.lifecycle.home.as_ref().unwrap().released);
    assert!(fixture
        .backend
        .calls
        .snapshot()
        .contains(&"delete-retained".into()));
}
