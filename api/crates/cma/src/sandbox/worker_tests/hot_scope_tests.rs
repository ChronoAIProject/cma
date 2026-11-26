use super::*;
use layer2_core::sandbox::store::{SandboxTransaction, SandboxTransactionStore};

async fn hot_scope(harness: &Harness, active: &Sandbox, now: SystemTime) -> u64 {
    let mut granted = active.clone();
    let mut services = active.credential.effective_reaches();
    services.push(ServiceId::new("api-github"));
    services.sort();
    let scope = granted
        .revise_live_service_access(&active.owner, &services, rfc3339_at(now))
        .unwrap();
    let floor = scope.credential_generation_floor;
    harness
        .store
        .commit(SandboxTransaction::SaveSandboxAndAppend {
            command_revival: None,
            released_runtime: None,
            expected_revision: active.revision,
            sandbox: Box::new(granted),
            event_at: rfc3339_at(now),
            event: SandboxEventKind::ServiceAccessRevised {
                scope: Box::new(scope),
            },
        })
        .await
        .unwrap();
    floor
}

#[tokio::test]
async fn one_due_sweep_delivers_hot_scope_and_revokes_old_key_without_lifecycle_or_runtime_change()
{
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let created = harness.create().await;
    let now = start_time(&created);
    let active = harness.drive_active(&created.id, now).await;
    let floor = hot_scope(&harness, &active, now).await;
    assert_eq!(harness.worker.run_due(now).await.unwrap(), 1);
    let after = harness.stored(&created.id).await;
    assert_eq!(after.status, SandboxStatus::Active, "{:?}", after.failure);
    assert_eq!(after.runtime, active.runtime);
    assert_eq!(after.lifecycle, active.lifecycle);
    assert!(after.credential.current.as_ref().unwrap().generation > floor);
    assert!(after.credential.delivered && after.credential.revoke_debt.is_empty());
    assert_eq!(
        harness.actions.snapshot(),
        ["mint:1", "mint:3", "revoke:provider-key-1"]
    );
    assert_eq!(
        harness
            .backend
            .requests()
            .await
            .iter()
            .filter(|(_, request)| matches!(request, RequestFrame::Credential(_)))
            .count(),
        2
    );
    assert_eq!(harness.worker.run_due(now).await.unwrap(), 0);
}

#[tokio::test]
async fn due_sweep_stops_on_retry_and_never_reports_active_before_old_key_revocation() {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let created = harness.create().await;
    let now = start_time(&created);
    let active = harness.drive_active(&created.id, now).await;
    hot_scope(&harness, &active, now).await;
    harness.credentials.fail_revokes(1);
    assert_eq!(harness.worker.run_due(now).await.unwrap(), 1);
    let blocked = harness.stored(&created.id).await;
    assert_eq!(
        blocked.status,
        SandboxStatus::CredentialRotating,
        "{:?}",
        blocked.failure
    );
    assert_eq!(blocked.work.step, SandboxWorkStep::RevokeRotatedCredential);
    assert!(!blocked.credential.revoke_debt.is_empty());
    assert!(blocked.work.attempts > 0);
    assert_eq!(harness.worker.run_due(now).await.unwrap(), 0);
    assert_eq!(harness.stored(&created.id).await, blocked);
    assert_eq!(
        harness
            .worker
            .run_due(stamp_time(&blocked.work.ready_at))
            .await
            .unwrap(),
        1
    );
    let recovered = harness.stored(&created.id).await;
    assert_eq!(recovered.status, SandboxStatus::Active);
    assert!(recovered.credential.revoke_debt.is_empty());
    assert_eq!(recovered.runtime, active.runtime);
}
