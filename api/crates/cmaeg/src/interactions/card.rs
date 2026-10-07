//! Interaction card records, their click and destination facts, and callback decoding.
use cmaeg_core::{
    ids::{ChannelId, EventId, MessageRef},
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
pub(super) fn decode(value: &str) -> Result<(String, usize), Error> {
    if value.len() > 64 {
        return Err(Error::Denied);
    }
    let mut parts = value.split(':');
    if parts.next() != Some("ce1") {
        return Err(Error::Denied);
    }
    let id =
        uuid::Uuid::parse_str(parts.next().ok_or(Error::Denied)?).map_err(|_| Error::Denied)?;
    let index = parts
        .next()
        .ok_or(Error::Denied)?
        .parse()
        .map_err(|_| Error::Denied)?;
    if id.get_version_num() != 4 || parts.next().is_some() {
        return Err(Error::Denied);
    }
    Ok((id.to_string(), index))
}
