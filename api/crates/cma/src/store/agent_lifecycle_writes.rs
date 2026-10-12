//! Every worker mutation is conditional on its current, unexpired claim.
use super::*;
use mongodb::bson::Document;

impl LifecycleLease {
    fn filter(&self) -> Document {
        doc! { "_id": &self.id, "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "status": "running", "lease_token": &self.token,
        "lease_expires_at": { "$gt": DateTime::now() } }
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
        let retry_at = SystemTime::now()
            .checked_add(delay)
            .ok_or_else(|| StoreError::Malformed("lifecycle retry time overflowed".into()))?;
        self.write(
            lease,
            doc! { "$set": { "status": "queued",
            "retry_at": DateTime::from_system_time(retry_at),
            "lease_token": Bson::Null, "lease_expires_at": Bson::Null,
            "updated_at": DateTime::now() } },
        )
        .await
    }

    async fn write(&self, lease: &LifecycleLease, update: Document) -> Result<(), StoreError> {
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
        let deadline = SystemTime::now()
            .checked_add(LEASE)
            .ok_or_else(|| StoreError::Malformed("lifecycle lease overflowed".into()))?;
        self.write(lease, doc! { "$set": { "lease_expires_at": DateTime::from_system_time(deadline), "updated_at": DateTime::now() } }).await
    }

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
            doc! { "$set": { "status": "succeeded", "phase": "key_scope_ready",
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
    ) -> Result<(), StoreError> {
        let connection = AgentLifecycleConnectionRequired { services, target };
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

    pub(crate) async fn fail(&self, lease: &LifecycleLease, error: &str) -> Result<(), StoreError> {
        self.write(lease, doc! { "$set": { "status": "failed", "phase": "failed", "error": error,
            "lease_token": Bson::Null, "lease_expires_at": Bson::Null, "updated_at": DateTime::now() } }).await
    }
}
