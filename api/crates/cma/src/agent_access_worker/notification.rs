use super::*;
use layer2_core::sandbox::agent_io::{
    AgentCommand, CommandAdmissionError, CommandLedgerError, CommandSubmission,
    CommandSubmissionSource,
};
use layer2_core::sandbox::{Agent, AgentOperationDelivery, Sandbox};

impl AgentAccessWorker {
    pub(super) async fn notify(
        &self,
        record: &AgentAccessRecord,
        context: &CallerContext,
        credential: &str,
        agent: &Agent,
        sandbox: &Sandbox,
    ) -> Result<(), AgentAccessError> {
        let lease = record.lease.as_ref().ok_or(AgentAccessError::Conflict)?;
        let status = if let Some(id) = &record.progress.notification_command_id {
            self.commands
                .command_status(context, Some(credential), id)
                .await
                .map_err(command_error)?
        } else {
            // Original creation/conversation was checked by the lineage read;
            // admission binds that exact Agent revision in its transaction.
            // A reply lost after admission replays the same key and message.
            // CommandReservation::replay deliberately excludes the observation
            // revision, so normal Agent progress cannot create a second turn.
            let submission = CommandSubmission {
                agent_id: record.requester.agent_id.clone(),
                command: AgentCommand::message(notification(record)?),
                idempotency_key: format!("agent-access:{}:ready", record.id),
                expected_agent_revision: Some(agent.revision),
                requested_model: None,
            };
            // Recover an admission whose reply was lost even if the runtime has
            // since paused or its credential has rotated. This only reads the
            // original command and cannot create another notification.
            let status = match self
                .commands
                .lookup_admitted(
                    context,
                    credential,
                    CommandSubmissionSource::Api,
                    submission.clone(),
                )
                .await
                .map_err(command_error)?
            {
                Some(status) => status,
                None => {
                    if matches!(
                        sandbox.status,
                        SandboxStatus::Pausing | SandboxStatus::Paused
                    ) {
                        // A later explicit pause must not be undone by a
                        // fresh notification command's implicit revival.
                        return Err(AgentAccessError::Invalid);
                    }
                    if !notification_ready(record, agent, sandbox)? {
                        return Err(AgentAccessError::Conflict);
                    }
                    self.commands
                        .admit(
                            context,
                            credential,
                            CommandSubmissionSource::Api,
                            submission,
                        )
                        .await
                        .map_err(command_error)?
                }
            };
            let mut progress = record.progress.clone();
            progress.notification_command_id = Some(status.receipt.command_id.clone());
            self.store
                .advance_access_request(lease, Phase::Notifying, Phase::Notifying, progress)
                .await?;
            // A fresh leased read on the next tick includes the durable receipt.
            return Ok(());
        };
        if matches!(
            status.delivery,
            Some(
                AgentOperationDelivery::WriteConfirmed | AgentOperationDelivery::ExecutionObserved
            )
        ) {
            self.advance(record, Phase::Succeeded).await?;
        } else if status.status.is_terminal() {
            self.store
                .fail_access_request(lease, "agent_notification_not_delivered")
                .await?;
        }
        Ok(())
    }
}

fn notification_ready(
    record: &AgentAccessRecord,
    agent: &Agent,
    sandbox: &Sandbox,
) -> Result<bool, AgentAccessError> {
    let delivered = record
        .progress
        .delivered_credential
        .as_ref()
        .ok_or(AgentAccessError::Invalid)?;
    let Some(current) = &sandbox.credential.current else {
        return Ok(false);
    };
    // Later normal credential rotation remains valid. A different authority at
    // the same generation or a key predating the proved delivery never does.
    let valid_generation = current == delivered || current.generation > delivered.generation;
    let unexpired = layer2_core::seconds_from_rfc3339(&current.expires_at).is_some_and(|expiry| {
        layer2_core::seconds_from_rfc3339(&layer2_core::now_rfc3339())
            .is_some_and(|now| expiry > now)
    });
    let same_hot_runtime = record.progress.hot_update.as_ref().is_none_or(|hot| {
        sandbox.runtime.as_ref() == Some(&hot.runtime)
            && sandbox.lifecycle.next_authority == hot.lifecycle_next_authority
    });
    Ok(same_hot_runtime
        && sandbox.status == SandboxStatus::Active
        && effects::settled(sandbox)
        && sandbox.runtime.is_some()
        && sandbox.credential.delivered
        && valid_generation
        && unexpired
        && agent.id == record.requester.agent_id
        && sandbox.id == record.requester.sandbox_id
        && agent.conversation.conversation_id.as_deref() == Some(&record.requester.conversation)
        && record
            .approved_services()?
            .iter()
            .all(|service| sandbox.credential.effective_reaches().contains(service))
        && agent.conversation.runtime_generation
            == sandbox
                .runtime
                .as_ref()
                .and_then(|value| value.provision_generation))
}

#[cfg(test)]
#[path = "notification_tests.rs"]
mod tests;

fn notification(record: &AgentAccessRecord) -> Result<String, AgentAccessError> {
    let services = record
        .approved_services()?
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    Ok(format!("The owner approved service access request {}. The updated agent key has been delivered to this Sandbox with access to: {}. Continue the original task in this conversation and retry the service operation that needed access. Do not print or reveal credentials.", record.id, services))
}

fn command_error(error: CommandAdmissionError) -> AgentAccessError {
    match error {
        CommandAdmissionError::Authority(CommandAuthorizationError::Revoked) => {
            AgentAccessError::Forbidden
        }
        CommandAdmissionError::Ledger(CommandLedgerError::NotFound) => AgentAccessError::NotFound,
        CommandAdmissionError::Ledger(
            CommandLedgerError::Invalid
            | CommandLedgerError::IdempotencyConflict
            | CommandLedgerError::Corrupt,
        )
        | CommandAdmissionError::Input(_)
        | CommandAdmissionError::ModelMismatch => AgentAccessError::Invalid,
        _ => AgentAccessError::Unavailable,
    }
}
