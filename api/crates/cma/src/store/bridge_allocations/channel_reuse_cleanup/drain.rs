//! Close only new turns, then obtain fresh fenced observations of accepted work.
use super::*;

const ADMISSIONS: &str = "channel_reuse_cleanup_admissions";
const ADMISSION_AUDITS: &str = "channel_reuse_cleanup_admission_audits";

impl MongoBridgeAllocationStore {
    pub(crate) async fn close_channel_reuse_cleanup_admission(
        &self,
        permit: &ChannelReuseCleanupAuthority,
        request: &ChannelReuseCleanupRequest,
    ) -> Result<ChannelReuseCleanupAdmission, AllocationError> {
        self.cleanup_drain(permit, request, true)
            .await
            .map(|observation| observation.admission)
    }

    pub(crate) async fn observe_channel_reuse_cleanup_quiescence(
        &self,
        permit: &ChannelReuseCleanupAuthority,
        request: &ChannelReuseCleanupRequest,
    ) -> Result<ChannelReuseCleanupObservation, AllocationError> {
        self.cleanup_drain(permit, request, false).await
    }

    async fn cleanup_drain(
        &self,
        permit: &ChannelReuseCleanupAuthority,
        request: &ChannelReuseCleanupRequest,
        close: bool,
    ) -> Result<ChannelReuseCleanupObservation, AllocationError> {
        request.require(permit.allocation())?;
        for attempt in 0..16 {
            if attempt > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(1 << (attempt - 1).min(5)))
                    .await;
            }
            permit.require_current()?;
            let mut session = self
                .bridges
                .client
                .start_session()
                .await
                .map_err(|_| AllocationError::Unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(|_| AllocationError::Unavailable)?;
            let result = match self
                .cleanup_drain_transaction(permit, request, close, &mut session)
                .await
            {
                Ok(value) => value,
                Err(error) => {
                    let _ = session.abort_transaction().await;
                    match error {
                        MutationError::Mongo(error)
                            if is_transient(&error) || is_duplicate_key(&error) =>
                        {
                            continue;
                        }
                        MutationError::Mongo(_) => return Err(AllocationError::Unavailable),
                        MutationError::Core(error) => return Err(error),
                    }
                }
            };
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(result),
                CommitOutcome::Retry(_) => continue,
                CommitOutcome::Unknown(_) | CommitOutcome::Failed(_) => {
                    return Err(AllocationError::Unavailable);
                }
            }
        }
        Err(AllocationError::Unavailable)
    }

    pub(super) async fn cleanup_drain_transaction(
        &self,
        permit: &ChannelReuseCleanupAuthority,
        request: &ChannelReuseCleanupRequest,
        close: bool,
        session: &mut ClientSession,
    ) -> Result<ChannelReuseCleanupObservation, MutationError> {
        let allocation = permit.allocation();
        let database = self.bridges.client.database(&self.records.namespace().db);
        // Require already committed stage A evidence. The call below can only
        // take its replay branch in this transaction's consistent snapshot.
        read(
            &database,
            REQUESTS,
            &ChannelReuseCleanupState::id(&allocation.id),
            session,
        )
        .await?;
        let saved = self
            .cleanup_request_transaction(permit, request, session)
            .await?;
        let row = read(&database, "bridge_allocations", &allocation.id, session).await?;
        let (snapshot, mut holds) = inventory::read_owned(self, permit, &row, session).await?;
        let mut current_seal = ChannelReuseCleanupSeal::from_snapshot(&snapshot)?;
        let runtime = current_seal.runtime.clone();
        // Runtime identity is freshly observed after pause/revive. Original
        // creation, paths and allocation evidence must remain exactly retained.
        current_seal.runtime = saved.seal.runtime.clone();
        if current_seal != saved.seal {
            return Err(AllocationError::IdentityMismatch.into());
        }
        let admission = self
            .require_cleanup_admission(&database, &saved, close, session)
            .await?;
        holds.extend(effects::fence_holds(&database, allocation, session).await?);
        if agent_busy(&snapshot) {
            holds.insert(ChannelReuseCleanupHold::AgentBusy);
        }
        let workspace = snapshot
            .workspace
            .as_ref()
            .ok_or(AllocationError::Corrupt)?;
        if workspace.work.step != layer2_core::sandbox::WorkspaceWorkStep::None
            || workspace.work.lease.is_some()
            || workspace.work.execution.is_some()
        {
            holds.insert(ChannelReuseCleanupHold::WorkspaceBusy);
        }
        permit.require_current()?;
        Ok(ChannelReuseCleanupObservation {
            admission,
            holds: holds.into_iter().collect(),
            runtime,
            sandbox_revision: snapshot.sandbox.revision,
            workspace_revision: workspace.revision,
            agent_revision: snapshot
                .agent
                .as_ref()
                .ok_or(AllocationError::Corrupt)?
                .revision,
            observed_at_ms: mongodb::bson::DateTime::now().timestamp_millis(),
        })
    }

    pub(super) async fn require_cleanup_admission(
        &self,
        database: &Database,
        saved: &ChannelReuseCleanupState,
        close: bool,
        session: &mut ClientSession,
    ) -> Result<ChannelReuseCleanupAdmission, MutationError> {
        let id = ChannelReuseCleanupAdmission::id(&saved.id);
        let rows: Collection<Document> = majority_collection(database, ADMISSIONS);
        let agent = read(
            database,
            "agents",
            saved.seal.target.agent_id.as_str(),
            session,
        )
        .await?;
        if let Some(row) = rows
            .find_one(doc! {"_id":&id})
            .session(&mut *session)
            .await?
        {
            let admission = read_admission(&row, saved)?;
            let audit = read(database, ADMISSION_AUDITS, &id, session).await?;
            let marker = doc! {"id":&admission.id,"digest":&admission.digest};
            if read_admission(&audit, saved)? != admission
                || agent.get_document(CLEANUP_ADMISSION_FIELD).ok() != Some(&marker)
            {
                return Err(AllocationError::Corrupt.into());
            }
            fence_row(database, ADMISSIONS, row, session).await?;
            return Ok(admission);
        }
        if agent.contains_key(CLEANUP_ADMISSION_FIELD) {
            return Err(AllocationError::Corrupt.into());
        }
        if !close {
            return Err(AllocationError::InvalidTransition.into());
        }
        let admission = ChannelReuseCleanupAdmission::new(
            saved,
            saved.requested_by.clone(),
            mongodb::bson::DateTime::now().timestamp_millis(),
        )?;
        let row = admission_document(&admission)?;
        rows.insert_one(row.clone()).session(&mut *session).await?;
        majority_collection::<Document>(database, ADMISSION_AUDITS)
            .insert_one(row)
            .session(&mut *session)
            .await?;
        let changed = majority_collection::<Document>(database, "agents").update_one(
            agent,
            doc! {"$set":{CLEANUP_ADMISSION_FIELD:{"id":&admission.id,"digest":&admission.digest}}},
        ).session(session).await?;
        if changed.matched_count != 1 {
            return Err(AllocationError::Conflict.into());
        }
        Ok(admission)
    }
}

fn admission_document(value: &ChannelReuseCleanupAdmission) -> Result<Document, AllocationError> {
    Ok(
        doc! {"_id":&value.id,"document_schema_version":DOCUMENT_SCHEMA_VERSION,
        "agent_id":value.agent_id.as_str(),"request_id":&value.request_id,
        "record":record_bson(value).map_err(|_| AllocationError::Corrupt)?},
    )
}

pub(super) fn read_admission(
    row: &Document,
    saved: &ChannelReuseCleanupState,
) -> Result<ChannelReuseCleanupAdmission, AllocationError> {
    let value: ChannelReuseCleanupAdmission =
        read_record(row).map_err(|_| AllocationError::Corrupt)?;
    value.require(saved)?;
    if admission_document(&value)?
        .iter()
        .any(|(key, value)| row.get(key) != Some(value))
    {
        return Err(AllocationError::Corrupt);
    }
    Ok(value)
}
