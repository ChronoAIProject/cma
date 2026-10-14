//! Global database authority and atomic recovery from an observed missing runtime.

use std::collections::BTreeSet;

use async_trait::async_trait;

use crate::sandbox::runtime_recovery_agents::recover_agent;
use crate::sandbox::{
    Agent, AgentLogKind, RuntimeBinding, Sandbox, SandboxEventKind, SandboxStatus,
};
use crate::{seconds_from_rfc3339, SandboxId};

use super::{Page, PageRequest, SandboxStoreError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxRuntimeLossRequest {
    pub sandbox_id: SandboxId,
    pub expected_revision: u64,
    pub expected_runtime: Option<RuntimeBinding>,
    pub observed_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxRuntimeLossOutcome {
    RecoveryScheduled,
    Failed,
    Superseded,
    NotEligible,
}

#[doc(hidden)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxRuntimeLossAgentMutation {
    pub expected_revision: u64,
    pub expected_last_seq: u64,
    pub agent: Agent,
    pub logs: Vec<AgentLogKind>,
}

#[doc(hidden)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxRuntimeLossMutation {
    pub expected_revision: u64,
    pub sandbox: Sandbox,
    pub agents: Vec<SandboxRuntimeLossAgentMutation>,
    pub events: Vec<SandboxEventKind>,
    pub outcome: SandboxRuntimeLossOutcome,
}

#[doc(hidden)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxRuntimeLossApplication {
    Persist(Box<SandboxRuntimeLossMutation>),
    Complete(SandboxRuntimeLossOutcome),
}

#[async_trait]
pub trait SandboxReconciliationStore: Send + Sync {
    /// Read the exact unowned home reservation before the fresh owned Sandbox read.
    async fn unowned_home_reservation(
        &self,
        _id: &SandboxId,
    ) -> Result<Option<crate::sandbox::SandboxHome>, SandboxStoreError> {
        Ok(None)
    }

    /// Read unowned home authority before owned authority, across atomic warm claims.
    async fn unowned_home_authority(
        &self,
        _request: &layer3_adapter::HomeVolumeRequest,
    ) -> Result<bool, SandboxStoreError> {
        Ok(false)
    }

    /// Read this before the owned Sandbox authority. A claim commits both records
    /// atomically, so an observed Claimed reservation must precede that fresh read.
    async fn unowned_runtime_authority(
        &self,
        _id: &SandboxId,
        _generation: u64,
        _runtime_id: &str,
    ) -> Result<bool, SandboxStoreError> {
        Ok(false)
    }

    /// Page every Sandbox, regardless of owner or lifecycle state, by stable ID.
    /// Cursors are exclusive. Runtime reclaim requires reaching the end of this scan.
    async fn sandbox_runtime_authority(
        &self,
        page: &PageRequest,
    ) -> Result<Page<Sandbox>, SandboxStoreError>;

    /// Fence the observed Sandbox revision and runtime, read every Agent and exact
    /// log head, and atomically publish recovery plus the resulting logs/events.
    async fn recover_runtime_loss(
        &self,
        request: SandboxRuntimeLossRequest,
    ) -> Result<SandboxRuntimeLossOutcome, SandboxStoreError>;
}

#[doc(hidden)]
pub fn validate_sandbox_runtime_loss_request(
    request: &SandboxRuntimeLossRequest,
) -> Result<(), SandboxStoreError> {
    if request.sandbox_id.as_str().trim().is_empty()
        || request.expected_revision == 0
        || seconds_from_rfc3339(&request.observed_at).is_none()
        || request.expected_runtime.as_ref().is_some_and(|runtime| {
            runtime.runtime_id.trim().is_empty()
                || runtime.runtime_version.trim().is_empty()
                || seconds_from_rfc3339(&runtime.bound_at).is_none()
        })
    {
        return Err(SandboxStoreError::InvalidTransaction(
            "runtime-loss recovery request is invalid",
        ));
    }
    Ok(())
}

