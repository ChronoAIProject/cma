//! Mongo work discovery and fenced lease claims for agents.

use futures_util::TryStreamExt as _;
use layer2_core::ids::AgentId;
use layer2_core::sandbox::store::{
    AgentStore, AgentWorkMutation, AgentWorkerStore, SandboxStoreError, MAX_PAGE_SIZE,
};
use mongodb::bson::{doc, Bson};

use super::sandbox_mongo::{MongoSandboxStore, DOCUMENT_SCHEMA_VERSION, TRANSACTION_ATTEMPTS};

#[async_trait::async_trait]
impl AgentWorkerStore for MongoSandboxStore {
    async fn due_agent_work(
        &self,
        now: &str,
        limit: u16,
    ) -> Result<Vec<AgentId>, SandboxStoreError> {
        if limit == 0 || limit > MAX_PAGE_SIZE {
            return Err(SandboxStoreError::InvalidPage);
        }
        let mut cursor = self
            .agents
            .find(doc! {
                "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                "work_pending": true,
                "work_ready_at": { "$lte": now },
                "$or": [
                    { "work_lease_expires_at": Bson::Null },
                    { "work_lease_expires_at": { "$lte": now } },
                ],
            })
            .projection(doc! { "_id": 1 })
            .sort(doc! { "_id": 1 })
            .limit(i64::from(limit))
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
        let mut ids = Vec::new();
        while let Some(document) = cursor
            .try_next()
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
        {
            ids.push(AgentId(
                document
                    .get_str("_id")
                    .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?
                    .to_string(),
            ));
        }
        Ok(ids)
    }

    async fn claim_agent_work(
        &self,
        id: &AgentId,
        holder: &str,
        now: String,
        lease_expires_at: String,
    ) -> Result<Option<layer2_core::sandbox::Agent>, SandboxStoreError> {
        for _ in 0..TRANSACTION_ATTEMPTS {
            let Some(mut agent) = self.get_agent(id).await? else {
                return Ok(None);
            };
            if !agent.work.is_due(&now) {
                return Ok(None);
            }
            let expected = agent.revision;
            agent
                .work
                .claim(holder.to_string(), &now, lease_expires_at.clone())
                .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
            agent.revision = expected.checked_add(1).ok_or(SandboxStoreError::Conflict)?;
            match self.save_agent(expected, agent.clone()).await {
                Ok(()) => return Ok(Some(agent)),
                Err(SandboxStoreError::Conflict) => tokio::task::yield_now().await,
                Err(error) => return Err(error),
            }
        }
        Err(SandboxStoreError::Conflict)
    }

    async fn commit_agent_work(
        &self,
        mutation: AgentWorkMutation,
    ) -> Result<(), SandboxStoreError> {
        super::agent_worker_transaction::commit_agent_work(self, mutation).await
    }
}
