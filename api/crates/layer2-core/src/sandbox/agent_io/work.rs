use serde::{Deserialize, Serialize};

use crate::ids::AgentCommandId;
use crate::sandbox::AgentOperationDelivery;
use crate::{seconds_from_rfc3339, AgentId};

use super::{AdmittedAgentCommand, AgentResponseStatus, CommandLedgerError};

const MAX_COMMAND_LEASE_SECONDS: u64 = 60;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandClaim {
    pub agent_id: AgentId,
    pub command_id: AgentCommandId,
    pub holder: String,
    pub fence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandLease {
    pub claim: CommandClaim,
    pub expires_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandDispatch {
    pub claim: CommandClaim,
    pub operation_sequence: u64,
    pub turn_sequence: u64,
    pub runtime_generation: u64,
    pub fenced_at: String,
    pub delivery: AgentOperationDelivery,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandFailureCode {
    QueueTimeout,
    AuthorityRevoked,
    AuthorityUnavailable,
    CredentialSetupRequired,
    ConnectionsRequired,
    AgentUnavailable,
    TargetTurnChanged,
    RuntimeUnavailable,
    RuntimeLost,
    ExecutionFailed,
    OwnerBudgetExhausted,
    TurnBudgetExhausted,
    BudgetUnavailable,
    PlatformBudgetExhausted,
    SessionCapacityUnavailable,
}

impl CommandFailureCode {
    /// Preserve the predecessor reader contract for new failures and default
    /// public projections. Missing prerequisites prevent execution; they do not
    /// establish revoked authority, a missing Agent, or a transient outage.
    /// Keep the precise variants for diagnostics and reading retained records.
    pub fn budget_error(self) -> Option<crate::budget::BudgetError> {
        use crate::budget::BudgetError;
        match self {
            Self::OwnerBudgetExhausted => Some(BudgetError::OwnerExhausted),
            Self::TurnBudgetExhausted => Some(BudgetError::TurnExhausted),
            Self::BudgetUnavailable => Some(BudgetError::Unavailable),
            Self::PlatformBudgetExhausted => Some(BudgetError::PlatformExhausted),
            _ => None,
        }
    }

    pub fn from_budget(reason: crate::budget::BudgetError) -> Self {
        use crate::budget::BudgetError;
        match reason {
            BudgetError::OwnerExhausted => Self::OwnerBudgetExhausted,
            BudgetError::TurnExhausted => Self::TurnBudgetExhausted,
            BudgetError::Unavailable | BudgetError::InvalidPolicy => Self::BudgetUnavailable,
            BudgetError::PlatformExhausted => Self::PlatformBudgetExhausted,
        }
    }

    pub fn legacy_code(self) -> Self {
        match self {
            Self::CredentialSetupRequired
            | Self::ConnectionsRequired
            | Self::OwnerBudgetExhausted
            | Self::TurnBudgetExhausted
            | Self::BudgetUnavailable
            | Self::PlatformBudgetExhausted
            | Self::SessionCapacityUnavailable => Self::ExecutionFailed,
            code => code,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandFailure {
    pub code: CommandFailureCode,
    pub at: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandWorkState {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deferred_attempts: Vec<super::DeferredCommandAttempt>,
    pub lease: Option<CommandLease>,
    pub dispatch: Option<CommandDispatch>,
    pub failure: Option<CommandFailure>,
    #[serde(default)]
    pub observation: Option<CommandObservation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandObservation {
    pub event_cursor: u64,
    pub delivery: AgentOperationDelivery,
}

#[derive(Debug, Clone)]
pub struct CommandClaimRequest {
    pub agent_id: AgentId,
    pub holder: String,
    pub now: String,
    pub expires_at: String,
}

impl CommandClaimRequest {
    pub fn validate(&self) -> Result<(), CommandLedgerError> {
        let duration = seconds_from_rfc3339(&self.now)
            .zip(seconds_from_rfc3339(&self.expires_at))
            .and_then(|(start, end)| end.checked_sub(start));
        if self.holder.trim().is_empty()
            || self.holder.len() > 256
            || self.holder.chars().any(char::is_control)
            || !duration.is_some_and(|duration| (1..=MAX_COMMAND_LEASE_SECONDS).contains(&duration))
        {
            return Err(CommandLedgerError::Invalid);
        }
        Ok(())
    }
}

impl CommandLease {
    pub fn live_at(&self, now: &str) -> bool {
        seconds_from_rfc3339(now)
            .zip(seconds_from_rfc3339(&self.expires_at))
            .is_some_and(|(now, until)| now < until)
    }
}

impl AdmittedAgentCommand {
    pub fn queue_expired(&self, now: &str) -> bool {
        seconds_from_rfc3339(now)
            .zip(seconds_from_rfc3339(&self.queue_deadline))
            .is_some_and(|(now, deadline)| now >= deadline)
    }

    pub fn require_claim(&self, claim: &CommandClaim, now: &str) -> Result<(), CommandLedgerError> {
        self.validate()?;
        if claim.agent_id != self.receipt.envelope.agent_id
            || claim.command_id != self.receipt.envelope.command_id
            || self.status.is_terminal()
            || !self
                .work
                .lease
                .as_ref()
                .is_some_and(|lease| &lease.claim == claim && lease.live_at(now))
        {
            return Err(CommandLedgerError::StaleClaim);
        }
        Ok(())
    }

    pub fn claim(
        &mut self,
        request: &CommandClaimRequest,
        fence: u64,
    ) -> Result<CommandClaim, CommandLedgerError> {
        request.validate()?;
        if self.status != AgentResponseStatus::Queued
            || (!self.queue_expired(&request.now)
                && !self.continuation_retry_delay(&request.now).is_zero())
            || self.work.dispatch.is_some()
            || self.receipt.envelope.agent_id != request.agent_id
            || fence == 0
            || fence > i64::MAX as u64
            || self
                .work
                .lease
                .as_ref()
                .is_some_and(|lease| lease.live_at(&request.now))
        {
            return Err(CommandLedgerError::Conflict);
        }
        let claim = CommandClaim {
            agent_id: request.agent_id.clone(),
            command_id: self.receipt.envelope.command_id.clone(),
            holder: request.holder.clone(),
            fence,
        };
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or(CommandLedgerError::Corrupt)?;
        self.work.lease = Some(CommandLease {
            claim: claim.clone(),
            expires_at: request.expires_at.clone(),
        });
        Ok(claim)
    }

    pub fn reject_before_dispatch(
        &mut self,
        claim: &CommandClaim,
        code: CommandFailureCode,
        now: String,
    ) -> Result<(), CommandLedgerError> {
        self.require_claim(claim, &now)?;
        if self.work.dispatch.is_some() || self.status != AgentResponseStatus::Queued {
            return Err(CommandLedgerError::Conflict);
        }
        self.record_refusal(code, now)
    }

    /// Refuse waiting work without taking an execution lane. The store must
    /// atomically compare the revision, so a concurrent claim, renewal or
    /// binding always defeats this refusal. An expired lane needs no release.
    pub fn reject_unclaimed(
        &mut self,
        expected_revision: u64,
        code: CommandFailureCode,
        now: String,
    ) -> Result<(), CommandLedgerError> {
        self.validate()?;
        if seconds_from_rfc3339(&now).is_none() {
            return Err(CommandLedgerError::Invalid);
        }
        if self.revision != expected_revision || !self.awaiting_claim(&now) {
            return Err(CommandLedgerError::StaleClaim);
        }
        self.record_refusal(code, now)
    }

    pub fn awaiting_claim(&self, now: &str) -> bool {
        self.status == AgentResponseStatus::Queued
            && self.work.dispatch.is_none()
            && self
                .work
                .lease
                .as_ref()
                .is_none_or(|lease| !lease.live_at(now))
    }

    fn record_refusal(
        &mut self,
        code: CommandFailureCode,
        now: String,
    ) -> Result<(), CommandLedgerError> {
        if code == CommandFailureCode::QueueTimeout && !self.queue_expired(&now) {
            return Err(CommandLedgerError::Invalid);
        }
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or(CommandLedgerError::Corrupt)?;
        self.status = AgentResponseStatus::Failed;
        self.work.lease = None;
        if code != code.legacy_code() {
            tracing::warn!(
                event = "command_refusal",
                command_id = %self.receipt.envelope.command_id.as_str().chars().take(128).collect::<String>(),
                agent_id = %self.receipt.envelope.agent_id.as_str().chars().take(128).collect::<String>(),
                error_kind = ?code,
                "Command prerequisite refused"
            );
        }
        self.work.failure = Some(CommandFailure {
            // Durable stores keep refusal precision in additive sidecars.
            code: if code.budget_error().is_some()
                || code == CommandFailureCode::SessionCapacityUnavailable
            {
                code
            } else {
                code.legacy_code()
            },
            at: now,
        });
        Ok(())
    }
}

pub(super) fn validate_work(record: &AdmittedAgentCommand) -> Result<(), CommandLedgerError> {
    super::continuation_retry::validate_attempts(record)?;
    let work = &record.work;
    if work.observation.as_ref().is_some_and(|observation| {
        work.dispatch
            .as_ref()
            .is_none_or(|dispatch| observation.event_cursor < dispatch.operation_sequence)
    }) {
        return Err(CommandLedgerError::Corrupt);
    }
    if let Some(lease) = &work.lease {
        if lease.claim.fence == 0
            || lease.claim.holder.trim().is_empty()
            || lease.claim.agent_id != record.receipt.envelope.agent_id
            || lease.claim.command_id != record.receipt.envelope.command_id
            || seconds_from_rfc3339(&lease.expires_at).is_none()
            || record.status.is_terminal()
        {
            return Err(CommandLedgerError::Corrupt);
        }
    }
    if work.dispatch.as_ref().is_some_and(|dispatch| {
        dispatch.claim.agent_id != record.receipt.envelope.agent_id
            || dispatch.claim.command_id != record.receipt.envelope.command_id
            || dispatch.claim.fence == 0
            || dispatch.claim.holder.trim().is_empty()
            || dispatch.operation_sequence == 0
            || dispatch.turn_sequence == 0
            || dispatch.runtime_generation == 0
            || seconds_from_rfc3339(&dispatch.fenced_at).is_none()
    }) || (record.status == AgentResponseStatus::Queued
        && (work.dispatch.is_some() || work.failure.is_some()))
        || work
            .failure
            .as_ref()
            .is_some_and(|failure| seconds_from_rfc3339(&failure.at).is_none())
    {
        return Err(CommandLedgerError::Corrupt);
    }
    Ok(())
}
