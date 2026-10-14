//! Mongo work discovery and fenced lease claims for agents.

use futures_util::TryStreamExt as _;
use layer2_core::ids::AgentId;
use layer2_core::sandbox::store::{
    apply_agent_connection_recovery, AgentConnectionRecoveryApplication,
    AgentConnectionRecoveryCandidate, AgentConnectionRecoveryOutcome,
    AgentConnectionRecoveryRequest, AgentStore, AgentWorkMutation, AgentWorkerStore, Page,
    PageRequest, SandboxStoreError, MAX_PAGE_SIZE,
};
use mongodb::bson::{doc, Bson};

use super::child_writer_claims::{persist, ChildWriterClaim};
use super::sandbox_mongo::{MongoSandboxStore, DOCUMENT_SCHEMA_VERSION, TRANSACTION_ATTEMPTS};

#[async_trait::async_trait]
impl AgentWorkerStore for MongoSandboxStore {
    async fn prepared_agent_replay_matches(
        &self,
        agent: &layer2_core::sandbox::Agent,
        sandbox: &layer2_core::sandbox::Sandbox,
        workspace: &layer2_core::sandbox::Workspace,
        worktree: &layer2_core::sandbox::Worktree,
    ) -> Result<bool, SandboxStoreError> {
        if agent.work.warm_conversation_id.is_none() {
            return Ok(false);
        }
        let mut session = self
            .client
            .start_session()
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
        Ok(super::native_prepared::replay_proof_revision(
            self,
            agent,
            sandbox,
            workspace,
            worktree,
            &mut session,
        )
        .await?
        .is_some())
    }

    async fn commit_prepared_agent_open_ready(
        &self,
        mutation: AgentWorkMutation,
        observed: layer2_core::sandbox::store::PreparedAgentOpenContext,
    ) -> Result<bool, SandboxStoreError> {
        super::agent_worker_transaction::commit_prepared_agent_open_ready(self, mutation, observed)
            .await
    }

    async fn live_agent_connections(
        &self,
        page: &PageRequest,
    ) -> Result<Page<AgentConnectionRecoveryCandidate>, SandboxStoreError> {
        if page.limit == 0 || page.limit > MAX_PAGE_SIZE {
            return Err(SandboxStoreError::InvalidPage);
        }
        let mut filter = doc! {
            "document_schema_version": DOCUMENT_SCHEMA_VERSION,
            "status": { "$in": super::agents::LIVE_STATUSES },
            "record.conversation.conversation_id": { "$type": "string", "$ne": "" },
            "record.conversation.runtime_generation": { "$gt": 0_i64 },
        };
        if let Some(cursor) = page.cursor.as_ref() {
            filter.insert("_id", doc! { "$gt": cursor });
        }
        let mut cursor = self
            .agents
            .find(filter)
            .sort(doc! { "_id": 1 })
            .limit(i64::from(page.limit) + 1)
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
        let mut items = Vec::new();
        while let Some(document) = cursor
            .try_next()
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
        {
            let agent = super::agents::read_agent(&document)?;
            let candidate =
                AgentConnectionRecoveryCandidate::from_agent(&agent).ok_or_else(|| {
                    SandboxStoreError::Corrupt(
                        "agent connection query returned an invalid candidate".into(),
                    )
                })?;
            items.push(candidate);
        }
        let has_more = items.len() > usize::from(page.limit);
        if has_more {
            items.pop();
        }
        let next = has_more.then(|| {
            items
                .last()
                .expect("a bounded connection page has a last item")
                .agent_id
                .to_string()
        });
        Ok(Page { items, next })
    }

    async fn schedule_agent_connection_recovery(
        &self,
        request: AgentConnectionRecoveryRequest,
    ) -> Result<AgentConnectionRecoveryOutcome, SandboxStoreError> {
        let current = self
            .get_agent(&request.agent_id)
            .await?
            .ok_or(SandboxStoreError::NotFound)?;
        match apply_agent_connection_recovery(&current, &request)? {
            AgentConnectionRecoveryApplication::Complete(outcome) => Ok(outcome),
            AgentConnectionRecoveryApplication::Persist(agent) => {
                match self.save_agent(current.revision, *agent).await {
                    Ok(()) => Ok(AgentConnectionRecoveryOutcome::Scheduled),
                    Err(SandboxStoreError::Conflict) => {
                        let current = self
                            .get_agent(&request.agent_id)
                            .await?
                            .ok_or(SandboxStoreError::NotFound)?;
                        match apply_agent_connection_recovery(&current, &request)? {
                            AgentConnectionRecoveryApplication::Complete(outcome) => Ok(outcome),
                            AgentConnectionRecoveryApplication::Persist(_) => {
                                Ok(AgentConnectionRecoveryOutcome::Superseded)
                            }
                        }
                    }
                    Err(error) => Err(error),
                }
            }
        }
    }

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
            match persist(
                self,
                expected,
                ChildWriterClaim::Agent(Box::new(agent.clone())),
            )
            .await
            {
                Ok(true) => return Ok(Some(agent)),
                Ok(false) => return Ok(None),
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
