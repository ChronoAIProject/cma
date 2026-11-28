use std::sync::Arc;

use crate::ids::AgentId;
use crate::resource_access::{
    AccessDecision, ResourceAccessService, ResourceCapability, ResourceTarget,
};
use crate::{Caller, Capability, SubjectId};

use super::agent_reads_fold::fold_agent_log;
use super::store::{AgentReadStore, Page, PageRequest, SandboxStoreError, MAX_PAGE_SIZE};
use super::{
    Agent, AgentLineage, AgentListQuery, AgentLogEntry, AgentReadError, AgentResourceReading,
    AgentTranscript, AgentUsageReading, Sandbox, SandboxResourceReadError, SandboxResourceReader,
    Workspace, Worktree,
};

pub struct AgentReadService {
    observation: Option<Arc<super::agent_observation::AgentObservationService>>,
    store: Arc<dyn AgentReadStore>,
    sessions: Option<Arc<dyn super::store::SandboxSessionStore>>,
    resources: Arc<dyn SandboxResourceReader>,
    access: Option<Arc<ResourceAccessService>>,
    response_logs: Option<Arc<dyn super::store::AgentResponseLogStore>>,
}

#[path = "agent_reads/authorized.rs"]
mod authorized;
#[path = "agent_reads/composition.rs"]
mod composition;
#[path = "agent_reads/observation.rs"]
mod observation;
#[path = "agent_reads/original_request.rs"]
mod original_request;
pub use original_request::AgentOriginalRequest;
#[path = "agent_reads/privacy_export.rs"]
pub mod privacy_export;
#[path = "agent_reads/recovery.rs"]
mod recovery;
#[path = "agent_reads/reply_preview.rs"]
mod reply_preview;
#[path = "agent_reads/transcript_pages.rs"]
mod transcript_pages;
pub use authorized::{AgentDetailReading, SharedAgentDetail};
#[path = "agent_reads/retained_output.rs"]
mod retained_output;
pub use retained_output::{
    AgentRetainedOutput, AgentRetainedOutputAvailability, AgentRetainedOutputPage,
    RETAINED_OUTPUT_CHUNK_BYTES,
};
#[path = "agent_reads/history.rs"]
mod history;
pub use history::AgentHistoryReading;
#[path = "agent_reads/configuration.rs"]
mod configuration;
pub use configuration::AgentConfigurationRead;
#[path = "agent_reads/source.rs"]
mod source;
pub use source::{AgentSourceReadError, AgentSourceReading, SharedAgentOrigin};
#[path = "agent_reads/visibility.rs"]
mod visibility;
pub use visibility::AgentVisibleLogEntry;
#[path = "agent_reads/legacy_snapshot.rs"]
mod legacy_snapshot;
#[path = "agent_reads/sessions.rs"]
mod sessions;
#[path = "agent_reads/stream.rs"]
mod stream;
pub use stream::{AgentEventStreamAuthorization, AgentEventStreamSnapshot};
#[path = "agent_reads/cursor.rs"]
mod cursor;
pub use cursor::AgentHistoryCursor;
#[path = "agent_reads/output_memo.rs"]
mod output_memo;
#[path = "agent_reads/response.rs"]
mod response;
pub use response::{
    indexed::AgentOutputObservation, AgentResponseObservation, AgentResponseObservationError,
};
#[path = "agent_reads/buffered.rs"]
mod buffered;
pub use buffered::AgentBufferedEventRequest;
#[path = "agent_reads/window.rs"]
mod window;
pub use window::{
    AgentEventStreamHeader, AgentReplayEntry, AgentReplayWindow, AgentResponseBinding,
};

#[path = "agent_reads/answer.rs"]
mod answer;
#[path = "agent_reads/log_pages.rs"]
mod log_pages;
pub use answer::AgentFinalAnswer;
#[path = "agent_reads/usage.rs"]
mod usage;
use usage::fold_agent_usage;

