//! Reuse #1591's proof and retain its Agent lineage for read-only fallback.
use layer2_core::{
    ids::{AgentId, WorkspaceId, WorktreeId},
    sandbox::{store::SandboxStoreError, Agent, AgentLogEntry, AgentLogKind},
};
use mongodb::ClientSession;

use crate::store::sandbox_mongo::MongoSandboxStore;

pub(in crate::store) struct ReadHead {
    pub(in crate::store) last_seq: u64,
    /// Requires the shared prepared-cohort proof, never metadata presence alone.
    pub(in crate::store) trusted: bool,
    pub(in crate::store) segmented: Option<crate::store::agent_log_segments::Head>,
    pub(super) workspace_id: WorkspaceId,
    pub(super) creation_worktree_id: WorktreeId,
}

impl ReadHead {
    pub(in crate::store) fn validate_lineage(
        &self,
        entry: &AgentLogEntry,
    ) -> Result<(), SandboxStoreError> {
        if let AgentLogKind::Created {
            workspace_id,
            worktree_id,
        } = &entry.kind
        {
            if entry.seq != 1
                || workspace_id != &self.workspace_id
                || worktree_id != &self.creation_worktree_id
            {
                return Err(SandboxStoreError::Corrupt(
                    "Agent creation log lineage does not match".into(),
                ));
            }
        }
        Ok(())
    }

    fn from_agent(agent: &Agent, trusted: bool) -> Self {
        Self {
            last_seq: agent.conversation.last_seq,
            trusted,
            segmented: None,
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
        use mongodb::{bson::doc, options::ReadConcern};
        let row = store
            .agents
            .find_one(doc! {"_id":agent_id.as_str()})
            .read_concern(ReadConcern::snapshot())
            .session(&mut *session)
            .await
            .map_err(super::unavailable)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let agent = crate::store::agents::read_agent(&row)?;
        if crate::store::agent_log_segments::read_format(&row)?.is_some() {
            let segmented = crate::store::agent_log_segments::read_head(store, &row, session, true)
                .await
                .map_err(super::transaction_error)?;
            let mut head = ReadHead::from_agent(&agent, false);
            head.segmented = Some(segmented);
            return Ok(Some(head));
        }
        // Keep the current trusted legacy proof authoritative. Its queries use
        // this same snapshot and cannot initiate preparation or quarantine writes.
        let trusted =
            super::super::agent_log_integrity::read::legacy_proof(store, &agent, &row, session)
                .await?;
        Ok(Some(ReadHead::from_agent(&agent, trusted)))
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
