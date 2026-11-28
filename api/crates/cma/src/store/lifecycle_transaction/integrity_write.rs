//! Lifecycle publication retains the full validator and exact Agent fence.
use super::*;

pub(super) async fn advance_agent(
    store: &MongoSandboxStore,
    current: &Agent,
    next: &Agent,
    integrity: mongodb::bson::Document,
    session: &mut mongodb::ClientSession,
) -> Result<(), ApplyError> {
    let mut fields = agent_replacement_fields(next).map_err(ApplyError::Domain)?;
    let row = store
        .agents
        .find_one(doc! { "_id": current.id.as_str() })
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
        .ok_or(ApplyError::Domain(SandboxStoreError::NotFound))?;
    crate::store::agent_conversation_store::carry_checkpoint_fields(
        &row,
        current,
        next,
        &mut fields,
    )
    .map_err(ApplyError::Domain)?;
    fields.insert("agent_log_integrity_v1", integrity);
    fields.insert(
        "last_seq",
        Bson::Int64(
            signed(next.conversation.last_seq, "lifecycle agent log sequence")
                .map_err(SandboxStoreError::Corrupt)
                .map_err(ApplyError::Domain)?,
        ),
    );
    let advanced = store
        .agents
        .update_one(
            doc! {
                "_id": current.id.as_str(),
                "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                "sandbox_id": current.sandbox_id.as_str(),
                "owner": current.owner.as_str(),
                "revision": signed(current.revision, "lifecycle agent revision")
                    .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?,
                "last_seq": signed(current.conversation.last_seq, "lifecycle agent log sequence")
                    .map_err(SandboxStoreError::Corrupt)
                    .map_err(ApplyError::Domain)?,
            },
            doc! { "$set": fields },
        )
        .session(session)
        .await
        .map_err(classify_aggregate_write)?;
    if advanced.matched_count == 1 {
        Ok(())
    } else {
        Err(ApplyError::Domain(SandboxStoreError::Conflict))
    }
}

pub(super) async fn finish_published_text(
    store: &MongoSandboxStore,
    current: &Agent,
    next: &mut Agent,
    logs: &mut Vec<AgentLogEntry>,
    session: &mut mongodb::ClientSession,
) -> Result<(), ApplyError> {
    let row = store
        .agents
        .find_one(doc! {"_id":current.id.as_str()})
        .session(session)
        .await
        .map_err(ApplyError::Driver)?
        .ok_or(ApplyError::Domain(SandboxStoreError::NotFound))?;
    crate::store::agent_conversation_store::checkpoint::finish_published_text(
        &row, current, next, logs,
    )
    .map_err(ApplyError::Domain)
}

pub(super) fn classify_aggregate_write(error: mongodb::error::Error) -> ApplyError {
    if is_duplicate_key(&error) {
        ApplyError::Domain(SandboxStoreError::Duplicate)
    } else {
        ApplyError::Driver(error)
    }
}

pub(super) fn classify_log_write(error: mongodb::error::Error) -> ApplyError {
    if is_duplicate_key(&error) {
        ApplyError::Domain(SandboxStoreError::SequenceConflict)
    } else {
        ApplyError::Driver(error)
    }
}
