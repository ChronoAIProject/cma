//! Lease insertion and its source Agent fence commit together.
use super::*;
use crate::store::sandbox_mongo::{
    commit_transaction, is_duplicate_key, is_transient, start_transaction, CommitOutcome,
    TRANSACTION_ATTEMPTS,
};
use crate::store::transaction_terminal::GuardedTerminal as _;
use mongodb::ClientSession;

enum Error {
    Read(AgentReadError),
    Driver(mongodb::error::Error),
}
impl From<mongodb::error::Error> for Error {
    fn from(error: mongodb::error::Error) -> Self {
        Self::Driver(error)
    }
}

#[cfg(test)]
#[path = "acquisition_tests.rs"]
mod tests;

impl MongoAgentObservation {
    pub(super) async fn acquire(
        &self,
        id: &AgentId,
        created_at: &str,
        instance: &str,
    ) -> Result<Document, AgentReadError> {
        for attempt in 0..TRANSACTION_ATTEMPTS {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(10 << (attempt - 1))).await;
                // A concurrent reader may already have committed the lease.
                // Reusing it requires no source write or native ownership.
                if let Some(row) = self
                    .leases
                    .find_one(doc! {
                        "_id": id.as_str(), "$expr":{"$gt":["$expires_at","$$NOW"]},
                    })
                    .await
                    .map_err(unavailable)?
                {
                    return Ok(row);
                }
            }
            let mut session = self
                .source
                .client
                .start_session()
                .await
                .map_err(unavailable)?;
            start_transaction(&mut session).await.map_err(unavailable)?;
            let row = match self
                .acquire_in_session(id, created_at, instance, &mut session)
                .await
            {
                Ok(row) => row,
                Err(error) => {
                    if session.abort_guarded().await.is_err() {
                        tracing::warn!("observation acquisition abort was not acknowledged");
                    }
                    match error {
                        Error::Driver(error)
                            if is_transient(&error) || is_duplicate_key(&error) =>
                        {
                            continue
                        }
                        Error::Driver(error) => return Err(unavailable(error)),
                        Error::Read(error) => return Err(error),
                    }
                }
            };
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(row),
                CommitOutcome::Retry(_) => continue,
                CommitOutcome::Unknown(error) | CommitOutcome::Failed(error) => {
                    return Err(unavailable(error));
                }
            }
        }
        Err(unavailable("observation acquisition contention"))
    }

    async fn acquire_in_session(
        &self,
        id: &AgentId,
        created_at: &str,
        instance: &str,
        session: &mut ClientSession,
    ) -> Result<Document, Error> {
        // Cleanup removes the Agent before its observer. This write conflicts
        // with source deletion, so a transaction with an old snapshot cannot
        // commit a new observer after cleanup has passed it.
        let source = self
            .source
            .agents
            .find_one_and_update(
                doc! {"_id": id.as_str(), "record.created_at": created_at},
                doc! {"$inc":{"resource_share_guard":1_i64}},
            )
            .session(&mut *session)
            .await?
            .ok_or_else(|| Error::Read(unavailable("observation source absent or replaced")))?;
        crate::store::agents::read_agent(&source)
            .map_err(|error| Error::Read(unavailable(error)))?;
        self.leases.update_one(doc! {"_id": id.as_str()}, doc! {"$setOnInsert": {
            "document_schema_version": 1_i64, "created_at": created_at, "instance_id": instance,
            "epoch": 0_i64, "expires_at": mongodb::bson::DateTime::from_millis(0),
            "next_refresh_at": mongodb::bson::DateTime::from_millis(0), "round": ""
        }}).upsert(true).session(&mut *session).await?;
        let row = self.leases.find_one_and_update(
            doc! {"_id": id.as_str(), "document_schema_version": 1_i64, "epoch": {"$gte": 0_i64, "$lt": i64::MAX}, "$expr": {"$lte": ["$expires_at", "$$NOW"]}},
            vec![doc! {"$set": {
                "document_schema_version": 1_i64,
                "created_at": created_at,
                "instance_id": instance,
                "epoch": {"$add": [{"$ifNull": ["$epoch", 0_i64]}, 1_i64]},
                "expires_at": {"$add": ["$$NOW", LEASE_MS]},
                "next_refresh_at": {"$max": [{"$ifNull": ["$next_refresh_at", "$$NOW"]}, {"$add": ["$$NOW", 1000_i64]}]},
                "round": "",
            }}],
        ).return_document(mongodb::options::ReturnDocument::After).session(&mut *session).await?;
        match row {
            Some(row) => Ok(row),
            None => self
                .leases
                .find_one(doc! {"_id": id.as_str()})
                .session(session)
                .await?
                .ok_or_else(|| Error::Read(unavailable("observation acquisition absent"))),
        }
    }
}
