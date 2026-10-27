//! Authenticated delivery facts annotate the original consumer; never a new turn.
use super::{routing_store::PlanStore, IngressState};
use crate::{
    activity::{ActivityDelivery, ActivityRecipient, Change, ConsumerState},
    channels::resolver::ResolvedChannel,
    whatsapp::{
        receipts,
        status::{Kind, Status},
    },
};
use cmaeg_core::channel::Source;

impl IngressState {
    pub(crate) async fn observe_whatsapp_status(
        &self,
        channel: &ResolvedChannel,
        source: &Source,
        status: &Status,
        now: i64,
    ) -> Result<(), ()> {
        if status.timestamp > now.saturating_add(300)
            || status.timestamp < now.saturating_sub(receipts::RETENTION)
        {
            return Ok(());
        }
        let source_kind = super::whatsapp::source_kind(source).ok_or(())?;
        let store = self.whatsapp_receipts.as_ref().ok_or(())?;
        let found = if status.group {
            store
                .load_group(
                    &channel.id,
                    &source_kind,
                    &status.recipient,
                    &status.message_id,
                    now,
                )
                .await
        } else {
            store
                .load(
                    &channel.id,
                    &source_kind,
                    &status.recipient,
                    &status.message_id,
                    now,
                )
                .await
        };
        let receipt = match found {
            Ok(Some(receipt)) => receipt,
            // Meta can deliver its callback before our accepted-send receipt
            // commit. A bounded retry opportunity never resends the message.
            Ok(None) if now.saturating_sub(status.timestamp) < 120 => return Err(()),
            Ok(None) | Err(receipts::Error::Expired) => return Ok(()),
            Err(_) => return Err(()),
        };
        if receipt.original.owner != channel.definition.owner.subject
            || receipt.original.record_id != channel.definition.record_id
            || status.timestamp < receipt.confirmed_at.saturating_sub(300)
        {
            return Ok(());
        }
        let Some(plan) = PlanStore::new(self.resolver.record_client(), self.master_keys.clone())
            .load(&receipt.original)
            .await
            .map_err(|_| ())?
        else {
            return Ok(());
        };
        if plan.activity.source != source_kind
            || !plan.targets.iter().any(|target| {
                target.launch.consumer == receipt.consumer
                    && target.launch.provider == receipt.provider
                    && target.launch.event.partition_key == receipt.partition_key
                    && target.launch.event.conversation_id == receipt.conversation_id
            })
        {
            return Ok(());
        }
        let mut recipients = vec![ActivityRecipient {
            kind: if status.group {
                "whatsapp_group"
            } else {
                "whatsapp_chat"
            }
            .into(),
            id: status.recipient.clone(),
            thread_id: None,
        }];
        if let Some(participant) = &status.participant {
            recipients.push(ActivityRecipient {
                kind: "whatsapp_participant".into(),
                id: participant.clone(),
                thread_id: None,
            });
        }
        let occurred = time::OffsetDateTime::from_unix_timestamp(status.timestamp)
            .map_err(|_| ())?
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|_| ())?;
        let mut change = Change::state(receipt.consumer.as_str(), ConsumerState::Received, None);
        change.state = None;
        change.delivery = Some(ActivityDelivery {
            // Keep failure evidence separate from later success evidence. An
            // out-of-order callback cannot erase either fact or lower success.
            step: if status.kind == Kind::Failed {
                "platform_failure"
            } else {
                "platform_status"
            }
            .into(),
            destination: "origin".into(),
            outcome: status.kind.as_str().into(),
            observed_at: crate::activity::now(),
            confirmed_at: Some(occurred),
            platform_message_id: Some(status.message_id.clone()),
            recipients,
        });
        self.activity
            .platform_receipt(
                &receipt.original.record_id,
                &receipt.original.event_id,
                change,
            )
            .await
    }
}
