use std::sync::Arc;

use crate::ids::AgentId;
use crate::resource_access::{
    AccessDecision, ResourceAccessService, ResourceCapability, ResourceTarget,
};
use crate::{Caller, Capability, SubjectId};

use super::agent_reads_fold::{fold_agent_log, AgentLogProjection};
use super::store::{AgentReadStore, Page, PageRequest, SandboxStoreError, MAX_PAGE_SIZE};
use super::{
    Agent, AgentLineage, AgentListQuery, AgentLogEntry, AgentReadError, AgentResourceReading,
    AgentTranscript, AgentUsageReading, Sandbox, SandboxResourceReadError, SandboxResourceReader,
    Workspace, Worktree,
};

pub struct AgentReadService {
    store: Arc<dyn AgentReadStore>,
    resources: Arc<dyn SandboxResourceReader>,
    access: Option<Arc<ResourceAccessService>>,
}

#[path = "agent_reads/authorized.rs"]
mod authorized;
pub use authorized::{AgentDetailReading, SharedAgentDetail};
#[path = "agent_reads/configuration.rs"]
mod configuration;
pub use configuration::AgentConfigurationRead;
#[path = "agent_reads/source.rs"]
mod source;
pub use source::{AgentSourceReadError, AgentSourceReading, SharedAgentOrigin};
#[path = "agent_reads/visibility.rs"]
mod visibility;
pub use visibility::AgentVisibleLogEntry;
#[path = "agent_reads/stream.rs"]
mod stream;
pub use stream::{AgentEventStreamAuthorization, AgentEventStreamSnapshot};
#[path = "agent_reads/window.rs"]
mod window;
pub use window::{
    AgentEventStreamHeader, AgentReplayEntry, AgentReplayWindow, AgentResponseBinding,
};

#[path = "agent_reads/answer.rs"]
mod answer;
pub use answer::AgentFinalAnswer;

impl AgentReadService {
    pub fn new<S, R>(store: Arc<S>, resources: Arc<R>) -> Arc<Self>
    where
        S: AgentReadStore + 'static,
        R: SandboxResourceReader + 'static,
    {
        Arc::new(Self {
            store,
            resources,
            access: None,
        })
    }

    pub fn with_resource_access(self: &Arc<Self>, access: Arc<ResourceAccessService>) -> Arc<Self> {
        Arc::new(Self {
            store: self.store.clone(),
            resources: self.resources.clone(),
            access: Some(access),
        })
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
        self.validate_page(caller, query, page, &result).await?;
        Ok(result)
    }

    pub async fn read(&self, caller: &Caller, id: &AgentId) -> Result<Agent, AgentReadError> {
        require(caller)?;
        Ok(self.owned_lineage(caller, id).await?.agent)
    }

    pub(super) async fn require_owned_final_answer(
        &self,
        caller: &Caller,
        id: &AgentId,
        sequence: u64,
    ) -> Result<(), AgentReadError> {
        let transcript = self.transcript(caller, id).await?;
        if transcript.turns.iter().any(|turn| {
            turn.final_answer()
                .is_some_and(|answer| answer.sequence == sequence)
        }) {
            Ok(())
        } else {
            Err(AgentReadError::NotFound)
        }
    }

    /// Read the Agent and every parent needed to project its public context.
    pub async fn lineage(
        &self,
        caller: &Caller,
        id: &AgentId,
    ) -> Result<AgentLineage, AgentReadError> {
        require(caller)?;
        self.owned_lineage(caller, id).await
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
        let projection = self
            .projection(caller, id, crate::RuntimeCapability::Conversation)
            .await?;
        Ok(projection.transcript)
    }

    /// Read a validated log through the exact aggregate captured for this read.
    pub async fn log_snapshot(
        &self,
        caller: &Caller,
        id: &AgentId,
    ) -> Result<(Agent, Vec<AgentLogEntry>), AgentReadError> {
        require(caller)?;
        let agent = self.owned_lineage(caller, id).await?.agent;
        let entries = self.complete_log(&agent).await?;
        fold_agent_log(&agent, &entries).map_err(AgentReadError::Corrupt)?;
        Ok((agent, entries))
    }

