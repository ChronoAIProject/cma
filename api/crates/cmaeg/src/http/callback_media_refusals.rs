use super::*;

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

use crate::{channels::resolver::ResolvedChannel, ingress::relay::payload::CallbackPayload};
use cmaeg_core::{
    activity::{ActivityConversation, Actor, ActorKind, ConversationKind, SourceKind},
    channel::Platform,
};
use sha2::{Digest, Sha256};

pub(super) struct Receipt {
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
    let received = time::OffsetDateTime::now_utc();
    let row = crate::activity::ChannelActivity {
        event_id: format!(
            "telegram:relay-refusal:{:x}",
            Sha256::digest(event.as_str().as_bytes())
        ),
        source_refusal: None,
        quoted_context: None,
        kind: Some("message".into()),
        event_type: None,
        source_occurred_at: None,
        source: SourceKind::NyxidRelay {
            platform: Platform::Telegram,
            route_id: payload.conversation.id.clone(),
        },
        actor: Actor {
            id: "redacted".into(),
            display_name: None,
            kind: ActorKind::Unknown,
        },
        conversation: ActivityConversation {
            id: "redacted".into(),
            kind,
            thread_id: None,
        },
        occurred_at: payload.timestamp.clone(),
        received_at: received
            .format(&time::format_description::well_known::Rfc3339)
            .ok()?,
        consumers: Vec::new(),
    };
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
        let mut row = self.row.clone();
        row.source_refusal = Some(label.into());
        state
            .activity
            .source_refused(&self.owner, record, row, self.received)
            .await
    }
}
