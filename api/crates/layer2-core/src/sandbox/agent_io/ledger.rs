//! Durable reservation port. A reservation stores input and its response
//! identity together; it grants no permission to dispatch the runtime input.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::ids::{AgentCommandId, AgentResponseId};
use crate::sandbox::store::SandboxMutationGuard;
use crate::sandbox::{Agent, AgentStatus, Sandbox, SandboxStatus};
use crate::RuntimeCapability;

use super::{AdmissionReceipt, AgentCommandEnvelope, AgentOwner, AgentResponseStatus};

pub const MAX_COMMAND_QUEUE_CAPACITY: u16 = 64;

#[derive(Debug, Clone)]
pub struct CommandReservation {
    /// Live binding/conversation proof, consumed inside the reservation transaction.
    pub binding: Option<crate::sandbox::binding::BindingConversationAuthority>,
    pub envelope: AgentCommandEnvelope,
    pub key_sha256: String,
    pub response_id: Option<AgentResponseId>,
    pub admitted_at: String,
    pub queue_deadline: String,
    pub queue_capacity: u16,
    pub expected_agent_revision: u64,
    pub parent: SandboxMutationGuard,
    /// Current resource authorization, distinct from credential/grant revision.
    /// Every new reservation proves this fence atomically with resource guards.
    pub policy: crate::resource_access::ResourcePolicyFence,
    /// Nonrenewing live authorization; internal only, never deserialized from a
    /// client or retained as permission for a later execution attempt.
    pub authority_deadline: Option<tokio::time::Instant>,
    /// Historical evidence only; never a substitute for the live policy fence.
    pub admission_audit: Option<crate::resource_access::ResourceAuthorizationAudit>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdmittedAgentCommand {
    #[serde(skip)]
    pub queued_input: Option<super::QueuedInputRevision>,
    pub receipt: AdmissionReceipt,
    pub key_sha256: String,
    pub queue_deadline: String,
    pub status: AgentResponseStatus,
    pub revision: u64,
    #[serde(default)]
    pub target_turn: Option<u64>,
    #[serde(default)]
    pub work: super::CommandWorkState,
}

impl AdmittedAgentCommand {
    pub fn validate(&self) -> Result<(), CommandLedgerError> {
        self.receipt
            .validate_integrity()
            .map_err(|_| CommandLedgerError::Corrupt)?;
        if !valid_digest(&self.key_sha256)
            || self.revision == 0
            || !ordered_times(&self.receipt.admitted_at, &self.queue_deadline)
        {
            return Err(CommandLedgerError::Corrupt);
        }
        if let Some(proof) = &self.receipt.envelope.continuation {
            let admitted = crate::seconds_from_rfc3339(&self.receipt.admitted_at)
                .ok_or(CommandLedgerError::Corrupt)?;
            let deadline = crate::seconds_from_rfc3339(&self.queue_deadline)
                .ok_or(CommandLedgerError::Corrupt)?;
            if deadline > proof.source_expires_at as u64
                || deadline > admitted.saturating_add(86_400)
            {
                return Err(CommandLedgerError::Corrupt);
            }
        }
        if let Some(revision) = &self.queued_input {
            revision.validate(self)?;
        }
        super::work::validate_work(self)
    }

