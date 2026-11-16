use layer2_core::agent_ownership::{execution_authority, PROTOCOL_VERSION};
use layer2_core::sandbox::{agent_io::CommandLedgerError, store::SandboxStoreError};
use layer2_core::AgentId;
use mongodb::ClientSession;

use super::{ApplyError, MongoSandboxStore};

/// A request-scope token alone cannot serialize takeover with an admission.
/// Join the durable fence to the very transaction that writes the command and
/// Agent, and reject a remote token presented on this process's store instance.
pub(in crate::store) async fn fence_owner(
    store: &MongoSandboxStore,
    id: &AgentId,
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    if store.agent_ownership.is_none() {
        return Ok(());
    }
    let authority = execution_authority()
        .filter(|authority| {
            authority.is_current()
                && authority.token.agent_id == *id
                && authority.token.epoch > 0
                && authority.token.protocol == PROTOCOL_VERSION
                && store.agent_manager_instance() == Some(authority.token.instance_id.as_str())
        })
        .ok_or(CommandLedgerError::Conflict)?;
    store
        .fence_agent_effect(id, session)
        .await
        .map_err(map_error)?;
    if !authority.is_current() {
        return Err(CommandLedgerError::Conflict.into());
    }
    Ok(())
}

fn map_error(error: SandboxStoreError) -> ApplyError {
    ApplyError::Domain(match error {
        SandboxStoreError::Conflict => CommandLedgerError::Conflict,
        SandboxStoreError::NotFound => CommandLedgerError::NotFound,
        SandboxStoreError::Corrupt(_) | SandboxStoreError::InvalidTransaction(_) => {
            CommandLedgerError::Corrupt
        }
        // The fence helper distinguishes known-aborted transactions from
        // unknown outcomes. Never promote availability failures to CAS retries.
        _ => CommandLedgerError::Unavailable,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_fence_preserves_conflict_corruption_and_uncertainty() {
        for (error, expected) in [
            (SandboxStoreError::Conflict, CommandLedgerError::Conflict),
            (
                SandboxStoreError::Corrupt("fixture".into()),
                CommandLedgerError::Corrupt,
            ),
            (
                SandboxStoreError::Unavailable("unknown outcome".into()),
                CommandLedgerError::Unavailable,
            ),
        ] {
            assert!(matches!(map_error(error), ApplyError::Domain(actual) if actual == expected));
        }
    }
}
