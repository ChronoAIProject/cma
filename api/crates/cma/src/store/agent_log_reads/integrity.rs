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
        validate_lineage(entry, &self.workspace_id, &self.creation_worktree_id)
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

/// Shared creation-lineage check for legacy and certified segmented readers.
pub(in crate::store) fn validate_lineage(
    entry: &AgentLogEntry,
    expected_workspace: &WorkspaceId,
    expected_worktree: &WorktreeId,
) -> Result<(), SandboxStoreError> {
    if let AgentLogKind::Created {
        workspace_id,
        worktree_id,
    } = &entry.kind
    {
        if entry.seq != 1 || workspace_id != expected_workspace || worktree_id != expected_worktree
        {
            return Err(SandboxStoreError::Corrupt(
                "Agent creation log lineage does not match".into(),
            ));
        }
    }
    Ok(())
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
        read_head(store, agent_id, session, None)
            .await
            .map(|view| view.map(|(head, _)| head))
    }
}

pub(in crate::store) struct CapturedIntegrity<'a>(pub &'a Agent);

#[async_trait::async_trait]
impl LogReadIntegrity for CapturedIntegrity<'_> {
    async fn read_head(
        &self,
        store: &MongoSandboxStore,
        agent_id: &AgentId,
        session: &mut ClientSession,
    ) -> Result<Option<ReadHead>, SandboxStoreError> {
        read_head(store, agent_id, session, Some(self.0))
            .await
            .map(|view| view.map(|(head, _)| head))
    }
}

pub(in crate::store) async fn read_response_head(
    store: &MongoSandboxStore,
    agent_id: &AgentId,
    session: &mut ClientSession,
) -> Result<(ReadHead, Agent), SandboxStoreError> {
    read_head(store, agent_id, session, None)
        .await?
        .ok_or(SandboxStoreError::NotFound)
}

async fn read_head(
    store: &MongoSandboxStore,
    agent_id: &AgentId,
    session: &mut ClientSession,
    expected: Option<&Agent>,
) -> Result<Option<(ReadHead, Agent)>, SandboxStoreError> {
    use mongodb::{bson::doc, options::ReadConcern};
    let row = store
        .agents
        .find_one(doc! {"_id":agent_id.as_str()})
        .read_concern(ReadConcern::snapshot())
        .session(&mut *session)
        .await
        .map_err(super::unavailable)?;
    let Some(row) = row else {
        return if expected.is_some() {
            Err(SandboxStoreError::NotFound)
        } else {
            Ok(None)
        };
    };
    let agent = crate::store::agents::read_agent(&row)?;
    if let Some(expected) = expected {
        // A later committed append is valid; a reused ID or moved lineage
        // cannot supply proof for the previously authorized projection.
        if agent.id != expected.id
            || agent.owner != expected.owner
            || agent.sandbox_id != expected.sandbox_id
            || agent.workspace_id != expected.workspace_id
            || agent.worktree_id != expected.worktree_id
            || agent.created_at != expected.created_at
            || agent.creation_worktree_id() != expected.creation_worktree_id()
            || agent.conversation.last_seq < expected.conversation.last_seq
        {
            return Err(SandboxStoreError::SequenceConflict);
        }
    }
    if crate::store::agent_log_segments::read_format(&row)?.is_some() {
        let segmented = crate::store::agent_log_segments::read_head(store, &row, session, true)
            .await
            .map_err(super::transaction_error)?;
        let mut head = ReadHead::from_agent(&agent, false);
        head.segmented = Some(segmented);
        return Ok(Some((head, agent)));
    }
    // Keep the current trusted legacy proof authoritative. Its queries use
    // this same snapshot and cannot initiate preparation or quarantine writes.
    let trusted =
        super::super::agent_log_integrity::read::legacy_proof(store, &agent, &row, session).await?;
    Ok(Some((ReadHead::from_agent(&agent, trusted), agent)))
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