    pub fn scope(&self) -> String {
        let envelope = &self.receipt.envelope;
        envelope
            .authorization
            .idempotency_scope(&envelope.agent_id, envelope.command.kind())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandReservationResult {
    Created(Box<AdmittedAgentCommand>),
    Existing(Box<AdmittedAgentCommand>),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CommandLedgerError {
    #[error(transparent)]
    Budget(crate::budget::BudgetError),
    #[error("new work is temporarily paused; existing work continues")]
    AdmissionPaused,
    #[error("active session capacity is unavailable")]
    SessionCapacityUnavailable,
    #[error("invalid command reservation")]
    Invalid,
    #[error("command authority or Agent is unavailable")]
    NotFound,
    #[error("command key was already used with different input")]
    IdempotencyConflict,
    #[error("Agent admission observation is stale")]
    Conflict,
    #[error("response control reservation is temporarily busy")]
    ControlBusy,
    /// The reservation transaction was explicitly aborted and its abort was
    /// acknowledged before any commit could be sent. Callers may refresh the
    /// observation within the original request's retry budget.
    #[error("Agent admission reservation was conclusively aborted")]
    KnownAborted,
    #[error("command worker claim is stale")]
    StaleClaim,
    #[error("Agent cannot accept this command")]
    NotAdmissible,
    #[error("Agent command queue is full")]
    QueueFull,
    #[error("command ledger is corrupt")]
    Corrupt,
    #[error("command ledger is unavailable")]
    Unavailable,
}

impl CommandLedgerError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Budget(reason) => reason.code(),
            Self::AdmissionPaused => "admission_paused",
            Self::SessionCapacityUnavailable => "session_capacity_unavailable",
            Self::Invalid => "invalid_agent_command",
            Self::NotFound => "agent_not_found",
            Self::IdempotencyConflict => "agent_command_key_conflict",
            Self::Conflict => "agent_command_admission_conflict",
            Self::ControlBusy => "response_control_busy",
            Self::KnownAborted => "agent_command_admission_conflict",
            Self::StaleClaim => "agent_command_claim_stale",
            Self::NotAdmissible => "agent_command_not_admissible",
            Self::QueueFull => "agent_command_queue_full",
            Self::Corrupt => "agent_command_ledger_corrupt",
            Self::Unavailable => "agent_command_ledger_unavailable",
        }
    }
}

#[async_trait]
pub trait AgentCommandStore: Send + Sync {
    /// One transaction admits cancellation and its prioritized new response.
    async fn reserve_interrupting_message(
        &self,
        _request: super::InterruptingMessageReservation,
    ) -> Result<super::AdmittedInterruptingMessage, CommandLedgerError> {
        Err(CommandLedgerError::Unavailable)
    }

    /// Atomically cancel an exact unbound response and retain its effect receipt.
    /// Older stores cannot claim that a cancellation was applied.
    async fn cancel_queued_response(
        &self,
        _request: super::QueuedCancellationReservation,
    ) -> Result<super::QueuedCancellationOutcome, CommandLedgerError> {
        Err(CommandLedgerError::Unavailable)
    }

    async fn edit_queued_input(
        &self,
        _request: super::QueuedInputReservation,
    ) -> Result<super::QueuedInputOutcome, CommandLedgerError> {
        Err(CommandLedgerError::Unavailable)
    }
    async fn list_queued_inputs(
        &self,
        _agent_id: &crate::AgentId,
    ) -> Result<Vec<AdmittedAgentCommand>, CommandLedgerError> {
        Err(CommandLedgerError::Unavailable)
    }

    /// Atomic explicit-target admission; older store compositions fail closed.
    async fn reserve_targeted_control(
        &self,
        _request: super::TargetedControlReservation,
    ) -> Result<super::AdmittedTargetedControl, CommandLedgerError> {
        Err(CommandLedgerError::Unavailable)
    }

    /// Recover an exact targeted receipt under a fresh mutation observation.
    /// This never creates work and still fences current resource authority.
    async fn lookup_targeted_control(
        &self,
        _request: super::TargetedControlReservation,
    ) -> Result<Option<super::AdmittedTargetedControl>, CommandLedgerError> {
        Err(CommandLedgerError::Unavailable)
    }

    /// Recover an existing receipt, or prove an absent original intent is stale.
    /// Older stores retain lookup semantics and cannot manufacture refusal proof.
    async fn inspect_targeted_control(
        &self,
        request: super::TargetedControlReservation,
    ) -> Result<super::TargetedControlInspection, CommandLedgerError> {
        Ok(match self.lookup_targeted_control(request).await? {
            Some(existing) => super::TargetedControlInspection::Existing(Box::new(existing)),
            None => super::TargetedControlInspection::Missing,
        })
    }

    /// Read the permanent scope/key index without reserving, changing queue
    /// order or creating a receipt. Unsupported older compositions fail closed.
    async fn get_admitted_command(
        &self,
        _scope: &str,
        _key_sha256: &str,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        Err(CommandLedgerError::Unavailable)
    }

