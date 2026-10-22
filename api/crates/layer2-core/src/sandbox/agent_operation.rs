use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::seconds_from_rfc3339;

use super::SandboxModelError;

const MAX_APPROVAL_ID_BYTES: usize = 512;
const MAX_OPERATION_FAILURE_CODE_BYTES: usize = 128;
const MAX_OPERATION_FAILURE_DETAIL_BYTES: usize = 4 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentApprovalDecision {
    Allow,
    Deny,
    Abort,
}

impl AgentApprovalDecision {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Deny => "deny",
            Self::Abort => "abort",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentOperationKind {
    SendMessage,
    Steer,
    Interrupt,
    AnswerApproval,
    UpdateSettings,
    RefreshContext,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentOperationDelivery {
    DispatchFenced,
    WriteConfirmed,
    ExecutionObserved,
    PreWriteFailed,
    RecoveryRequired,
    RuntimeLost,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentOperationIntent {
    RefreshContext {
        revision: u64,
        readiness_receipt: String,
        guidance: String,
        previous_guidance: Vec<String>,
        settings: super::AgentSettings,
        working_directory: Option<String>,
    },
    UpdateSettings {
        settings: super::AgentSettings,
        previous: Option<super::AgentSettings>,
    },
    SendMessage {
        payload_sha256: String,
    },
    Steer {
        payload_sha256: String,
    },
    Interrupt,
    AnswerApproval {
        approval_id: String,
        decision: AgentApprovalDecision,
    },
}

impl AgentOperationIntent {
    pub fn send_message(content: &str) -> Self {
        Self::SendMessage {
            payload_sha256: hex_sha256(content.as_bytes()),
        }
    }

    pub fn steer(content: &str) -> Self {
        Self::Steer {
            payload_sha256: hex_sha256(content.as_bytes()),
        }
    }

    pub fn answer_approval(
        approval_id: String,
        decision: AgentApprovalDecision,
    ) -> Result<Self, SandboxModelError> {
        let intent = Self::AnswerApproval {
            approval_id,
            decision,
        };
        intent.validate()?;
        Ok(intent)
    }

    pub const fn kind(&self) -> AgentOperationKind {
        match self {
            Self::RefreshContext { .. } => AgentOperationKind::RefreshContext,
            Self::UpdateSettings { .. } => AgentOperationKind::UpdateSettings,
            Self::SendMessage { .. } => AgentOperationKind::SendMessage,
            Self::Steer { .. } => AgentOperationKind::Steer,
            Self::Interrupt => AgentOperationKind::Interrupt,
            Self::AnswerApproval { .. } => AgentOperationKind::AnswerApproval,
        }
    }

    fn validate(&self) -> Result<(), SandboxModelError> {
        let valid = match self {
            Self::RefreshContext {
                revision,
                readiness_receipt,
                guidance,
                previous_guidance,
                settings,
                working_directory,
            } => {
                *revision > 0
                    && valid_sha256(readiness_receipt)
                    && !guidance.is_empty()
                    && guidance.len() <= 16 * 1024
                    && !guidance.contains('\0')
                    && previous_guidance.len() <= 8
                    && previous_guidance
                        .iter()
                        .all(|p| !p.is_empty() && p.len() <= 16 * 1024 && !p.contains('\0'))
                    && settings.validate().is_ok()
                    && working_directory.as_ref().is_none_or(|cwd| {
                        cwd.starts_with('/') && cwd.len() <= 4096 && !cwd.contains('\0')
                    })
            }
            Self::UpdateSettings { settings, previous } => {
                settings.validate().is_ok()
                    && previous
                        .as_ref()
                        .is_none_or(|value| value.validate().is_ok())
            }
            Self::SendMessage { payload_sha256 } | Self::Steer { payload_sha256 } => {
                valid_sha256(payload_sha256)
            }
            Self::Interrupt => true,
            Self::AnswerApproval { approval_id, .. } => {
                !approval_id.trim().is_empty()
                    && approval_id.len() <= MAX_APPROVAL_ID_BYTES
                    && !approval_id.chars().any(char::is_control)
            }
        };
        valid
            .then_some(())
            .ok_or_else(|| SandboxModelError::invalid_value("agent.operation.intent"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentOperationFailure {
    pub code: String,
    pub detail: String,
    pub occurred_at: String,
}

impl AgentOperationFailure {
    pub fn new(
        code: impl Into<String>,
        detail: impl Into<String>,
        occurred_at: impl Into<String>,
    ) -> Result<Self, SandboxModelError> {
        let failure = Self {
            code: code.into(),
            detail: detail.into(),
            occurred_at: occurred_at.into(),
        };
        failure.validate()?;
        Ok(failure)
    }

    fn validate(&self) -> Result<(), SandboxModelError> {
        if self.code.trim().is_empty()
            || self.code.len() > MAX_OPERATION_FAILURE_CODE_BYTES
            || self.detail.trim().is_empty()
            || self.detail.len() > MAX_OPERATION_FAILURE_DETAIL_BYTES
            || seconds_from_rfc3339(&self.occurred_at).is_none()
        {
            return Err(SandboxModelError::invalid_value("agent.operation.failure"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentOperationRecord {
    pub sequence: u64,
    pub runtime_generation: u64,
    pub turn_sequence: u64,
    pub intent: AgentOperationIntent,
    pub state: AgentOperationDelivery,
    pub dispatch_fenced_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub write_confirmed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_observed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<AgentOperationFailure>,
}

impl AgentOperationRecord {
    pub fn new(
        sequence: u64,
        runtime_generation: u64,
        turn_sequence: u64,
        intent: AgentOperationIntent,
        dispatch_fenced_at: String,
    ) -> Result<Self, SandboxModelError> {
        let record = Self {
            sequence,
            runtime_generation,
            turn_sequence,
            intent,
            state: AgentOperationDelivery::DispatchFenced,
            dispatch_fenced_at,
            write_confirmed_at: None,
            execution_observed_at: None,
            failed_at: None,
            failure: None,
        };
        record.validate()?;
        Ok(record)
    }

    pub fn validate(&self) -> Result<(), SandboxModelError> {
        self.intent.validate()?;
        if self.sequence == 0
            || self.runtime_generation == 0
            || self.turn_sequence == 0
            || seconds_from_rfc3339(&self.dispatch_fenced_at).is_none()
            || self
                .write_confirmed_at
                .as_deref()
                .is_some_and(|value| seconds_from_rfc3339(value).is_none())
            || self
                .execution_observed_at
                .as_deref()
                .is_some_and(|value| seconds_from_rfc3339(value).is_none())
            || self
                .failed_at
                .as_deref()
                .is_some_and(|value| seconds_from_rfc3339(value).is_none())
            || self
                .failure
                .as_ref()
                .is_some_and(|failure| failure.validate().is_err())
        {
            return Err(SandboxModelError::invalid_value("agent.operation"));
        }
        let no_failure = self.failed_at.is_none() && self.failure.is_none();
        let valid_shape = match self.state {
            AgentOperationDelivery::DispatchFenced => {
                self.write_confirmed_at.is_none()
                    && self.execution_observed_at.is_none()
                    && no_failure
            }
            AgentOperationDelivery::WriteConfirmed => {
                self.write_confirmed_at.is_some()
                    && self.execution_observed_at.is_none()
                    && no_failure
            }
            AgentOperationDelivery::ExecutionObserved => {
                self.execution_observed_at.is_some() && no_failure
            }
            AgentOperationDelivery::PreWriteFailed => {
                self.write_confirmed_at.is_none()
                    && self.execution_observed_at.is_none()
                    && self.failed_at.is_some()
                    && self.failure.is_some()
            }
            AgentOperationDelivery::RecoveryRequired | AgentOperationDelivery::RuntimeLost => {
                self.execution_observed_at.is_none()
                    && self.failed_at.is_some()
                    && self.failure.is_some()
            }
        };
        valid_shape
            .then_some(())
            .ok_or_else(|| SandboxModelError::invalid_value("agent.operation.state"))
    }

    pub fn mark_write_confirmed(&mut self, at: String) -> Result<(), SandboxModelError> {
        if !matches!(
            self.state,
            AgentOperationDelivery::DispatchFenced | AgentOperationDelivery::RecoveryRequired
        ) {
            return Err(SandboxModelError::invalid_value("agent.operation.write"));
        }
        self.clear_failure();
        self.state = AgentOperationDelivery::WriteConfirmed;
        self.write_confirmed_at = Some(at);
        self.validate()
    }

    pub fn mark_execution_observed(&mut self, at: String) -> Result<(), SandboxModelError> {
        if !matches!(
            self.state,
            AgentOperationDelivery::DispatchFenced
                | AgentOperationDelivery::WriteConfirmed
                | AgentOperationDelivery::RecoveryRequired
        ) {
            return Err(SandboxModelError::invalid_value(
                "agent.operation.execution",
            ));
        }
        self.clear_failure();
        self.state = AgentOperationDelivery::ExecutionObserved;
        self.execution_observed_at = Some(at);
        self.validate()
    }

    pub fn mark_pre_write_failed(
        &mut self,
        failure: AgentOperationFailure,
    ) -> Result<(), SandboxModelError> {
        self.require_fenced()?;
        self.state = AgentOperationDelivery::PreWriteFailed;
        self.failed_at = Some(failure.occurred_at.clone());
        self.failure = Some(failure);
        self.validate()
    }

    pub fn require_recovery(
        &mut self,
        failure: AgentOperationFailure,
    ) -> Result<(), SandboxModelError> {
        self.require_fenced()?;
        self.state = AgentOperationDelivery::RecoveryRequired;
        self.failed_at = Some(failure.occurred_at.clone());
        self.failure = Some(failure);
        self.validate()
    }

    pub const fn blocks_dispatch(&self) -> bool {
        matches!(
            self.state,
            AgentOperationDelivery::DispatchFenced | AgentOperationDelivery::RecoveryRequired
        )
    }

    /// Retire an unresolved write when its runtime is gone, preserving uncertainty.
    pub fn retire_lost_runtime(
        &mut self,
        failure: AgentOperationFailure,
    ) -> Result<(), SandboxModelError> {
        if !self.blocks_dispatch() {
            return Err(SandboxModelError::invalid_value("agent.operation.state"));
        }
        self.state = AgentOperationDelivery::RuntimeLost;
        self.failed_at = Some(failure.occurred_at.clone());
        self.failure = Some(failure);
        self.validate()
    }

    fn require_fenced(&self) -> Result<(), SandboxModelError> {
        (self.state == AgentOperationDelivery::DispatchFenced)
            .then_some(())
            .ok_or_else(|| SandboxModelError::invalid_value("agent.operation.state"))
    }

    fn clear_failure(&mut self) {
        self.failed_at = None;
        self.failure = None;
    }
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn hex_sha256(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}
