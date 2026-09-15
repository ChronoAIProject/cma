use super::*;

enum Query<'a> {
    Allocations {
        after: Option<&'a str>,
        limit: u32,
    },
    Allocation(&'a str),
    Status(&'a CleanupAccess),
    Recovery {
        access: &'a CleanupAccess,
        id: &'a str,
        digest: &'a str,
    },
}
#[derive(Default)]
struct ResultValue {
    allocations: Vec<AllocationRecord>,
    status: Option<ChildCleanupStatus>,
}

impl MongoBridgeAllocationStore {
    pub(crate) async fn managed_cleanup_allocations(
        &self,
        manager: &BridgeManagementLease,
        after: Option<&str>,
        limit: u32,
    ) -> Result<Vec<AllocationRecord>, CleanupError> {
        if !(1..=100).contains(&limit) {
            return Err(CleanupError::Invalid);
        }
        Ok(self
            .cleanup_job_read(manager, Query::Allocations { after, limit })
            .await?
            .allocations)
    }
    pub(crate) async fn managed_cleanup_allocation(
        &self,
        manager: &BridgeManagementLease,
        id: &str,
    ) -> Result<Option<AllocationRecord>, CleanupError> {
        Ok(self
            .cleanup_job_read(manager, Query::Allocation(id))
            .await?
            .allocations
            .pop())
    }
    pub(crate) async fn managed_cleanup_status(
        &self,
        access: &CleanupAccess,
    ) -> Result<ChildCleanupStatus, CleanupError> {
        self.cleanup_job_read(access.management(), Query::Status(access))
            .await?
            .status
            .ok_or(CleanupError::Corrupt)
    }
    pub(crate) async fn recover_cleanup_job(
        &self,
        access: &CleanupAccess,
        id: &str,
        digest: &str,
    ) -> Result<Option<ChildCleanupStatus>, CleanupError> {
        Ok(self
            .cleanup_job_read(access.management(), Query::Recovery { access, id, digest })
            .await?
            .status)
    }

    async fn cleanup_job_read(
        &self,
        manager: &BridgeManagementLease,
        query: Query<'_>,
    ) -> Result<ResultValue, CleanupError> {
        for attempt in 0..16 {
            if attempt > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(1 << (attempt - 1).min(5)))
                    .await;
            }
            manager.require_current()?;
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
                .cleanup_job_read_transaction(manager, &query, &mut session)
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
            if let Err(error) = manager.require_current() {
                let _ = session.abort_transaction().await;
                return Err(error.into());
            }
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => {
                    manager.require_current()?;
                    return Ok(result);
                }
                CommitOutcome::Retry(_) => continue,
                CommitOutcome::Unknown(_) | CommitOutcome::Failed(_) => {
                    return Err(CleanupError::Unavailable);
                }
            }
        }
        Err(CleanupError::Unavailable)
    }

    async fn cleanup_job_read_transaction(
        &self,
        manager: &BridgeManagementLease,
        query: &Query<'_>,
        session: &mut ClientSession,
    ) -> Result<ResultValue, JobError> {
        self.cleanup_job_manager(manager, session).await?;
        let database = self.bridges.client.database(&self.records.namespace().db);
        let mut result = ResultValue::default();
        match query {
            Query::Allocations { after, limit } => {
                let mut filter = doc! {"bridge_id":manager.record().id.as_str()};
                if let Some(after) = after {
                    filter.insert("_id", doc! {"$gt":*after});
                }
                let mut cursor = self
                    .records
                    .find(filter)
                    .sort(doc! {"_id":1})
                    .limit(i64::from(*limit) + 1)
                    .session(&mut *session)
                    .await?;
                while cursor.advance(&mut *session).await? {
                    result
                        .allocations
                        .push(managed_allocation(manager, &cursor.deserialize_current()?)?);
                }
            }
            Query::Allocation(id) => {
                if let Some(row) = self
                    .records
                    .find_one(doc! {"_id":*id,"bridge_id":manager.record().id.as_str()})
                    .session(&mut *session)
                    .await?
                {
                    result.allocations.push(managed_allocation(manager, &row)?);
                }
            }
            Query::Status(access) | Query::Recovery { access, .. } => {
                let row = self.cleanup_job_access(access, session).await?;
                let allocation = access.allocation();
                let job = self
                    .require_cleanup_job(&database, allocation, &row, session)
                    .await?;
                match query {
                    Query::Recovery { id, digest, .. } => {
                        // Another key for this allocation cannot recover the
                        // original receipt or replace its permanent intent.
                        if let Some(job) = job.filter(|job| job.admission.receipt.id == *id) {
                            if job.admission.request_sha256 != *digest
                                || job.admission.receipt.requested_by != *manager.actor()
                            {
                                return Err(CleanupError::Conflict.into());
                            }
                            result.status = Some(job.status()?);
                        }
                    }
                    _ => {
                        result.status = Some(match job {
                            Some(job) => job.status()?,
                            None => ChildCleanupStatus {
                                bridge_id: allocation.identity.bridge_id.clone(),
                                allocation_id: allocation.id.clone(),
                                generation: allocation.identity.generation,
                                allocation_revision: allocation.revision,
                                target: allocation.placement.target().clone(),
                                receipt: None,
                                progress: ChildCleanupProgress::NotRequested,
                                holds: Vec::new(),
                                observed_at_ms: None,
                                agent_capacity_released: false,
                                workspace_retained: true,
                                history_retained: true,
                            },
                        })
                    }
                }
                if let Some(status) = &result.status {
                    access.require_status(status)?;
                }
            }
        }
        Ok(result)
    }

    pub(in crate::store::bridge_allocations::channel_reuse_cleanup) async fn require_cleanup_job(
        &self,
        database: &Database,
        allocation: &AllocationRecord,
        row: &Document,
        session: &mut ClientSession,
    ) -> Result<Option<CleanupJob>, JobError> {
        let job_row = majority_collection::<Document>(database, JOBS)
            .find_one(doc! {"allocation_id":&allocation.id})
            .session(&mut *session)
            .await?;
        let removal_id = format!(
            "{}:child-removal:v1",
            ChannelReuseCleanupState::id(&allocation.id)
        );
        let tombstone = majority_collection::<Document>(database, CLEANUP_TOMBSTONES)
            .find_one(doc! {"_id":&removal_id})
            .session(&mut *session)
            .await?;
        let Some(job_row) = job_row else {
            if row.contains_key(JOB_MARKER)
                || tombstone.is_some()
                || majority_collection::<Document>(database, JOB_AUDITS)
                    .find_one(doc! {"allocation_id":&allocation.id})
                    .session(&mut *session)
                    .await?
                    .is_some()
            {
                return Err(CleanupError::Corrupt.into());
            }
            return Ok(None);
        };
        let job = read_job(&job_row)?;
        let admission = &job.admission;
        if admission.receipt.allocation_id != allocation.id
            || admission.receipt.bridge_id != allocation.identity.bridge_id
            || admission.receipt.generation != allocation.identity.generation
            || admission.owner != allocation.owner
            || &admission.receipt.target != allocation.placement.target()
            || admission.receipt.request.expected_allocation_revision != allocation.revision
            || admission.allocation_sha256 != digest(allocation)?
            || row.get_document(JOB_MARKER).ok()
                != Some(&doc! {"id":&admission.receipt.id,"digest":&admission.digest})
        {
            return Err(CleanupError::Corrupt.into());
        }
        let audit = read(database, JOB_AUDITS, &admission.receipt.id, session).await?;
        if read_admission(&audit)? != *admission {
            return Err(CleanupError::Corrupt.into());
        }
        let saved =
            read_state(&read(database, REQUESTS, &admission.cleanup_request_id, session).await?)?;
        if saved.digest != admission.cleanup_request_sha256
            || saved.requested_by != admission.receipt.requested_by
            || saved.seal.allocation_sha256 != admission.allocation_sha256
            || saved.seal.owner != admission.owner
            || saved.seal.target != admission.receipt.target
            || saved.request.expected_allocation_revision
                != admission.receipt.request.expected_allocation_revision
            || read_state(&read(database, AUDITS, &saved.id, session).await?)? != saved
            || row.get_document("channel_reuse_cleanup_request").ok()
                != Some(&doc! {"id":&saved.id,"digest":&saved.digest})
        {
            return Err(CleanupError::Corrupt.into());
        }
        if job.progress != ChildCleanupProgress::Requested {
            self.require_cleanup_admission(database, &saved, false, session)
                .await?;
        }
        if let Some(tombstone) = tombstone {
            let removal = read_removal(&tombstone)?;
            let closed = self
                .require_cleanup_admission(database, &saved, false, session)
                .await?;
            if removal.request_sha256 != saved.digest
                || removal.admission_sha256 != closed.digest
                || removal.seal != saved.seal
            {
                return Err(CleanupError::Corrupt.into());
            }
            let audit = read(
                database,
                super::super::removal::REMOVAL_AUDITS,
                &super::super::removal::audit_id(&removal),
                session,
            )
            .await?;
            if read_removal(&audit)? != removal {
                return Err(CleanupError::Corrupt.into());
            }
            // Runtime publication can lead the scheduling observation after a
            // lost response. A job may lag proved effects, never lead them.
            let proved = match removal.phase {
                ChannelReuseRemovalPhase::Intent => ChildCleanupProgress::RemovalStarted,
                ChannelReuseRemovalPhase::ProcessClosed => ChildCleanupProgress::ProcessClosed,
                ChannelReuseRemovalPhase::HomeRemoved => ChildCleanupProgress::HomeRemoved,
                ChannelReuseRemovalPhase::WorktreeRemoved => ChildCleanupProgress::WorktreeRemoved,
                ChannelReuseRemovalPhase::Complete => ChildCleanupProgress::Complete,
            };
            if job.progress > proved {
                return Err(CleanupError::Corrupt.into());
            }
            if job.progress == ChildCleanupProgress::Complete {
                if removal.phase != ChannelReuseRemovalPhase::Complete {
                    return Err(CleanupError::Corrupt.into());
                }
                require_complete(database, &removal, session).await?;
            }
            fence_row(database, CLEANUP_TOMBSTONES, tombstone, session).await?;
        } else if !matches!(
            job.progress,
            ChildCleanupProgress::Requested | ChildCleanupProgress::AdmissionClosed
        ) {
            return Err(CleanupError::Corrupt.into());
        }
        fence_row(database, JOBS, job_row, session).await?;
        fence_row(database, "bridge_allocations", row.clone(), session).await?;
        Ok(Some(job))
    }
}

fn managed_allocation(
    manager: &BridgeManagementLease,
    row: &Document,
) -> Result<AllocationRecord, JobError> {
    let record = records::decode_record(row)?;
    if record.identity.bridge_id != manager.record().id
        || record.owner != manager.record().owner
        || manager.record().pinned(&record.pin)?.config != record.config
    {
        return Err(CleanupError::NotFound.into());
    }
    Ok(record)
}
