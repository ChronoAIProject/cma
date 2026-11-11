//! Durable recovery work semantics. This carrier grants no storage authority;
//! admission must bind it to S4's immutable publication in the same transaction.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::ports::RecoveryError as Error;
use super::{
    home_identity, valid_sha256, CapacityRecoveryContext, CapacityRecoveryDisposition,
    CapacityRecoveryPlan,
};
use crate::sandbox::store::AgentLogPublicationIdentity;
use crate::sandbox::{
    Agent, AgentLogKind, AgentStatus, AgentWorkLease, AgentWorkReceipt, AgentWorkStep,
    RuntimeBinding,
};
use crate::{seconds_from_rfc3339, AgentId};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryRequest {
    pub agent_id: AgentId,
    pub creation_key: String,
    pub creation_fingerprint: String,
    pub failure_seq: u64,
    pub requested_at: String,
}

impl RecoveryRequest {
    pub fn validate(&self) -> Result<(), Error> {
        if self.agent_id.as_str().is_empty()
            || self.creation_key.trim().is_empty()
            || self.creation_key.len() > 256
            || !valid_sha256(&self.creation_fingerprint)
            || self.failure_seq < 2
            || self.failure_seq == u64::MAX
            || seconds_from_rfc3339(&self.requested_at).is_none()
        {
            return Err(Error::InvalidEvidence);
        }
        Ok(())
    }

