//! Validated snapshot reads for conversation projections.
use super::*;

pub(in crate::store) async fn pending_approvals_snapshot(
    store: &MongoSandboxStore,
    id: &AgentId,
    session: &mut mongodb::ClientSession,
) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
    use crate::store::agent_log_integrity::scan_collection;
    use crate::store::agent_log_reads::integrity::{LogReadIntegrity, PreparedIntegrity};
    let head = PreparedIntegrity
        .read_head(store, id, session)
        .await?
        .ok_or(SandboxStoreError::NotFound)?;
    if store.bounded_approval_reads
        && (head.trusted || head.segmented.is_some())
        && head.last_seq > 200
    {
        if let Some(entries) = crate::store::agent_log_reads::read_bounded(
            store,
            id,
            head.last_seq - 200,
            head.last_seq,
            &head,
            session,
            4 * 1024 * 1024,
        )
        .await?
        {
            if let Some(pending) = PendingAgentApprovals::from_reset_suffix(&entries) {
                return Ok(pending);
            }
        }
    }
    // A read cannot audit, quarantine or repair. Retain complete validation
    // when the trusted suffix cannot prove a reset of the outstanding set.
    let mut pending = PendingAgentApprovals::default();
    scan_collection(
        ScanCaller::PublicWindowPrefix,
        &store.agent_log,
        id,
        head.last_seq,
        session,
        true,
        |entry| {
            head.validate_lineage(&entry)?;
            pending.observe(&entry);
            Ok(())
        },
    )
    .await
    .map_err(|error| snapshot_error(error.into()))?;
    Ok(pending.into_entries())
}

impl MongoSandboxStore {
    pub(crate) fn with_bounded_approval_reads(mut self, enabled: bool) -> Self {
        self.bounded_approval_reads = enabled;
        self
    }
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
