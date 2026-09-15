use super::*;

enum Change<'a> {
    Reconcile {
        claim: &'a CleanupJobClaim,
    },
    Claim {
        holder: &'a str,
    },
    Observe {
        claim: &'a CleanupJobClaim,
        progress: ChildCleanupProgress,
        holds: &'a [ChildCleanupHold],
    },
}

impl MongoBridgeAllocationStore {
    pub(crate) async fn initialize_cleanup_jobs(&self) -> Result<(), CleanupError> {
        let database = self.bridges.client.database(&self.records.namespace().db);
        let jobs = majority_collection::<Document>(&database, JOBS);
        jobs.create_index(
            IndexModel::builder()
                .keys(doc! {"allocation_id":1})
                .options(
                    mongodb::options::IndexOptions::builder()
                        .unique(true)
                        .build(),
                )
                .build(),
        )
        .await
        .map_err(|_| CleanupError::Unavailable)?;
        jobs.create_index(
            IndexModel::builder()
                .keys(doc! {"complete":1,"_id":1,"next_attempt_at_ms":1,"lease_expires_at_ms":1})
                .build(),
        )
        .await
        .map_err(|_| CleanupError::Unavailable)?;
        majority_collection::<Document>(&database, JOB_AUDITS)
            .create_index(
                IndexModel::builder()
                    .keys(doc! {"allocation_id":1})
                    .options(
                        mongodb::options::IndexOptions::builder()
                            .unique(true)
                            .build(),
                    )
                    .build(),
            )
            .await
            .map_err(|_| CleanupError::Unavailable)?;
        self.records
            .create_index(
                IndexModel::builder()
                    .keys(doc! {"bridge_id":1,"_id":1})
                    .build(),
            )
            .await
            .map_err(|_| CleanupError::Unavailable)?;
        Ok(())
    }

