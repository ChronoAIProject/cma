//! Validate the structural certificate against the current immutable log tip.
use crate::store::sandbox_mongo::DOCUMENT_SCHEMA_VERSION;

use super::*;

pub(super) async fn require_latest(
    store: &MongoSandboxStore,
    agent: &Agent,
    proof: &Progress,
    session: &mut ClientSession,
) -> Result<(), TransactionLogError> {
    require_tip(store, agent, session).await?;
    let seq = signed(proof.through, "certified inactivity sequence")
        .map_err(SandboxStoreError::Corrupt)?;
    let mut cursor = store
        .agent_log
        .aggregate([
            doc! { "$match": {
                "_id": log_id(&agent.id, proof.through),
                "agent_id": agent.id.as_str(),
                "seq": seq,
                "document_schema_version": DOCUMENT_SCHEMA_VERSION,
            }},
            doc! { "$project": {
                "_id": 0,
                "bytes": { "$toLong": { "$bsonSize": "$$ROOT" } },
                "matches": {"$and": [
                    {"$eq": ["$record.agent_id", {"$literal": agent.id.as_str()}]},
                    {"$eq": ["$record.seq", seq]},
                    {"$eq": ["$record.appended_at", {"$literal": &proof.tail_appended_at}]},
                ]},
            }},
        ])
        .session(&mut *session)
        .await
        .map_err(TransactionLogError::Driver)?;
    if !cursor
        .advance(session)
        .await
        .map_err(TransactionLogError::Driver)?
    {
        return Err(invalid().into());
    }
    let row = cursor
        .deserialize_current()
        .map_err(TransactionLogError::Driver)?;
    if row.get_bool("matches").ok() != Some(true) || row_bytes(&row)? != proof.tail_bytes {
        return Err(invalid().into());
    }
    Ok(())
}

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
