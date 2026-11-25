use async_trait::async_trait;
use std::collections::{BTreeMap, BTreeSet};

use crate::ids::{AgentId, SandboxId};
use crate::sandbox::{Agent, AgentLogKind, AgentStatus, AgentWorkStep, SandboxEventKind, Worktree};

use super::{AgentWorkClaimError, Page, PageRequest, SandboxStoreError};

/// Advisory discovery for runtime-dependent background steps. A directed
/// probe remains discoverable even when its Sandbox is hidden in ordinary work.
#[derive(Clone, Copy, Default)]
pub enum StartupWorkFilter<'a> {
    #[default]
    All,
    Exclude(&'a BTreeSet<SandboxId>),
    Probe(&'a SandboxId),
}

impl StartupWorkFilter<'_> {
    pub const STEPS: [AgentWorkStep; 6] = [
        AgentWorkStep::InstallSkill,
        AgentWorkStep::AddWorktree,
        AgentWorkStep::MaterializeHome,
        AgentWorkStep::OpenConversation,
        AgentWorkStep::PublishReady,
        AgentWorkStep::PublishConnection,
    ];

    pub fn can_hold(agent: &Agent) -> bool {
        // An unreconciled lease or cleanup debt can represent an effect that
        // has already started. Keep these and explicit controls eligible.
        Self::STEPS.contains(&agent.work.step)
            && agent.work.lease.is_none()
            && agent.work.cleanup_debt.is_empty()
    }

    pub fn includes(self, agent: &Agent) -> bool {
        match self {
            Self::All => true,
            Self::Exclude(sandboxes) => {
                !sandboxes.contains(&agent.sandbox_id) || !Self::can_hold(agent)
            }
            Self::Probe(sandbox_id) => &agent.sandbox_id == sandbox_id && Self::can_hold(agent),
        }
    }
}

/// Identifies the pending task associated with a local acquisition refusal.
#[derive(Clone, PartialEq, Eq)]
pub struct PendingAgentWork {
    pub step: AgentWorkStep,
    pub status: AgentStatus,
    pub ready_at: String,
    pub attempts: u32,
    pub next_fence: u64,
    pub control_revision: Option<u64>,
}

impl PendingAgentWork {
    pub fn from_agent(agent: &Agent) -> Option<Self> {
        let work = &agent.work;
        work.step.is_pending().then(|| Self {
            step: work.step,
            status: agent.status,
            ready_at: work.ready_at.clone(),
            attempts: work.attempts,
            next_fence: work.next_fence,
            control_revision: (!AgentWorkStep::RECEIVER_PACED.contains(&work.step))
                .then_some(agent.revision),
        })
    }
}

/// Advisory filters applied to current work before the discovery result limit.
#[derive(Clone, Copy)]
pub struct AgentWorkDiscovery<'a> {
    pub receiver_holds: &'a BTreeSet<AgentId>,
    pub startup: StartupWorkFilter<'a>,
    pub acquisition_cooldowns: Option<&'a BTreeMap<AgentId, PendingAgentWork>>,
}

impl<'a> AgentWorkDiscovery<'a> {
    pub fn new(receiver_holds: &'a BTreeSet<AgentId>, startup: StartupWorkFilter<'a>) -> Self {
        Self {
            receiver_holds,
            startup,
            acquisition_cooldowns: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct VersionedWorktree {
    pub expected_revision: u64,
    pub worktree: Worktree,
}

/// One fenced, all-or-nothing agent lifecycle mutation.
#[derive(Debug, Clone)]
pub struct AgentWorkMutation {
    pub expected_agent_revision: u64,
    pub holder: String,
    pub fence: u64,
    pub agent: Agent,
    pub worktree: Option<VersionedWorktree>,
    pub appended_at: String,
    pub sandbox_event: Option<SandboxEventKind>,
    pub agent_logs: Vec<AgentLogKind>,
}

mod connection_recovery;
pub use connection_recovery::{
    apply_agent_connection_recovery, AgentConnectionRecoveryApplication,
    AgentConnectionRecoveryCandidate, AgentConnectionRecoveryCause, AgentConnectionRecoveryOutcome,
    AgentConnectionRecoveryRequest,
};

/// Work admission and authority observed before a failed read-only attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRetryObservation {
    pub agent_revision: u64,
    pub command_admission_generation: u64,
    pub sandbox_revision: u64,
    pub workspace_revision: u64,
    pub worktree_revision: u64,
}

/// Original durable admission boundary for the create completion measurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentCreateAdmission {
    pub admitted_at_ms: i64,
    pub desktop: Option<bool>,
    pub machine_included: Option<bool>,
}

mod publication;
pub use publication::{
    AgentWorkPublication, AgentWorkPublicationLineage, AgentWorkPublicationParents,
};

/// Durable agent work discovery, lease fencing, and atomic publication.
#[async_trait]
pub trait AgentWorkerStore: Send + Sync {
    /// Read the restart admission and any immutable dispatch from this claim.
    async fn agent_restart_work(
        &self,
        _claimed: &Agent,
    ) -> Result<super::AgentRestartWork, SandboxStoreError> {
        Err(SandboxStoreError::Conflict)
    }