    /// Creation and original failure determine identity; a new lease or later
    /// current head cannot allocate a replacement logical recovery operation.
    pub fn key(&self) -> Result<String, Error> {
        self.validate()?;
        Ok(format!(
            "capacity-recovery:{}",
            digest(&(
                "cma.capacity-recovery.key/v1",
                &self.agent_id,
                &self.creation_key,
                &self.creation_fingerprint,
                self.failure_seq,
            ))?
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryTarget {
    pub agent_id: AgentId,
    pub creation_key: String,
    pub creation_fingerprint: String,
    pub conversation_id: String,
    pub conversation_generation: u64,
    pub runtime: RuntimeBinding,
    pub home_identity: String,
    pub original_open: AgentWorkReceipt,
    pub active_turn: Option<u64>,
    /// Hashes original input facts, never copies input content into work rows.
    pub input_sha256: String,
}

impl RecoveryTarget {
    fn from_context(context: &CapacityRecoveryContext<'_>) -> Result<Self, Error> {
        let a = context.agent;
        Ok(Self {
            agent_id: a.id.clone(),
            creation_key: a.creation_key.clone(),
            creation_fingerprint: a.creation_fingerprint.clone(),
            conversation_id: a
                .conversation
                .conversation_id
                .clone()
                .ok_or(Error::InvalidEvidence)?,
            conversation_generation: a
                .conversation
                .runtime_generation
                .ok_or(Error::InvalidEvidence)?,
            runtime: context
                .sandbox
                .runtime
                .clone()
                .ok_or(Error::InvalidEvidence)?,
            home_identity: home_identity(context.sandbox).ok_or(Error::InvalidEvidence)?,
            original_open: a
                .work
                .receipts
                .iter()
                .find(|r| r.step == AgentWorkStep::OpenConversation)
                .cloned()
                .ok_or(Error::InvalidEvidence)?,
            active_turn: a.conversation.active_turn,
            input_sha256: input_digest(a)?,
        })
    }

    fn require_current(
        &self,
        context: &CapacityRecoveryContext<'_>,
        now: &str,
    ) -> Result<(), Error> {
        let disposition = context.classify(now);
        if disposition != CapacityRecoveryDisposition::EvidenceRequired {
            return Err(Error::Disposition(disposition));
        }
        if Self::from_context(context)? != *self {
            return Err(Error::Conflict);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryPhase {
    Admitted,
    Attaching,
    Completed,
    Superseded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryWork {
    pub format_version: u8,
    pub request: RecoveryRequest,
    pub target: RecoveryTarget,
    pub phase: RecoveryPhase,
    pub revision: u64,
    pub attempts: u32,
    pub next_fence: u64,
    pub lease: Option<AgentWorkLease>,
    pub ready_at: String,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryClaim {
    pub work: RecoveryWork,
    pub agent: Agent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfirmedNativeState {
    Idle,
    Working,
    WaitingApproval,
}

/// Constructed only from the restricted runtime handshake. The store still
/// checks current authority and exact durable recording before completion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryConfirmation {
    pub operation_key: String,
    pub target: RecoveryTarget,
    pub authority_fence: u64,
    pub native_state: ConfirmedNativeState,
}

impl RecoveryConfirmation {
    pub fn validate_for(&self, claim: &RecoveryClaim) -> Result<(), Error> {
        let lease = claim.work.lease.as_ref().ok_or(Error::Conflict)?;
        if self.operation_key != claim.work.request.key()?
            || self.target != claim.work.target
            || self.authority_fence != lease.fence
            || matches!(self.native_state, ConfirmedNativeState::Idle)
                != self.target.active_turn.is_none()
        {
            return Err(Error::InvalidEvidence);
        }
        Ok(())
    }
}

impl RecoveryWork {
    pub fn prepare(
        request: RecoveryRequest,
        plan: &CapacityRecoveryPlan,
        context: &CapacityRecoveryContext<'_>,
    ) -> Result<Self, Error> {
        request.validate()?;
        let source = plan.source();
        if source != context.agent
            || request.agent_id != source.id
            || request.creation_key != source.creation_key
            || request.creation_fingerprint != source.creation_fingerprint
            || request.failure_seq != plan.original_failure().seq
            || request.requested_at != plan.admitted_at()
        {
            return Err(Error::Conflict);
        }
        let fenced = plan
            .fenced_agent(context, &request.requested_at)
            .map_err(Error::Disposition)?;
        let work = Self {
            format_version: 1,
            ready_at: request.requested_at.clone(),
            request,
            target: RecoveryTarget::from_context(context)?,
            phase: RecoveryPhase::Admitted,
            revision: 1,
            attempts: 0,
            next_fence: fenced.work.next_fence,
            lease: None,
            completed_at: None,
        };
        work.validate()?;
        Ok(work)
    }

    pub fn admission_identity(&self) -> Result<AgentLogPublicationIdentity, Error> {
        Ok(AgentLogPublicationIdentity {
            key: self.request.key()?,
            fingerprint: Sha256::digest(
                serde_json::to_vec(&(
                    "cma.capacity-recovery.request/v1",
                    &self.request,
                    &self.target,
                ))
                .map_err(|_| Error::InvalidEvidence)?,
            )
            .into(),
            appended_at: self.request.requested_at.clone(),
        })
    }

    pub fn validate(&self) -> Result<(), Error> {
        self.request.validate()?;
        if serde_json::to_vec(self)
            .map_err(|_| Error::InvalidEvidence)?
            .len()
            > 16 * 1024
            || self.format_version != 1
            || self.revision == 0
            || self.next_fence == 0
            || self.target.agent_id != self.request.agent_id
            || self.target.creation_key != self.request.creation_key
            || self.target.creation_fingerprint != self.request.creation_fingerprint
            || self.target.conversation_id.trim().is_empty()
            || self.target.conversation_id.len() > 512
            || self.target.conversation_generation == 0
            || self
                .target
                .active_turn
                .is_some_and(|seq| seq == 0 || seq > self.request.failure_seq)
            || self.target.home_identity.trim().is_empty()
            || self.target.runtime.runtime_id.trim().is_empty()
            || self.target.runtime.runtime_version.trim().is_empty()
            || seconds_from_rfc3339(&self.target.runtime.bound_at).is_none()
            || !valid_sha256(&self.target.input_sha256)
            || self.target.original_open.operation_id
                != format!("agent:{}:open:v1", self.request.agent_id)
            || !valid_sha256(&self.target.original_open.request_sha256)
            || self.target.original_open.step != AgentWorkStep::OpenConversation
            || seconds_from_rfc3339(&self.target.original_open.completed_at)
                .is_none_or(|at| Some(at) > seconds_from_rfc3339(&self.request.requested_at))
            || seconds_from_rfc3339(&self.ready_at).is_none()
            || (self.phase == RecoveryPhase::Attaching) != self.lease.is_some()
            || (self.phase == RecoveryPhase::Completed) != self.completed_at.is_some()
            || self
                .completed_at
                .as_ref()
                .is_some_and(|at| seconds_from_rfc3339(at).is_none())
            || self.lease.as_ref().is_some_and(|l| {
                l.fence == 0
                    || l.fence >= self.next_fence
                    || l.holder.trim().is_empty()
                    || l.holder.len() > 256
                    || seconds_from_rfc3339(&l.expires_at).is_none()
            })
        {
            return Err(Error::InvalidEvidence);
        }
        Ok(())
    }

    /// The store persists both proposals under current Agent/work/parent CAS.
    pub fn claim(
        &self,
        context: &CapacityRecoveryContext<'_>,
        holder: &str,
        now: &str,
        expires_at: &str,
    ) -> Result<Option<RecoveryClaim>, Error> {
        self.validate()?;
        if matches!(
            self.phase,
            RecoveryPhase::Completed | RecoveryPhase::Superseded
        ) {
            return Ok(None);
        }
        self.target.require_current(context, now)?;
        let stamp = seconds_from_rfc3339(now).ok_or(Error::InvalidEvidence)?;
        let expiry = seconds_from_rfc3339(expires_at).ok_or(Error::InvalidEvidence)?;
        if holder.trim().is_empty() || holder.len() > 256 || expiry <= stamp {
            return Err(Error::InvalidEvidence);
        }
        if seconds_from_rfc3339(&self.ready_at).is_some_and(|at| at > stamp)
            || self
                .lease
                .as_ref()
                .is_some_and(|l| seconds_from_rfc3339(&l.expires_at).is_some_and(|at| at > stamp))
        {
            return Ok(None);
        }
        let mut work = self.clone();
        let fence = work.next_fence.max(context.agent.work.next_fence);
        work.next_fence = fence.checked_add(1).ok_or(Error::InvalidEvidence)?;
        work.revision = work.revision.checked_add(1).ok_or(Error::InvalidEvidence)?;
        work.attempts = work.attempts.checked_add(1).ok_or(Error::InvalidEvidence)?;
        work.phase = RecoveryPhase::Attaching;
        work.lease = Some(AgentWorkLease {
            holder: holder.into(),
            fence,
            expires_at: expires_at.into(),
        });
        let mut agent = context.agent.clone();
        agent.revision = agent
            .revision
            .checked_add(1)
            .ok_or(Error::InvalidEvidence)?;
        agent.work.next_fence = work.next_fence;
        Ok(Some(RecoveryClaim { work, agent }))
    }

    pub fn retry(
        &self,
        holder: &str,
        fence: u64,
        now: &str,
        retry_at: &str,
    ) -> Result<Self, Error> {
        self.require_lease(holder, fence, now)?;
        if seconds_from_rfc3339(retry_at).is_none_or(|at| Some(at) < seconds_from_rfc3339(now)) {
            return Err(Error::InvalidEvidence);
        }
        let mut next = self.clone();
        next.revision = next.revision.checked_add(1).ok_or(Error::InvalidEvidence)?;
        next.phase = RecoveryPhase::Admitted;
        next.lease = None;
        next.ready_at = retry_at.into();
        Ok(next)
    }

    /// Narrow Failed->live transition. Original failure remains in retained
    /// history and the original receipt. No ordinary command/work is scheduled.
    pub fn complete(
        &self,
        context: &CapacityRecoveryContext<'_>,
        claim: &RecoveryClaim,
        confirmation: &RecoveryConfirmation,
        now: &str,
    ) -> Result<(Self, Agent, AgentLogKind), Error> {
        let lease = claim.work.lease.as_ref().ok_or(Error::Conflict)?;
        if self != &claim.work {
            return Err(Error::Conflict);
        }
        self.require_lease(&lease.holder, lease.fence, now)?;
        self.target.require_current(context, now)?;
        confirmation.validate_for(claim)?;
        if context.agent != &claim.agent {
            return Err(Error::Conflict);
        }
        let mut next = context.agent.clone();
        let status = match confirmation.native_state {
            ConfirmedNativeState::Idle => AgentStatus::Ready,
            ConfirmedNativeState::Working => AgentStatus::Working,
            ConfirmedNativeState::WaitingApproval => AgentStatus::WaitingApproval,
        };
        next.status = status;
        next.failure = None;
        next.updated_at = now.into();
        if next.ready_at.is_none() {
            next.ready_at = Some(now.into());
        }
        next.revision = next.revision.checked_add(1).ok_or(Error::InvalidEvidence)?;
        next.conversation.last_seq = next
            .conversation
            .last_seq
            .checked_add(1)
            .ok_or(Error::InvalidEvidence)?;
        next.validate_conversation()
            .map_err(|_| Error::InvalidEvidence)?;
        let mut work = self.clone();
        work.revision = work.revision.checked_add(1).ok_or(Error::InvalidEvidence)?;
        work.phase = RecoveryPhase::Completed;
        work.completed_at = Some(now.into());
        work.lease = None;
        Ok((
            work,
            next,
            AgentLogKind::StatusChanged {
                from: AgentStatus::Failed,
                to: status,
            },
        ))
    }

    fn require_lease(&self, holder: &str, fence: u64, now: &str) -> Result<(), Error> {
        self.validate()?;
        let now = seconds_from_rfc3339(now).ok_or(Error::InvalidEvidence)?;
        if self.phase != RecoveryPhase::Attaching
            || self.lease.as_ref().is_none_or(|l| {
                l.holder != holder
                    || l.fence != fence
                    || seconds_from_rfc3339(&l.expires_at).is_none_or(|end| now >= end)
            })
        {
            return Err(Error::Conflict);
        }
        Ok(())
    }
}

fn input_digest(a: &Agent) -> Result<String, Error> {
    digest(&(
        "cma.capacity-recovery.input/v1",
        &a.conversation.first_message,
        a.conversation
            .first_message
            .as_ref()
            .and_then(|first| first.common_binding.as_ref()),
        &a.conversation.queued_messages,
        &a.conversation.operation,
        &a.conversation.settings,
        &a.conversation.preferences,
        a.conversation.managed_guidance_revision,
        &a.work.receipts,
    ))
}

fn digest(value: &impl Serialize) -> Result<String, Error> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(value).map_err(|_| Error::InvalidEvidence)?)
    ))
}
