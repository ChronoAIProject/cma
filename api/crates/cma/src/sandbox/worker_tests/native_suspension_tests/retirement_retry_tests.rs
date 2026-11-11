use super::*;

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