    /// Persist the exact Open before authorizing a send on this claim.
    async fn prepare_agent_restart_dispatch(
        &self,
        _request: super::AgentRestartDispatch,
    ) -> Result<Agent, SandboxStoreError> {
        Err(SandboxStoreError::Conflict)
    }

    async fn agent_restart_teardown(
        &self,
        _claimed: &Agent,
    ) -> Result<super::AgentRestartTeardown, SandboxStoreError> {
        Err(SandboxStoreError::Conflict)
    }

    async fn publish_agent_restart_teardown(
        &self,
        _publication: super::AgentRestartTeardownPublication,
    ) -> Result<(), SandboxStoreError> {
        Err(SandboxStoreError::Conflict)
    }

    /// Read the immutable Open request for a previously restarted live Agent.
    async fn restart_connection_open(
        &self,
        _agent: &Agent,
    ) -> Result<Option<layer3_adapter::AgentOpenRequest>, SandboxStoreError> {
        Ok(None)
    }

    /// Receipt for the exact admitted owner Stop on the captured revision.
    async fn has_owner_stop_receipt(&self, _agent: &Agent) -> Result<bool, SandboxStoreError> {
        Ok(false)
    }

    /// Original lifecycle admission in BSON millisecond precision, when this
    /// store can resolve the operation that admitted the Agent. Legacy and
    /// existing-workspace records intentionally return no boundary.
    async fn agent_create_admission(
        &self,
        _agent: &Agent,
    ) -> Result<Option<AgentCreateAdmission>, SandboxStoreError> {
        Ok(None)
    }

    /// Observe durable command admission before inspection. Stores without this
    /// evidence retain ordinary retries rather than pacing unobserved work.
    async fn agent_command_admission_generation(
        &self,
        _id: &AgentId,
    ) -> Result<Option<u64>, SandboxStoreError> {
        Ok(None)
    }

    /// Publish a failed read-only observation, optionally retaining a scheduling hint.
    /// A store may omit the hint: it grants no authority and losing it only causes
    /// an ordinary retry. Effect outcomes must use ordinary publication instead.
    async fn commit_agent_work_retry(
        &self,
        mutation: AgentWorkMutation,
        _observed: AgentRetryObservation,
    ) -> Result<(), SandboxStoreError> {
        self.commit_agent_work(mutation).await
    }

    /// A classified store may positively report that an attempt never committed.
    /// The default propagates ordinary errors without authorizing a rebuild.
    async fn try_publish_agent_work(
        &self,
        mutation: AgentWorkMutation,
        _parents: AgentWorkPublicationParents,
    ) -> Result<AgentWorkPublication, SandboxStoreError> {
        self.commit_agent_work(mutation).await?;
        Ok(AgentWorkPublication::Published)
    }

    /// Publish a metadata-only retry after verifying the supplied marker in the
    /// same snapshot as the Agent CAS. Unsupported stores must fail closed.
    async fn commit_agent_work_repeated_failure(
        &self,
        _retry: super::AgentRepeatedFailure,
        _observed: Option<AgentRetryObservation>,
    ) -> Result<(), SandboxStoreError> {
        Err(SandboxStoreError::InvalidTransaction(
            "repeated failure publication is unsupported",
        ))
    }

