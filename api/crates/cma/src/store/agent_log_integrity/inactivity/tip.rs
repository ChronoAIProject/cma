//! Validate the indexed log tip without loading its event payload.
use super::*;

pub(super) async fn require_tip(
    store: &MongoSandboxStore,
    agent: &Agent,
    session: &mut ClientSession,
) -> Result<(), TransactionLogError> {
    let tip = store
        .agent_log
        .find_one(doc! { "agent_id": agent.id.as_str() })
        .hint(Hint::Name("agent_log_sequence".into()))
        .sort(doc! { "seq": -1 })
        .projection(doc! { "_id": 0, "seq": 1 })
        .session(session)
        .await
        .map_err(TransactionLogError::Driver)?;
    if tip.as_ref().and_then(sequence) != Some(agent.conversation.last_seq) {
        return Err(invalid().into());
    }
    Ok(())
}
