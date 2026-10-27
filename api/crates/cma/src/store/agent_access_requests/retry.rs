//! A failed request remains immutable; an explicit retry asks for fresh consent.
use super::*;

pub(super) async fn validate(
    store: &MongoSandboxStore,
    requester: &AgentAccessRequester,
    proposal: &AgentAccessProposal,
    new_id: &str,
    session: &mut ClientSession,
) -> TxResult<()> {
    let Some(previous_id) = &proposal.retry_of else {
        return Ok(());
    };
    if previous_id == new_id {
        return Err(AgentAccessError::Conflict.into());
    }
    let previous = store
        .agent_access_requests
        .find_one(
            doc! {"_id":previous_id,"record.requester.agent_id":requester.agent_id.as_str(),
            "record.requester.owner":requester.owner.as_str()},
        )
        .session(&mut *session)
        .await?
        .ok_or(AgentAccessError::NotFound)?;
    let previous = read(&previous)?;
    if previous.requester.creation != requester.creation
        || previous.requester.conversation != requester.conversation
        || previous.requester.sandbox_id != requester.sandbox_id
    {
        return Err(AgentAccessError::NotFound.into());
    }
    if previous.phase != AgentAccessPhase::Failed {
        return Err(AgentAccessError::Conflict.into());
    }
    // The admission transaction also fences the exact Agent lineage. Concurrent
    // attempts conflict on that write, then observe the first child on retry.
    // A transport replay was handled by its own key before reaching this check.
    if store
        .agent_access_requests
        .count_documents(
            doc! {"record.requester.agent_id":requester.agent_id.as_str(),
            "record.proposal.retry_of":previous_id},
        )
        .session(session)
        .await?
        != 0
    {
        return Err(AgentAccessError::Conflict.into());
    }
    Ok(())
}
