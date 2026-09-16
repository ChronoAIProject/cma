use serde::{Deserialize, Serialize};

use super::{records::Invocation, *};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResponseHead {
    response_id: String,
    source_sha256: String,
    sequence: u64,
    last_item: String,
}
impl ResponseHead {
    fn document(&self) -> Result<Document, OutboxError> {
        if self.response_id.is_empty()
            || self.source_sha256.len() != 64
            || self.sequence == 0
            || self.sequence > i64::MAX as u64
            || !self.last_item.starts_with("bro_")
            || self.last_item.len() != 68
        {
            return Err(OutboxError::Corrupt);
        }
        Ok(
            doc! {"_id":&self.response_id,"document_schema_version":DOCUMENT_SCHEMA_VERSION,
            "sequence":self.sequence as i64,"record":record_bson(self).map_err(|_|OutboxError::Corrupt)?},
        )
    }
    fn read(document: &Document) -> Result<Self, OutboxError> {
        let value: Self = read_record(document).map_err(|_| OutboxError::Corrupt)?;
        let expected = value.document()?;
        if ["_id", "sequence"]
            .iter()
            .any(|key| document.get(*key) != expected.get(*key))
        {
            return Err(OutboxError::Corrupt);
        }
        Ok(value)
    }
}

impl MongoBridgeReplyToolStore {
    /// Authentication and creation/runtime fences apply even when the original
    /// Channel turn is complete. No source lookup or new admission occurs here.
    pub(crate) async fn recover(
        &self,
        capability: &ChannelReplySessionAuthorization,
        request: &ChannelReplyToolRequest,
    ) -> Result<Option<ReplyToolReceipt>, OutboxError> {
        let (id, request_sha256) = capability.request_identity(request)?;
        self.recover_identity(capability, &id, &request_sha256)
            .await
    }
    pub(crate) async fn recover_edit(
        &self,
        capability: &ChannelReplySessionAuthorization,
        request: &ChannelReplyEditRequest,
    ) -> Result<Option<ReplyToolReceipt>, OutboxError> {
        let (id, request_sha256) = capability.edit_request_identity(request)?;
        self.recover_identity(capability, &id, &request_sha256)
            .await
    }
    async fn recover_identity(
        &self,
        capability: &ChannelReplySessionAuthorization,
        id: &str,
        request_sha256: &str,
    ) -> Result<Option<ReplyToolReceipt>, OutboxError> {
        for _ in 0..TRANSACTION_ATTEMPTS {
            capability.require_current()?;
            let mut session = self
                .invokes
                .bridges
                .client
                .start_session()
                .await
                .map_err(|_| OutboxError::Unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(|_| OutboxError::Unavailable)?;
            let result = async {
                let observed = self.session_snapshot(capability, &mut session).await?;
                crate::store::channel_tools::validate_and_fence_session(
                    &self.invokes.resources,
                    capability,
                    &mut session,
                )
                .await?;
                crate::store::resource_access::fence_lineage(
                    &self.invokes.resources,
                    &observed.resource,
                    &mut session,
                )
                .await?;
                let Some(document) = self
                    .invocations
                    .find_one(doc! {"_id":&id})
                    .session(&mut session)
                    .await?
                else {
                    return Ok(None);
                };
                let invocation = Invocation::read(&document)?;
                invocation.require_request(capability.namespace(), request_sha256)?;
                Ok::<_, MutationError>(Some(self.receipt(&invocation, &mut session).await?))
            }
            .await;
            let result = match result {
                Ok(value) => value,
                Err(error) => {
                    let _ = session.abort_transaction().await;
                    if error.retryable() {
                        continue;
                    }
                    return Err(error.into_core());
                }
            };
            if let Err(error) = capability.require_current() {
                let _ = session.abort_transaction().await;
                return Err(error);
            }
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(result),
                CommitOutcome::Retry(_) => continue,
                CommitOutcome::Unknown(_) | CommitOutcome::Failed(_) => {
                    return Err(OutboxError::Unavailable);
                }
            }
        }
        Err(OutboxError::Unavailable)
    }

