use super::*;

#[tokio::test]
async fn native_resume_binds_the_single_fully_validated_recovery_status() {
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
    for _ in 0..4 {
        let current = fixture.harness.stored(&paused.id).await;
        if current.lifecycle.runtime_provision.is_some() {
            break;
        }
        now += Duration::from_secs(6);
        fixture.step(&paused.id, now, &runtime_image()).await;
    }
    let reserved = fixture.harness.stored(&paused.id).await;
    assert!(reserved.runtime.is_none());
    assert!(reserved.lifecycle.runtime_provision.is_some());
    let before = fixture.harness.backend.requests().await.len();
    now += Duration::from_secs(6);
    fixture.step(&paused.id, now, &runtime_image()).await;
    let bound = fixture.harness.stored(&paused.id).await;
    assert!(matches!(
        bound.lifecycle.operation,
        Some(SandboxLifecycleOperation::Revive {
            phase: SandboxRevivePhase::CredentialDelivery,
            ..
        })
    ));
    let requests = fixture.harness.backend.requests().await;
    let statuses = requests[before..]
        .iter()
        .filter_map(|(_, request)| match request {
            RequestFrame::RuntimeStatus(request) => Some(request),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        statuses.len(),
        1,
        "one coherent recovery observation also supplies the bound boot identity"
    );
    let binding = bound.runtime.as_ref().unwrap();
    assert_eq!(
        statuses[0].generation,
        binding.provision_generation.unwrap()
    );
    assert_eq!(statuses[0].sandbox_id, bound.id.as_str());
    assert_ne!(binding.boot_id, active.runtime.as_ref().unwrap().boot_id);
    assert!(binding.boot_id.is_some());
    assert!(!fixture.backend.snapshot().await.credential_delivered);
    assert_eq!(bound.lifecycle.home, paused.lifecycle.home);
    let resumed = fixture
        .drive(&bound.id, &mut now, SandboxStatus::Active, &runtime_image())
        .await;
    assert_eq!(resumed.runtime.as_ref().unwrap().boot_id, binding.boot_id);
    assert!(resumed.credential.delivered);
    assert!(fixture.backend.snapshot().await.credential_delivered);
}
