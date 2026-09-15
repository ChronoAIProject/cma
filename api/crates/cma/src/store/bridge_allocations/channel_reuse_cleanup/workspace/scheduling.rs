use super::*;
use crate::sandbox::owned_workspace_cleanup::WorkspaceCleanupState;

enum Change<'a> {
    Claim(&'a str),
    Reconcile(&'a WorkspaceCleanupJobClaim),
    Observe(
        &'a WorkspaceCleanupJobClaim,
        WorkspaceCleanupProgress,
        &'a [WorkspaceCleanupHold],
    ),
}
impl MongoBridgeAllocationStore {
    pub(crate) async fn discover_workspace_cleanup_jobs(
        &self,
        after: Option<&str>,
    ) -> Result<Vec<String>, CleanupError> {
        use futures_util::TryStreamExt;
        let database = self.bridges.client.database(&self.records.namespace().db);
        let now = mongodb::bson::DateTime::now().timestamp_millis();
        let mut cursor = majority_collection::<Document>(&database, WORKSPACE_JOBS)
            .find(doc! {"complete":false,"next_attempt_at_ms":{"$lte":now},
            "_id":{"$type":"string","$gt":after.unwrap_or("")},
            "$or":[{"lease_expires_at_ms":null},{"lease_expires_at_ms":{"$lte":now}}]})
            .sort(doc! {"_id":1})
            .limit(32)
            .projection(doc! {"_id":1})
            .await
            .map_err(|_| CleanupError::Unavailable)?;
        let mut ids = Vec::new();
        while let Some(row) = cursor
            .try_next()
            .await
            .map_err(|_| CleanupError::Unavailable)?
        {
            ids.push(
                row.get_str("_id")
                    .map_err(|_| CleanupError::Corrupt)?
                    .into(),
            );
        }
        Ok(ids)
    }
    pub(crate) async fn claim_workspace_cleanup_job(
        &self,
        id: &str,
        holder: &str,
    ) -> Result<(WorkspaceCleanupJob, AllocationRecord, WorkspaceCleanupState), CleanupError> {
        self.change_workspace_cleanup_job(id, Change::Claim(holder))
            .await
    }
    pub(crate) async fn reconcile_workspace_cleanup_job(
        &self,
        id: &str,
        claim: &WorkspaceCleanupJobClaim,
    ) -> Result<WorkspaceCleanupJob, CleanupError> {
        Ok(self
            .change_workspace_cleanup_job(id, Change::Reconcile(claim))
            .await?
            .0)
    }
    pub(crate) async fn observe_workspace_cleanup_job(
        &self,
        id: &str,
        claim: &WorkspaceCleanupJobClaim,
        progress: WorkspaceCleanupProgress,
        holds: &[WorkspaceCleanupHold],
    ) -> Result<WorkspaceCleanupJob, CleanupError> {
        Ok(self
            .change_workspace_cleanup_job(id, Change::Observe(claim, progress, holds))
            .await?
            .0)
    }
    async fn change_workspace_cleanup_job(
        &self,
        id: &str,
        change: Change<'_>,
    ) -> Result<(WorkspaceCleanupJob, AllocationRecord, WorkspaceCleanupState), CleanupError> {
        for attempt in 0..16 {
            if attempt > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(1 << (attempt - 1).min(5)))
                    .await;
            }
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
                .workspace_scheduling_transaction(id, &change, &mut session)
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
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(result),
                CommitOutcome::Retry(_) => continue,
                _ => return Err(CleanupError::Unavailable),
            }
        }
        Err(CleanupError::Unavailable)
    }
    async fn workspace_scheduling_transaction(
        &self,
        id: &str,
        change: &Change<'_>,
        session: &mut ClientSession,
    ) -> Result<(WorkspaceCleanupJob, AllocationRecord, WorkspaceCleanupState), JobError> {
        let database = self.bridges.client.database(&self.records.namespace().db);
        let initial = records::read_job(&read(&database, WORKSPACE_JOBS, id, session).await?)?;
        let allocation = super::super::super::records::decode_record(
            &read(
                &database,
                "bridge_allocations",
                &initial.admission.receipt.allocation_id,
                session,
            )
            .await?,
        )?;
        let sandbox = crate::store::sandboxes::read_sandbox(
            &read(
                &database,
                "sandboxes",
                initial.admission.receipt.target.sandbox_id.as_str(),
                session,
            )
            .await?,
        )
        .map_err(|_| CleanupError::Corrupt)?;
        let (mut job, state, _) = proof::retained(&database, &allocation, &sandbox, session)
            .await?
            .ok_or(CleanupError::Corrupt)?;
        if job != initial || job.admission.receipt.id != id {
            return Err(CleanupError::Corrupt.into());
        }
        let now = mongodb::bson::DateTime::now().timestamp_millis();
        match change {
            Change::Claim(holder) => job.claim(
                holder,
                now,
                now.checked_add(60_000).ok_or(CleanupError::Corrupt)?,
            )?,
            Change::Reconcile(claim) => job.reconcile_progress(claim, state.progress, now)?,
            Change::Observe(claim, progress, holds) => {
                if *progress > state.progress {
                    return Err(CleanupError::Corrupt.into());
                }
                job.observe(claim, *progress, holds.to_vec(), now)?;
            }
        }
        let previous = read(&database, WORKSPACE_JOBS, id, session).await?;
        if records::read_job(&previous)? != initial {
            return Err(CleanupError::Conflict.into());
        }
        let changed = majority_collection::<Document>(&database, WORKSPACE_JOBS)
            .update_one(previous, doc! {"$set":records::job_document(&job)?})
            .session(session)
            .await?;
        if changed.matched_count != 1 {
            return Err(CleanupError::Conflict.into());
        }
        Ok((job, allocation, state))
    }
}
