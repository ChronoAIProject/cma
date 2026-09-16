use std::{sync::Arc, time::Duration};

use crate::sandbox::agent_io::{
    AdmittedAgentCommand, AgentActor, AgentResponseStatus, CommandFailure, CommandFailureCode,
    CommandWorkState,
};

use super::{
    ChannelAgentCommandService, ChannelCommandAdmissionError as Error,
    ChannelCommandExecutionSource, ChannelCommandHold,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelHoldRefusalReason {
    QueueTimeout,
    GrantRevoked,
    TargetMissing,
    BridgeMissing,
    GrantMissing,
}

/// Refusal-only evidence for exactly one unchanged, unleased Channel hold.
/// The store proves current expiry or durable revocation in the same write.
pub struct ChannelHoldRefusal {
    command: AdmittedAgentCommand,
    source: ChannelCommandExecutionSource,
    reason: ChannelHoldRefusalReason,
    deadline: tokio::time::Instant,
}

impl ChannelAgentCommandService {
    pub async fn refuse_execution_hold(
        &self,
        command: &AdmittedAgentCommand,
        reason: ChannelHoldRefusalReason,
    ) -> Result<(), Error> {
        let source = self.store.channel_execution_source(command).await?;
        let refusal = Arc::new(ChannelHoldRefusal {
            command: command.clone(),
            source,
            reason,
            deadline: tokio::time::Instant::now() + Duration::from_secs(15),
        });
        refusal.require_current()?;
        tokio::time::timeout_at(
            refusal.deadline,
            self.store.refuse_channel_hold(refusal.clone()),
        )
        .await
        .map_err(|_| Error::Unavailable)??;
        Ok(())
    }
}

impl ChannelHoldRefusal {
    pub fn command(&self) -> &AdmittedAgentCommand {
        &self.command
    }
    pub fn source(&self) -> &ChannelCommandExecutionSource {
        &self.source
    }
    pub fn reason(&self) -> ChannelHoldRefusalReason {
        self.reason
    }
    pub fn require_current(&self) -> Result<(), Error> {
        if tokio::time::Instant::now() >= self.deadline {
            return Err(Error::Unavailable);
        }
        self.command.validate()?;
        let command = &self.command;
        let source = &self.source;
        let sidecar = &source.sidecar;
        if command.status != AgentResponseStatus::WaitingAction
            || command.work != CommandWorkState::default()
            || !matches!(command.receipt.envelope.actor, AgentActor::Channel { .. })
            || sidecar.schema_version != 1
            || sidecar.execution_hold != ChannelCommandHold::WorkerUnavailable
            || sidecar.command_id != command.receipt.envelope.command_id
            || command.receipt.response_id.as_ref() != Some(&sidecar.response_id)
            || sidecar.request_fingerprint != command.receipt.envelope.request_fingerprint
            || sidecar.inbox_id != source.inbox.id
            || sidecar.body_sha256 != source.inbox.attestation.body_sha256
            || sidecar.allocation_id != source.allocation.id
            || command.receipt.envelope.correlation_id != source.inbox.id
            || command.receipt.envelope.source.reference != source.inbox.id
            || (self.reason == ChannelHoldRefusalReason::QueueTimeout
                && !command.queue_expired(&crate::now_rfc3339()))
        {
            return Err(Error::SourceMismatch);
        }
        Ok(())
    }
    pub fn refused_command(&self) -> Result<AdmittedAgentCommand, Error> {
        self.require_current()?;
        let mut command = self.command.clone();
        command.status = AgentResponseStatus::Failed;
        command.revision = command
            .revision
            .checked_add(1)
            .ok_or(Error::SourceMismatch)?;
        command.work.failure = Some(CommandFailure {
            code: match self.reason {
                ChannelHoldRefusalReason::QueueTimeout => CommandFailureCode::QueueTimeout,
                ChannelHoldRefusalReason::GrantRevoked => CommandFailureCode::AuthorityRevoked,
                ChannelHoldRefusalReason::TargetMissing => CommandFailureCode::AgentUnavailable,
                ChannelHoldRefusalReason::BridgeMissing
                | ChannelHoldRefusalReason::GrantMissing => CommandFailureCode::AuthorityRevoked,
            },
            at: crate::now_rfc3339(),
        });
        command.validate()?;
        Ok(command)
    }
}
