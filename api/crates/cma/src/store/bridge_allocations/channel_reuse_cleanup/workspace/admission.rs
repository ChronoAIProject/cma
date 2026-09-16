use super::*;
use crate::sandbox::owned_workspace_cleanup::WorkspaceCleanupState;

impl MongoBridgeAllocationStore {
    pub(crate) async fn initialize_workspace_cleanup(&self) -> Result<(), CleanupError> {
        let database = self.bridges.client.database(&self.records.namespace().db);
        for collection in [WORKSPACE_JOBS, WORKSPACE_JOB_AUDITS, WORKSPACE_TOMBSTONES] {
            for key in ["allocation_id", "workspace_id"] {
                majority_collection::<Document>(&database, collection)
                    .create_index(
                        IndexModel::builder()
                            .keys(doc! {key:1})
                            .options(
                                mongodb::options::IndexOptions::builder()
                                    .unique(true)
                                    .build(),
                            )
                            .build(),
                    )
                    .await
                    .map_err(|_| CleanupError::Unavailable)?;
            }
        }
        majority_collection::<Document>(&database, WORKSPACE_TOMBSTONES)
            .create_index(IndexModel::builder().keys(doc! {"sandbox_id":1}).build())
            .await
            .map_err(|_| CleanupError::Unavailable)?;
        majority_collection::<Document>(&database, WORKSPACE_JOBS)
            .create_index(
                IndexModel::builder()
                    .keys(
                        doc! {"complete":1,"_id":1,"next_attempt_at_ms":1,"lease_expires_at_ms":1},
                    )
                    .build(),
            )
            .await
            .map_err(|_| CleanupError::Unavailable)?;
        Ok(())
    }
    pub(crate) async fn admit_workspace_cleanup_job(
        &self,
        write: &WorkspaceCleanupWrite,
        permit: &ChannelReuseCleanupAuthority,
        support: &layer3_adapter::WorkspaceRemovalSupport,
    ) -> Result<WorkspaceCleanupStatus, CleanupError> {
        for attempt in 0..16 {
            if attempt > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(1 << (attempt - 1).min(5)))
                    .await;
            }
            write.access().require_current()?;
            let mut session = self
                .bridges
                .client
                .start_session()
                .await
                .map_err(|_| CleanupError::Unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(|_| CleanupError::Unavailable)?;
            let result = self
                .workspace_consent_transaction(write, permit, support, &mut session)
                .await;
            let result = match result {
                Ok(value) => value,
                Err(error) => {
                    let _ = session.abort_transaction().await;
                    if error.retryable() {
                        continue;
                    }
                    return Err(error.core());
                }
            };
            if let Err(error) = write.access().require_current() {
                let _ = session.abort_transaction().await;
                return Err(error);
            }
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(result),
                CommitOutcome::Retry(_) => continue,
                _ => return Err(CleanupError::Unavailable),
            }
        }
        Err(CleanupError::Unavailable)
    }
    async fn workspace_consent_transaction(
        &self,
        write: &WorkspaceCleanupWrite,
        permit: &ChannelReuseCleanupAuthority,
        support: &layer3_adapter::WorkspaceRemovalSupport,
        session: &mut ClientSession,
    ) -> Result<WorkspaceCleanupStatus, JobError> {
        let access = write.access();
        self.cleanup_job_access(access, session).await?;
        let database = self.bridges.client.database(&self.records.namespace().db);
        let allocation = access.allocation();
        let sandbox = &access.parent().lineage.sandbox;
        if let Some((job, _, workspace)) =
            proof::retained(&database, allocation, sandbox, session).await?
        {
            if job.admission.receipt.id != write.receipt_id()
                || job.admission.request_sha256 != write.request_sha256()
                || job.admission.receipt.requested_by != *access.management().actor()
                || job.admission.issuer != write.issuer()
            {
                return Err(CleanupError::Conflict.into());
            }
            return Ok(job.status(workspace.revision, false)?);
        }
        write.require_current()?;
        permit.require_current()?;
        if permit.allocation() != allocation
            || permit.management().record() != access.management().record()
        {
            return Err(CleanupError::Conflict.into());
        }
        if permit.management().actor() != access.management().actor() {
            return Err(CleanupError::OriginalActorRequired.into());
        }
        require_support(sandbox, support)?;
        // Common original-human/resource authority only; the completed child
        // job can never lend its scheduling claim to this independent consent.
        super::super::authority::fence_common(self, permit, session).await?;
        let (_, child) = child::complete(
            &database,
            allocation,
            &write.request().child_cleanup_receipt_id,
            session,
        )
        .await?;
        let (workspace, holds) =
            ownership::require(&database, allocation, sandbox, &child, false, session).await?;
        if !holds.is_empty() || workspace.revision != write.request().expected_workspace_revision {
            return Err(CleanupError::Conflict.into());
        }
        let mut admission = WorkspaceCleanupJobAdmission {
            schema_version: 1,
            receipt: WorkspaceCleanupReceipt {
                id: write.receipt_id().into(),
                bridge_id: allocation.identity.bridge_id.clone(),
                allocation_id: allocation.id.clone(),
                generation: allocation.identity.generation,
                target: allocation.placement.target().clone(),
                request: write.request().clone(),
                requested_by: access.management().actor().clone(),
                accepted_at_ms: mongodb::bson::DateTime::now().timestamp_millis(),
            },
            owner: allocation.owner.clone(),
            issuer: write.issuer().into(),
            allocation_sha256: digest(allocation)?,
            request_sha256: write.request_sha256().into(),
            child_removal_sha256: child.digest.clone(),
            workspace_creation_sha256: child
                .seal
                .workspace_creation_sha256
                .clone()
                .ok_or(CleanupError::Corrupt)?,
            digest: String::new(),
        };
        admission.digest = admission.compute_digest()?;
        let state = WorkspaceCleanupState::new(admission.clone(), child)?;
        let job = WorkspaceCleanupJob::new(admission, Vec::new(), state.updated_at_ms)?;
        for (collection, row) in [
            (WORKSPACE_JOBS, records::job_document(&job)?),
            (
                WORKSPACE_JOB_AUDITS,
                records::consent_document(&job.admission)?,
            ),
            (WORKSPACE_TOMBSTONES, records::state_document(&state)?),
        ] {
            majority_collection::<Document>(&database, collection)
                .insert_one(row)
                .session(&mut *session)
                .await?;
        }
        records::append_audit(&database, &state, session).await?;
        for (collection, id) in [
            ("bridge_allocations", allocation.id.as_str()),
            ("workspaces", workspace.id.as_str()),
        ] {
            let row = read(&database, collection, id, session).await?;
            if row.contains_key(WORKSPACE_JOB_FIELD) || row.contains_key(WORKSPACE_ADMISSION_FIELD)
            {
                return Err(CleanupError::Conflict.into());
            }
            let changed = majority_collection::<Document>(&database, collection)
                .update_one(row, doc! {"$set":{WORKSPACE_JOB_FIELD:records::marker(&job.admission)},"$inc":{"owned_workspace_cleanup_fence":1_i64}}).session(&mut *session).await?;
            if changed.matched_count != 1 {
                return Err(CleanupError::Conflict.into());
            }
        }
        permit.require_current()?;
        write.require_current()?;
        Ok(job.status(workspace.revision, true)?)
    }
}

pub(super) fn require_support(
    sandbox: &Sandbox,
    support: &layer3_adapter::WorkspaceRemovalSupport,
) -> Result<(), CleanupError> {
    if support.protocol_version != layer3_adapter::WORKSPACE_REMOVAL_PROTOCOL
        || !sandbox
            .runtime_process_matches(&support.runtime)
            .map_err(|_| CleanupError::Unavailable)?
        || !support.runtime.installation_ready
        || !support.runtime.credential_ready
        || support.runtime.restore_pending
    {
        return Err(CleanupError::Unavailable);
    }
    Ok(())
}
