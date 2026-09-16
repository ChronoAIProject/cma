use super::*;
use crate::store::{
    agent_commands::command_document,
    sandbox_mongo::{commit_transaction, is_transient, CommitOutcome, TRANSACTION_ATTEMPTS},
};
use layer2_core::bridge::channel_control::{AdmittedChannelControl, ChannelControlHold};

impl MongoChannelControlExecutionStore {
    pub(super) async fn release(
        &self,
        fence: &ChannelControlExecutionFence,
        command: &AdmittedAgentCommand,
    ) -> Result<(), Error> {
        self.transition(command, Some(fence)).await
    }
    pub(super) async fn expire(&self, command: &AdmittedAgentCommand) -> Result<(), Error> {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
        tokio::time::timeout_at(deadline, self.transition(command, None))
            .await
            .map_err(|_| Error::Unavailable)?
    }
    async fn transition(
        &self,
        command: &AdmittedAgentCommand,
        fence: Option<&ChannelControlExecutionFence>,
    ) -> Result<(), Error> {
        for attempt in 0..TRANSACTION_ATTEMPTS {
            if attempt > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(10 << attempt.min(4))).await;
            }
            if let Some(fence) = fence {
                fence.require_held(command)?;
            }
            let mut session = self
                .invokes
                .bridges
                .client
                .start_session()
                .await
                .map_err(|_| Error::Unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(|_| Error::Unavailable)?;
            let result = self
                .transition_transaction(command, fence, &mut session)
                .await;
            if let Err(error) = result {
                let _ = session.abort_transaction().await;
                if matches!(&error, MutationError::Driver(driver) if is_transient(driver)) {
                    continue;
                }
                return Err(error.into_core());
            }
            if let Some(fence) = fence {
                if let Err(error) = fence.require_current() {
                    let _ = session.abort_transaction().await;
                    return Err(error);
                }
            }
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(()),
                CommitOutcome::Retry(_) => continue,
                _ => return Err(Error::Unavailable),
            }
        }
        Err(Error::Unavailable)
    }
    async fn transition_transaction(
        &self,
        command: &AdmittedAgentCommand,
        fence: Option<&ChannelControlExecutionFence>,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        let row = self
            .invokes
            .resources
            .agent_commands
            .find_one(doc! {"_id":command.receipt.envelope.command_id.as_str()})
            .session(&mut *session)
            .await?
            .ok_or(Error::SourceUnavailable)?;
        let mut admitted = super::super::channel_controls::records::read_admitted(&row)?;
        if admitted.command != *command
            || admitted.sidecar.execution_hold != ChannelControlHold::WorkerUnavailable
            || command.status != AgentResponseStatus::WaitingAction
            || command.work != CommandWorkState::default()
        {
            return Err(Error::Conflict.into());
        }
        let now = layer2_core::now_rfc3339();
        if let Some(fence) = fence {
            if command.queue_expired(&now) {
                return Err(Error::Conflict.into());
            }
            self.invokes
                .fence_channel_control_execution(fence, command, session)
                .await?;
            admitted.sidecar.execution_hold = ChannelControlHold::Released;
            admitted.command.status = AgentResponseStatus::Queued;
        } else {
            if !command.queue_expired(&now) {
                return Err(Error::Conflict.into());
            }
            admitted.sidecar.execution_hold = ChannelControlHold::Refused;
            admitted.command.status = AgentResponseStatus::Failed;
            admitted.command.work.failure = Some(CommandFailure {
                code: CommandFailureCode::QueueTimeout,
                at: now,
            });
        }
        admitted.command.revision = command.revision.checked_add(1).ok_or(Error::Corrupt)?;
        admitted.validate()?;
        write_transition(&self.invokes, command, &admitted, session).await
    }
}
pub(super) async fn write_transition(
    invokes: &MongoBridgeInvokeStore,
    previous: &AdmittedAgentCommand,
    next: &AdmittedChannelControl,
    session: &mut ClientSession,
) -> Result<(), MutationError> {
    let mut fields = command_document(&next.command).map_err(|_| Error::Corrupt)?;
    fields.remove("_id");
    fields.insert(
        "channel_control_admission",
        record_bson(&next.sidecar).map_err(|_| Error::Corrupt)?,
    );
    let changed = invokes.resources.agent_commands.update_one(
        doc! {"_id":previous.receipt.envelope.command_id.as_str(),"revision":previous.revision as i64,"status":"waiting_action"},
        doc! {"$set":fields},
    ).session(session).await?;
    if changed.matched_count != 1 {
        return Err(Error::Conflict.into());
    }
    Ok(())
}
