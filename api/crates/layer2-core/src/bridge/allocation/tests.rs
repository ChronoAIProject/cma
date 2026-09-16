use super::*;
mod channel_creation;
mod fixtures;
mod renewal;
mod reuse;
mod sandbox_admission;
use fixtures::*;

#[test]
fn reset_identity_and_resource_operation_namespace_are_independent_from_configuration() {
    let mut identity = AllocationIdentity {
        bridge_id: crate::bridge::BridgeId("brg_fixture".into()),
        key: serde_json::from_value(serde_json::json!({"version":1,"hash":"a".repeat(64)}))
            .unwrap(),
        generation: 1,
    };
    let first = identity.id().unwrap();
    let head = identity.head_id().unwrap();
    identity.generation = 2;
    assert_ne!(identity.id().unwrap(), first);
    assert_eq!(identity.head_id().unwrap(), head);
    identity.generation = 0;
    assert_eq!(identity.id(), Err(AllocationError::Corrupt));
}

#[tokio::test]
async fn first_message_race_has_one_deterministic_tree_and_operation() {
    let (admission, authority) = fixture();
    let writes = (0..16)
        .map(|i| {
            AllocationReservation::new(admission.clone(), authority.clone(), 1000 + i).unwrap()
        })
        .collect::<Vec<_>>();
    for write in &writes {
        assert_eq!(write.record().id, writes[0].record().id);
        assert_eq!(write.record().operation_id, writes[0].record().operation_id);
        assert_eq!(
            write.record().placement.target(),
            writes[0].record().placement.target()
        );
    }
    let record = writes[0].record();
    let round_trip: AllocationRecord =
        serde_json::from_slice(&serde_json::to_vec(record).unwrap()).unwrap();
    assert!(round_trip == *record);
    round_trip.validate().unwrap();
}

#[tokio::test]
async fn lost_provider_receipt_requires_original_identity_reconciliation_after_takeover() {
    let (record, authority) = reserved();
    let claim =
        AllocationMutation::claim(record, authority.clone(), "worker-one", 1001, 100).unwrap();
    let lease = claim.record().lease.clone().unwrap();
    let started = AllocationMutation::progress(
        claim.record().clone(),
        authority.clone(),
        &lease,
        AllocationProgress::Start,
        1002,
    )
    .unwrap();
    let original = started.record().clone();
    assert!(AllocationMutation::claim(
        original.clone(),
        authority.clone(),
        "worker-two",
        1050,
        100
    )
    .is_err());
    let taken =
        AllocationMutation::claim(original.clone(), authority.clone(), "worker-two", 1101, 100)
            .unwrap();
    assert_eq!(taken.record().operation_id, original.operation_id);
    assert_eq!(
        taken.record().placement.target(),
        original.placement.target()
    );
    assert!(taken.record().effect_started);
    assert_eq!(taken.record().lease_fence, 2);
    assert!(AllocationMutation::progress(
        taken.record().clone(),
        authority.clone(),
        &lease,
        AllocationProgress::Confirm(observation(&original)),
        1102
    )
    .is_err());
    let lease = taken.record().lease.as_ref().unwrap();
    assert!(matches!(
        AllocationMutation::progress(
            taken.record().clone(),
            authority.clone(),
            lease,
            AllocationProgress::Start,
            1102
        ),
        Err(AllocationError::InvalidTransition)
    ));
    let mut wrong = observation(&original);
    wrong.target.agent_id = crate::AgentId("agt_substitution".into());
    assert!(matches!(
        AllocationMutation::progress(
            taken.record().clone(),
            authority.clone(),
            lease,
            AllocationProgress::Confirm(wrong),
            1102
        ),
        Err(AllocationError::IdentityMismatch)
    ));
    let confirmed = AllocationMutation::progress(
        taken.record().clone(),
        authority,
        lease,
        AllocationProgress::Confirm(observation(&original)),
        1102,
    )
    .unwrap();
    assert_eq!(confirmed.record().state, AllocationState::Ready);
    assert!(confirmed.record().lease.is_none());
    assert_eq!(
        confirmed.record().receipt.as_ref().unwrap().operation_id,
        original.operation_id
    );
}

#[tokio::test(start_paused = true)]
async fn original_authority_expiry_and_snapshot_revoke_cannot_be_renewed_by_a_lease() {
    let (record, authority) = reserved();
    let claim =
        AllocationMutation::claim(record, authority.clone(), "worker", 1001, 60_000).unwrap();
    let mut revoked = authority.bridge().clone();
    revoked.emergency.execution_stopped = true;
    revoked.revision += 1;
    assert_eq!(
        authority.require_snapshot(&revoked),
        Err(AllocationError::Conflict)
    );
    tokio::time::advance(std::time::Duration::from_secs(16)).await;
    assert_eq!(
        claim.require_current(),
        Err(AllocationError::AuthorityUnavailable)
    );
    assert!(matches!(
        AllocationMutation::progress(
            claim.record().clone(),
            authority,
            claim.record().lease.as_ref().unwrap(),
            AllocationProgress::Start,
            1002
        ),
        Err(AllocationError::AuthorityUnavailable)
    ));
}

