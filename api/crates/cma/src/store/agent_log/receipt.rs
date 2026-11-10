//! Exact single-row reconciliation in the pinned commit snapshot.
use super::*;

pub(super) async fn reconcile_append_after_unknown(
    store: &MongoSandboxStore,
    expected: &AgentLogEntry,
    snapshot_time: mongodb::bson::Timestamp,
) -> Result<Option<AgentLogEntry>, SandboxStoreError> {
    let mut session = store
        .client
        .start_session()
        .snapshot(true)
        .snapshot_time(snapshot_time)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
    let row = store
        .agents
        .find_one(doc! { "_id": expected.agent_id.as_str() })
        .read_concern(ReadConcern::snapshot())
        .session(&mut session)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
    let Some(row) = row else {
        return Ok(None);
    };
    let agent = read_agent(&row)?;
    if agent.conversation.last_seq < expected.seq {
        return Ok(None);
    }
    let trusted = trusted_summary(
        store,
        &agent,
        &row,
        ScanCaller::UnknownCommitProof,
        &mut session,
        true,
    )
    .await
    .map_err(|error| match error {
        TransactionLogError::Domain(error) => error,
        TransactionLogError::Driver(error) => SandboxStoreError::Unavailable(error.to_string()),
    })?;
    if trusted.is_some() {
        let mut observation =
            crate::store::mongo_read_telemetry::Read::find(store.agent_log.name());
        let raw_log = store
            .agent_log
            .clone_with_type::<mongodb::bson::RawDocumentBuf>();
        let raw = raw_log
            .find_one(doc! { "_id": log_id(&expected.agent_id, expected.seq) })
            .read_concern(ReadConcern::snapshot())
            .session(&mut session)
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
        if let Some(raw) = &raw {
            observation.document(raw.as_bytes().len());
        }
        let Some(raw) = raw else {
            observation.complete();
            return Ok(None);
        };
        let document = mongodb::bson::Document::try_from(&raw)
            .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
        let entry = read_entry(&document)?;
        observation.complete();
        return if &entry == expected {
            Ok(Some(entry))
        } else {
            Err(SandboxStoreError::SequenceConflict)
        };
    }
    let mut found = None;
    scan_complete_log_with_snapshot_session(
        ScanCaller::UnknownCommitProof,
        store,
        &expected.agent_id,
        agent.conversation.last_seq,
        &mut session,
        |entry| {
            if entry.seq == expected.seq {
                if &entry != expected {
                    return Err(SandboxStoreError::SequenceConflict);
                }
                found = Some(entry);
            }
            Ok(())
        },
    )
    .await
    .map_err(|error| match error {
        TransactionLogError::Domain(error) => error,
        TransactionLogError::Driver(error) => SandboxStoreError::Unavailable(error.to_string()),
    })?;
    Ok(found)
}
