//! Owner-visible recent activity and actual delivery observations.
use super::{ActivityConversation, Actor, SourceKind};
use serde::{Deserialize, Serialize};

/// Complete bounded snapshot; replace prior items atomically.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ActivityPage {
    /// Recent activity, not a complete audit log.
    pub items: Vec<ChannelActivity>,
    /// Opaque channel-bound snapshot watermark, usable as Last-Event-ID.
    pub cursor: String,
    /// Retention window advertised by the gateway.
    pub retention_days: u32,
    /// Maximum number of items returned in this snapshot.
    pub limit: u32,
    /// More retained activity exists beyond this snapshot's limit.
    pub has_more: bool,
}

/// One received event and its consumer observations.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelActivity {
    /// Source event identifier.
    pub event_id: String,
    /// Source transport.
    pub source: SourceKind,
    /// Source actor.
    pub actor: Actor,
    /// Source conversation.
    pub conversation: ActivityConversation,
    /// Normalized event time: source timestamp when supplied, otherwise gateway normalization time.
    pub occurred_at: String,
    /// Gateway receive timestamp, persisted before provider launch.
    pub received_at: String,
    /// Consumer execution and delivery observations.
    pub consumers: Vec<ActivityConsumer>,
}

/// Persisted consumer processing state; completion alone does not prove delivery.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsumerState {
    /// Gateway received the event.
    Received,
    /// Waiting for dispatch.
    Queued,
    /// Provider dispatch started.
    Dispatching,
    /// Provider accepted the event.
    Accepted,
    /// Provider produced a response.
    Produced,
    /// Provider processing completed.
    Completed,
    /// Admission was refused.
    Refused,
    /// Processing failed.
    Failed,
    /// The outcome is not known.
    Unknown,
}

/// Observations for one consumer of an event.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ActivityConsumer {
    /// Stable consumer identity.
    pub consumer_id: String,
    /// Display name when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Provider agent identity when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// Provider name when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Provider conversation identity when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    /// Persisted processing state.
    pub state: ConsumerState,
    /// Dispatch observation timestamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dispatched_at: Option<String>,
    /// Provider acceptance timestamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepted_at: Option<String>,
    /// Response production timestamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub produced_at: Option<String>,
    /// Processing completion timestamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    /// Provider response identity when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_id: Option<String>,
    /// Actual transport outcomes, independent of processing state.
    pub deliveries: Vec<ActivityDelivery>,
}

/// One observed transport outcome.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ActivityDelivery {
    /// Delivery step.
    pub step: String,
    /// Destination transport.
    pub destination: String,
    /// Actual transport outcome.
    pub outcome: String,
    /// When the gateway observed the actual transport outcome.
    pub observed_at: String,
    /// Present only for a confirmed platform/transport receipt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmed_at: Option<String>,
    /// Platform message identifier when returned by the transport.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform_message_id: Option<String>,
    /// Actual delivery recipients.
    pub recipients: Vec<ActivityRecipient>,
}

/// One recipient observed during delivery.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ActivityRecipient {
    /// Recipient kind.
    pub kind: String,
    /// Recipient identity.
    pub id: String,
    /// Native thread identity when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
}
