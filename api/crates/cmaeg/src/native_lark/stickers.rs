//! Lark exposes received sticker keys, but explicitly forbids downloading stickers.
//! The key stays in sealed ingress storage; only this module can send a proved key.
use super::{message::Message, Client, Error};
use crate::{channels::resolver::ResolvedChannel, records::RecordClient};
use cmaeg_core::{
    activity::{ActivityKind, Attachment, AttachmentKind, MediaVariant},
    channel::{lark_resources::safe_id, NativeLarkReply, Secret},
    seal::{EventRefPayload, MasterKeyRing, ReplyAddress},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::Arc;

pub(crate) const DESCRIPTION: &str =
    "A sticker was received. Lark does not expose its image or visual meaning.";
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReceivedSticker {
    file_key: String,
}
impl std::fmt::Debug for ReceivedSticker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ReceivedSticker([retained key])")
    }
}
impl ReceivedSticker {
    pub(crate) fn parse(content: &Value) -> Result<Self, ()> {
        // Never accept URL, image or upload fallbacks, or invented visual metadata.
        if content.as_object().is_none_or(|v| v.len() != 1) {
            return Err(());
        }
        let file_key = content["file_key"]
            .as_str()
            .filter(|v| safe_id(v, 1024))
            .ok_or(())?;
        Ok(Self {
            file_key: file_key.into(),
        })
    }
    pub(crate) fn attachment(&self, message: &str) -> Attachment {
        let hash = Sha256::digest(
            serde_json::to_vec(&(message, &self.file_key)).expect("sticker identity"),
        );
        Attachment {
            reference: format!("native-lark-sticker:{hash:x}"),
            kind: AttachmentKind::Other,
            filename: None,
            mime_type: None,
            size_bytes: None,
            source: None,
            thumbnail: None,
            duration_seconds: None,
            variant: Some(MediaVariant::Sticker),
            width: None,
            height: None,
            emoji: None,
        }
    }
}
/// A sticker proved against original ingress custody and its exact selected target.
/// No public constructor, no caller-provided key or recipient, and no Debug output.
pub(crate) struct RetainedSticker {
    sticker: ReceivedSticker,
    reply: NativeLarkReply,
}
pub(crate) async fn load(
    records: RecordClient,
    keys: Arc<MasterKeyRing>,
    channel: &ResolvedChannel,
    event: &EventRefPayload,
) -> Option<RetainedSticker> {
    let ReplyAddress::NativeLark(reply) = &event.reply else {
        return None;
    };
    let plan = crate::turns::routed::retained_received_source(records, keys, channel, event)
        .await
        .ok()?;
    let Message::Text {
        message_id,
        chat_id,
        sender,
        text,
        resources,
        received_sticker: Some(sticker),
        thread_id,
        created_ms,
        group,
        mentioned,
        ..
    } = serde_json::from_slice(&plan.source_body).ok()?
    else {
        return None;
    };
    let ActivityKind::Message {
        text: public_text,
        attachments,
        ..
    } = &plan.activity.kind
    else {
        return None;
    };
    let occurred = time::OffsetDateTime::from_unix_timestamp_nanos(created_ms as i128 * 1_000_000)
        .ok()?
        .format(&time::format_description::well_known::Rfc3339)
        .ok()?;
    if !safe_id(&sticker.file_key, 1024)
        || !resources.is_empty()
        || text != DESCRIPTION
        || public_text != &text
        || attachments != &[sticker.attachment(&message_id)]
        || message_id != reply.message_id
        || chat_id != reply.chat_id
        || thread_id != reply.thread_id
        || mentioned != reply.mentioned
        || group
            != (plan.activity.conversation.kind == cmaeg_core::activity::ConversationKind::Group)
        || plan.activity.actor.kind != cmaeg_core::activity::ActorKind::Human
        || plan.activity.actor.id != sender
        || plan.activity.occurred_at != occurred
        || super::message::original_id(&reply.identity, &message_id).ok()? != event.event_id
    {
        return None;
    }
    Some(RetainedSticker {
        sticker,
        reply: reply.clone(),
    })
}
impl Client {
    pub(crate) async fn send_received_sticker(
        &self,
        token: &Secret,
        proved: RetainedSticker,
    ) -> Result<String, Error> {
        self.send_content(
            token,
            &proved.reply,
            "sticker",
            json!({"file_key":proved.sticker.file_key}),
        )
        .await
    }
}