    async fn enqueue_transaction(
        &self,
        intent: &ChannelReplyToolIntent,
        validation: &BridgeValidation,
        session: &mut ClientSession,
    ) -> Result<ReplyToolReceipt, MutationError> {
        let scope = intent.scope();
        scope.require_current()?;
        // The active-turn proof is independently reconstructed before any new
        // item admission. HTTP retries normally use session recover first.
        self.fence_turn(scope, validation, session).await?;
        if let Some(document) = self
            .invocations
            .find_one(doc! {"_id":intent.id()})
            .session(&mut *session)
            .await?
        {
            let invocation = Invocation::read(&document)?;
            invocation.require_request(scope.namespace(), intent.request_sha256())?;
            return self.receipt(&invocation, session).await;
        }
        self.validator
            .check_feature(
                layer2_core::bridge::BridgeFeature::Outbound,
                &scope.source().config().connector.platform,
            )
            .map_err(|_| OutboxError::Suspended)?;
        let head = self
            .heads
            .find_one(doc! {"_id":scope.source().response_id().as_str()})
            .session(&mut *session)
            .await?
            .as_ref()
            .map(ResponseHead::read)
            .transpose()?;
        let source_sha256 = scope.source().fingerprint()?;
        if head
            .as_ref()
            .is_some_and(|head| head.source_sha256 != source_sha256)
        {
            return Err(OutboxError::SourceMismatch.into());
        }
        // All explicit tool calls use deliberately retained matching custody.
        // No single-use reply token can be accidentally consumed twice.
        let credential = scope.selected_key(validation)?;
        let sequence = head.as_ref().map_or(1, |head| head.sequence + 1);
        let predecessor = head.as_ref().map(|head| head.last_item.clone());
        let items = if intent.original_outbox_id().is_some() {
            let original = self.original_edit_target(intent, session).await?;
            OutboxItem::from_edit_intent(
                intent,
                &original,
                credential,
                sequence,
                predecessor,
                now_ms(),
            )?
        } else {
            OutboxItem::from_tool_intent(intent, credential, sequence, predecessor, now_ms())?
        };
        let last = items.last().ok_or(OutboxError::Corrupt)?;
        let next = ResponseHead {
            response_id: scope.source().response_id().as_str().into(),
            source_sha256: source_sha256.clone(),
            sequence: last.sequence,
            last_item: last.id.clone(),
        };
        let invocation = Invocation {
            id: intent.id().into(),
            namespace: scope.namespace().into(),
            request_sha256: intent.request_sha256().into(),
            source_sha256,
            items: items.iter().map(|item| item.id.clone()).collect(),
            created_at_ms: now_ms(),
        };
        if let Some(previous) = head {
            let result = self
                .heads
                .replace_one(
                    doc! {"_id":&previous.response_id,"sequence":previous.sequence as i64},
                    next.document()?,
                )
                .session(&mut *session)
                .await?;
            if result.matched_count != 1 {
                return Err(OutboxError::Conflict.into());
            }
        } else {
            self.heads
                .insert_one(next.document()?)
                .session(&mut *session)
                .await?;
        }
        for item in &items {
            self.items
                .insert_one(records::item_document(item)?)
                .session(&mut *session)
                .await?;
        }
        self.invocations
            .insert_one(invocation.document()?)
            .session(&mut *session)
            .await?;
        crate::store::channel_history::outbox(
            &self
                .invokes
                .bridges
                .client
                .database(&self.items.namespace().db),
            scope.source().allocation_id(),
            &items,
            &invocation.document()?,
            session,
        )
        .await
        .map_err(|error| match error {
            crate::store::bridge_allocations::MutationError::Mongo(error) => {
                MutationError::Driver(error)
            }
            crate::store::bridge_allocations::MutationError::Core(_) => {
                MutationError::Core(OutboxError::SourceMismatch)
            }
        })?;
        scope.require_current()?;
        Ok(invocation.receipt(&items)?)
    }
}

#[async_trait::async_trait]
impl BridgeReplyToolStore for MongoBridgeReplyToolStore {
    async fn enqueue(
        &self,
        intent: ChannelReplyToolIntent,
    ) -> Result<ReplyToolReceipt, OutboxError> {
        intent.scope().require_current()?;
        let validation = self
            .validator
            .validate_connection(
                intent.scope().source().owner(),
                intent.scope().source().config(),
            )
            .await
            .map_err(|_| OutboxError::AuthorityUnavailable)?;
        for _ in 0..TRANSACTION_ATTEMPTS {
            intent.scope().require_current()?;
            let mut session = self
                .invokes
                .bridges
                .client
                .start_session()
                .await
                .map_err(|_| OutboxError::Unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(|_| OutboxError::Unavailable)?;
            let result = match self
                .enqueue_transaction(&intent, &validation, &mut session)
                .await
            {
                Ok(value) => value,
                Err(error) => {
                    let _ = session.abort_transaction().await;
                    if error.retryable() {
                        continue;
                    }
                    return Err(error.into_core());
                }
            };
            if let Err(error) = intent.scope().require_current() {
                let _ = session.abort_transaction().await;
                return Err(error);
            }
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => {
                    self.wake.notify_waiters();
                    return Ok(result);
                }
                CommitOutcome::Retry(_) => continue,
                // No network send occurs on enqueue. The original request key
                // recovers any successful commit through authenticated recover.
                CommitOutcome::Unknown(_) | CommitOutcome::Failed(_) => {
                    return Err(OutboxError::Unavailable);
                }
            }
        }
        Err(OutboxError::Unavailable)
    }
}
