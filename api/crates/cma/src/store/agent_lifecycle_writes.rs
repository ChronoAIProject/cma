//! Every worker mutation is conditional on its current, unexpired claim.
use super::*;
use mongodb::bson::Document;

impl LifecycleLease {
    fn filter(&self) -> Document {
        doc! { "_id": &self.id, "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "status": "running", "lease_token": &self.token,
        "$expr": { "$and": [
            { "$eq": [{ "$type": "$lease_expires_at" }, "date"] },
            { "$gt": ["$lease_expires_at", "$$NOW"] },
        ] } }
    }
}

impl MongoAgentLifecycleStore {
    /// Preserve all immutable input and admitted child IDs across transient
    /// dependency failures. Backoff is durable, bounded and claim-fenced.
    pub(crate) async fn retry(
        &self,
        lease: &LifecycleLease,
        attempts: u32,
    ) -> Result<(), StoreError> {
        let delay = Duration::from_secs((5_u64 << attempts.saturating_sub(1).min(4)).min(60));
        self.write(
            lease,
            vec![doc! { "$set": { "status": "queued",
            "retry_at": { "$add": ["$$NOW", delay.as_millis() as i64] },
            "lease_token": Bson::Null, "lease_expires_at": Bson::Null,
            "updated_at": "$$NOW" } }],
        )
        .await
    }

    async fn write(
        &self,
        lease: &LifecycleLease,
        update: impl Into<mongodb::options::UpdateModifications>,
    ) -> Result<(), StoreError> {
        let result = self
            .operations
            .update_one(lease.filter(), update)
            .await
            .map_err(driver)?;
        if result.matched_count != 1 {
            return Err(StoreError::RevisionConflict {
                expected: 0,
                actual: 1,
            });
        }
        Ok(())
    }

    pub(crate) async fn renew(&self, lease: &LifecycleLease) -> Result<(), StoreError> {
        self.write(
            lease,
            vec![doc! { "$set": {
                "lease_expires_at": { "$add": ["$$NOW", LEASE.as_millis() as i64] },
                "updated_at": "$$NOW",
            } }],
        )
        .await
    }

    #[cfg(test)]
    pub(crate) async fn append_event(
        &self,
        lease: &LifecycleLease,
        sequence: u64,
        phase: &str,
        status: &str,
        message: &str,
    ) -> Result<(), StoreError> {
        let event = LifecycleEvent {
            sequence,
            phase: phase.to_owned(),
            status: status.to_owned(),
            message: message.to_owned(),
            occurred_at: now_rfc3339(),
        };
        let mut filter = lease.filter();
        filter.insert("events.sequence", doc! { "$ne": sequence as i64 });
        let result = self.operations.update_one(filter, doc! {
            "$set": { "phase": phase, "updated_at": DateTime::now() },
            "$push": { "events": { "$each": [mongodb::bson::serialize_to_bson(&event)
                .map_err(|error| StoreError::Malformed(error.to_string()))?], "$slice": -MAX_EVENTS } },
        }).await.map_err(driver)?;
        // A replayed gate is harmless, but a lost lease must stop the worker.
        if result.matched_count == 0 {
            self.write(lease, doc! { "$set": { "updated_at": DateTime::now() } })
                .await?;
        }
        Ok(())
    }

    /// A leased worker records each observed phase transition once. Deriving
    /// the next sequence from retained events also supports operations admitted
    /// by older workers, whose four fixed event numbers had different timing.
    pub(crate) async fn record_progress(
        &self,
        lease: &LifecycleLease,
        phase: &str,
        status: &str,
        message: &str,
    ) -> Result<(), StoreError> {
        self.record_progress_many(lease, &[(phase, status, message)])
            .await
    }

    /// Adjacent observations share one read/write, never a provisioning effect.
    /// Keep the batch small and retain the same per-phase replay semantics.
    pub(crate) async fn record_progress_many(
        &self,
        lease: &LifecycleLease,
        observations: &[(&str, &str, &str)],
    ) -> Result<(), StoreError> {
        if observations.is_empty() || observations.len() > 2 {
            return Err(StoreError::Malformed(
                "lifecycle progress requires one or two adjacent observations".into(),
            ));
        }
        let current = self
            .operations
            .find_one(lease.filter())
            .await
            .map_err(driver)?
            .ok_or(StoreError::RevisionConflict {
                expected: 0,
                actual: 1,
            })?;
        let mut sequence = current
            .events
            .iter()
            .map(|event| event.sequence)
            .max()
            .unwrap_or(0);
        let mut appended: Vec<LifecycleEvent> = Vec::with_capacity(observations.len());
        for &(phase, status, message) in observations {
            if current.events.iter().chain(&appended).any(|event| {
                event.phase == phase && (event.status == status || event.status == "ready")
            }) {
                continue;
            }
            sequence = sequence
                .checked_add(1)
                .filter(|sequence| *sequence <= i64::MAX as u64)
                .ok_or_else(|| {
                    StoreError::Malformed("lifecycle event sequence exhausted".into())
                })?;
            appended.push(LifecycleEvent {
                sequence,
                phase: phase.into(),
                status: status.into(),
                message: message.into(),
                occurred_at: now_rfc3339(),
            });
        }
        let Some(last) = appended.last() else {
            return Ok(());
        };
        // Recheck lease expiry after the read. The event CAS protects all
        // observations if another writer changes history under the same token.
        let mut filter = lease.filter();
        filter.insert(
            "events",
            mongodb::bson::serialize_to_bson(&current.events)
                .map_err(|error| StoreError::Malformed(error.to_string()))?,
        );
        let events = mongodb::bson::serialize_to_bson(&appended)
            .map_err(|error| StoreError::Malformed(error.to_string()))?;
        let result = self
            .operations
            .update_one(
                filter,
                doc! {
                    "$set": { "phase": &last.phase, "updated_at": DateTime::now() },
                    "$push": { "events": { "$each": events, "$slice": -MAX_EVENTS } },
                },
            )
            .await
            .map_err(driver)?;
        if result.matched_count != 1 {
            return Err(StoreError::RevisionConflict {
                expected: 0,
                actual: 1,
            });
        }
        Ok(())
    }

