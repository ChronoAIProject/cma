use super::*;

impl MongoBridgeAllocationStore {
    pub(crate) async fn admit_cleanup_job(
        &self,
        write: &CleanupWrite,
        permit: &ChannelReuseCleanupAuthority,
    ) -> Result<ChildCleanupStatus, CleanupError> {
        for attempt in 0..16 {
            if attempt > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(1 << (attempt - 1).min(5)))
                    .await;
            }
            write.access().require_current()?;
            permit.require_current().map_err(allocation_error)?;
            let mut session = self
                .bridges
                .client
                .start_session()
                .await
                .map_err(|_| CleanupError::Unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(|_| CleanupError::Unavailable)?;
            let result = match self
                .cleanup_job_admission_transaction(write, permit, &mut session)
                .await
            {
                Ok(result) => result,
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
                CommitOutcome::Unknown(_) | CommitOutcome::Failed(_) => {
                    return Err(CleanupError::Unavailable)
                }
            }
        }
        Err(CleanupError::Unavailable)
    }

    async fn cleanup_job_admission_transaction(
        &self,
        write: &CleanupWrite,
        permit: &ChannelReuseCleanupAuthority,
        session: &mut ClientSession,
    ) -> Result<ChildCleanupStatus, JobError> {
        let access = write.access();
        let allocation = access.allocation();
        let database = self.bridges.client.database(&self.records.namespace().db);
        let row = self.cleanup_job_access(access, session).await?;
        if let Some(previous) = self
            .require_cleanup_job(&database, allocation, &row, session)
            .await?
        {
            if previous.admission.receipt.id != write.receipt_id()
                || previous.admission.request_sha256 != write.request_sha256()
                || previous.admission.receipt.requested_by != *access.management().actor()
                || previous.admission.issuer != write.issuer()
            {
                return Err(CleanupError::Conflict.into());
            }
            return Ok(previous.status()?);
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
        let saved_id = ChannelReuseCleanupState::id(&allocation.id);
        let request = match majority_collection::<Document>(&database, REQUESTS)
            .find_one(doc! {"_id":&saved_id})
            .session(&mut *session)
            .await?
        {
            Some(row) => {
                let saved = read_state(&row)?;
                if saved.requested_by != *access.management().actor()
                    || saved.request.expected_allocation_revision != allocation.revision
                {
                    return Err(CleanupError::Conflict.into());
                }
                // The existing request transaction verifies its permanent
                // audit, allocation marker and complete immutable seal.
                saved.request
            }
            None => ChannelReuseCleanupRequest {
                request_id: write.receipt_id().into(),
                allocation_id: allocation.id.clone(),
                expected_allocation_revision: allocation.revision,
            },
        };
        let saved = self
            .cleanup_request_transaction(permit, &request, session)
            .await?;
        let now = mongodb::bson::DateTime::now().timestamp_millis();
        if now < saved.requested_at_ms {
            return Err(CleanupError::Conflict.into());
        }
        let mut admission = CleanupJobAdmission {
            schema_version: 1,
            receipt: ChildCleanupReceipt {
                id: write.receipt_id().into(),
                bridge_id: allocation.identity.bridge_id.clone(),
                allocation_id: allocation.id.clone(),
                generation: allocation.identity.generation,
                target: allocation.placement.target().clone(),
                request: write.request().clone(),
                requested_by: access.management().actor().clone(),
                accepted_at_ms: now,
            },
            owner: allocation.owner.clone(),
            issuer: write.issuer().into(),
            allocation_sha256: digest(allocation)?,
            request_sha256: write.request_sha256().into(),
            cleanup_request_id: saved.id.clone(),
            cleanup_request_sha256: saved.digest.clone(),
            digest: String::new(),
        };
        admission.digest = admission.compute_digest()?;
        admission.validate()?;
        let job = CleanupJob::new(admission, holds(&saved.holds), saved.requested_at_ms)?;
        majority_collection::<Document>(&database, JOBS)
            .insert_one(job_document(&job)?)
            .session(&mut *session)
            .await?;
        majority_collection::<Document>(&database, JOB_AUDITS)
            .insert_one(admission_document(&job.admission)?)
            .session(&mut *session)
            .await?;
        // Stage A may have fenced/marked this row in the same transaction.
        // Match its new exact representation, never an earlier pre-write row.
        let current = read(&database, "bridge_allocations", &allocation.id, session).await?;
        if records::decode_record(&current)? != *allocation || current.contains_key(JOB_MARKER) {
            return Err(CleanupError::Conflict.into());
        }
        let changed = self.records.update_one(current,
            doc! {"$set":{JOB_MARKER:{"id":&job.admission.receipt.id,"digest":&job.admission.digest}},"$inc":{"channel_reuse_cleanup_fence":1_i64}})
            .session(&mut *session).await?;
        if changed.matched_count != 1 {
            return Err(CleanupError::Conflict.into());
        }
        permit.require_current()?;
        write.require_current()?;
        let status = job.status()?;
        access.require_status(&status)?;
        Ok(status)
    }
}
