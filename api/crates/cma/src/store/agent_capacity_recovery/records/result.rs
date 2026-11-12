//! Bounded witness for the exact published result, without retaining user input.
//! This witness is untrusted until its digest matches the immutable Publication.
use layer2_core::sandbox::{
    agent_capacity_recovery::state::{RecoveryPhase, RecoveryWork},
    store::SandboxStoreError,
    Agent, AgentLogEntry, AgentLogKind, AgentStatus,
};
use mongodb::bson::{spec::BinarySubtype, Binary};
use serde::{Deserialize, Serialize};

use crate::store::agent_log_segments::publication::{self, Publication};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::store) struct ResultBinding {
    pub agent_digest: Binary,
    pub logs_digest: Binary,
    pub agent_revision: u64,
    pub through_seq: u64,
    pub published_at: String,
}

impl ResultBinding {
    pub(super) fn new(
        agent: &Agent,
        work: &RecoveryWork,
        logs: &[AgentLogEntry],
        published_at: &str,
    ) -> Result<Self, SandboxStoreError> {
        if agent.id != work.request.agent_id
            || agent.creation_key != work.request.creation_key
            || agent.creation_fingerprint != work.request.creation_fingerprint
        {
            return Err(super::invalid());
        }
        let value = Self {
            agent_digest: publication::result_digest(agent)?,
            logs_digest: publication::result_digest(&logs)?,
            agent_revision: agent.revision,
            through_seq: agent.conversation.last_seq,
            published_at: published_at.into(),
        };
        value.validate(work)?;
        value.require_logs(work, logs)?;
        Ok(value)
    }

    pub(super) fn validate(&self, work: &RecoveryWork) -> Result<(), SandboxStoreError> {
        work.validate().map_err(|_| super::invalid())?;
        let valid_digest =
            |digest: &Binary| digest.subtype == BinarySubtype::Generic && digest.bytes.len() == 32;
        if !valid_digest(&self.agent_digest)
            || !valid_digest(&self.logs_digest)
            || self.agent_revision == 0
            || self.agent_revision > i64::MAX as u64
            || self.through_seq < work.request.failure_seq
            || self.through_seq > crate::store::agent_log_segments::MAX_SEQUENCE as u64
            || self.published_at.len() > 64
            || layer2_core::seconds_from_rfc3339(&self.published_at).is_none()
            || (work.phase == RecoveryPhase::Completed
                && work.completed_at.as_deref() != Some(self.published_at.as_str()))
        {
            return Err(super::invalid());
        }
        Ok(())
    }

    pub(in crate::store) fn digest(
        &self,
        work: &RecoveryWork,
    ) -> Result<Binary, SandboxStoreError> {
        self.validate(work)?;
        publication::result_digest(&("cma.capacity-recovery.result/v2", self, work))
    }

    pub(in crate::store) fn require_publication(
        &self,
        work: &RecoveryWork,
        receipt: &Publication,
    ) -> Result<(), SandboxStoreError> {
        receipt.validate()?;
        if receipt.result_digest != self.digest(work)?
            || u64::try_from(receipt.result_revision).ok() != Some(self.agent_revision)
            || u64::try_from(receipt.last_seq).ok() != Some(self.through_seq)
            || receipt.expected_revision.checked_add(1) != Some(receipt.result_revision)
            || receipt.appended_at != self.published_at
            || receipt.last_seq - receipt.expected_seq
                != i64::from(work.phase == RecoveryPhase::Completed)
        {
            return Err(super::invalid());
        }
        Ok(())
    }

    pub(in crate::store) fn require_logs(
        &self,
        work: &RecoveryWork,
        logs: &[AgentLogEntry],
    ) -> Result<(), SandboxStoreError> {
        let valid = if work.phase == RecoveryPhase::Completed {
            matches!(logs, [entry] if entry.agent_id == work.request.agent_id
            && entry.seq == self.through_seq
            && entry.appended_at == self.published_at
            && matches!(entry.kind, AgentLogKind::StatusChanged {
                from: AgentStatus::Failed, to,
            } if match to {
                AgentStatus::Ready => work.target.active_turn.is_none(),
                AgentStatus::Working | AgentStatus::WaitingApproval => work.target.active_turn.is_some(),
                _ => false,
            }))
        } else {
            logs.is_empty()
        };
        if !valid || self.logs_digest != publication::result_digest(&logs)? {
            return Err(super::invalid());
        }
        Ok(())
    }
}
