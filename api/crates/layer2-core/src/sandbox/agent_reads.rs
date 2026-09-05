use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::ids::{AgentId, SandboxId, WorkspaceId};
use crate::{Caller, Capability, SubjectId};

use super::agent_reads_fold::{fold_agent_log, AgentLogProjection};
use super::store::{AgentReadStore, Page, PageRequest, SandboxStoreError, MAX_PAGE_SIZE};
use super::{Agent, AgentLogEntry, AgentStatus, Sandbox, TypedFailure};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentListQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox_id: Option<SandboxId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<WorkspaceId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<AgentStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentTurnOutcome {
    Active,
    Completed { at: String },
    Failed { at: String, failure: TypedFailure },
    Interrupted { at: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentTranscriptTurn {
    pub turn_sequence: u64,
    pub started_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_key: Option<String>,
    pub owner_message: String,
    pub agent_messages: Vec<String>,
    pub outcome: AgentTurnOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentTranscript {
    pub agent_id: AgentId,
    pub through_seq: u64,
    pub turns: Vec<AgentTranscriptTurn>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub context_used: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentUsageReading {
    pub agent_id: AgentId,
    pub through_seq: u64,
    pub usage: AgentUsage,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxResourceReading {
    pub uptime_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_usage_usec: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_millis_limit: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_bytes_used: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_bytes_limit: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_bytes_peak: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stall_permille: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ceiling_hits: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allocation_failures: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kills: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub received_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentResourceReading {
    pub agent_id: AgentId,
    pub sandbox_id: SandboxId,
    pub reading: SandboxResourceReading,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SandboxResourceReadError {
    #[error("sandbox does not support resource readings: {0}")]
    Unsupported(String),
    #[error("sandbox resource binding is corrupt: {0}")]
    Corrupt(String),
    #[error("sandbox resource reader is unavailable: {0}")]
    Unavailable(String),
}

#[async_trait]
pub trait SandboxResourceReader: Send + Sync {
    async fn sandbox_resources(
        &self,
        sandbox: &Sandbox,
    ) -> Result<SandboxResourceReading, SandboxResourceReadError>;
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AgentReadError {
    #[error("forbidden")]
    Forbidden,
    #[error("agent not found")]
    NotFound,
    #[error("invalid page request")]
    InvalidPage,
    #[error("agent sandbox does not support resource readings: {0}")]
    ResourcesUnsupported(String),
    #[error("agent data is not usable: {0}")]
    Corrupt(String),
    #[error("agent read dependency is unavailable: {0}")]
    Unavailable(String),
}

impl AgentReadError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Forbidden => "forbidden",
            Self::NotFound => "agent_not_found",
            Self::InvalidPage => "invalid_page",
            Self::ResourcesUnsupported(_) => "agent_resources_unsupported",
            Self::Corrupt(_) => "stored_agent_corrupt",
            Self::Unavailable(_) => "agent_read_unavailable",
        }
    }
}

pub struct AgentReadService {
    store: Arc<dyn AgentReadStore>,
    resources: Arc<dyn SandboxResourceReader>,
}

impl AgentReadService {
    pub fn new<S, R>(store: Arc<S>, resources: Arc<R>) -> Arc<Self>
    where
        S: AgentReadStore + 'static,
        R: SandboxResourceReader + 'static,
    {
        Arc::new(Self { store, resources })
    }

    pub async fn list(
        &self,
        caller: &Caller,
        query: &AgentListQuery,
        page: &PageRequest,
    ) -> Result<Page<Agent>, AgentReadError> {
        require(caller)?;
        let result = self
            .store
            .list_owned_agents(&caller.subject, query, page)
            .await
            .map_err(store_error)?;
        self.validate_page(&caller.subject, query, page, &result)
            .await?;
        Ok(result)
    }

    pub async fn read(&self, caller: &Caller, id: &AgentId) -> Result<Agent, AgentReadError> {
        require(caller)?;
        Ok(self.owned_lineage(caller, id).await?.agent)
    }

    pub async fn log(
        &self,
        caller: &Caller,
        id: &AgentId,
        after: u64,
        limit: u16,
    ) -> Result<Vec<AgentLogEntry>, AgentReadError> {
        require(caller)?;
        let lineage = self.owned_lineage(caller, id).await?;
        validate_limit(limit)?;
        let entries = self.complete_log(&lineage.agent).await?;
        fold_agent_log(&lineage.agent, &entries).map_err(AgentReadError::Corrupt)?;
        Ok(entries
            .into_iter()
            .filter(|entry| entry.seq > after)
            .take(usize::from(limit))
            .collect())
    }

    pub async fn transcript(
        &self,
        caller: &Caller,
        id: &AgentId,
    ) -> Result<AgentTranscript, AgentReadError> {
        let projection = self.projection(caller, id).await?;
        Ok(projection.transcript)
    }

    pub async fn usage(
        &self,
        caller: &Caller,
        id: &AgentId,
    ) -> Result<AgentUsageReading, AgentReadError> {
        let projection = self.projection(caller, id).await?;
        Ok(projection.usage)
    }

    pub async fn resources(
        &self,
        caller: &Caller,
        id: &AgentId,
    ) -> Result<AgentResourceReading, AgentReadError> {
        require(caller)?;
        let lineage = self.owned_lineage(caller, id).await?;
        let reading = self
            .resources
            .sandbox_resources(&lineage.sandbox)
            .await
            .map_err(resource_error)?;
        Ok(AgentResourceReading {
            agent_id: lineage.agent.id,
            sandbox_id: lineage.sandbox.id,
            reading,
        })
    }

    async fn projection(
        &self,
        caller: &Caller,
        id: &AgentId,
    ) -> Result<AgentLogProjection, AgentReadError> {
        require(caller)?;
        let lineage = self.owned_lineage(caller, id).await?;
        let entries = self.complete_log(&lineage.agent).await?;
        fold_agent_log(&lineage.agent, &entries).map_err(AgentReadError::Corrupt)
    }

    async fn owned_lineage(
        &self,
        caller: &Caller,
        id: &AgentId,
    ) -> Result<OwnedAgentLineage, AgentReadError> {
        let agent = self
            .store
            .get_agent(id)
            .await
            .map_err(store_error)?
            .filter(|agent| agent.id == *id && agent.owner == caller.subject)
            .ok_or(AgentReadError::NotFound)?;
        let sandbox = self.validate_lineage(&caller.subject, &agent).await?;
        Ok(OwnedAgentLineage { agent, sandbox })
    }

    async fn validate_lineage(
        &self,
        owner: &SubjectId,
        agent: &Agent,
    ) -> Result<Sandbox, AgentReadError> {
        agent
            .validate_conversation()
            .map_err(|error| AgentReadError::Corrupt(error.to_string()))?;
        let worktree = self
            .store
            .get_worktree(&agent.worktree_id)
            .await
            .map_err(lineage_store_error)?
            .ok_or_else(|| corrupt("agent worktree is missing"))?;
        if worktree.id != agent.worktree_id
            || worktree.workspace_id != agent.workspace_id
            || worktree.sandbox_id != agent.sandbox_id
            || worktree.owner != *owner
            || worktree.owner != agent.owner
            || worktree.agent_id.as_ref() != Some(&agent.id)
        {
            return Err(corrupt("agent worktree lineage does not match"));
        }
        let workspace = self
            .store
            .get_workspace(&agent.workspace_id)
            .await
            .map_err(lineage_store_error)?
            .ok_or_else(|| corrupt("agent workspace is missing"))?;
        if workspace.id != agent.workspace_id
            || workspace.id != worktree.workspace_id
            || workspace.sandbox_id != agent.sandbox_id
            || workspace.sandbox_id != worktree.sandbox_id
            || workspace.owner != *owner
            || workspace.owner != agent.owner
        {
            return Err(corrupt("agent workspace lineage does not match"));
        }
        let sandbox = self
            .store
            .get_sandbox(&agent.sandbox_id)
            .await
            .map_err(lineage_store_error)?
            .ok_or_else(|| corrupt("agent sandbox is missing"))?;
        if sandbox.id != agent.sandbox_id
            || sandbox.id != workspace.sandbox_id
            || sandbox.owner != *owner
            || sandbox.owner != agent.owner
        {
            return Err(corrupt("agent sandbox lineage does not match"));
        }
        Ok(sandbox)
    }

    async fn validate_page(
        &self,
        owner: &SubjectId,
        query: &AgentListQuery,
        request: &PageRequest,
        result: &Page<Agent>,
    ) -> Result<(), AgentReadError> {
        validate_limit(request.limit)?;
        if result.items.len() > usize::from(request.limit) {
            return Err(corrupt("agent page exceeds its requested bound"));
        }
        if result.next.is_some() && result.items.len() != usize::from(request.limit) {
            return Err(corrupt("agent page cursor precedes its requested bound"));
        }
        let mut previous = request.cursor.as_deref();
        for agent in &result.items {
            if agent.owner != *owner || !query.matches(agent) {
                return Err(corrupt("agent page contains an out-of-scope row"));
            }
            if previous.is_some_and(|cursor| agent.id.as_str() <= cursor) {
                return Err(corrupt("agent page order is invalid"));
            }
            self.validate_lineage(owner, agent).await?;
            previous = Some(agent.id.as_str());
        }
        if result.next.as_deref()
            != result
                .next
                .as_ref()
                .and_then(|_| result.items.last().map(|agent| agent.id.as_str()))
        {
            return Err(corrupt("agent page cursor is invalid"));
        }
        Ok(())
    }

    async fn complete_log(&self, agent: &Agent) -> Result<Vec<AgentLogEntry>, AgentReadError> {
        let through_seq = agent.conversation.last_seq;
        if through_seq == 0 {
            return Err(corrupt("agent log has no creation record"));
        }
        let mut entries = Vec::new();
        let mut after = 0;
        while after < through_seq {
            let remaining = through_seq - after;
            let limit = remaining.min(u64::from(MAX_PAGE_SIZE)) as u16;
            let batch = self
                .store
                .agent_log(&agent.id, after, limit)
                .await
                .map_err(log_store_error)?;
            if batch.len() != usize::from(limit) {
                return Err(corrupt("agent log ended before its high-water mark"));
            }
            for entry in &batch {
                let expected = after
                    .checked_add(1)
                    .ok_or_else(|| corrupt("agent log sequence overflowed"))?;
                if entry.agent_id != agent.id || entry.seq != expected || entry.seq > through_seq {
                    return Err(corrupt("agent log identity or sequence is invalid"));
                }
                after = entry.seq;
            }
            entries.extend(batch);
        }
        self.reject_uncommitted_tail(agent, through_seq).await?;
        Ok(entries)
    }

    async fn reject_uncommitted_tail(
        &self,
        agent: &Agent,
        through_seq: u64,
    ) -> Result<(), AgentReadError> {
        if through_seq == u64::MAX {
            return Ok(());
        }
        let tail = self
            .store
            .agent_log(&agent.id, through_seq, 1)
            .await
            .map_err(log_store_error)?;
        let Some(extra) = tail.first() else {
            return Ok(());
        };
        if tail.len() != 1
            || extra.agent_id != agent.id
            || extra.seq != through_seq.saturating_add(1)
        {
            return Err(corrupt("agent log has an invalid tail"));
        }
        let current = self
            .store
            .get_agent(&agent.id)
            .await
            .map_err(log_store_error)?
            .ok_or_else(|| corrupt("agent disappeared while its log was read"))?;
        if current.id != agent.id
            || current.owner != agent.owner
            || current.worktree_id != agent.worktree_id
            || current.workspace_id != agent.workspace_id
            || current.sandbox_id != agent.sandbox_id
            || current.conversation.last_seq < extra.seq
        {
            return Err(corrupt("agent log extends past its aggregate"));
        }
        Ok(())
    }
}

struct OwnedAgentLineage {
    agent: Agent,
    sandbox: Sandbox,
}

fn validate_limit(limit: u16) -> Result<(), AgentReadError> {
    (limit > 0 && limit <= MAX_PAGE_SIZE)
        .then_some(())
        .ok_or(AgentReadError::InvalidPage)
}

fn require(caller: &Caller) -> Result<(), AgentReadError> {
    caller
        .authorization
        .allows(Capability::UseOwnSandboxes)
        .then_some(())
        .ok_or(AgentReadError::Forbidden)
}

fn corrupt(detail: &'static str) -> AgentReadError {
    AgentReadError::Corrupt(detail.into())
}

fn store_error(error: SandboxStoreError) -> AgentReadError {
    match error {
        SandboxStoreError::NotFound => AgentReadError::NotFound,
        SandboxStoreError::InvalidPage => AgentReadError::InvalidPage,
        SandboxStoreError::Corrupt(detail) => AgentReadError::Corrupt(detail),
        SandboxStoreError::InvalidTransaction(detail) => AgentReadError::Corrupt(detail.into()),
        SandboxStoreError::SequenceConflict => {
            corrupt("agent store reported an inconsistent sequence")
        }
        other => AgentReadError::Unavailable(other.to_string()),
    }
}

fn lineage_store_error(error: SandboxStoreError) -> AgentReadError {
    match error {
        SandboxStoreError::NotFound => corrupt("agent lineage lookup was inconsistent"),
        other => store_error(other),
    }
}

fn log_store_error(error: SandboxStoreError) -> AgentReadError {
    match error {
        SandboxStoreError::NotFound | SandboxStoreError::SequenceConflict => {
            corrupt("agent log is incomplete or inconsistent")
        }
        other => store_error(other),
    }
}

fn resource_error(error: SandboxResourceReadError) -> AgentReadError {
    match error {
        SandboxResourceReadError::Unsupported(detail) => {
            AgentReadError::ResourcesUnsupported(detail)
        }
        SandboxResourceReadError::Corrupt(detail) => AgentReadError::Corrupt(detail),
        SandboxResourceReadError::Unavailable(detail) => AgentReadError::Unavailable(detail),
    }
}
