//! Full semantic audits must revoke a disproved, previously trusted checkpoint.
use super::*;
use crate::store::transaction_terminal::GuardedTerminal as _;

pub(in crate::store) async fn audit_checkpoint(
    store: &MongoSandboxStore,
    agent: &Agent,
    entries: &[AgentLogEntry],
    session: &mut mongodb::ClientSession,
) -> Result<SemanticCheckpoint, TransactionLogError> {
    let checkpoint = SemanticCheckpoint::from_entries(agent, entries)?;
    if checkpoint.valid {
        return Ok(checkpoint);
    }
    // The complete structural validator has already checked this snapshot.
    // Only an invalid semantic fold needs this additional indexed lookup.
    let row = store
        .agents
        .find_one(doc! { "_id": agent.id.as_str() })
        .session(&mut *session)
        .await
        .map_err(TransactionLogError::Driver)?
        .ok_or(SandboxStoreError::NotFound)?;
    let previously_trusted = parse_integrity(&row)?.is_some_and(|prior| {
        prior.bound_agent_revision == agent.revision
            && prior.head_seq == agent.conversation.last_seq
            && prior.history_identity == history_identity(agent)
            && prior.semantic.valid
            && prior.semantic.matches_agent(agent)
    });
    if previously_trusted {
        if let Err(corruption) = SemanticCheckpoint::validate_retained_semantics(agent, entries) {
            let _ = session.abort_guarded().await;
            proof::quarantine_row(store, &row).await?;
            return Err(corruption.into());
        }
    }
    // Legacy incomplete/unsupported histories remain explicitly untrusted;
    // their existing structural append contract is preserved.
    Ok(checkpoint)
}
