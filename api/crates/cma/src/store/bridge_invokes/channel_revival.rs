//! A queued Channel message may request revival only with the original human
//! lifecycle grant. Scheduling preserves the command and performs no runtime IO.
use super::channel::MutationError;
use super::MongoBridgeInvokeStore;
use crate::bridge::provisioning::reuse_revival::ChannelReuseRevivalAuthority;
use crate::bridge::provisioning::revival::ChannelRevivalAuthority;
use crate::store::{
    agent_commands::{read_command, read_command_lane},
    bridge_allocations,
    durability::majority_collection,
    sandbox_events::{append_in_transaction, TransactionEventError},
    sandbox_mongo::{
        commit_transaction, is_transient, record_bson, start_transaction, CommitOutcome,
        DOCUMENT_SCHEMA_VERSION, TRANSACTION_ATTEMPTS,
    },
    sandboxes::sandbox_replacement_fields,
    MongoBridgeAllocationStore,
};
use layer2_core::sandbox::{
    agent_io::{
        AdmittedAgentCommand, AgentCommand, ChannelCommandAdmissionError as Error,
        ChannelExecutionFence,
    },
    store::validate_command_revival_transition,
    SandboxEventKind, SandboxReviveCause, SandboxStatus,
};
use mongodb::{
    bson::{doc, Document},
    ClientSession,
};

mod permit;
use permit::RevivalPermit;

impl MongoBridgeInvokeStore {
    pub(crate) async fn schedule_channel_command_revival(
        &self,
        allocations: &MongoBridgeAllocationStore,
        permit: &ChannelRevivalAuthority,
        channel: &ChannelExecutionFence,
        command: &AdmittedAgentCommand,
    ) -> Result<(), Error> {
        self.schedule_proven_channel_revival(
            allocations,
            RevivalPermit::Created(permit),
            channel,
            command,
        )
        .await
    }

    pub(crate) async fn schedule_reused_channel_command_revival(
        &self,
        allocations: &MongoBridgeAllocationStore,
        permit: &ChannelReuseRevivalAuthority,
        channel: &ChannelExecutionFence,
        command: &AdmittedAgentCommand,
    ) -> Result<(), Error> {
        self.schedule_proven_channel_revival(
            allocations,
            RevivalPermit::Reused(permit),
            channel,
            command,
        )
        .await
    }

