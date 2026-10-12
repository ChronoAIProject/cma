//! Source-independent sender, chat and group admission.
use cmaeg_core::channel::{AdmissionPolicy, GroupAdmission, IdentityAdmission};
use serde::Serialize;
use utoipa::ToSchema;

pub struct AdmissionFacts<'a> {
    pub sender_id: &'a str,
    pub chat_id: &'a str,
    pub is_group: bool,
    pub is_bot_sender: bool,
    pub is_system: bool,
    pub addressed: Option<bool>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Refusal {
    BotSender,
    SystemSender,
    UnknownSender,
    MessageEdit,
    InteractiveActionUnsupported,
    InteractionExpiredOrInvalid,
    SystemEvent,
    ContentUnsupported,
    AttachmentUnsupported,
    SenderDenied,
    ChatDenied,
    GroupDenied,
    GroupNotAddressed,
    AddressingEvidenceUnavailable,
    ThreadEvidenceUnavailable,
    // The platform has no raw-event verifier and the source did not opt in to
    // trusting the normalized envelope.
    PlatformVerifierUnavailable,
}
pub fn admit(policy: &AdmissionPolicy, facts: &AdmissionFacts<'_>) -> Result<(), Refusal> {
    if facts.is_bot_sender {
        return Err(Refusal::BotSender);
    }
    if facts.is_system {
        return Err(Refusal::SystemSender);
    }
    let groups = match policy {
        AdmissionPolicy::All {} => GroupAdmission::All,
        AdmissionPolicy::Allowlist { sender_ids } => {
            if !sender_ids.iter().any(|id| id == facts.sender_id) {
                return Err(Refusal::SenderDenied);
            }
            GroupAdmission::All
        }
        AdmissionPolicy::Scoped {
            senders,
            chats,
            groups,
        } => {
            if !permits(senders, facts.sender_id) {
                return Err(Refusal::SenderDenied);
            }
            if !permits(chats, facts.chat_id) {
                return Err(Refusal::ChatDenied);
            }
            *groups
        }
    };
    if !facts.is_group || groups == GroupAdmission::All {
        return Ok(());
    }
    if groups == GroupAdmission::Deny {
        return Err(Refusal::GroupDenied);
    }
    match facts.addressed {
        Some(true) => Ok(()),
        Some(false) => Err(Refusal::GroupNotAddressed),
        None => Err(Refusal::AddressingEvidenceUnavailable),
    }
}
fn permits(policy: &IdentityAdmission, id: &str) -> bool {
    match policy {
        IdentityAdmission::Deny => false,
        IdentityAdmission::Open => true,
        IdentityAdmission::Allowlist(ids) => ids.iter().any(|value| value == id),
    }
}