    pub(crate) async fn discover_cleanup_jobs(
        &self,
        after: Option<&str>,
    ) -> Result<Vec<String>, CleanupError> {
        use futures_util::TryStreamExt;
        let database = self.bridges.client.database(&self.records.namespace().db);
        let now = mongodb::bson::DateTime::now().timestamp_millis();
        let mut filter = doc! {"complete":false,"next_attempt_at_ms":{"$lte":now},"$or":[{"lease_expires_at_ms":null},{"lease_expires_at_ms":{"$lte":now}}]};
        // Invalid identities cannot be addressed by a worker and must not
        // starve later valid rows in the bounded ordered scan.
        filter.insert("_id", doc! {"$type":"string", "$gt":after.unwrap_or("")});
        let mut cursor = majority_collection::<Document>(&database, JOBS)
            .find(filter)
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
    pub(crate) async fn claim_cleanup_job(
        &self,
        id: &str,
        holder: &str,
    ) -> Result<(CleanupJob, AllocationRecord, ChannelReuseCleanupRequest), CleanupError> {
        self.change_cleanup_job(id, Change::Claim { holder }).await
    }
    /// Publish only already-proved effects after a lost scheduling update.
    /// Nonterminal jobs retain their claim so the worker can continue safely.
    pub(crate) async fn reconcile_cleanup_job_progress(
        &self,
        id: &str,
        claim: &CleanupJobClaim,
    ) -> Result<CleanupJob, CleanupError> {
        Ok(self
            .change_cleanup_job(id, Change::Reconcile { claim })
            .await?
            .0)
    }
    pub(crate) async fn observe_cleanup_job(
        &self,
        id: &str,
        claim: &CleanupJobClaim,
        progress: ChildCleanupProgress,
        holds: &[ChildCleanupHold],
    ) -> Result<CleanupJob, CleanupError> {
        Ok(self
            .change_cleanup_job(
                id,
                Change::Observe {
                    claim,
                    progress,
                    holds,
                },
            )
            .await?
            .0)
    }

    async fn change_cleanup_job(
        &self,
        id: &str,
        change: Change<'_>,
    ) -> Result<(CleanupJob, AllocationRecord, ChannelReuseCleanupRequest), CleanupError> {
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
            let result = match self
                .cleanup_job_scheduling_transaction(id, &change, &mut session)
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
    async fn cleanup_job_scheduling_transaction(
        &self,
        id: &str,
        change: &Change<'_>,
        session: &mut ClientSession,
    ) -> Result<(CleanupJob, AllocationRecord, ChannelReuseCleanupRequest), JobError> {
        let database = self.bridges.client.database(&self.records.namespace().db);
        let initial = read(&database, JOBS, id, session).await?;
        let initial_job = read_job(&initial)?;
        let allocation_row = read(
            &database,
            "bridge_allocations",
            &initial_job.admission.receipt.allocation_id,
            session,
        )
        .await?;
        let allocation = records::decode_record(&allocation_row)?;
        let mut job = self
            .require_cleanup_job(&database, &allocation, &allocation_row, session)
            .await?
            .ok_or(CleanupError::Corrupt)?;
        if job != initial_job || job.admission.receipt.id != id {
            return Err(CleanupError::Corrupt.into());
        }
        let saved = read_state(
            &read(
                &database,
                REQUESTS,
                &job.admission.cleanup_request_id,
                session,
            )
            .await?,
        )?;
        let now = mongodb::bson::DateTime::now().timestamp_millis();
        match change {
            Change::Reconcile { claim } => {
                job.require_claim(claim, now)?;
                let progress = self
                    .proved_cleanup_job_progress(&database, &saved, session)
                    .await?;
                job.reconcile_progress(claim, progress, now)?;
            }
            Change::Claim { holder } => job.claim(
                holder,
                now,
                now.checked_add(60_000).ok_or(CleanupError::Corrupt)?,
            )?,
            Change::Observe {
                claim,
                progress,
                holds,
            } => {
                if *progress != job.progress {
                    self.require_cleanup_job_progress(&database, &saved, *progress, session)
                        .await?;
                }
                job.observe(claim, *progress, holds.to_vec(), now)?;
            }
        }
        // Proof reads write fences; fetch the transaction's updated row for CAS.
        let previous = read(&database, JOBS, id, session).await?;
        if read_job(&previous)? != initial_job {
            return Err(CleanupError::Conflict.into());
        }
        let changed = majority_collection::<Document>(&database, JOBS)
            .update_one(previous, doc! {"$set":job_document(&job)?})
            .session(&mut *session)
            .await?;
        if changed.matched_count != 1 {
            return Err(CleanupError::Conflict.into());
        }
        Ok((job, allocation, saved.request))
    }

    async fn proved_cleanup_job_progress(
        &self,
        database: &Database,
        saved: &ChannelReuseCleanupState,
        session: &mut ClientSession,
    ) -> Result<ChildCleanupProgress, JobError> {
        let id = format!("{}:child-removal:v1", saved.id);
        let row = majority_collection::<Document>(database, CLEANUP_TOMBSTONES)
            .find_one(doc! {"_id":id})
            .session(&mut *session)
            .await?;
        let progress = if let Some(row) = row {
            match read_removal(&row)?.phase {
                ChannelReuseRemovalPhase::Intent => ChildCleanupProgress::RemovalStarted,
                ChannelReuseRemovalPhase::ProcessClosed => ChildCleanupProgress::ProcessClosed,
                ChannelReuseRemovalPhase::HomeRemoved => ChildCleanupProgress::HomeRemoved,
                ChannelReuseRemovalPhase::WorktreeRemoved => ChildCleanupProgress::WorktreeRemoved,
                ChannelReuseRemovalPhase::Complete => ChildCleanupProgress::Complete,
            }
        } else if majority_collection::<Document>(database, "channel_reuse_cleanup_admissions")
            .find_one(doc! {"_id":ChannelReuseCleanupAdmission::id(&saved.id)})
            .session(&mut *session)
            .await?
            .is_some()
        {
            ChildCleanupProgress::AdmissionClosed
        } else {
            ChildCleanupProgress::Requested
        };
        if progress != ChildCleanupProgress::Requested {
            self.require_cleanup_job_progress(database, saved, progress, session)
                .await?;
        }
        Ok(progress)
    }

    async fn require_cleanup_job_progress(
        &self,
        database: &Database,
        saved: &ChannelReuseCleanupState,
        progress: ChildCleanupProgress,
        session: &mut ClientSession,
    ) -> Result<(), JobError> {
        let closed = self
            .require_cleanup_admission(database, saved, false, session)
            .await?;
        if progress == ChildCleanupProgress::AdmissionClosed {
            return Ok(());
        }
        let id = format!("{}:child-removal:v1", saved.id);
        let row = read(database, CLEANUP_TOMBSTONES, &id, session).await?;
        let removal = read_removal(&row)?;
        if removal.request_sha256 != saved.digest
            || removal.admission_sha256 != closed.digest
            || removal.seal != saved.seal
        {
            return Err(CleanupError::Corrupt.into());
        }
        let expected = match removal.phase {
            ChannelReuseRemovalPhase::Intent => ChildCleanupProgress::RemovalStarted,
            ChannelReuseRemovalPhase::ProcessClosed => ChildCleanupProgress::ProcessClosed,
            ChannelReuseRemovalPhase::HomeRemoved => ChildCleanupProgress::HomeRemoved,
            ChannelReuseRemovalPhase::WorktreeRemoved => ChildCleanupProgress::WorktreeRemoved,
            ChannelReuseRemovalPhase::Complete => ChildCleanupProgress::Complete,
        };
        let audit = read(
            database,
            super::super::removal::REMOVAL_AUDITS,
            &super::super::removal::audit_id(&removal),
            session,
        )
        .await?;
        if expected != progress || read_removal(&audit)? != removal {
            return Err(CleanupError::Conflict.into());
        }
        if progress == ChildCleanupProgress::Complete {
            require_complete(database, &removal, session).await?;
        }
        fence_row(database, CLEANUP_TOMBSTONES, row, session).await?;
        Ok(())
    }
}
