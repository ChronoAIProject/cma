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
    Ok(())
}
