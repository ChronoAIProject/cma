//! Public admission and scheduler roots fence the durable owner/member with
//! their Agent effects. Atomic creation roots retain their own CAS authority.
use super::{records, worker::Change, ApplyError};
use crate::store::sandbox_mongo::MongoSandboxStore;
use layer2_core::agent_ownership::execution_authority;
use layer2_core::sandbox::{agent_io::CommandLedgerError, store::SandboxStoreError};
use layer2_core::AgentId;
use mongodb::{bson::doc, ClientSession};

pub(in crate::store) async fn fence_change(
    store: &MongoSandboxStore,
    change: &Change,
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    if store.agent_manager_instance().is_none() {
        return Ok(());
    }
    let agent = match change {
        Change::Reconcile(id) => {
            let row = store
                .agent_commands
                .find_one(doc! {"_id": id.as_str()})
                .session(&mut *session)
                .await?
                .ok_or(CommandLedgerError::NotFound)?;
            records::read(&row)?.receipt.envelope.agent_id
        }
        Change::Claim { request, .. } => request.agent_id.clone(),
        Change::Renew { claim, .. }
        | Change::Release { claim, .. }
        | Change::Reject { claim, .. } => claim.agent_id.clone(),
    };
    fence(store, &agent, session).await
}

pub(in crate::store) async fn fence(
    store: &MongoSandboxStore,
    agent: &AgentId,
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    require_local(store, agent)?;
    if store.agent_manager_instance().is_none() {
        return Ok(());
    }
    store
        .fence_agent_effect_transaction(agent, session)
        .await
        .map_err(|error| match error {
            crate::store::agent_ownership::FenceError::Driver(error) => ApplyError::Driver(error),
            crate::store::agent_ownership::FenceError::Domain(error) => {
                ApplyError::Domain(match error {
                    SandboxStoreError::Unavailable(_) => CommandLedgerError::Unavailable,
                    SandboxStoreError::Corrupt(_) => CommandLedgerError::Corrupt,
                    _ => CommandLedgerError::Conflict,
                })
            }
        })
}

pub(in crate::store) fn require_local(
    store: &MongoSandboxStore,
    agent: &AgentId,
) -> Result<(), CommandLedgerError> {
    let Some(instance) = store.agent_manager_instance() else {
        return Ok(());
    };
    let current = execution_authority().is_some_and(|authority| {
        authority.is_current()
            && authority.token.agent_id == *agent
            && authority.token.instance_id == instance
    });
    if !current {
        return Err(CommandLedgerError::Conflict);
    }
    Ok(())
}
