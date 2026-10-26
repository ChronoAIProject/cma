//! Platform-neutral facts observed from an authenticated relay callback.
use super::{
    discord, generic, lark, payload::CallbackPayload, telegram, telegram_media::MediaIngress,
    whatsapp, VerificationError,
};
use crate::ingress::admission::Refusal;
use cmaeg_core::{
    activity::{ActorKind, Attachment, ConversationKind},
    channel::{AdmissionPolicy, GroupAdmission, Platform, Source},
    cloudevent::CloudEvent,
};

#[derive(Clone)]
pub struct CallbackFacts {
    pub id: String,
    pub data: String,
    pub message_id: String,
}
/// Facts a platform observer vouches for. `None` means the platform supplied no
/// evidence, which admission treats differently from a verified `Some(false)`.
pub struct RelayFacts {
    pub thread: Option<Box<cmaeg_core::thread::ThreadFacts>>,
    pub callback: Option<CallbackFacts>,
    pub event: Option<Box<CloudEvent>>,
    pub conversation: ConversationKind,
    pub thread_id: Option<String>,
    pub mentioned: Option<bool>,
    pub replied_to_bot: Option<bool>,
    pub actor: ActorKind,
    /// Authenticated text derived by the platform observer from the raw event,
    /// which the callback JWT binds through `body_sha256`. `None` keeps the
    /// envelope's `content.text`.
    pub text: Option<String>,
    /// Media the observer extracted, already normalized; empty for text.
    pub attachments: Vec<Attachment>,
    /// The platform media group (a Telegram album) the message belongs to.
    pub album_id: Option<String>,
}
impl RelayFacts {
    pub fn addressed(&self, policy: &AdmissionPolicy) -> Option<bool> {
        match policy {
            AdmissionPolicy::Scoped {
                groups: GroupAdmission::Mention,
                ..
            } => self.mentioned,
            AdmissionPolicy::Scoped {
                groups: GroupAdmission::ReplyToBot,
                ..
            } => self.replied_to_bot,
            _ => match (self.mentioned, self.replied_to_bot) {
                (Some(true), _) | (_, Some(true)) => Some(true),
                (Some(false), Some(false)) => Some(false),
                _ => None,
            },
        }
    }
}
pub enum Observation {
    Message(RelayFacts),
    Refused(Refusal),
}
/// Dispatch to the observer for the relay source's platform. `media_enabled` is
/// the operator's inbound media switch; it admits Telegram and Lark/Feishu media.
pub fn observe(
    source: &Source,
    payload: &CallbackPayload,
    media_enabled: bool,
) -> Result<Observation, VerificationError> {
    let Source::NyxidRelay {
        platform,
        bot_username,
        bot_id,
        trust_normalized,
        verify_raw_event,
        admission,
        ..
    } = source
    else {
        return Err(VerificationError("relay_source_missing"));
    };
    match platform {
        Platform::Telegram => telegram::observe(
            payload,
            bot_username.as_deref(),
            bot_id.as_deref(),
            admission.native_bot_mentions(),
            MediaIngress::relay(media_enabled),
        ),
        Platform::Lark | Platform::Feishu => lark::observe(
            payload,
            bot_id.as_deref(),
            admission.native_bot_mentions(),
            media_enabled,
        ),
        Platform::Discord if *verify_raw_event => {
            discord::observe(payload, bot_id.as_deref(), media_enabled)
        }
        Platform::Whatsapp if *verify_raw_event => whatsapp::observe(payload, media_enabled),
        Platform::Discord | Platform::Slack | Platform::Whatsapp if *trust_normalized => {
            generic::observe(payload)
        }
        Platform::Discord | Platform::Slack | Platform::Whatsapp => {
            Ok(Observation::Refused(Refusal::PlatformVerifierUnavailable))
        }
    }
}
