use super::*;
use crate::sandbox::{Sandbox, SandboxStatus, SandboxWorkStep};

#[tokio::test]
async fn real_sandbox_admission_holds_old_workers_and_prevents_abandoned_reset() {
    let (record, authority) = reserved();
    let request = ChannelSandboxReservation::prepare(authority.clone(), record.clone()).unwrap();
    let sandbox = request.sandbox();
    assert_eq!(sandbox.id, record.placement.target().sandbox_id);
    assert_eq!(sandbox.status, SandboxStatus::Provisioning);
    assert_eq!(sandbox.work.step, SandboxWorkStep::None);
    assert!(!sandbox.work.is_due("2999-01-01T00:00:00Z"));
    assert!(!sandbox.requires_runtime_loss_recovery());
    assert!(sandbox.runtime.is_none());
    assert!(sandbox.credential.current.is_none());
    assert_eq!(sandbox.credential.key_generation, 0);
    assert_eq!(request.allocation().state, AllocationState::NeedsConnection);
    assert!(request.allocation().effect_started);
    assert!(request.allocation().receipt.is_none());
    assert!(request.allocation().lease.is_none());
    let old_shape: Sandbox =
        serde_json::from_value(serde_json::to_value(sandbox).unwrap()).unwrap();
    assert_eq!(&old_shape, sandbox);
    request
        .sidecar()
        .require_held(request.allocation(), &old_shape)
        .unwrap();
    let replay =
        ChannelSandboxReservation::prepare(authority, request.allocation().clone()).unwrap();
    assert!(replay.requires_existing());
    assert_eq!(replay.sandbox(), sandbox);
    assert_eq!(replay.sidecar(), request.sidecar());
    assert_eq!(replay.sidecar().source, record.first_source);
}

#[tokio::test(start_paused = true)]
async fn held_admission_cannot_reuse_expired_authority_or_claimed_allocation() {
    let (record, authority) = reserved();
    let request = ChannelSandboxReservation::prepare(authority.clone(), record.clone()).unwrap();
    let claimed =
        AllocationMutation::claim(record, authority.clone(), "worker", 1001, 1000).unwrap();
    assert!(matches!(
        ChannelSandboxReservation::prepare(authority, claimed.record().clone()),
        Err(AllocationError::InvalidTransition)
    ));
    tokio::time::advance(std::time::Duration::from_secs(16)).await;
    assert_eq!(
        request.require_current(),
        Err(AllocationError::AuthorityUnavailable)
    );
}

#[tokio::test]
async fn held_creation_rejects_profile_source_owner_and_runtime_substitution() {
    let (record, authority) = reserved();
    let request = ChannelSandboxReservation::prepare(authority.clone(), record.clone()).unwrap();
    let mut sandbox = request.sandbox().clone();
    sandbox.work.step = SandboxWorkStep::ProvisionRuntime;
    assert_eq!(
        request.sidecar().require_held(&record, &sandbox),
        Err(AllocationError::Corrupt)
    );
    let mut wrong = record;
    wrong.owner = crate::resource_access::ResourceOwner::Organization {
        subject: sandbox.owner.clone(),
    };
    assert!(matches!(
        ChannelSandboxReservation::prepare(authority, wrong),
        Err(AllocationError::IdentityMismatch)
    ));
    let mut sidecar = request.sidecar().clone();
    sidecar.initial_fingerprint = "f".repeat(64);
    assert_eq!(
        sidecar.require_held(request.allocation(), request.sandbox()),
        Err(AllocationError::Corrupt)
    );
}
