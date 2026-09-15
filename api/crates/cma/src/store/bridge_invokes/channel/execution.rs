use std::sync::Arc;

use layer2_core::sandbox::agent_io::{
    AdmittedAgentCommand, AgentResponseStatus, ChannelCommandAdmissionError as Error,
    ChannelCommandExecutionSource, ChannelCommandHold, ChannelCommandSidecar,
    ChannelExecutionFence, CommandLedgerError,
};
use mongodb::{
    bson::{doc, Document},
    ClientSession,
};

use super::super::MongoBridgeInvokeStore;
use super::{MongoChannelCommandStore, MutationError};
use crate::store::{
    agent_commands::{command_document, read_command},
    sandbox_mongo::{
        commit_transaction, record_bson, start_transaction, CommitOutcome, TRANSACTION_ATTEMPTS,
    },
};

impl MongoChannelCommandStore {
    pub(super) async fn read_execution_source(
        &self,
        command: &AdmittedAgentCommand,
    ) -> Result<ChannelCommandExecutionSource, Error> {
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
            .invokes
            .read_channel_source(command, &mut session)
            .await;
        let _ = session.abort_transaction().await;
        result.map_err(error)
    }

    pub(super) async fn release_held(
        &self,
        fence: Arc<ChannelExecutionFence>,
        command: &AdmittedAgentCommand,
    ) -> Result<(), Error> {
        for _ in 0..TRANSACTION_ATTEMPTS {
            fence.require_held(command)?;
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
                .release_transaction(&fence, command, &mut session)
                .await;
            if let Err(failure) = result {
                let _ = session.abort_transaction().await;
                if matches!(&failure, MutationError::Driver(e) if crate::store::sandbox_mongo::is_transient(e))
                {
                    continue;
                }
                return Err(error(failure));
            }
            if let Err(failure) = fence.require_current() {
                let _ = session.abort_transaction().await;
                return Err(failure);
            }
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(()),
                CommitOutcome::Retry(_) => continue,
                CommitOutcome::Unknown(_) | CommitOutcome::Failed(_) => {
                    return Err(Error::Unavailable);
                }
            }
        }
        Err(CommandLedgerError::Conflict.into())
    }

    async fn release_transaction(
        &self,
        fence: &ChannelExecutionFence,
        command: &AdmittedAgentCommand,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        fence.require_held(command)?;
        self.invokes
            .fence_channel_execution(fence, command, session)
            .await?;
        let mut next = command.clone();
        next.status = AgentResponseStatus::Queued;
        next.revision = next.revision.checked_add(1).ok_or(Error::SourceMismatch)?;
        next.validate().map_err(Error::from)?;
        let mut fields = command_document(&next).map_err(Error::from)?;
        fields.remove("_id");
        let mut sidecar = fence.sidecar().clone();
        sidecar.execution_hold = ChannelCommandHold::Released;
        fields.insert(
            "channel_admission",
            record_bson(&sidecar).map_err(|_| Error::SourceMismatch)?,
        );
        fields.insert(
            "channel_execution_release",
            doc! {
                "grant_id": fence.proof().grant().id.as_str(),
                "grant_revision": fence.proof().grant().revision as i64,
                "bridge_revision": fence.proof().bridge().revision as i64,
                "at": layer2_core::now_rfc3339(),
            },
        );
        let changed = self.invokes.resources.agent_commands.update_one(
            doc! {"_id": command.receipt.envelope.command_id.as_str(), "revision": command.revision as i64, "status": "waiting_action"},
            doc! {"$set": fields},
        ).session(session).await?;
        if changed.matched_count != 1 {
            return Err(Error::SourceMismatch.into());
        }
        Ok(())
    }
}

impl MongoBridgeInvokeStore {
    pub(super) async fn read_channel_source(
        &self,
        command: &AdmittedAgentCommand,
        session: &mut ClientSession,
    ) -> Result<ChannelCommandExecutionSource, MutationError> {
        let document = self
            .resources
            .agent_commands
            .find_one(doc! {"_id": command.receipt.envelope.command_id.as_str()})
            .session(&mut *session)
            .await?
            .ok_or(Error::SourceMismatch)?;
        if read_command(&document).map_err(Error::from)? != *command {
            return Err(Error::SourceMismatch.into());
        }
        let sidecar: ChannelCommandSidecar = mongodb::bson::deserialize_from_bson(
            document
                .get("channel_admission")
                .cloned()
                .ok_or(Error::SourceMismatch)?,
        )
        .map_err(|_| Error::SourceMismatch)?;
        let database = self
            .bridges
            .client
            .database(&self.bridges.definitions.namespace().db);
        let inbox = super::super::channel_source::read(self, &document, &sidecar, session).await?;
        let allocation_document = database
            .collection::<Document>("bridge_allocations")
            .find_one(doc! {"_id": &sidecar.allocation_id})
            .session(&mut *session)
            .await?
            .ok_or(Error::SourceMismatch)?;
        let allocation = crate::store::bridge_allocations::decode_record(&allocation_document)
            .map_err(|_| Error::SourceMismatch)?;
        Ok(ChannelCommandExecutionSource {
            inbox,
            allocation,
            sidecar,
        })
    }

    /// This method shares the caller's Agent/log/lease transaction. It performs
    /// actual writes on invoke, retained accepted source and allocation records.
    pub(crate) async fn fence_channel_execution(
        &self,
        fence: &ChannelExecutionFence,
        command: &AdmittedAgentCommand,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        fence.require_envelope(&command.receipt.envelope)?;
        let current = self.read_channel_source(command, session).await?;
        fence.source().require_result(command)?;
        fence
            .source()
            .require_records(&current.inbox, &current.allocation)?;
        if &current.sidecar != fence.sidecar() {
            return Err(Error::SourceMismatch.into());
        }
        self.fence_transaction(fence.proof(), session).await?;
        super::super::channel_source::fence(
            self,
            command,
            &current.sidecar,
            &current.inbox,
            session,
        )
        .await?;
        let database = self
            .bridges
            .client
            .database(&self.bridges.definitions.namespace().db);
        let changed = database
            .collection::<Document>("bridge_allocations")
            .update_one(
                doc! {"_id": &current.allocation.id},
                doc! {"$inc": {"command_execution_fence": 1_i64}},
            )
            .session(&mut *session)
            .await?;
        if changed.matched_count != 1 {
            return Err(Error::SourceMismatch.into());
        }
        fence.require_current()?;
        Ok(())
    }
}

pub(super) fn error(failure: MutationError) -> Error {
    match failure {
        MutationError::Core(error) => error,
        MutationError::Driver(_) => Error::Unavailable,
    }
}
