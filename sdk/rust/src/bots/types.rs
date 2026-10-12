use serde::{Deserialize, Serialize};

/// Owner-visible Event Gateway connection available for a consumer.
#[derive(Clone, Serialize, Deserialize)]
pub struct ConsumerConnection {
    /// Opaque owned connection reference.
    pub connection_ref: String,
    /// Human-readable connection label.
    pub label: String,
}

/// Published profile pinned by Bot creation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BotProfileRef {
    /// Profile identifier.
    pub id: String,
    /// Published immutable revision.
    pub revision: u32,
}
/// Source admission policy, independently applied from relay route binding.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BotAdmission {
    /// Admit all supported senders and chats.
    All {},
    /// Admit only the selected senders.
    Allowlist {
        /// Platform sender identities.
        sender_ids: Vec<String>,
    },
    /// Combine sender, chat and group restrictions.
    Scoped {
        /// Sender restriction.
        senders: BotIdentityPolicy,
        /// Chat restriction.
        chats: BotIdentityPolicy,
        /// Group addressing rule.
        groups: BotGroupPolicy,
    },
}
/// An identity dimension within scoped admission.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BotIdentityPolicy {
    /// Refuse every identity.
    Deny {},
    /// Admit every identity.
    Open {},
    /// Admit selected identities.
    Allowlist {
        /// Exact platform identities.
        ids: Vec<String>,
    },
}
/// Group addressing rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BotGroupPolicy {
    /// Refuse groups.
    Deny,
    /// Admit supported group messages.
    All,
    /// Require an explicit bot mention.
    Mention,
    /// Require a reply to the bot.
    ReplyToBot,
    /// Require either a mention or a reply.
    MentionOrReplyToBot,
}
/// Delivery preferences.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BotReply {
    /// Automatically deliver committed provider answers.
    pub auto_final: bool,
    /// Milliseconds until progress acknowledgement; zero disables it.
    pub ack_after_ms: u32,
}
/// Bot creation request. No credential values are accepted or returned.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateBot {
    /// Owner-facing label.
    pub name: String,
    /// Personally owned channel bot already registered with NyxID.
    pub nyxid_bot_id: String,
    /// Published Agent profile.
    pub profile: BotProfileRef,
    /// Personally owned Sandbox profile; omit to pin the platform default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox_profile: Option<BotProfileRef>,
    /// Personally owned Workspace profile; omit to pin the platform default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_profile: Option<BotProfileRef>,
    /// Complete worker revision; the three source references must match it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_worker_profile: Option<BotProfileRef>,
    /// Source admission rules.
    pub admission: BotAdmission,
    /// Automatic reply and progress settings.
    pub reply: BotReply,
}
/// Durable Bot state, including an explicit unknown value for future additions.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BotState {
    /// Creation is still running.
    Creating,
    /// Ready to receive channel events.
    Active,
    /// Source admission is disabled.
    Disabled,
    /// Creation failed; consult operation and failure.
    Failed,
    /// Cleanup is still running.
    Deleting,
    /// Cleanup completed.
    Deleted,
    /// Unknown future state; do not interpret it as active.
    #[serde(other)]
    Unknown,
}
/// Durable progress without worker claims or credential material.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BotOperation {
    /// Operation kind.
    pub operation: String,
    /// Most recently persisted step.
    pub step: String,
}
/// CMAEG channel identity and public ingress endpoints. Treat references as opaque.
#[derive(Clone, Serialize, Deserialize)]
pub struct BotChannel {
    /// Complete opaque channel reference; do not decode or log it.
    pub channel_id: String,
    /// Reserved durable record identity.
    pub record_id: String,
    /// Current record version.
    pub version: u64,
    /// Prepared provider binding identity.
    pub binding_id: String,
    /// Advertised provider level.
    pub level: String,
    /// Complete callback URL, including the opaque channel reference.
    pub callback_url: String,
    /// Complete custom event URL.
    pub event_url: String,
}
/// A synthetic provider test result; not proof of external channel delivery.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BotTest {
    /// Time until provider acceptance.
    pub accepted_after_ms: u64,
    /// First bounded provider output.
    pub first_text: String,
    /// Observed provider level.
    pub level: String,
    /// Test completion timestamp.
    pub tested_at: String,
}
/// Owner-facing Bot projection.
#[derive(Clone, Serialize, Deserialize)]
pub struct Bot {
    /// CMA Bot identifier.
    pub bot_id: String,
    /// Owning human subject.
    pub owner: String,
    /// Display label.
    pub name: String,
    /// Selected NyxID channel bot identifier.
    pub nyxid_bot_id: String,
    /// Pinned Agent profile reference.
    pub profile: BotProfileRef,
    /// Pinned Sandbox profile; absent on servers predating environment selection.
    pub sandbox_profile: Option<BotProfileRef>,
    /// Pinned Workspace profile; absent on servers predating environment selection.
    pub workspace_profile: Option<BotProfileRef>,
    /// Complete worker revision, when selected at creation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_worker_profile: Option<BotProfileRef>,
    /// Provider slug.
    pub provider: String,
    /// Durable lifecycle state.
    pub state: BotState,
    /// Failed creation step, if any.
    pub failure: Option<String>,
    /// Work currently in progress.
    pub operation: Option<BotOperation>,
    /// Channel metadata once allocated.
    pub channel: Option<BotChannel>,
    /// Earliest current credential expiry.
    pub credentials_expire_at: Option<String>,
    /// Creation timestamp.
    pub created_at: String,
    /// Latest durable change timestamp.
    pub updated_at: String,
    /// Most recent synthetic test.
    pub last_test: Option<BotTest>,
}
/// Personally owned channel bot available for selection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalBot {
    /// NyxID channel bot identifier.
    pub id: String,
    /// Display label.
    pub name: String,
    /// Platform name.
    pub platform: String,
}
/// One page of owned bots.
#[derive(Clone, Serialize, Deserialize)]
pub struct BotPage {
    /// Bots in stable identity order.
    pub items: Vec<Bot>,
    /// Exclusive cursor for the next page, when present.
    pub next_after: Option<String>,
}
/// Bot listing options.
#[derive(Debug, Clone, Default)]
pub struct BotListOptions {
    /// Exclusive cursor returned by the previous page.
    pub after: Option<String>,
    /// Page size between 1 and 200.
    pub limit: Option<u32>,
}
