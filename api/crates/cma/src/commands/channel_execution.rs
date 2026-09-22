use layer2_core::sandbox::agent_io::*;
use layer2_core::sandbox::SandboxStatus;

use super::execution::ResourceCommandExecution;

pub(super) fn is_channel(command: &AdmittedAgentCommand) -> bool {
    matches!(command.receipt.envelope.actor, AgentActor::Channel { .. })
        || command.receipt.envelope.source.kind == AgentSourceKind::Channel
}

impl ResourceCommandExecution {
    pub(super) async fn observe_channel_readiness(
        &self,
        command: &AdmittedAgentCommand,
    ) -> CommandReadiness {
        let (Some(channel), Some(invokes)) = (&self.channel, &self.invokes) else {
            return observe_readiness("composition", "unavailable", CommandReadiness::Retry);
        };
        match channel.observe_execution_readiness(invokes, command).await {
            Ok(snapshot) => require_live_target(&snapshot.lineage)
                .map(|()| CommandReadiness::Ready)
                .unwrap_or_else(|readiness| readiness),
            Err(error) => readiness("observe_execution_readiness", error),
        }
    }

    pub(super) async fn channel_checked(
        &self,
        command: &AdmittedAgentCommand,
    ) -> Result<ChannelExecutionAuthorization, CommandReadiness> {
        let (Some(channel), Some(invokes)) = (&self.channel, &self.invokes) else {
            return Err(observe_readiness(
                "composition",
                "unavailable",
                CommandReadiness::Retry,
            ));
        };
        let authority = channel
            .authorize_execution(invokes, command)
            .await
            .map_err(|error| readiness("authorize_execution", error))?;
        authority
            .fence()
            .require_dispatch(command)
            .map_err(|error| readiness("dispatch_fence", error))?;
        let fence = authority.fence();
        require_live_target(&fence.proof().resources().lineage)?;
        Ok(authority)
    }

    pub(super) async fn execute_channel(&self, command: &AdmittedAgentCommand) -> CommandAttempt {
        let authority = match self.channel_checked(command).await {
            Ok(authority) => authority,
            Err(CommandReadiness::Reject(code)) => return CommandAttempt::Reject(code),
            Err(_) => return CommandAttempt::Retry,
        };
        self.execute_channel_authorized(command, authority).await
    }

    pub(super) async fn execute_channel_authorized(
        &self,
        command: &AdmittedAgentCommand,
        authority: ChannelExecutionAuthorization,
    ) -> CommandAttempt {
        // Lease renewal may have advanced its expiry during provider IO. Reuse
        // the current lease, but never substitute a new claim or receipt.
        let Ok(Some(current)) = self
            .store
            .get_command(&command.receipt.envelope.command_id)
            .await
        else {
            return CommandAttempt::Retry;
        };
        if current.receipt != command.receipt
            || current.key_sha256 != command.key_sha256
            || current.target_turn != command.target_turn
            || current.work.lease.as_ref().map(|lease| &lease.claim)
                != command.work.lease.as_ref().map(|lease| &lease.claim)
            || current.work.dispatch.is_some()
            || current.status.is_terminal()
        {
            return CommandAttempt::Retry;
        }
        let started = tokio::time::Instant::now();
        let result = match self
            .operations
            .execute_channel_claim(&authority, &current)
            .await
        {
            Ok(_) => CommandAttempt::Finished,
            Err(error) => super::execution::classify(error),
        };
        tracing::info!(
            event = "channel_operation",
            operation = "command_execution",
            stage = "claimed_dispatch",
            command_id = %command.receipt.envelope.command_id,
            outcome = match result {
                CommandAttempt::Finished => "finished",
                CommandAttempt::Retry => "retry",
                CommandAttempt::Reject(_) => "rejected",
            },
            elapsed_ms = started.elapsed().as_millis() as u64,
            "Channel claimed execution attempt finished"
        );
        result
    }
}

