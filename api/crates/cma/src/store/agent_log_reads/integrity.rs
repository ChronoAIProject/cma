//! Reuse #1591's proof and retain its Agent lineage for read-only fallback.
use layer2_core::{
    ids::{AgentId, WorkspaceId, WorktreeId},
    sandbox::{store::SandboxStoreError, Agent},
};
use mongodb::ClientSession;

use crate::store::sandbox_mongo::MongoSandboxStore;

pub(in crate::store) struct ReadHead {
    pub(in crate::store) last_seq: u64,
    /// Requires the shared prepared-cohort proof, never metadata presence alone.
    pub(in crate::store) trusted: bool,
    pub(super) workspace_id: WorkspaceId,
    pub(super) creation_worktree_id: WorktreeId,
}

impl ReadHead {
    fn from_agent(agent: &Agent, trusted: bool) -> Self {
        Self {
            last_seq: agent.conversation.last_seq,
            trusted,
            workspace_id: agent.workspace_id.clone(),
            creation_worktree_id: agent.creation_worktree_id().clone(),
        }
    }
}

#[async_trait::async_trait]
pub(in crate::store) trait LogReadIntegrity: Send + Sync {
    /// Keep every query in the supplied snapshot. Never prepare, repair,
    /// quarantine or operate a transaction from the public read.
    async fn read_head(
        &self,
        store: &MongoSandboxStore,
        agent_id: &AgentId,
        session: &mut ClientSession,
    ) -> Result<Option<ReadHead>, SandboxStoreError>;
}

pub(in crate::store) struct PreparedIntegrity;

#[async_trait::async_trait]
impl LogReadIntegrity for PreparedIntegrity {
    async fn read_head(
        &self,
        store: &MongoSandboxStore,
        agent_id: &AgentId,
        session: &mut ClientSession,
    ) -> Result<Option<ReadHead>, SandboxStoreError> {
        Ok(
            super::super::agent_log_integrity::read::read_proven_agent(store, agent_id, session)
                .await?
                .map(|(agent, trusted)| ReadHead::from_agent(&agent, trusted)),
        )
    }
}

/// Baseline for paired fixtures; production always validates shared metadata.
#[cfg(test)]
pub(in crate::store) struct LegacyIntegrity;

#[cfg(test)]
#[async_trait::async_trait]
impl LogReadIntegrity for LegacyIntegrity {
    async fn read_head(
        &self,
        store: &MongoSandboxStore,
        agent_id: &AgentId,
        session: &mut ClientSession,
    ) -> Result<Option<ReadHead>, SandboxStoreError> {
        let row = store
            .agents
            .find_one(mongodb::bson::doc! { "_id": agent_id.as_str() })
            .read_concern(mongodb::options::ReadConcern::snapshot())
            .session(session)
            .await
            .map_err(super::unavailable)?;
        row.as_ref()
            .map(|row| {
                super::super::agents::read_agent(row)
                    .map(|agent| ReadHead::from_agent(&agent, false))
            })
            .transpose()
    }
}