    pub async fn usage(
        &self,
        caller: &Caller,
        id: &AgentId,
    ) -> Result<AgentUsageReading, AgentReadError> {
        let projection = self
            .projection(caller, id, crate::RuntimeCapability::Usage)
            .await?;
        Ok(projection.usage)
    }

    pub async fn resources(
        &self,
        caller: &Caller,
        id: &AgentId,
    ) -> Result<AgentResourceReading, AgentReadError> {
        require(caller)?;
        let lineage = self.owned_lineage(caller, id).await?;
        require_granted(&lineage.agent, crate::RuntimeCapability::Resources)?;
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
        capability: crate::RuntimeCapability,
    ) -> Result<AgentLogProjection, AgentReadError> {
        require(caller)?;
        let lineage = self.owned_lineage(caller, id).await?;
        require_granted(&lineage.agent, capability)?;
        let entries = self.complete_log(&lineage.agent).await?;
        fold_agent_log(&lineage.agent, &entries).map_err(AgentReadError::Corrupt)
    }

    async fn owned_lineage(
        &self,
        caller: &Caller,
        id: &AgentId,
    ) -> Result<AgentLineage, AgentReadError> {
        if let Some(access) = &self.access {
            let authorized = access
                .authorize_personal(
                    caller,
                    &ResourceTarget::Agent {
                        agent_id: id.clone(),
                    },
                    ResourceCapability::ViewHistory,
                )
                .await
                .map_err(authorized::access_error)?;
            return authorized::lineage(authorized.snapshot.lineage);
        }
        let agent = self
            .store
            .get_agent(id)
            .await
            .map_err(store_error)?
            .filter(|agent| {
                agent.id == *id
                    && AccessDecision::personal(caller, &agent.owner)
                        .allows(ResourceCapability::ViewHistory)
            })
            .ok_or(AgentReadError::NotFound)?;
        let (worktree, workspace, sandbox) = self.validate_lineage(&caller.subject, &agent).await?;
        Ok(AgentLineage {
            agent,
            worktree,
            workspace,
            sandbox,
        })
    }

    async fn validate_lineage(
        &self,
        owner: &SubjectId,
        agent: &Agent,
    ) -> Result<(Worktree, Workspace, Sandbox), AgentReadError> {
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
        Ok((worktree, workspace, sandbox))
    }

    async fn validate_page(
        &self,
        caller: &Caller,
        query: &AgentListQuery,
        request: &PageRequest,
        result: &Page<Agent>,
    ) -> Result<(), AgentReadError> {
        let owner = &caller.subject;
        validate_limit(request.limit)?;
        if result.items.len() > usize::from(request.limit) {
            return Err(corrupt("agent page exceeds its requested bound"));
        }
        if result.next.is_some() && result.items.len() != usize::from(request.limit) {
            return Err(corrupt("agent page cursor precedes its requested bound"));
        }
        let mut previous = request.cursor.as_deref();
        for agent in &result.items {
            if let Some(access) = &self.access {
                access
                    .authorize_personal(
                        caller,
                        &ResourceTarget::Agent {
                            agent_id: agent.id.clone(),
                        },
                        ResourceCapability::ViewDetails,
                    )
                    .await
                    .map_err(authorized::access_error)?;
            }
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
        let entries = self
            .store
            .agent_log_prefix(&agent.id, through_seq)
            .await
            .map_err(log_store_error)?;
        if u64::try_from(entries.len()).ok() != Some(through_seq) {
            return Err(corrupt("agent log ended before its high-water mark"));
        }
        for (offset, entry) in entries.iter().enumerate() {
            let expected = u64::try_from(offset)
                .ok()
                .and_then(|offset| offset.checked_add(1))
                .ok_or_else(|| corrupt("agent log sequence overflowed"))?;
            if entry.agent_id != agent.id || entry.seq != expected || entry.seq > through_seq {
                return Err(corrupt("agent log identity or sequence is invalid"));
            }
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

fn require_granted(
    agent: &Agent,
    capability: crate::RuntimeCapability,
) -> Result<(), AgentReadError> {
    agent
        .capabilities
        .require([capability])
        .map_err(|capability| AgentReadError::CapabilityUnsupported { capability })
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