impl AgentReadService {
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
        if let Some(access) = &self.access {
            if access
                .personal_agent_page(caller, query, page, &result)
                .await
                .map_err(authorized::access_error)?
            {
                return Ok(result);
            }
        }
        self.validate_page(caller, query, page, &result).await?;
        Ok(result)
    }

    pub async fn read(&self, caller: &Caller, id: &AgentId) -> Result<Agent, AgentReadError> {
        require(caller)?;
        Ok(self.owned_lineage(caller, id).await?.agent)
    }

    pub async fn lineage(
        &self,
        caller: &Caller,
        id: &AgentId,
    ) -> Result<AgentLineage, AgentReadError> {
        require(caller)?;
        self.owned_lineage(caller, id).await
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
        self.usage_bounded(caller, id).await
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
            let lineage_validated = if let Some(access) = &self.access {
                let authorized = access
                    .authorize_personal(
                        caller,
                        &ResourceTarget::Agent {
                            agent_id: agent.id.clone(),
                        },
                        ResourceCapability::ViewDetails,
                    )
                    .await
                    .map_err(authorized::access_error)?;
                // Authorization already validates all parents and conversation
                // invariants in one current snapshot. Reuse that proof only
                // for the exact listed row; changed rows keep the legacy reads.
                authorized.snapshot.lineage.agent.as_ref() == Some(agent)
            } else {
                false
            };
            if agent.owner != *owner || !query.matches(agent) {
                return Err(corrupt("agent page contains an out-of-scope row"));
            }
            if previous.is_some_and(|cursor| agent.id.as_str() <= cursor) {
                return Err(corrupt("agent page order is invalid"));
            }
            if !lineage_validated {
                self.validate_lineage(owner, agent).await?;
            }
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
        let proven = self
            .store
            .agent_log_prefix_with_tail(agent)
            .await
            .map_err(log_store_error)?;
        let tail_proven = proven.is_some();
        let entries = match proven {
            Some(entries) => entries,
            None => self
                .store
                .agent_log_prefix(&agent.id, through_seq)
                .await
                .map_err(log_store_error)?,
        };
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
        if !tail_proven {
            self.reject_uncommitted_tail(agent, through_seq).await?;
        }
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
            .agent_log_entry(&agent.id, through_seq.saturating_add(1))
            .await
            .map_err(log_store_error)?;
        let Some(extra) = tail else {
            return Ok(());
        };
        if extra.agent_id != agent.id || extra.seq != through_seq.saturating_add(1) {
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
        other => {
            use crate::dependency_diagnostics::{Dependency, Failure, Reason};
            Failure::new(Dependency::Storage, Reason::Unavailable).record();
            AgentReadError::Unavailable(other.to_string())
        }
    }
}

fn lineage_store_error(error: SandboxStoreError) -> AgentReadError {
    match error {
        SandboxStoreError::NotFound => corrupt("agent lineage lookup was inconsistent"),
        other => store_error(other),
    }
}

fn capture_open_error(error: SandboxStoreError) -> AgentReadError {
    match error {
        SandboxStoreError::CaptureAdvanced => AgentReadError::CaptureAdvanced,
        other => log_store_error(other),
    }
}

fn log_store_error(error: SandboxStoreError) -> AgentReadError {
    match error {
        SandboxStoreError::NotFound
        | SandboxStoreError::SequenceConflict
        | SandboxStoreError::CaptureAdvanced => corrupt("agent log is incomplete or inconsistent"),
        other => store_error(other),
    }
}

fn resource_error(error: SandboxResourceReadError) -> AgentReadError {
    match error {
        SandboxResourceReadError::Unsupported(detail) => {
            AgentReadError::ResourcesUnsupported(detail)
        }
        SandboxResourceReadError::RuntimeCapacity(capacity) => {
            AgentReadError::RuntimeCapacity(capacity)
        }
        SandboxResourceReadError::Corrupt(detail) => AgentReadError::Corrupt(detail),
        SandboxResourceReadError::Unavailable(detail) => AgentReadError::Unavailable(detail),
    }
}
