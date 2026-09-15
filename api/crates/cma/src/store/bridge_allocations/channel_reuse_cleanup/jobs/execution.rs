//! Bind each existing A/B/C effect transaction to its explicit live job claim.
use super::*;

impl MongoBridgeAllocationStore {
    pub(in crate::store::bridge_allocations::channel_reuse_cleanup) async fn fence_cleanup_execution_job(
        &self,
        permit: &ChannelReuseCleanupAuthority,
        row: &Document,
        session: &mut ClientSession,
    ) -> Result<Document, MutationError> {
        self.cleanup_execution_job(permit, row, session)
            .await
            .map_err(|error| match error {
                JobError::Mongo(error) => MutationError::Mongo(error),
                JobError::Core(error) => MutationError::Core(match error {
                    CleanupError::Corrupt | CleanupError::Invalid => AllocationError::Corrupt,
                    CleanupError::Conflict => AllocationError::Conflict,
                    _ => AllocationError::AuthorityUnavailable,
                }),
            })
    }
    async fn cleanup_execution_job(
        &self,
        permit: &ChannelReuseCleanupAuthority,
        row: &Document,
        session: &mut ClientSession,
    ) -> Result<Document, JobError> {
        let database = self.bridges.client.database(&self.records.namespace().db);
        let allocation = permit.allocation();
        let Some((id, claim)) = permit.job() else {
            // Existing direct A/B/C paths may inspect their own historical
            // work, but cannot bypass an explicit job's retained claim.
            if row.contains_key(JOB_MARKER) {
                return Err(CleanupError::Unavailable.into());
            }
            for collection in [JOBS, JOB_AUDITS] {
                if majority_collection::<Document>(&database, collection)
                    .find_one(doc! {"allocation_id":&allocation.id})
                    .session(&mut *session)
                    .await?
                    .is_some()
                {
                    return Err(CleanupError::Unavailable.into());
                }
            }
            return Ok(row.clone());
        };
        let job = self
            .require_cleanup_job(&database, allocation, row, session)
            .await?
            .ok_or(CleanupError::Corrupt)?;
        if job.admission.receipt.id != id
            || job.admission.receipt.requested_by != *permit.management().actor()
            || job.admission.owner != allocation.owner
        {
            return Err(CleanupError::Corrupt.into());
        }
        job.require_claim(claim, mongodb::bson::DateTime::now().timestamp_millis())?;
        // Proof checks write the allocation fence; callers need its updated
        // same-transaction row for their exact compare-and-swap.
        read(&database, "bridge_allocations", &allocation.id, session)
            .await
            .map_err(Into::into)
    }
}
