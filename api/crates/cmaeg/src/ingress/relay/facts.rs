//! Platform-neutral facts observed from an authenticated relay callback.
use super::{generic, lark, payload::CallbackPayload, telegram, VerificationError};
use crate::ingress::admission::Refusal;
use cmaeg_core::{
    activity::{ActorKind, ConversationKind},
    channel::{AdmissionPolicy, GroupAdmission, Platform, Source},
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
    pub callback: Option<CallbackFacts>,
    pub conversation: ConversationKind,
    pub thread_id: Option<String>,
    pub mentioned: Option<bool>,
    pub replied_to_bot: Option<bool>,
    pub actor: ActorKind,
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
/// Dispatch to the observer for the relay source's platform.
pub fn observe(
    source: &Source,
    payload: &CallbackPayload,
) -> Result<Observation, VerificationError> {
    let Source::NyxidRelay {
        platform,
        bot_username,
        bot_id,
        trust_normalized,
        ..
    } = source
    else {
        return Err(VerificationError("relay_source_missing"));
    };
    match platform {
        Platform::Telegram => telegram::observe(payload, bot_username.as_deref()),
        Platform::Lark | Platform::Feishu => lark::observe(payload, bot_id.as_deref()),
        Platform::Discord | Platform::Slack | Platform::Whatsapp if *trust_normalized => {
            generic::observe(payload)
        }
        Platform::Discord | Platform::Slack | Platform::Whatsapp => {
            Ok(Observation::Refused(Refusal::PlatformVerifierUnavailable))
        }
    }
}