#[tokio::test]
async fn exact_profile_revision_content_ancestry_and_trust_digest_are_immutable() {
    let (record, _) = reserved();
    let AllocationPlacement::NewSandbox { profiles, .. } = &record.placement else {
        unreachable!()
    };
    profiles.validate(&record.config).unwrap();
    let mut wrong = profiles.clone();
    wrong.agent.content.system_prompt = "injected".into();
    assert_eq!(
        wrong.validate(&record.config),
        Err(AllocationError::InvalidProfiles)
    );
    let mut wrong = profiles.clone();
    wrong.agent.revision += 1;
    assert_eq!(
        wrong.validate(&record.config),
        Err(AllocationError::InvalidProfiles)
    );
    let mut wrong = profiles.clone();
    wrong.trust_digest = "f".repeat(64);
    assert_eq!(
        wrong.validate(&record.config),
        Err(AllocationError::InvalidProfiles)
    );
    let mut wrong = profiles.clone();
    wrong.workspace.content.agent_profiles = vec![crate::profiles::AgentProfileRef::new(
        wrong.agent.id.clone(),
        2,
    )];
    wrong.workspace.digest = crate::profiles::content_digest(&wrong.workspace.content).unwrap();
    wrong.trust_digest =
        AllocationProfiles::digest(&wrong.sandbox, &wrong.workspace, &wrong.agent).unwrap();
    assert_eq!(
        wrong.validate(&record.config),
        Err(AllocationError::InvalidProfiles)
    );
}

#[tokio::test]
async fn failure_states_preserve_started_operation_and_config_updates_retain_generation() {
    let (record, authority) = reserved();
    let claim =
        AllocationMutation::claim(record.clone(), authority.clone(), "worker", 1001, 100).unwrap();
    let lease = claim.record().lease.as_ref().unwrap();
    let started = AllocationMutation::progress(
        claim.record().clone(),
        authority.clone(),
        lease,
        AllocationProgress::Start,
        1002,
    )
    .unwrap();
    for progress in [
        AllocationProgress::NeedsConnection,
        AllocationProgress::FailedRetryable,
        AllocationProgress::FailedTerminal,
    ] {
        let failed = AllocationMutation::progress(
            started.record().clone(),
            authority.clone(),
            lease,
            progress,
            1003,
        )
        .unwrap();
        assert!(failed.record().effect_started);
        assert_eq!(failed.record().operation_id, record.operation_id);
        assert_eq!(failed.record().placement, record.placement);
        assert!(failed.record().lease.is_none());
    }
    let mut proposal = record.clone();
    proposal.pin.config_revision = 2;
    proposal.config.display_name = "Updated name".into();
    record.require_same_partition(&proposal).unwrap();
    assert_eq!(record.pin.config_revision, 1);
    let mut damaged = record.clone();
    damaged.operation_id = "replacement-operation".into();
    assert_eq!(damaged.validate(), Err(AllocationError::Corrupt));
}

#[tokio::test]
async fn retained_generation_drain_uses_original_profiles_and_custody_and_suspend_refuses() {
    use crate::bridge::{AcceptedWorkPolicy, BridgeSecretRef, BridgeState};
    let (record, authority) = reserved();
    let mut current = authority.bridge().clone();
    current.revision += 1;
    current.config_revision = 2;
    let mut next = current.generations[0].clone();
    next.config_revision = 2;
    next.config.connector.key_id = "key-two".into();
    next.config.connector.custody =
        BridgeSecretRef::new("brsec_00000000000000000000000000000002".into()).unwrap();
    next.previous_work = Some(AcceptedWorkPolicy::DrainOriginal);
    current.generations.push(next);
    current.state = BridgeState::Disabled {
        accepted_work: AcceptedWorkPolicy::DrainOriginal,
    };
    let proof = renewed(current.clone(), &record).unwrap();
    proof.require_record(&record).unwrap();
    assert_eq!(proof.config().unwrap().connector.key_id, "key-one");
    assert_eq!(
        proof.custody_pin().reference(),
        &record.config.connector.custody
    );
    let claim = AllocationMutation::claim(record.clone(), proof, "worker", 1001, 100).unwrap();
    assert_eq!(claim.record().placement, record.placement);
    assert_eq!(claim.record().pin, record.pin);
    current.generations[1].previous_work = Some(AcceptedWorkPolicy::SuspendOriginal);
    assert!(matches!(
        renewed(current.clone(), &record),
        Err(AllocationError::AuthorityUnavailable)
    ));
    current.generations[1].previous_work = Some(AcceptedWorkPolicy::DrainOriginal);
    current.state = BridgeState::Retired {
        accepted_work: AcceptedWorkPolicy::DrainOriginal,
    };
    renewed(current.clone(), &record)
        .unwrap()
        .require_record(&record)
        .unwrap();
    current.state = BridgeState::Retired {
        accepted_work: AcceptedWorkPolicy::SuspendOriginal,
    };
    assert!(matches!(
        renewed(current, &record),
        Err(AllocationError::AuthorityUnavailable)
    ));
}

#[tokio::test]
async fn delayed_storage_cannot_commit_expired_lease_or_forged_resource_ids() {
    let (record, authority) = reserved();
    let claim = AllocationMutation::claim(record.clone(), authority, "worker", 1001, 100).unwrap();
    claim.require_at(1100).unwrap();
    assert_eq!(
        claim.require_at(1101),
        Err(AllocationError::LeaseUnavailable)
    );
    assert_eq!(
        claim.require_at(1000),
        Err(AllocationError::LeaseUnavailable)
    );
    let mut corrupt = record;
    if let AllocationPlacement::NewSandbox { target, .. } = &mut corrupt.placement {
        target.agent_id = crate::AgentId("agt_foreign".into());
    }
    assert_eq!(corrupt.validate(), Err(AllocationError::Corrupt));
}
