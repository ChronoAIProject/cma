//! Facts for platforms without a raw-event verifier, taken from the normalized
//! envelope alone. The callback JWT binds that envelope through `body_sha256`, so
//! it is authentic NyxID output, but nothing proves who sent it or whether it
//! addressed the bot. Sources must opt in with `trust_normalized`.
use super::{
    facts::{Observation, RelayFacts},
    payload::CallbackPayload,
    VerificationError,
};
use crate::ingress::admission::Refusal;
use cmaeg_core::activity::{ActorKind, ConversationKind};

pub fn observe(payload: &CallbackPayload) -> Result<Observation, VerificationError> {
    if payload.received_attachments() > 0 {
        return Ok(Observation::Refused(Refusal::AttachmentUnsupported));
    }
    if payload.content.kind != "text"
        || payload
            .content
            .text
            .as_deref()
            .is_none_or(|text| text.is_empty() || text.len() > 32 * 1024)
    {
        return Ok(Observation::Refused(Refusal::ContentUnsupported));
    }
    let conversation = match payload.conversation.kind.as_str() {
        "private" => ConversationKind::Private,
        "group" => ConversationKind::Group,
        "channel" => ConversationKind::Channel,
        _ => return Ok(Observation::Refused(Refusal::SystemEvent)),
    };
    // Bot senders cannot be told apart, so the actor kind stays unknown, and there
    // is no addressing or thread evidence for group admission or thread partitions.
    Ok(Observation::Message(RelayFacts {
        event: None,
        callback: None,
        conversation,
        thread_id: None,
        mentioned: None,
        replied_to_bot: None,
        actor: ActorKind::Unknown,
        text: None,
        attachments: Vec::new(),
        album_id: None,
    }))
}