    /// Exact durable preparation authority for replaying a worktree/home pair.
    /// Stores without such evidence retain the ordinary individual work steps.
    async fn prepared_agent_replay_matches(
        &self,
        _agent: &Agent,
        _sandbox: &crate::sandbox::Sandbox,
        _workspace: &crate::sandbox::Workspace,
        _worktree: &Worktree,
    ) -> Result<bool, SandboxStoreError> {
        Ok(false)
    }

    /// Publish a proved prepared open and Ready under the same fresh parent observation.
    /// False means this store has no matching preparation; use ordinary publication.
    async fn commit_prepared_agent_open_ready(
        &self,
        _mutation: AgentWorkMutation,
        _observed: super::PreparedAgentOpenContext,
    ) -> Result<bool, SandboxStoreError> {
        Ok(false)
    }

    async fn live_agent_connections(
        &self,
        page: &PageRequest,
    ) -> Result<Page<AgentConnectionRecoveryCandidate>, SandboxStoreError>;

    async fn schedule_agent_connection_recovery(
        &self,
        request: AgentConnectionRecoveryRequest,
    ) -> Result<AgentConnectionRecoveryOutcome, SandboxStoreError>;

    async fn due_agent_work(
        &self,
        now: &str,
        limit: u16,
    ) -> Result<Vec<AgentId>, SandboxStoreError> {
        self.due_agent_work_with_cleanup_holds(now, limit, &Default::default())
            .await
    }

    /// Apply local receiver holds to the current work step before counting the result limit.
    async fn due_agent_work_with_cleanup_holds(
        &self,
        now: &str,
        limit: u16,
        holds: &BTreeSet<AgentId>,
    ) -> Result<Vec<AgentId>, SandboxStoreError> {
        self.due_agent_work_filtered(
            now,
            limit,
            AgentWorkDiscovery::new(holds, StartupWorkFilter::All),
        )
        .await
    }

    async fn due_agent_work_filtered(
        &self,
        now: &str,
        limit: u16,
        filter: AgentWorkDiscovery<'_>,
    ) -> Result<Vec<AgentId>, SandboxStoreError>;

    async fn claim_agent_work(
        &self,
        id: &AgentId,
        holder: &str,
        now: String,
        lease_expires_at: String,
    ) -> Result<Option<Agent>, SandboxStoreError>;

    fn agent_inspection_parking_enabled(&self) -> bool {
        false
    }

    /// Check a durable inspection park before attempting ownership acquisition.
    async fn agent_inspection_parked(&self, _id: &AgentId) -> Result<bool, SandboxStoreError> {
        Ok(false)
    }

    /// Persist a failed read against its original claim and runtime observation.
    async fn park_agent_inspection(
        &self,
        _owner: &crate::agent_ownership::OwnershipToken,
        _claimed: &Agent,
        _binding: &crate::sandbox::RuntimeBinding,
        _failure: crate::agent_ownership::OwnershipFailure,
    ) -> Result<bool, SandboxStoreError> {
        Ok(false)
    }

    /// Retire the consumed inspection park before attempting another effect.
    async fn confirm_agent_inspection(
        &self,
        _claimed: &Agent,
        _binding: &crate::sandbox::RuntimeBinding,
    ) -> Result<(), SandboxStoreError> {
        Ok(())
    }

    async fn claim_agent_work_observed(
        &self,
        id: &AgentId,
        holder: &str,
        now: String,
        lease_expires_at: String,
    ) -> Result<Option<Agent>, AgentWorkClaimError> {
        self.claim_agent_work(id, holder, now, lease_expires_at)
            .await
            .map_err(Into::into)
    }

    /// Publish a skill result under the captured parent environment. Unsupported
    /// stores must refuse rather than discard the parent authority.
    async fn commit_agent_skill_result(
        &self,
        _mutation: AgentWorkMutation,
        _guard: super::AgentSkillResultGuard,
    ) -> Result<(), SandboxStoreError> {
        Err(SandboxStoreError::InvalidTransaction(
            "skill-result parent fencing is unsupported",
        ))
    }

    async fn commit_agent_work(&self, mutation: AgentWorkMutation)
        -> Result<(), SandboxStoreError>;

    /// Publish an original startup or connection result with the parent
    /// revisions observed while proving its native attachment.
    async fn commit_agent_connection_work(
        &self,
        mutation: AgentWorkMutation,
        _parents: AgentWorkPublicationParents,
    ) -> Result<(), SandboxStoreError> {
        self.commit_agent_work(mutation).await
    }
}
