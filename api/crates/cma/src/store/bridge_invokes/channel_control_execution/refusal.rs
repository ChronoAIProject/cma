use super::*;
use crate::store::sandbox_mongo::{
    commit_transaction, is_transient, CommitOutcome, TRANSACTION_ATTEMPTS,
};
use layer2_core::bridge::channel_control::ChannelControlHold;
use tokio::time::Instant;

mod fence;

impl MongoChannelControlExecutionStore {
    pub(super) async fn refuse(
        &self,
        command: &AdmittedAgentCommand,
        deadline: Instant,
    ) -> Result<(), Error> {
        tokio::time::timeout_at(deadline, self.refuse_bounded(command, deadline))
            .await
            .map_err(|_| Error::Unavailable)?
    }

    async fn refuse_bounded(
        &self,
        command: &AdmittedAgentCommand,
        deadline: Instant,
    ) -> Result<(), Error> {
        for attempt in 0..TRANSACTION_ATTEMPTS {
            require_deadline(deadline)?;
            if attempt > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(10 << attempt.min(4))).await;
                require_deadline(deadline)?;
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
            if let Err(error) = self
                .refuse_transaction(command, deadline, &mut session)
                .await
            {
                let _ = session.abort_transaction().await;
                if matches!(&error, MutationError::Driver(driver) if is_transient(driver)) {
                    continue;
                }
                return Err(error.into_core());
            }
            if let Err(error) = require_deadline(deadline) {
                let _ = session.abort_transaction().await;
                return Err(error);
            }
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(()),
                CommitOutcome::Retry(_) => continue,
                // The worker rereads the original command after this outcome.
                // No receipt is replaced and no runtime write is attempted.
                _ => return Err(Error::Unavailable),
            }
        }
        Err(Error::Unavailable)
    }

    async fn refuse_transaction(
        &self,
        command: &AdmittedAgentCommand,
        deadline: Instant,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        let snapshot = self.refusal_snapshot(command, deadline, session).await?;
        let code = snapshot.failure()?;
        self.fence_refusal(&snapshot, session).await?;
        require_deadline(deadline)?;
        let mut next = snapshot.admitted.clone();
        next.sidecar.execution_hold = ChannelControlHold::Refused;
        next.command.status = AgentResponseStatus::Failed;
        next.command.work.failure = Some(CommandFailure {
            code,
            at: layer2_core::now_rfc3339(),
        });
        next.command.revision = command.revision.checked_add(1).ok_or(Error::Corrupt)?;
        next.validate()?;
        super::release::write_transition(&self.invokes, command, &next, session).await
    }

    async fn refusal_snapshot(
        &self,
        command: &AdmittedAgentCommand,
        deadline: Instant,
        session: &mut ClientSession,
    ) -> Result<ChannelControlRefusalSnapshot, MutationError> {
        require_deadline(deadline)?;
        let (admitted, source_command, source) = self
            .invokes
            .read_retained_control(command, deadline, session)
            .await?;
        if admitted.command != *command {
            return Err(Error::Conflict.into());
        }
        let binding = admitted.sidecar.intent.reference.binding();
        let bridge = self
            .invokes
            .read_bridge(&binding.source.pin().bridge_id, session)
            .await?;
        let grant_row = self
            .invokes
            .grants
            .find_one(doc! {"_id":binding.consent.view.grant_id.as_str()})
            .session(&mut *session)
            .await?
            .ok_or(Error::Conflict)?;
        let grant = super::super::read_grant(&grant_row).map_err(|_| Error::Corrupt)?;
        let consent = self
            .invokes
            .refusal_consent(
                &grant,
                &binding.consent,
                &binding.consent_receipt,
                deadline,
                session,
            )
            .await?;
        let agent_row = self
            .invokes
            .resources
            .agents
            .find_one(doc! {"_id":command.receipt.envelope.agent_id.as_str()})
            .session(&mut *session)
            .await?
            .ok_or(Error::Conflict)?;
        let agent = crate::store::agents::read_agent(&agent_row).map_err(|_| Error::Corrupt)?;
        require_deadline(deadline)?;
        Ok(ChannelControlRefusalSnapshot {
            admitted,
            source_command,
            source,
            bridge,
            grant,
            consent,
            agent,
        })
    }
}

fn require_deadline(deadline: Instant) -> Result<(), Error> {
    if Instant::now() >= deadline {
        return Err(Error::Unavailable);
    }
    Ok(())
}
