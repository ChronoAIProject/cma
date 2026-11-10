//! Validated snapshot reads for conversation projections.
use super::*;

pub(super) async fn pending_approvals_snapshot(
    store: &MongoSandboxStore,
    id: &AgentId,
    session: &mut mongodb::ClientSession,
) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
    let agent = store
        .agents
        .find_one(doc! { "_id": id.as_str() })
        .read_concern(ReadConcern::snapshot())
        .session(&mut *session)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
        .as_ref()
        .map(read_agent)
        .transpose()?
        .ok_or(SandboxStoreError::NotFound)?;
    let mut pending = PendingAgentApprovals::default();
    scan_complete_log_with_snapshot_session(
        ScanCaller::PublicWindowPrefix,
        store,
        id,
        agent.conversation.last_seq,
        session,
        |entry| {
            pending.observe(&entry);
            Ok(())
        },
    )
    .await
    .map_err(|error| snapshot_error(error.into()))?;
    Ok(pending.into_entries())
}

pub(super) async fn snapshot_session(
    store: &MongoSandboxStore,
    snapshot_time: Option<mongodb::bson::Timestamp>,
) -> Result<mongodb::ClientSession, SandboxStoreError> {
    let session = store.client.start_session().snapshot(true);
    let session = match snapshot_time {
        Some(snapshot_time) => session.snapshot_time(snapshot_time),
        None => session,
    };
    let session = session
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
    Ok(session)
}

pub(super) fn snapshot_error(error: ApplyError) -> SandboxStoreError {
    match error {
        ApplyError::Domain(error) => error,
        ApplyError::Driver(error) => SandboxStoreError::Unavailable(error.to_string()),
    }
}