    /// Persistence-only permanent external-event lookup. The service must prove
    /// current binding and conversation access before exposing the payload.
    async fn get_external_event_command(
        &self,
        _key: &str,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        Err(CommandLedgerError::Unavailable)
    }

    /// Atomically deduplicate, assign a per-Agent order, check the bounded queue
    /// and retain the receipt. No TTL may silently make an old key reusable.
    /// `KnownAborted` proves an acknowledged pre-commit abort and permits a
    /// bounded refresh. A generic `Conflict` is lookup-only. An uncertain commit must
    /// return `Unavailable` so callers retain the original key for recovery.
    async fn reserve_command(
        &self,
        request: CommandReservation,
    ) -> Result<CommandReservationResult, CommandLedgerError>;

    /// Persistence only; the calling service must authorize every status read.
    async fn get_command(
        &self,
        id: &AgentCommandId,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError>;

    /// Persistence-only exact dispatch lookup. Duplicate operation bindings are corrupt.
    async fn get_operation_command(
        &self,
        _agent_id: &crate::AgentId,
        _operation_sequence: u64,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        Err(CommandLedgerError::Unavailable)
    }

    /// Resolve the response index only. The service authorizes the associated
    /// resource before exposing any command/response status or receipt.
    async fn get_response_command(
        &self,
        id: &AgentResponseId,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError>;
}

pub fn command_key_digest(key: &str) -> Result<String, CommandLedgerError> {
    if key.trim().is_empty() || key.len() > 256 || key.chars().any(char::is_control) {
        return Err(CommandLedgerError::Invalid);
    }
    Ok(format!("{:x}", Sha256::digest(key.as_bytes())))
}

impl CommandReservation {
    pub fn scope(&self) -> String {
        self.envelope
            .authorization
            .idempotency_scope(&self.envelope.agent_id, self.envelope.command.kind())
    }

    pub fn validate(&self) -> Result<(), CommandLedgerError> {
        self.require_current_authority()?;
        self.validate_binding()?;
        self.envelope
            .validate_integrity()
            .map_err(|_| CommandLedgerError::Invalid)?;
        if !valid_digest(&self.key_sha256)
            || !(1..=MAX_COMMAND_QUEUE_CAPACITY).contains(&self.queue_capacity)
            || self.expected_agent_revision == 0
            || self.policy.sandbox_id != self.parent.sandbox_id
            || crate::seconds_from_rfc3339(&self.policy.sandbox_created_at).is_none()
            || self.policy.policy_revision > i64::MAX as u64
            || !ordered_times(&self.admitted_at, &self.queue_deadline)
        {
            return Err(CommandLedgerError::Invalid);
        }
        self.record(1, 0, None)?.validate()
    }

    pub fn require_current_authority(&self) -> Result<(), CommandLedgerError> {
        if self
            .authority_deadline
            .is_some_and(|deadline| tokio::time::Instant::now() >= deadline)
        {
            return Err(CommandLedgerError::Conflict);
        }
        Ok(())
    }

    pub fn record(
        &self,
        order: u64,
        cursor: u64,
        target_turn: Option<u64>,
    ) -> Result<AdmittedAgentCommand, CommandLedgerError> {
        if order == 0 || order > i64::MAX as u64 {
            return Err(CommandLedgerError::Corrupt);
        }
        Ok(AdmittedAgentCommand {
            queued_input: None,
            receipt: AdmissionReceipt {
                envelope: self.envelope.clone(),
                response_id: self.response_id.clone(),
                admitted_at: self.admitted_at.clone(),
                agent_order: order,
                event_cursor: cursor,
            },
            key_sha256: self.key_sha256.clone(),
            queue_deadline: self.queue_deadline.clone(),
            // Existing status/work variants remain readable by old workers,
            // which cannot execute this unbound wait. Current selectors also
            // preserve its normal-message queue position. Exact hold identity
            // lives in the versioned Channel sidecar, outside the ordinary record.
            status: if matches!(self.envelope.actor, super::AgentActor::Channel { .. })
                || self.envelope.source.kind == super::AgentSourceKind::Channel
            {
                AgentResponseStatus::WaitingAction
            } else {
                AgentResponseStatus::Queued
            },
            revision: 1,
            target_turn,
            work: Default::default(),
        })
    }

    pub fn replay(
        &self,
        current: AdmittedAgentCommand,
    ) -> Result<CommandReservationResult, CommandLedgerError> {
        current.validate()?;
        if current.scope() != self.scope() || current.key_sha256 != self.key_sha256 {
            return Err(CommandLedgerError::Corrupt);
        }
        if current.receipt.envelope.owner != self.envelope.owner {
            return Err(CommandLedgerError::NotFound);
        }
        if current.receipt.envelope.request_fingerprint != self.envelope.request_fingerprint
            || current.receipt.envelope.continuation != self.envelope.continuation
        {
            return Err(CommandLedgerError::IdempotencyConflict);
        }
        Ok(CommandReservationResult::Existing(Box::new(current)))
    }
}

/// The store rechecks these observations in its reservation transaction.
/// Owner kind is established by authorization, never inferred from a subject.
pub fn validate_command_reservation(
    request: &CommandReservation,
    agent: &Agent,
    parent: &Sandbox,
) -> Result<(), CommandLedgerError> {
    let owner = match &request.envelope.owner {
        AgentOwner::Person { subject } | AgentOwner::Organization { subject } => subject,
    };
    if request.envelope.agent_id != agent.id
        || owner != &agent.owner
        || parent.id != agent.sandbox_id
        || parent.owner != agent.owner
    {
        return Err(CommandLedgerError::NotFound);
    }
    // A closed Agent cannot become eligible by refreshing an observation. Exact
    // accepted-key replay has already returned before this new-admission check.
    if !agent.status.is_live()
        || request
            .envelope
            .command
            .task_acceptance()
            .is_some_and(|intent| {
                !intent.eligible(agent.conversation.observations.task_acceptance.as_ref())
            })
    {
        return Err(CommandLedgerError::NotAdmissible);
    }
    if request.expected_agent_revision != agent.revision
        || request.parent != SandboxMutationGuard::capture(parent)
    {
        return Err(CommandLedgerError::Conflict);
    }
    if !matches!(
        agent.status,
        AgentStatus::Starting
            | AgentStatus::Ready
            | AgentStatus::Working
            | AgentStatus::WaitingApproval
            | AgentStatus::Paused
    ) || agent.requested_stop_reason.is_some()
        || !(matches!(
            parent.status,
            SandboxStatus::Active
                | SandboxStatus::CredentialRotating
                | SandboxStatus::Pausing
                | SandboxStatus::Paused
                | SandboxStatus::Reviving
                | SandboxStatus::Rotating
        ) || (matches!(
            request.envelope.command,
            super::AgentCommand::Message { .. }
        ) && request.binding.is_none()
            && agent.bootstrap_operation(parent).is_some()))
        || !agent.capabilities.supports(RuntimeCapability::Conversation)
    {
        return Err(CommandLedgerError::NotAdmissible);
    }
    let required_control = match request.envelope.command.kind() {
        crate::sandbox::AgentOperationKind::Steer => Some(RuntimeCapability::Steer),
        crate::sandbox::AgentOperationKind::Interrupt => Some(RuntimeCapability::Interrupt),
        _ => None,
    };
    if required_control.is_some_and(|capability| !agent.capabilities.supports(capability)) {
        return Err(CommandLedgerError::NotAdmissible);
    }
    if !matches!(
        request.envelope.command,
        super::AgentCommand::Message { .. }
    ) && agent.conversation.active_turn.is_none()
    {
        return Err(CommandLedgerError::NotAdmissible);
    }
    Ok(())
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn ordered_times(start: &str, end: &str) -> bool {
    crate::seconds_from_rfc3339(start)
        .zip(crate::seconds_from_rfc3339(end))
        .is_some_and(|(start, end)| end > start)
}

impl CommandReservation {
    /// Historical channel receipts remain readable; CMAEG admits new work through provider bindings.
    pub fn require_non_channel(&self) -> Result<(), CommandLedgerError> {
        if matches!(self.envelope.actor, super::AgentActor::Channel { .. })
            || self.envelope.source.kind == super::AgentSourceKind::Channel
        {
            return Err(CommandLedgerError::Invalid);
        }
        Ok(())
    }
}
