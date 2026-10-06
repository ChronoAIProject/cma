//! Private connection lifecycle and public platform-neutral observations.
use cmaeg_core::{
    activity::{ActivityConversation, SourceKind},
    seal::EventRefPayload,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

pub const SETUP_TTL: i64 = 20 * 60;

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "record_kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Stored {
    Connection(Box<Connection>),
    Pending(Pending),
    BotClaim(BotClaim),
}

// Deliberately no Debug: serialization is exclusively into authenticated encryption.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Connection {
    pub id: String,
    pub owner: String,
    pub organization: bool,
    pub created_at: i64,
    pub expires_at: i64,
    pub fingerprint: String,
    pub origin: Origin,
    pub suggested_name: Option<String>,
    pub status: Status,
    pub setup_nonce: String,
    pub browser_nonce: Option<String>,
    #[serde(default)]
    pub browser_claimed: bool,
    pub requested_manager: bool,
    pub material: Option<Material>,
    pub channel_record: Option<String>,
    #[serde(default)]
    pub channel_id: Option<String>,
    pub notified: bool,
    #[serde(default)]
    pub completion_ref: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Origin {
    pub event: EventRefPayload,
    pub actor: String,
    pub conversation: ActivityConversation,
    pub source: SourceKind,
    pub binding_id: String,
    pub agent_key_hash: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Draft,
    AwaitingPlatform,
    Ready,
    Cancelled,
    Expired,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Material {
    pub bot_id: String,
    pub username: String,
    pub name: String,
    pub token: String,
    pub webhook_secret: String,
    pub can_manage_bots: bool,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pending {
    pub owner: String,
    pub manager_record: String,
    pub actor: String,
    pub connection_id: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub prompt_message: Option<i64>,
    pub receipt: Option<Receipt>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub event_id: String,
    pub bot_id: String,
    pub message_id: i64,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BotClaim {
    pub owner: String,
    pub connection_id: String,
    #[serde(default)]
    pub released: bool,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct ConnectionView {
    pub connection_ref: String,
    pub status: Status,
    pub label: Option<String>,
    pub url: Option<String>,
    pub setup_url: Option<String>,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateConnection {
    pub record_id: String,
    pub event_ref: String,
    #[serde(default)]
    pub suggested_name: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupCapability {
    pub purpose: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_nonce: Option<String>,
    pub record_id: String,
    pub nonce: String,
    pub expires_at: i64,
}

pub fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}
