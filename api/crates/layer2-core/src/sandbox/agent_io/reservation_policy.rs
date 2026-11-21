use crate::resource_access::{ResourceOwner, ResourcePolicyRecord};
use crate::sandbox::Sandbox;

use super::{CommandLedgerError, CommandReservation};

/// Run under the same store lock/transaction as command reservation and the
/// parent write fence. Missing policy is explicitly personal; a matching raw
/// subject cannot convert an organization into personal command authority.
pub fn validate_command_reservation_policy(
    request: &CommandReservation,
    parent: &Sandbox,
    policy: Option<&ResourcePolicyRecord>,
) -> Result<(), CommandLedgerError> {
    let owner = match policy {
        Some(policy) => {
            policy
                .personal_owner_of(parent)
                .map_err(|_| CommandLedgerError::Corrupt)?;
            policy.owner.clone()
        }
        None => ResourceOwner::Person {
            subject: parent.owner.clone(),
        },
    };
    if request.envelope.owner != owner {
        return Err(CommandLedgerError::NotFound);
    }
    if request.policy.sandbox_id != parent.id
        || request.policy.sandbox_created_at != parent.created_at
        || request.policy.policy_revision != policy.map_or(0, |policy| policy.revision)
    {
        return Err(CommandLedgerError::Conflict);
    }
    if let Some(audit) = &request.admission_audit {
        audit.validate().map_err(|_| CommandLedgerError::Invalid)?;
        if audit.actor != request.envelope.actor
            || audit.owner != owner
            || audit.target
                != (crate::resource_access::ResourceTarget::Agent {
                    agent_id: request.envelope.agent_id.clone(),
                })
            || audit.sandbox_id != request.policy.sandbox_id
            || audit.sandbox_created_at != request.policy.sandbox_created_at
            || audit.policy_revision != request.policy.policy_revision
            || audit.capability
                != super::admission_service::command_capability(&request.envelope.command)
        {
            return Err(CommandLedgerError::Invalid);
        }
    }
    Ok(())
}