/// The adapter supplies the complete Agent set and independently observed log heads.
#[doc(hidden)]
pub fn apply_sandbox_runtime_loss(
    current: &Sandbox,
    agents: &[(Agent, u64)],
    request: &SandboxRuntimeLossRequest,
) -> Result<SandboxRuntimeLossApplication, SandboxStoreError> {
    validate_sandbox_runtime_loss_request(request)?;
    if current.id != request.sandbox_id {
        return Err(corrupt("stored Sandbox key disagrees with its identity"));
    }
    current
        .validate()
        .map_err(|error| corrupt(&error.to_string()))?;
    if current.revision != request.expected_revision || current.runtime != request.expected_runtime
    {
        return Ok(SandboxRuntimeLossApplication::Complete(
            SandboxRuntimeLossOutcome::Superseded,
        ));
    }
    if !current.requires_runtime_loss_recovery()
        || seconds_from_rfc3339(&current.updated_at)
            .is_none_or(|at| Some(at) > seconds_from_rfc3339(&request.observed_at))
    {
        return Ok(SandboxRuntimeLossApplication::Complete(
            SandboxRuntimeLossOutcome::NotEligible,
        ));
    }
    let mut ids = BTreeSet::new();
    let mut changes = Vec::new();
    let mut events = Vec::new();
    for (agent, log_head) in agents {
        if agent.sandbox_id != current.id
            || agent.owner != current.owner
            || agent.revision == 0
            || agent.work.next_fence == 0
            || !ids.insert(&agent.id)
        {
            return Err(corrupt("stored Agent identity is invalid"));
        }
        if agent.conversation.last_seq != *log_head {
            return Err(SandboxStoreError::SequenceConflict);
        }
        let updated_at = seconds_from_rfc3339(&agent.updated_at)
            .ok_or_else(|| corrupt("stored Agent update time is invalid"))?;
        if Some(updated_at) > seconds_from_rfc3339(&request.observed_at) {
            return Ok(SandboxRuntimeLossApplication::Complete(
                SandboxRuntimeLossOutcome::NotEligible,
            ));
        }
        if !agent.status.is_live() {
            continue;
        }
        let (next, logs) = recover_agent(current, agent, &request.observed_at)
            .map_err(|error| corrupt(&error.to_string()))?;
        if next.status != agent.status {
            events.push(SandboxEventKind::AgentStatusChanged {
                agent_id: agent.id.clone(),
                from: agent.status,
                to: next.status,
            });
        }
        changes.push(SandboxRuntimeLossAgentMutation {
            expected_revision: agent.revision,
            expected_last_seq: *log_head,
            agent: next,
            logs,
        });
    }
    changes.sort_by(|left, right| left.agent.id.cmp(&right.agent.id));
    let recovered = changes
        .iter()
        .map(|change| change.agent.clone())
        .collect::<Vec<_>>();
    let mut sandbox = current.clone();
    sandbox
        .recover_runtime_loss(&recovered, request.observed_at.clone())
        .map_err(|error| corrupt(&error.to_string()))?;
    if sandbox.status != current.status {
        events.push(SandboxEventKind::StatusChanged {
            from: current.status,
            to: sandbox.status,
        });
    }
    let outcome = if sandbox.status == SandboxStatus::Failed {
        events.push(SandboxEventKind::Failed {
            failure: sandbox
                .failure
                .clone()
                .expect("failed Sandbox has a failure"),
        });
        SandboxRuntimeLossOutcome::Failed
    } else {
        SandboxRuntimeLossOutcome::RecoveryScheduled
    };
    Ok(SandboxRuntimeLossApplication::Persist(Box::new(
        SandboxRuntimeLossMutation {
            expected_revision: current.revision,
            sandbox,
            agents: changes,
            events,
            outcome,
        },
    )))
}

fn corrupt(detail: &str) -> SandboxStoreError {
    SandboxStoreError::Corrupt(detail.into())
}
