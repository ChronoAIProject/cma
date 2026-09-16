use super::*;

impl MongoBridgeInvokeStore {
    /// Shares the exact common command/claim, parent and conversation transaction.
    /// Driver identities survive to its bounded transaction retry classifier.
    pub(crate) async fn fence_channel_control_execution(
        &self,
        fence: &ChannelControlExecutionFence,
        command: &AdmittedAgentCommand,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        let current = self
            .read_control_execution(command, fence.deadline(), session)
            .await?;
        fence.require_snapshot(&current)?;
        self.fence_observed_transaction(fence.proof(), Some(&current.source.allocation), session)
            .await?;
        super::super::channel_source::fence(
            self,
            &current.source_command,
            &current.source.sidecar,
            &current.source.inbox,
            session,
        )
        .await?;
        let database = self
            .bridges
            .client
            .database(&self.bridges.definitions.namespace().db);
        let intent = &current.admitted.sidecar.intent;
        crate::store::MongoCallbackInbox::fence_accepted_control(&database, intent, session)
            .await?;
        for (collection, query, counter) in [
            (
                "bridge_channel_control_intents",
                doc! {"_id":&intent.id,"intent_sha256":intent.fingerprint()?,"record":record_bson(intent).map_err(|_| Error::Corrupt)?},
                "control_execution_fence",
            ),
            (
                "bridge_channel_control_references",
                doc! {"_id":&intent.reference.reference,"lookup_sha256":&intent.reference.lookup_sha256,"record":record_bson(&intent.reference).map_err(|_| Error::Corrupt)?},
                "control_admission_fence",
            ),
            (
                "bridge_allocations",
                doc! {"_id":&current.source.allocation.id,"revision":current.source.allocation.revision as i64},
                "control_execution_fence",
            ),
        ] {
            let mut increment = Document::new();
            increment.insert(counter, 1_i64);
            let changed = database
                .collection::<Document>(collection)
                .update_one(query, doc! {"$inc":increment})
                .session(&mut *session)
                .await?;
            if changed.matched_count != 1 {
                return Err(Error::Conflict.into());
            }
        }
        let changed = self.resources.agent_commands.update_one(
            doc! {"_id":command.receipt.envelope.command_id.as_str(),"revision":current.admitted.command.revision as i64,
                "channel_control_admission":record_bson(&current.admitted.sidecar).map_err(|_| Error::Corrupt)?},
            doc! {"$inc":{"channel_control_execution_fence":1_i64}},
        ).session(&mut *session).await?;
        if changed.matched_count != 1 {
            return Err(Error::Conflict.into());
        }
        if let Some(lane) = &current.lane {
            let changed = self.resources.agent_command_orders.update_one(
                doc! {"_id":command.receipt.envelope.agent_id.as_str(),"lease":record_bson(&lane.lease).map_err(|_| Error::Corrupt)?},
                doc! {"$inc":{"channel_control_execution_fence":1_i64}},
            ).session(&mut *session).await?;
            if changed.matched_count != 1 {
                return Err(Error::Conflict.into());
            }
        }
        fence.require_current()?;
        Ok(())
    }
}
