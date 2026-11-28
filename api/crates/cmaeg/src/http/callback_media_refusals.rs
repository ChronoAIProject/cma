use super::*;
use crate::{channels::resolver::ResolvedChannel, ingress::relay::payload::CallbackPayload};
use cmaeg_core::{
    activity::{ActivityConversation, Actor, ActorKind, ConversationKind, SourceKind},
    channel::Platform,
};
use sha2::{Digest, Sha256};
use time::format_description::well_known::Rfc3339;

pub(super) fn refuse(
    state: &IngressState,
    record: &RecordId,
    channel_id: &str,
    media_event_id: Option<&EventId>,
    reason: Refusal,
) -> Response {
    if let (Refusal::AttachmentUnsupported, Some(event_id)) = (reason, media_event_id) {
        state
            .activity
            .refused(record, SOURCE, Some(event_id), "attachment_unsupported");
        tracing::info!(
            source = SOURCE,
            channel = %crate::telemetry::channel_hash(channel_id),
            media_enabled = state.media_enabled,
            "Telegram relay attachment refused"
        );
        return refused(reason);
    }
    authenticated_refusal(state, record, SOURCE, reason)
}

pub(in crate::http) struct Receipt {
    owner: String,
    row: crate::activity::ChannelActivity,
    received: time::OffsetDateTime,
}

pub(super) fn receipt(
    channel: &ResolvedChannel,
    payload: &CallbackPayload,
    event: &EventId,
) -> Option<Receipt> {
    let message = payload.raw_platform_data.as_ref()?.get("message")?;
    let kind = match message.pointer("/chat/type")?.as_str()? {
        "private" => ConversationKind::Private,
        "group" | "supergroup" => ConversationKind::Group,
        _ => return None,
    };
    if payload.received_attachments() == 0
        && !crate::ingress::relay::telegram_media::MEDIA_FIELDS
            .iter()
            .any(|field| message.get(*field).is_some())
    {
        return None;
    }
    receipt_for_event(
        channel,
        event,
        SourceKind::NyxidRelay {
            platform: Platform::Telegram,
            route_id: payload.conversation.id.clone(),
        },
        kind,
        payload.timestamp.clone(),
    )
}

pub(in crate::http) fn receipt_for_event(
    channel: &ResolvedChannel,
    event: &EventId,
    source: SourceKind,
    kind: ConversationKind,
    occurred_at: String,
) -> Option<Receipt> {
    let received = time::OffsetDateTime::now_utc();
    let digest = Sha256::digest(event.as_str().as_bytes());
    let mut row = crate::activity::ChannelActivity::metadata(
        format!("telegram:relay-refusal:{digest:x}"),
        source,
        Actor {
            id: "redacted".into(),
            display_name: None,
            kind: ActorKind::Unknown,
        },
        ActivityConversation {
            id: "redacted".into(),
            kind,
            thread_id: None,
        },
        occurred_at,
        received.format(&Rfc3339).ok()?,
    );
    row.kind = Some("message".into());
    Some(Receipt {
        owner: channel.definition.owner.subject.clone(),
        row,
        received,
    })
}

impl Receipt {
    pub(super) async fn persist(
        &self,
        state: &IngressState,
        record: &RecordId,
        reason: Refusal,
    ) -> Result<(), ()> {
        let label = match reason {
            Refusal::AttachmentUnsupported => "attachment_unsupported",
            Refusal::ContentUnsupported => "content_unsupported",
            _ => return Ok(()),
        };
        self.persist_label(state, record, label).await
    }

    pub(in crate::http) async fn persist_label(
        &self,
        state: &IngressState,
        record: &RecordId,
        label: &str,
    ) -> Result<(), ()> {
        let mut row = self.row.clone();
        row.source_refusal = Some(label.into());
        state
            .activity
            .source_refused(&self.owner, record, row, self.received)
            .await
    }
}

/// Establish the same message and admission facts the enabled path requires.
pub(super) fn eligible_disabled_photo(
    source: &Source,
    partition: PartitionKey,
    verified: &verify::VerifiedCallback,
) -> bool {
    use cmaeg_core::activity::{AttachmentSource, MediaVariant};
    let Source::NyxidRelay {
        platform: Platform::Telegram,
        admission,
        bot_id,
        ..
    } = source
    else {
        return false;
    };
    let payload = &verified.payload;
    if payload
        .content
        .text
        .as_ref()
        .is_none_or(|caption| caption.is_empty())
    {
        return false;
    }
    let Ok(Observation::Message(facts)) = observe(source, payload, true) else {
        return false;
    };
    if facts.actor != ActorKind::Human
        || facts.attachments.len() != 1
        || !facts.attachments.iter().all(|attachment| {
            attachment.variant == Some(MediaVariant::Photo)
                && matches!(attachment.source, Some(AttachmentSource::NyxidRelay { .. }))
        })
        || (partition == PartitionKey::Thread && facts.thread_id.is_none())
    {
        return false;
    }
    admit(
        admission,
        &AdmissionFacts {
            sender_id: &payload.sender.platform_id,
            chat_id: &payload.conversation.platform_id,
            is_group: facts.conversation != ConversationKind::Private,
            is_bot_sender: false,
            receiving_bot_id: bot_id.as_deref(),
            mentioned: facts.mentioned,
            is_system: false,
            addressed: facts.addressed(admission),
        },
    )
    .is_ok()
}
