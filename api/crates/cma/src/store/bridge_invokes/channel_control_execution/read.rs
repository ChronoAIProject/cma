use super::super::channel_controls::records::{read_admitted, read_intent};
use super::*;
use crate::store::agent_commands::{read_command, read_command_lane};

impl MongoBridgeInvokeStore {
    pub(super) async fn read_control_execution(
        &self,
        command: &AdmittedAgentCommand,
        deadline: tokio::time::Instant,
        session: &mut ClientSession,
    ) -> Result<ChannelControlExecutionSnapshot, MutationError> {
        if tokio::time::Instant::now() >= deadline {
            return Err(Error::Unavailable.into());
        }
        let row = self
            .resources
            .agent_commands
            .find_one(doc! {"_id":command.receipt.envelope.command_id.as_str()})
            .session(&mut *session)
            .await?
            .ok_or(Error::SourceUnavailable)?;
        let admitted = read_admitted(&row)?;
        if admitted.command.receipt != command.receipt
            || admitted.command.key_sha256 != command.key_sha256
            || admitted.command.target_turn != command.target_turn
            || admitted
                .command
                .work
                .lease
                .as_ref()
                .map(|lease| &lease.claim)
                != command.work.lease.as_ref().map(|lease| &lease.claim)
        {
            return Err(Error::Conflict.into());
        }
        let database = self
            .bridges
            .client
            .database(&self.bridges.definitions.namespace().db);
        let stored = database
            .collection::<Document>("bridge_channel_control_intents")
            .find_one(doc! {"_id":&admitted.sidecar.intent.id})
            .session(&mut *session)
            .await?
            .ok_or(Error::SourceUnavailable)?;
        if read_intent(&stored)? != admitted.sidecar.intent {
            return Err(Error::Corrupt.into());
        }
        crate::store::MongoCallbackInbox::require_accepted_control(
            &database,
            &admitted.sidecar.intent,
            session,
        )
        .await?;
        let reference = database
            .collection::<Document>("bridge_channel_control_references")
            .find_one(doc! {"_id":&admitted.sidecar.intent.reference.reference})
            .session(&mut *session)
            .await?
            .ok_or(Error::SourceUnavailable)?;
        if super::super::outbox::read_control_reference(&reference).map_err(|_| Error::Corrupt)?
            != admitted.sidecar.intent.reference
        {
            return Err(Error::SourceUnavailable.into());
        }
        let binding = admitted.sidecar.intent.reference.binding();
        let source_row = self
            .resources
            .agent_commands
            .find_one(doc! {"_id":binding.source.command_id().as_str()})
            .session(&mut *session)
            .await?
            .ok_or(Error::SourceUnavailable)?;
        let source_command = read_command(&source_row).map_err(|_| Error::Corrupt)?;
        let source = self.read_channel_source(&source_command, session).await?;
        let grant = self
            .grants
            .find_one(doc! {"_id":binding.consent.view.grant_id.as_str()})
            .session(&mut *session)
            .await?
            .ok_or(Error::AuthorityUnavailable)?;
        let grant = super::super::read_grant(&grant).map_err(|_| Error::Corrupt)?;
        let (consent, consent_receipt) = self
            .reference_consent(&grant, deadline, session)
            .await?
            .ok_or(Error::AuthorityUnavailable)?;
        let lane = if admitted.command.work.lease.is_some() {
            let row = self
                .resources
                .agent_command_orders
                .find_one(doc! {"_id":command.receipt.envelope.agent_id.as_str()})
                .session(&mut *session)
                .await?
                .ok_or(Error::Conflict)?;
            Some(
                read_command_lane(&row, command.receipt.envelope.agent_id.as_str())
                    .map_err(|_| Error::Corrupt)?,
            )
        } else {
            None
        };
        if tokio::time::Instant::now() >= deadline {
            return Err(Error::Unavailable.into());
        }
        Ok(ChannelControlExecutionSnapshot {
            admitted,
            source_command,
            source,
            consent,
            consent_receipt,
            lane,
        })
    }
}
