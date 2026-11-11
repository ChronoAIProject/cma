//! The exact first task turn may fit in a fixed, already-proven prefix.
use super::*;
use crate::store::agent_log::read_entry;
use crate::store::agent_log_integrity::read::read_proven_agent;
use layer2_core::sandbox::AgentLogKind;
use mongodb::ClientSession;

const PREFIX_ENTRIES: u32 = 200;
const PREFIX_BYTES: usize = 4 * 1024 * 1024;

/// None requests the existing complete fold; it never means that no title is
/// eligible. In particular, setup and long first turns cannot be truncated or
/// replaced with a later turn. This reader does not prepare or repair metadata.
pub(super) async fn read(
    store: &MongoSandboxStore,
    expected: &Agent,
    session: &mut ClientSession,
) -> Result<Option<AgentTitleTranscript>, AgentTitleStoreError> {
    if !store.bounded_admission_agents.contains(&expected.id)
        || expected.conversation.last_seq <= u64::from(PREFIX_ENTRIES)
    {
        return Ok(None);
    }
    let (agent, trusted) = read_proven_agent(store, &expected.id, session)
        .await
        .map_err(map_store_error)?
        .ok_or(AgentTitleStoreError::NotFound)?;
    if &agent != expected {
        return Err(AgentTitleStoreError::SequenceConflict);
    }
    if !trusted {
        return Ok(None);
    }

    let mut read = crate::store::mongo_read_telemetry::Read::find(store.agent_log.name());
    let mut cursor = store
        .agent_log
        .find(doc! {
            "agent_id": agent.id.as_str(),
            "seq": { "$gte": 1_i64, "$lte": i64::from(PREFIX_ENTRIES) },
        })
        .sort(doc! { "seq": 1 })
        .limit(i64::from(PREFIX_ENTRIES))
        .batch_size(PREFIX_ENTRIES)
        .read_concern(ReadConcern::snapshot())
        .session(&mut *session)
        .await
        .map_err(unavailable)?;
    let mut entries = Vec::new();
    let mut bytes = 0;
    while let Some(row) = read
        .next_in_session(&mut cursor, session)
        .await
        .map_err(unavailable)?
    {
        bytes += cursor.current().as_bytes().len();
        if bytes > PREFIX_BYTES {
            // The complete validator still owns the legacy physical limit.
            // Do not turn this optimization's byte budget into a new refusal.
            read.complete();
            return Ok(None);
        }
        let entry = read_entry(&row).map_err(map_store_error)?;
        if entry.agent_id != agent.id || entry.seq != entries.len() as u64 + 1 {
            return Err(AgentTitleStoreError::SequenceConflict);
        }
        match &entry.kind {
            AgentLogKind::Created {
                workspace_id,
                worktree_id,
            } if entry.seq == 1
                && workspace_id == &agent.workspace_id
                && worktree_id == agent.creation_worktree_id() => {}
            AgentLogKind::Created { .. } => return Err(AgentTitleStoreError::Corrupt),
            _ if entry.seq == 1 => return Err(AgentTitleStoreError::Corrupt),
            _ => {}
        }
        entries.push(entry);
    }
    if entries.len() != PREFIX_ENTRIES as usize {
        return Err(AgentTitleStoreError::SequenceConflict);
    }
    read.complete();
    // This unchanged domain fold returns as soon as the first task turn is
    // complete. A positive prefix result is therefore identical to a full fold.
    Ok(AgentTitleTranscript::from_agent_log(&agent, &entries))
}