    async fn schedule_proven_channel_revival(
        &self,
        allocations: &MongoBridgeAllocationStore,
        permit: RevivalPermit<'_>,
        channel: &ChannelExecutionFence,
        command: &AdmittedAgentCommand,
    ) -> Result<(), Error> {
        allocations
            .require_channel_lifecycle_database(&self.resources.sandboxes.namespace().db)
            .map_err(|_| Error::SourceMismatch)?;
        for attempt in 0..TRANSACTION_ATTEMPTS {
            if attempt > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(5 << attempt.min(4))).await;
            }
            require(&permit, channel, command)?;
            let mut session = self
                .bridges
                .client
                .start_session()
                .await
                .map_err(|_| Error::Unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(|_| Error::Unavailable)?;
            let result = self
                .revive_channel_transaction(allocations, &permit, channel, command, &mut session)
                .await;
            if let Err(error) = result {
                let _ = session.abort_transaction().await;
                match error {
                    MutationError::Driver(error) if is_transient(&error) => continue,
                    MutationError::Driver(_) => return Err(Error::Unavailable),
                    MutationError::Core(error) => return Err(error),
                }
            }
            if let Err(error) = require(&permit, channel, command) {
                let _ = session.abort_transaction().await;
                return Err(error);
            }
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(()),
                CommitOutcome::Retry(_) => continue,
                // A later worker observes the durable parent operation. It
                // cannot repeat this scheduling against the old Paused proof.
                CommitOutcome::Unknown(_) | CommitOutcome::Failed(_) => {
                    return Err(Error::Unavailable)
                }
            }
        }
        Err(Error::Unavailable)
    }

    async fn revive_channel_transaction(
        &self,
        allocations: &MongoBridgeAllocationStore,
        permit: &RevivalPermit<'_>,
        channel: &ChannelExecutionFence,
        command: &AdmittedAgentCommand,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        require(permit, channel, command)?;
        self.fence_channel_execution(channel, command, session)
            .await?;
        permit
            .fence(allocations, session)
            .await
            .map_err(|error| match error {
                bridge_allocations::MutationError::Core(_) => {
                    MutationError::Core(Error::SourceMismatch)
                }
                bridge_allocations::MutationError::Mongo(error) => MutationError::Driver(error),
            })?;
        self.fence_channel_revival_order(command, session).await?;
        let current = permit.sandbox();
        let at = layer2_core::now_rfc3339();
        let mut next = current.clone();
        next.begin_revive(SandboxReviveCause::AgentMessage, at.clone())
            .map_err(|_| Error::SourceMismatch)?;
        let event = SandboxEventKind::StatusChanged {
            from: SandboxStatus::Paused,
            to: SandboxStatus::Reviving,
        };
        validate_command_revival_transition(current, &next, &at, &event)
            .map_err(|_| Error::SourceMismatch)?;
        let fields = sandbox_replacement_fields(&next).map_err(|_| Error::SourceMismatch)?;
        let changed = self.resources.sandboxes.update_one(
            doc! {"channel_reuse_cleanup_execution":{"$exists":false},"_id":current.id.as_str(),"revision":current.revision as i64,"record":record_bson(current).map_err(|_| Error::SourceMismatch)?},
            doc! {"$set":fields},
        ).session(&mut *session).await?;
        if changed.matched_count != 1 {
            return Err(Error::SourceMismatch.into());
        }
        append_in_transaction(&self.resources, session, &next.id, at.clone(), event)
            .await
            .map_err(|error| match error {
                TransactionEventError::Domain(_) => MutationError::Core(Error::SourceMismatch),
                TransactionEventError::Driver(error) => MutationError::Driver(error),
            })?;
        let human = permit.human();
        let grant_digest = layer2_core::profiles::content_digest(human.grant())
            .map_err(|_| Error::SourceMismatch)?;
        let row = doc! {"_id":format!("{}:{}",next.id,next.revision),"document_schema_version":DOCUMENT_SCHEMA_VERSION,
        "sandbox_id":next.id.as_str(),"sandbox_created_at":&next.created_at,"sandbox_revision":next.revision as i64,
        "allocation_id":&permit.allocation().id,"parent_kind":permit.kind(),"command_id":command.receipt.envelope.command_id.as_str(),
        "command_revision":command.revision as i64,"agent_order":command.receipt.agent_order as i64,
        "command_digest":layer2_core::profiles::content_digest(command).map_err(|_| Error::SourceMismatch)?,
        "channel_actor":record_bson(&command.receipt.envelope.actor).map_err(|_| Error::SourceMismatch)?,
        "invoke_grant_id":channel.proof().grant().id.as_str(),"invoke_grant_revision":channel.proof().grant().revision as i64,
        "human_actor":record_bson(&human.actor_context().actor).map_err(|_| Error::SourceMismatch)?,
        "human_grant_digest":grant_digest,"custody_revision":human.custody_revision() as i64,
        "operation":record_bson(&next.lifecycle.operation).map_err(|_| Error::SourceMismatch)?,"appended_at":&at};
        majority_collection::<Document>(
            &self.bridges.client.database(&self.audits.namespace().db),
            "channel_command_revival_audits",
        )
        .insert_one(row)
        .session(session)
        .await?;
        Ok(())
    }

    async fn fence_channel_revival_order(
        &self,
        command: &AdmittedAgentCommand,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        let id = command.receipt.envelope.agent_id.as_str();
        let row = self
            .resources
            .agent_command_orders
            .find_one(doc! {"_id":id})
            .session(&mut *session)
            .await?
            .ok_or(Error::SourceMismatch)?;
        let lane = read_command_lane(&row, id).map_err(Error::from)?;
        if lane.last_order < command.receipt.agent_order
            || lane
                .lease
                .as_ref()
                .is_some_and(|lease| lease.live_at(&layer2_core::now_rfc3339()))
        {
            return Err(Error::SourceMismatch.into());
        }
        let mut earlier = self
            .resources
            .agent_commands
            .find(doc! {"agent_id":id,
            "agent_order":{"$lt":command.receipt.agent_order as i64},
            "status":{"$in":["queued","waiting_action"]}})
            .sort(doc! {"agent_order":1})
            .session(&mut *session)
            .await?;
        while let Some(row) = earlier.next(&mut *session).await {
            let earlier = read_command(&row?).map_err(Error::from)?;
            if earlier.work.dispatch.is_none()
                && matches!(
                    earlier.receipt.envelope.command,
                    AgentCommand::Message { .. }
                )
            {
                return Err(Error::SourceMismatch.into());
            }
        }
        // These nonces are real writes even for concurrent identical requests.
        // They serialize with command/lane claims without changing receipts.
        let nonce = uuid::Uuid::new_v4().to_string();
        for (rows, filter) in [
            (&self.resources.agent_command_orders, doc! {"_id":id}),
            (
                &self.resources.agent_commands,
                doc! {"_id":command.receipt.envelope.command_id.as_str(),"revision":command.revision as i64},
            ),
        ] {
            let changed = rows
                .update_one(filter, doc! {"$set":{"channel_revival_fence":&nonce}})
                .session(&mut *session)
                .await?;
            if changed.matched_count != 1 || changed.modified_count != 1 {
                return Err(Error::SourceMismatch.into());
            }
        }
        Ok(())
    }
}

fn require(
    permit: &RevivalPermit<'_>,
    channel: &ChannelExecutionFence,
    command: &AdmittedAgentCommand,
) -> Result<(), Error> {
    command.validate()?;
    channel.require_dispatch(command)?;
    permit
        .require_current()
        .map_err(|_| Error::SourceMismatch)?;
    let now = layer2_core::now_rfc3339();
    if !matches!(
        command.receipt.envelope.command,
        AgentCommand::Message { .. }
    ) || !command.awaiting_claim(&now)
        || command.queue_expired(&now)
        || permit.sandbox().status != SandboxStatus::Paused
        || channel.source().allocation() != permit.allocation()
        || channel.proof().resources().lineage.sandbox != *permit.sandbox()
    {
        return Err(Error::SourceMismatch);
    }
    Ok(())
}