fn require_live_target(
    lineage: &layer2_core::resource_access::ResourceLineage,
) -> Result<(), CommandReadiness> {
    let agent = lineage
        .agent
        .as_ref()
        .ok_or_else(|| observe_readiness("target", "agent_missing", CommandReadiness::Retry))?;
    if !agent.status.is_live()
        || agent.requested_stop_reason.is_some()
        || matches!(
            lineage.sandbox.status,
            SandboxStatus::Failed | SandboxStatus::Retiring | SandboxStatus::Retired
        )
    {
        return Err(observe_readiness(
            "target",
            "agent_unavailable",
            CommandReadiness::Reject(CommandFailureCode::AgentUnavailable),
        ));
    }
    Ok(())
}

fn readiness(stage: &'static str, error: ChannelCommandAdmissionError) -> CommandReadiness {
    use layer2_core::bridge::invoke::BridgeInvokeError;
    use layer2_core::bridge::BridgeError;
    let reason = match &error {
        ChannelCommandAdmissionError::Invoke(error) => match error {
            BridgeInvokeError::Bridge(error) => match error {
                BridgeError::NotFound => "bridge_not_found",
                BridgeError::Forbidden => "bridge_forbidden",
                BridgeError::AuthorityUnavailable => "bridge_authority_unavailable",
                BridgeError::InvalidConfiguration(_) => "bridge_invalid_configuration",
                BridgeError::Conflict => "bridge_conflict",
                BridgeError::IdempotencyConflict => "bridge_idempotency_conflict",
                BridgeError::ImmutableIdentity => "bridge_immutable_identity",
                BridgeError::RebindingPolicyRequired => "bridge_rebinding_policy_required",
                BridgeError::Retired => "bridge_retired",
                BridgeError::ConnectionRequired(_) => "bridge_connection_required",
                BridgeError::AdmissionStopped => "bridge_admission_stopped",
                BridgeError::Unavailable => "bridge_unavailable",
                BridgeError::Corrupt => "bridge_corrupt",
                BridgeError::HistoryLimit => "bridge_history_limit",
            },
            BridgeInvokeError::NotFound => "invoke_not_found",
            BridgeInvokeError::Forbidden => "invoke_forbidden",
            BridgeInvokeError::Conflict => "invoke_conflict",
            BridgeInvokeError::IdempotencyConflict => "invoke_idempotency_conflict",
            BridgeInvokeError::Revoked => "invoke_revoked",
            BridgeInvokeError::TargetMismatch => "invoke_target_mismatch",
            BridgeInvokeError::Suspended => "invoke_suspended",
            BridgeInvokeError::Invalid(_) => "invoke_invalid",
            BridgeInvokeError::Unavailable => "invoke_unavailable",
            BridgeInvokeError::Corrupt => "invoke_corrupt",
        },
        ChannelCommandAdmissionError::Partition(_) => "partition",
        ChannelCommandAdmissionError::Ledger(error) => error.code(),
        ChannelCommandAdmissionError::Input(error) => error.code(),
        ChannelCommandAdmissionError::Refused(_) => "refused",
        ChannelCommandAdmissionError::SourceMismatch => "source_mismatch",
        ChannelCommandAdmissionError::Unavailable => "unavailable",
    };
    let result = match error {
        ChannelCommandAdmissionError::Invoke(BridgeInvokeError::Revoked) => {
            CommandReadiness::Reject(CommandFailureCode::AuthorityRevoked)
        }
        ChannelCommandAdmissionError::Invoke(BridgeInvokeError::NotFound) => {
            CommandReadiness::Reject(CommandFailureCode::AgentUnavailable)
        }
        _ => CommandReadiness::Retry,
    };
    observe_readiness(stage, reason, result)
}

fn observe_readiness(
    stage: &'static str,
    reason: &'static str,
    readiness: CommandReadiness,
) -> CommandReadiness {
    tracing::info!(
        event = "channel_operation",
        operation = "command_readiness",
        stage,
        reason,
        outcome = if matches!(readiness, CommandReadiness::Reject(_)) {
            "rejected"
        } else {
            "retry"
        },
        "Channel execution readiness unavailable"
    );
    readiness
}
