//! Exact confirmed reply lookup and fences; caller IDs never supply a route.
use super::*;

impl MongoBridgeReplyToolStore {
    pub(super) async fn original_edit_target(
        &self,
        intent: &ChannelReplyToolIntent,
        session: &mut ClientSession,
    ) -> Result<OutboxItem, MutationError> {
        let id = intent
            .original_outbox_id()
            .ok_or(OutboxError::SourceMismatch)?;
        let document = self
            .items
            .find_one(doc! {"_id":id})
            .session(&mut *session)
            .await?
            .ok_or(OutboxError::OutputUnavailable)?;
        let original = records::read_item(&document)?;
        let invocation = self
            .invocations
            .find_one(doc! {"_id":&original.tool_invocation_id})
            .session(&mut *session)
            .await?
            .ok_or(OutboxError::SourceMismatch)?;
        let invocation = records::Invocation::read(&invocation)?;
        if invocation.namespace != intent.scope().namespace()
            || invocation.source_sha256 != intent.scope().source().fingerprint()?
            || !invocation.items.contains(&original.id)
        {
            return Err(OutboxError::SourceMismatch.into());
        }
        // The typed constructor still checks exact source, platform and Sent.
        self.fence_original_reply(&original, session).await?;
        Ok(original)
    }

    async fn fence_original_reply(
        &self,
        original: &OutboxItem,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        let changed = self.items.update_one(
            doc! {"_id":&original.id,"revision":original.revision as i64,"state":"sent","channel_history_seal":{"$exists":false}},
            doc! {"$inc":{"edit_fence":1_i64}},
        ).session(session).await?;
        if changed.matched_count != 1 {
            return Err(OutboxError::OutputUnavailable.into());
        }
        Ok(())
    }

    pub(super) async fn fence_edit_delivery(
        &self,
        item: &OutboxItem,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        let RelayOperation::Edit {
            original_outbox_id, ..
        } = &item.operation
        else {
            return Ok(());
        };
        let document = self
            .items
            .find_one(doc! {"_id":original_outbox_id})
            .session(&mut *session)
            .await?
            .ok_or(OutboxError::SourceMismatch)?;
        let original = records::read_item(&document)?;
        item.require_edit_original(&original)?;
        let mut namespace = None;
        for candidate in [&original, item] {
            let document = self
                .invocations
                .find_one(doc! {"_id":&candidate.tool_invocation_id})
                .session(&mut *session)
                .await?
                .ok_or(OutboxError::SourceMismatch)?;
            let invocation = records::Invocation::read(&document)?;
            if !invocation.items.contains(&candidate.id)
                || invocation.source_sha256 != item.source.fingerprint()?
                || namespace
                    .as_ref()
                    .is_some_and(|value| value != &invocation.namespace)
            {
                return Err(OutboxError::SourceMismatch.into());
            }
            namespace = Some(invocation.namespace);
        }
        self.fence_original_reply(&original, session).await
    }

    pub(super) async fn bind_edit_request(
        &self,
        request: RelayDeliveryRequest,
    ) -> Result<RelayDeliveryRequest, OutboxError> {
        let Some(original) = self.read_edit_original(request.item()).await? else {
            return Ok(request);
        };
        request.with_edit_original(&original)
    }

    pub(super) async fn read_edit_original(
        &self,
        item: &OutboxItem,
    ) -> Result<Option<OutboxItem>, OutboxError> {
        let RelayOperation::Edit {
            original_outbox_id, ..
        } = &item.operation
        else {
            return Ok(None);
        };
        let document = self
            .items
            .find_one(doc! {"_id":original_outbox_id})
            .await
            .map_err(|_| OutboxError::Unavailable)?
            .ok_or(OutboxError::SourceMismatch)?;
        let original = records::read_item(&document)?;
        item.require_edit_original(&original)?;
        Ok(Some(original))
    }
}
