use super::*;
use crate::bridge::*;
use crate::resource_access::*;
use crate::{AgentId, SandboxId, SubjectId, WorkspaceId, WorktreeId};

mod fixtures;
use fixtures::{bridge, grant};

#[test]
fn immutable_consent_records_reject_substituted_issuing_actor_target_and_creation() {
    let original = grant();
    original.validate().unwrap();
    for mutation in 0..4 {
        let mut changed = original.clone();
        match mutation {
            0 => {
                changed.issued_by = ResourceActor::Human {
                    subject: SubjectId::new("other"),
                }
            }
            1 => changed.target.ids.agent_id = AgentId("other-agent".into()),
            2 => changed.target.sandbox_created_at = "2026-09-16T00:00:00Z".into(),
            _ => {
                changed.owner = ResourceOwner::Organization {
                    subject: SubjectId::new("owner"),
                }
            }
        }
        assert_eq!(changed.validate(), Err(BridgeInvokeError::Corrupt));
    }
    let mut revoked = original.clone();
    revoked.revision = 2;
    revoked.state = BridgeInvokeGrantState::Revoked {
        at: "2026-09-16T00:00:00Z".into(),
        actor: original.issued_by.clone(),
    };
    revoked.validate().unwrap();
    assert_eq!(revoked.require_active(), Err(BridgeInvokeError::Revoked));
    revoked.state = BridgeInvokeGrantState::Active;
    assert_eq!(revoked.validate(), Err(BridgeInvokeError::Corrupt));
}

#[test]
fn fixed_binding_and_historical_message_effects_obey_distinct_generation_controls() {
    let grant = grant();
    let mut bridge = bridge(&grant);
    let binding = BridgeInvokeEffect::BindExistingTarget {
        allocation_id: "allocation-one".into(),
    };
    let message = BridgeInvokeEffect::AdmitMessage {
        inbox_id: "inbox-one".into(),
    };
    super::proof::require_bridge(&bridge, &grant, &binding).unwrap();
    let mut next = bridge.generations[0].clone();
    next.config_revision = 2;
    next.config.display_name = "New generation".into();
    next.previous_work = Some(AcceptedWorkPolicy::DrainOriginal);
    bridge.generations.push(next);
    bridge.config_revision = 2;
    bridge.revision = 3;
    assert_eq!(
        super::proof::require_bridge(&bridge, &grant, &binding),
        Err(BridgeInvokeError::Suspended)
    );
    super::proof::require_bridge(&bridge, &grant, &message).unwrap();
    bridge.generations[1].previous_work = Some(AcceptedWorkPolicy::SuspendOriginal);
    assert_eq!(
        super::proof::require_bridge(&bridge, &grant, &message),
        Err(BridgeInvokeError::Suspended)
    );
    bridge.generations[1].previous_work = Some(AcceptedWorkPolicy::DrainOriginal);
    bridge.emergency.execution_stopped = true;
    assert_eq!(
        super::proof::require_bridge(&bridge, &grant, &message),
        Err(BridgeInvokeError::Suspended)
    );
}

#[test]
fn grant_receipt_replay_is_bound_to_actual_actor_owner_and_original_consent_request() {
    let grant = grant();
    let receipt = BridgeInvokeReceipt {
        id: "a".repeat(64),
        request_digest: "b".repeat(64),
        bridge_id: grant.pin.bridge_id.clone(),
        actor: grant.issued_by.clone(),
        result: grant.view(),
    };
    assert_eq!(match_invoke_replay(&receipt, &receipt).unwrap(), receipt);
    for mutation in 0..3 {
        let mut changed = receipt.clone();
        match mutation {
            0 => {
                changed.actor = ResourceActor::Human {
                    subject: SubjectId::new("other"),
                }
            }
            1 => changed.request_digest = "c".repeat(64),
            _ => {
                changed.result.owner = ResourceOwner::Organization {
                    subject: SubjectId::new("owner"),
                }
            }
        }
        assert_eq!(
            match_invoke_replay(&receipt, &changed),
            Err(BridgeInvokeError::IdempotencyConflict)
        );
    }
}
