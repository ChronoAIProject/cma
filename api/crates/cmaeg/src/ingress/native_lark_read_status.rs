//! Confirmed outbound read facts annotate one original consumer, never a turn.
use super::{
    routing_store::{AdmissionError, PlanStore},
    IngressState,
};
use crate::{
    activity::{ActivityDelivery, ActivityRecipient, Change, ConsumerState},
    channels::resolver::ResolvedChannel,
    native_lark::{read_status::ReadStatus, receipts},
};
use cmaeg_core::{
    activity::{ActorKind, ConversationKind, SourceKind},
    channel::{NativeLarkSource, Source},
    seal::ReplyAddress,
};

impl IngressState {
    pub(crate) async fn observe_native_lark_read(
        &self,
        channel: &ResolvedChannel,
        source: &NativeLarkSource,
        status: &ReadStatus,
        now: i64,
    ) -> Result<(), ()> {
        let source_kind = SourceKind::NativeLark(source.identity.clone());
        let store = receipts::Store::new(self.resolver.record_client(), self.master_keys.clone());
        let plans = PlanStore::new(self.resolver.record_client(), self.master_keys.clone());
        let occurred = time::OffsetDateTime::from_unix_timestamp_nanos(
            i128::from(status.occurred_ms) * 1_000_000,
        )
        .map_err(|_| ())?
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|_| ())?;
        let mut pending = false;
        for message in &status.message_ids {
            let receipt = match store.load(&channel.id, &source_kind, message, now).await {
                Ok(Some(receipt)) => receipt,
                // A callback can race the confirmed-send receipt commit. A
                // bounded retry retries only this observation, never the send.
                Ok(None) if now.saturating_sub(status.occurred_ms / 1000) < 120 => {
                    pending = true;
                    continue;
                }
                Ok(None) | Err(receipts::Error::Expired) => continue,
                Err(_) => return Err(()),
            };
            if receipt.original.owner != channel.definition.owner.subject
                || receipt.original.record_id != channel.definition.record_id
                || status.occurred_ms / 1000 < receipt.confirmed_at.saturating_sub(300)
                || receipt.reply.thread_id.is_some()
            {
                continue;
            }
            let plan = match plans.load(&receipt.original).await {
                Ok(Some(plan)) => plan,
                Ok(None) | Err(AdmissionError::Expired) => continue,
                Err(_) => return Err(()),
            };
            // The event has no chat ID. Require the exact retained private chat
            // and its human recipient, not a guessed chat or current routing.
            if plan.identity != receipt.original
                || plan.activity.source != source_kind
                || plan.activity.conversation.kind != ConversationKind::Private
                || plan.activity.conversation.thread_id.is_some()
                || plan.activity.conversation.id != receipt.recipient
                || plan.activity.actor.kind != ActorKind::Human
                || plan.activity.actor.id != status.reader
                || !crate::native_lark::same_messaging_source(
                    &plan.source,
                    &Source::NativeLark(source.clone()),
                )
                || !plan.targets.iter().any(|target| {
                    target.launch.consumer == receipt.consumer
                        && target.launch.provider == receipt.provider
                        && target.launch.event.partition_key == receipt.partition_key
                        && target.launch.event.conversation_id == receipt.conversation_id
                        && matches!(&target.launch.event.reply, ReplyAddress::NativeLark(reply)
                            if reply.identity == receipt.reply.identity
                                && reply.revision == receipt.reply.revision
                                && reply.chat_id == receipt.recipient
                                && reply.thread_id.is_none())
                })
            {
                continue;
            }
            let current = self
                .resolver
                .resolve_current(channel.id.as_str())
                .await
                .map_err(|_| ())?;
            if current.definition.owner.subject != receipt.original.owner
                || current.definition.record_id != receipt.original.record_id
                || crate::native_lark::credentials(
                    &current.definition,
                    &ReplyAddress::NativeLark(receipt.reply.clone()),
                )
                .is_none()
                || !current
                    .definition
                    .consumer(Some(&receipt.consumer))
                    .is_some_and(|consumer| consumer.provider == &receipt.provider)
            {
                continue;
            }
            let mut change =
                Change::state(receipt.consumer.as_str(), ConsumerState::Received, None);
            change.state = None;
            change.delivery = Some(ActivityDelivery {
                step: "platform_status".into(),
                destination: "origin".into(),
                outcome: "read".into(),
                observed_at: crate::activity::now(),
                confirmed_at: Some(occurred.clone()),
                platform_message_id: Some(message.clone()),
                recipients: vec![ActivityRecipient {
                    kind: "native_lark_chat".into(),
                    id: receipt.recipient.clone(),
                    thread_id: None,
                }],
            });
            self.activity
                .platform_receipt(
                    &receipt.original.record_id,
                    &receipt.original.event_id,
                    change,
                )
                .await?;
        }
        if pending {
            Err(())
        } else {
            Ok(())
        }
    }
}
