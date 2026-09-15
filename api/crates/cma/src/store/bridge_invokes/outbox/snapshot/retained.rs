//! Sending keeps the accepted original route after reset or turn completion.
use super::*;

impl MongoBridgeReplyToolStore {
    pub(in super::super) async fn fence_retained_delivery_source(
        &self,
        source: &OutboxSource,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        if !source.config().placement.creates_partition_agent() {
            return Ok(());
        }
        let observed = self
            .reply_source_snapshot(
                &source.target().agent_id,
                source.command_id().as_str(),
                false,
                session,
            )
            .await?;
        let original = OutboxSource::capture(
            &observed.bridge,
            &observed.inbox,
            &observed.source_allocation,
            &observed.command.receipt,
            &observed.sidecar,
        )?;
        if &original != source {
            return Err(OutboxError::SourceMismatch.into());
        }
        // No live-turn/Agent check: accepted replies survive Agent stop while
        // the original parent/tree identity remains available. This does not
        // authorize execution or allow delivery using missing/purged children.
        // The original current grant, accepted creation policy and canonical
        // reset lineage still fence the transition that permits transport IO.
        self.require_turn_grant(&observed, true, session).await?;
        crate::store::resource_access::fence_lineage(
            &self.invokes.resources,
            &observed.resources.resource,
            session,
        )
        .await?;
        crate::store::bridge_invokes::channel_source::fence(
            &self.invokes,
            &observed.command,
            &observed.sidecar,
            &observed.inbox,
            session,
        )
        .await
        .map_err(source_error)?;
        let changed = self.invokes.resources.agent_commands.update_one(
            doc! {"_id":source.command_id().as_str(),"revision":observed.command.revision as i64},
            doc! {"$inc":{"outbox_delivery_fence":1_i64}},
        ).session(session).await?;
        if changed.matched_count != 1 {
            return Err(OutboxError::Conflict.into());
        }
        Ok(())
    }
}
