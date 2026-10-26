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
    /// Absent on all legacy/Telegram records. New issuance is rollout-gated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lark: Option<LarkIdentity>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct LarkIdentity {
    pub platform: Platform,
    pub conversation: ConversationKind,
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
    if !matches!(kind, Some("ce1" | "cl1")) {
        return Err(Error::Denied);
    }
    let id =
        uuid::Uuid::parse_str(parts.next().ok_or(Error::Denied)?).map_err(|_| Error::Denied)?;
    let revision = if kind == Some("cl1") {
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