    pub(crate) async fn mark_sandbox(
        &self,
        lease: &LifecycleLease,
        sandbox_id: &str,
    ) -> Result<(), StoreError> {
        self.write(
            lease,
            doc! { "$set": { "sandbox_id": sandbox_id, "updated_at": DateTime::now() } },
        )
        .await
    }

    /// Pin the original probe result before allocating or building profiles.
    pub(crate) async fn pin_repository_commit(
        &self,
        lease: &LifecycleLease,
        commit: &str,
    ) -> Result<(), StoreError> {
        let mut filter = lease.filter();
        filter.insert("sandbox_id", Bson::Null);
        filter.insert(
            "repository_commit",
            doc! { "$in": [Bson::Null, Bson::String(commit.into())] },
        );
        let result = self
            .operations
            .update_one(
                filter,
                doc! {
                    "$set": { "repository_commit": commit, "updated_at": DateTime::now() },
                },
            )
            .await
            .map_err(driver)?;
        if result.matched_count != 1 {
            return Err(StoreError::RevisionConflict {
                expected: 0,
                actual: 1,
            });
        }
        Ok(())
    }

    pub(crate) async fn mark_agent(
        &self,
        lease: &LifecycleLease,
        agent_id: &str,
    ) -> Result<(), StoreError> {
        self.write(
            lease,
            doc! { "$set": { "agent_id": agent_id, "updated_at": DateTime::now() } },
        )
        .await
    }

    pub(crate) async fn succeed(
        &self,
        lease: &LifecycleLease,
        agent_id: &str,
    ) -> Result<(), StoreError> {
        self.write(
            lease,
            doc! { "$set": { "status": "succeeded", "phase": "agent_startup",
            "agent_id": agent_id, "lease_token": Bson::Null, "lease_expires_at": Bson::Null,
            "updated_at": DateTime::now() } },
        )
        .await
    }

    pub(crate) async fn wait_for_connection(
        &self,
        lease: &LifecycleLease,
        services: Vec<String>,
        target: serde_json::Value,
        repository_access: Option<layer1_public::rest::RepositoryAccessRefusal>,
    ) -> Result<(), StoreError> {
        let connection = AgentLifecycleConnectionRequired {
            services,
            target,
            repository_access,
        };
        let value = mongodb::bson::serialize_to_bson(&connection)
            .map_err(|error| StoreError::Malformed(error.to_string()))?;
        self.write(lease, doc! { "$set": { "status": "waiting_for_connection", "connection": value,
            "lease_token": Bson::Null, "lease_expires_at": Bson::Null, "updated_at": DateTime::now() } }).await
    }

    /// Only an authenticated replay of the same owner/request can resume a
    /// connection wait. Child IDs and the pinned input survive that transition.
    pub(crate) async fn resume_connection(
        &self,
        owner: &SubjectId,
        id: &str,
    ) -> Result<(), StoreError> {
        self.operations.update_one(doc! { "_id": id, "owner": owner.as_str(),
            "document_schema_version": DOCUMENT_SCHEMA_VERSION, "status": "waiting_for_connection" },
            doc! { "$set": { "status": "queued", "connection": Bson::Null, "updated_at": DateTime::now() } })
            .await.map_err(driver)?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) async fn fail(&self, lease: &LifecycleLease, error: &str) -> Result<(), StoreError> {
        self.fail_with_metadata(lease, error, None).await
    }

    pub(crate) async fn fail_with_metadata(
        &self,
        lease: &LifecycleLease,
        error: &str,
        failure: Option<layer1_public::rest::AgentLifecycleFailure>,
    ) -> Result<(), StoreError> {
        let mut fields = doc! { "status": "failed", "phase": "failed", "error": error,
        "lease_token": Bson::Null, "lease_expires_at": Bson::Null, "updated_at": DateTime::now() };
        if let Some(failure) = failure {
            fields.insert(
                "failure",
                mongodb::bson::serialize_to_bson(&failure)
                    .map_err(|error| StoreError::Malformed(error.to_string()))?,
            );
        }
        self.write(lease, doc! {"$set": fields}).await
    }
}
