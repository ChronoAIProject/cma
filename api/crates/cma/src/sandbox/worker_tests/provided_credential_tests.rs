use super::*;
use layer2_core::sandbox::binding::ProvidedCredential;
use layer2_core::sandbox::store::{SandboxAdmission, SandboxAdmissionName, SandboxAdmissionStore};
use layer2_core::sandbox::{SandboxCredentialAuthority, SandboxCredentialSource};

#[tokio::test]
async fn supplied_key_is_delivered_and_survives_pause_revival_without_scheduled_rotation() {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let mut created = harness.create().await;
    created.id = SandboxId::generate();
    created.name = "provided-key".into();
    let key = ProvidedCredential::new(format!("nyx_{}", uuid::Uuid::new_v4().simple())).unwrap();
    created.credential.source = SandboxCredentialSource::Provided {
        key_sha256: key.sha256(),
    };
    harness
        .store
        .admit_sandbox(
            SandboxAdmission::new(
                created.clone(),
                SandboxAdmissionName::Exact(created.name.clone()),
                5,
                created.created_at.clone(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let now = start_time(&created);
    harness.credentials.restore(CredentialMaterial {
        source: created.credential.source.clone(),
        github_route: None,
        sandbox_id: created.id.clone(),
        owner: created.owner.clone(),
        authority: SandboxCredentialAuthority {
            generation: 1,
            provider_key_id: format!("provided-{}", key.sha256()),
            provider_state_version: 1,
            issued_at: rfc3339_at(now),
            expires_at: String::new(),
            grant_fingerprint: "a".repeat(64),
        },
        value: SecretValue::new(key.expose().into()),
    });
    let active = harness.drive_active(&created.id, now).await;
    assert_eq!(active.work.step, SandboxWorkStep::None);
    assert!(harness.actions.snapshot().is_empty());
    assert!(harness.backend.requests().await.iter().any(|(_, request)|
        matches!(request, RequestFrame::Credential(frame) if frame.value.expose() == key.expose())));

    let pause_at = now + Duration::from_secs(1);
    harness
        .store
        .schedule_sandbox_lifecycle(SandboxLifecycleCommand::pause(
            active.id.clone(),
            active.owner.clone(),
            rfc3339_at(pause_at),
            None,
        ))
        .await
        .unwrap();
    for _ in 0..4 {
        harness.run(&active.id, pause_at).await;
    }
    assert_eq!(
        harness.stored(&active.id).await.status,
        SandboxStatus::Paused
    );
    let revive_at = pause_at + Duration::from_secs(1);
    harness
        .store
        .schedule_sandbox_lifecycle(SandboxLifecycleCommand::revive(
            active.id.clone(),
            active.owner.clone(),
            rfc3339_at(revive_at),
            SandboxReviveCause::ExplicitRequest,
        ))
        .await
        .unwrap();
    for _ in 0..8 {
        harness.run(&active.id, revive_at).await;
    }
    let revived = harness.stored(&active.id).await;
    assert_eq!(revived.status, SandboxStatus::Active);
    assert_eq!(revived.work.step, SandboxWorkStep::None);
    assert_eq!(revived.credential.current, active.credential.current);
    assert!(!harness
        .worker
        .run_one(&active.id, now + Duration::from_secs(31 * 86400))
        .await
        .unwrap());
    assert!(!harness
        .actions
        .snapshot()
        .iter()
        .any(|action| action.starts_with("mint:")));
}
