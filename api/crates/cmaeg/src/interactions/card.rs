//! Interaction card records, their click and destination facts, and callback decoding.
use cmaeg_core::{
    activity::ConversationKind,
    channel::Platform,
    ids::{ChannelId, ConsumerId, EventId, MessageRef},
    message::OutboundMessage,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Serialize, Deserialize)]
pub struct Card {
    pub id: String,
    pub(super) owner: String,
    pub(super) channel_id: ChannelId,
    pub(super) event_id: EventId,
    pub(super) route_id: String,
    pub(super) actor: String,
    pub(super) chat: String,
    pub(super) thread: Option<String>,
    pub expires_at: i64,
    pub revision: u64,
    pub message: OutboundMessage,
    pub destination: Option<Destination>,
    pub reference: Option<MessageRef>,
    pub consumed: Option<Click>,
    pub superseded: bool,
    /// The non-default consumer that owns the card; absent for the default
    /// consumer, so a default card keeps its exact stored bytes. A card that
    /// names a consumer is sealed under kind `ic`, never `in`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consumer: Option<ConsumerId>,
    /// Absent for default and historical cards. New named cards keep the exact
    /// serving identity and original conversation through retries and restarts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callback_owner: Option<super::owner::CallbackOwner>,
    /// Absent on all legacy/Telegram records. New issuance is rollout-gated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lark: Option<LarkIdentity>,
    /// Pending/confirmed/uncertain deletion keeps fresh callbacks fenced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lark_delete: Option<String>,
    /// New WhatsApp records retain the exact control kind. Legacy bytes are unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub whatsapp: Option<WhatsappKind>,
    /// Native Discord identity is absent from every historical card.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discord: Option<super::discord::Identity>,
    /// Native replacement claim and result; omitted on all historical cards.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discord_edit: Option<super::discord_edit::Edit>,
    /// Latest non-consuming native modal attempt; absent on historical cards.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discord_form_launch: Option<super::discord_form::Launch>,
    /// Retained native effect capability and deletion fence; absent on historical cards.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discord_effect: Option<super::discord_effect::State>,
}
impl Card {
    pub(crate) fn chat_id(&self) -> &str {
        &self.chat
    }
    pub(crate) fn consumed_at(&self) -> Option<&str> {
        self.consumed
            .as_ref()
            .map(|click| click.occurred_at.as_str())
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WhatsappKind {
    Button,
    List,
}
impl WhatsappKind {
    pub(super) fn prefix(self) -> &'static str {
        match self {
            Self::Button => "cw1b:",
            Self::List => "cw1l:",
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub struct LarkIdentity {
    pub platform: Platform,
    pub conversation: ConversationKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub form: Option<super::lark_form_authority::FormAuthority>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Destination {
    pub outbound_id: String,
    pub platform_id: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Click {
    pub(super) event_id: String,
    pub(super) callback_id: String,
    pub(super) action: usize,
    /// Canonical opaque option indices for one native Discord whole-set choice.
    /// Absent on historical clicks, buttons, single-selects and other transports.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) selection: Option<Vec<usize>>,
    /// Canonical whole-form result; absent on every pre-form click.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) submitted: Option<super::lark_form::Submitted>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) form_launch: Option<String>,
    pub(super) occurred_at: String,
}
pub struct PreparedCard {
    pub card: Card,
    pub version: u64,
    pub markup: Value,
}
#[derive(Debug)]
pub enum Error {
    Denied,
    Unavailable,
}
#[derive(Clone)]
pub(super) struct Action {
    pub(super) block: String,
    pub(super) id: String,
    pub(super) selected: Vec<String>,
}
pub(super) fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}
pub(super) fn decode(value: &str) -> Result<(String, usize, Option<u64>), Error> {
    if value.len() > 64 {
        return Err(Error::Denied);
    }
    let mut parts = value.split(':');
    let kind = parts.next();
    if !matches!(kind, Some("ce1" | "cl1" | "clf1" | "cw1b" | "cw1l" | "cd1")) {
        return Err(Error::Denied);
    }
    let id =
        uuid::Uuid::parse_str(parts.next().ok_or(Error::Denied)?).map_err(|_| Error::Denied)?;
    let revision = if kind != Some("ce1") {
        Some(
            parts
                .next()
                .ok_or(Error::Denied)?
                .parse::<u64>()
                .ok()
                .filter(|v| *v > 0)
                .ok_or(Error::Denied)?,
        )
    } else {
        None
    };
    let index = parts
        .next()
        .ok_or(Error::Denied)?
        .parse()
        .map_err(|_| Error::Denied)?;
    if id.get_version_num() != 4 || parts.next().is_some() {
        return Err(Error::Denied);
    }
    Ok((id.to_string(), index, revision))
}
