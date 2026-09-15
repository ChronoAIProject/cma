use super::*;
use crate::sandbox::channel_credentials::{ChannelGuardedCredentials, SandboxCredentialProvenance};
use layer2_core::sandbox::{SandboxLifecycleOperation, SandboxRevivePhase};

struct ChannelOrigin;
#[async_trait::async_trait]
impl SandboxCredentialProvenance for ChannelOrigin {
    async fn require_ordinary(&self, _: &SandboxId) -> Result<(), CredentialError> {
        Err(CredentialError::Unavailable(
            "original Channel human authority is required".into(),
        ))
    }
}

fn guarded_worker(harness: &Harness) -> Arc<SandboxWorker> {
    harness.worker_with(
        "channel-guard",
        ChannelGuardedCredentials::new(harness.credentials.clone(), Arc::new(ChannelOrigin)),
        Duration::from_secs(60),
    )
}

async fn require_held(harness: &Harness, before: &Sandbox, now: SystemTime) {
    let actions = harness.actions.snapshot();
    let requests = harness.backend.requests().await.len();
    let worker = guarded_worker(harness);
    assert!(worker.run_one(&before.id, now).await.unwrap());
    let held = harness.stored(&before.id).await;
    assert_eq!(held.status, before.status);
    assert_eq!(held.runtime, before.runtime);
    assert_eq!(held.credential, before.credential);
    assert_eq!(held.lifecycle.operation, before.lifecycle.operation);
    assert_eq!(
        held.lifecycle.runtime_provision,
        before.lifecycle.runtime_provision
    );
    assert_eq!(held.lifecycle.checkpoints, before.lifecycle.checkpoints);
    assert!(held.work.lease.is_none());
    assert_eq!(held.work.step, SandboxWorkStep::RunLifecycleOperation);
    assert!(stamp_time(&held.work.ready_at) > now);
    assert_eq!(harness.actions.snapshot(), actions);
    assert_eq!(harness.backend.requests().await.len(), requests);
    // Reading active custody is still allowed; only restore effects are held.
    let guarded =
        ChannelGuardedCredentials::new(harness.credentials.clone(), Arc::new(ChannelOrigin));
    assert!(guarded
        .material(
            &held.id,
            held.credential.current.as_ref().unwrap().generation
        )
        .await
        .unwrap()
        .is_some());
}

async fn scheduled_revive(harness: &Harness) -> (Sandbox, SystemTime) {
    let created = harness.create().await;
    let now = start_time(&created);
    let active = harness.drive_active(&created.id, now).await;
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
    let paused = harness.stored(&active.id).await;
    assert_eq!(paused.status, SandboxStatus::Paused);
    let revive_at = pause_at + Duration::from_secs(1);
    harness
        .store
        .schedule_sandbox_lifecycle(SandboxLifecycleCommand::revive(
            paused.id.clone(),
            paused.owner.clone(),
            rfc3339_at(revive_at),
            SandboxReviveCause::ExplicitRequest,
        ))
        .await
        .unwrap();
    (harness.stored(&active.id).await, revive_at)
}

#[tokio::test]
async fn channel_revive_is_held_before_runtime_provisioning() {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let (reviving, now) = scheduled_revive(&harness).await;
    assert!(reviving.runtime.is_none());
    require_held(&harness, &reviving, now).await;
    assert!(harness
        .adapter
        .runtime_inventory()
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn channel_revive_holds_retained_key_delivery_without_service_floor() {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let (reviving, now) = scheduled_revive(&harness).await;
    for _ in 0..3 {
        harness.run(&reviving.id, now).await;
    }
    let delivery = harness.stored(&reviving.id).await;
    assert!(matches!(
        delivery.lifecycle.operation,
        Some(SandboxLifecycleOperation::Revive {
            phase: SandboxRevivePhase::CredentialDelivery,
            ..
        })
    ));
    assert!(!delivery.needs_service_access_credential());
    assert!(delivery.runtime.is_some());
    require_held(&harness, &delivery, now).await;
}

#[tokio::test]
async fn channel_image_rotation_is_held_before_checkpoint_or_runtime_effects() {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let created = harness.create().await;
    let now = start_time(&created);
    let active = harness.drive_active(&created.id, now).await;
    let identity = SandboxRotationIdentity::capture(&active, &[]).unwrap();
    let mut rotating = active.clone();
    let now = now + Duration::from_secs(1);
    rotating
        .begin_image_rotation("runtime-v3".into(), identity, rfc3339_at(now))
        .unwrap();
    harness
        .store
        .save_sandbox(active.revision, rotating.clone())
        .await
        .unwrap();
    require_held(&harness, &rotating, now).await;
    assert!(harness
        .backend
        .snapshot(&RuntimeHandle(active.runtime.unwrap().runtime_id))
        .await
        .is_ok());
}

#[tokio::test]
async fn channel_retirement_without_dedicated_authority_holds_before_stopping_any_agent() {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(3600)).await;
    let created = harness.create().await;
    let now = start_time(&created);
    let active = harness.drive_active(&created.id, now).await;
    harness
        .service
        .delete(&caller("owner", [Capability::DeleteOwnSandbox]), &active.id)
        .await
        .unwrap();
    let before = harness.stored(&active.id).await;
    let actions = harness.actions.snapshot();
    let requests = harness.backend.requests().await.len();
    assert!(guarded_worker(&harness)
        .run_one(&active.id, now)
        .await
        .unwrap());
    let held = harness.stored(&active.id).await;
    assert_eq!(held.status, before.status);
    assert_eq!(held.runtime, before.runtime);
    assert_eq!(held.credential, before.credential);
    assert_eq!(held.work.step, SandboxWorkStep::StopAgents);
    assert!(held.work.lease.is_none());
    assert_eq!(harness.actions.snapshot(), actions);
    assert_eq!(harness.backend.requests().await.len(), requests);
}
