use super::{AgentCursor, AgentEventKind, AgentPageCursor};
use serde::{Deserialize, Deserializer};

pub(super) fn required_nullable<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> std::result::Result<Option<T>, D::Error> {
    Option::<T>::deserialize(d)
}
string_enum! { /// Exact transport eligibility resource discriminator.
    pub enum TransportObject { Transport => "cma.ui.transport" }
}
string_enum! { /// Exact snapshot resource discriminator.
    pub enum EventPageObject { EventPage => "cma.agent.event_page" }
}
string_enum! { /// Exact live-head resource discriminator.
    pub enum EventHeadObject { EventHead => "cma.agent.event_head" }
}
/// Current live baseline, independent of a complete historical snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentEventHead {
    /// Exact resource discriminator.
    pub object: EventHeadObject,
    /// Head schema, currently one.
    pub schema_version: u16,
    /// Requested Agent identity.
    pub agent_id: String,
    /// Incarnation bound into every coordinate.
    pub incarnation: String,
    /// Server-issued tail boundary; does not acknowledge historical rows.
    pub through_cursor: AgentCursor,
    /// Exact active turn-start row and its response binding, or null when idle.
    #[serde(deserialize_with = "required_nullable")]
    pub active_turn: Option<AgentEventItem>,
}
/// Authenticated observation for selecting a new tab's transport, never command authority.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UiTransport {
    /// Exact resource discriminator.
    pub object: TransportObject,
    /// Supported eligibility schema, currently one.
    pub schema_version: u16,
    /// Actual process flag and complete composed backend capability set.
    pub enabled: bool,
    /// Selection scope, exactly `new_tabs`.
    pub selection: String,
    /// Input contract, exactly `responses_v2`.
    pub input: String,
    /// Required snapshot schema.
    pub snapshot_schema: u16,
    /// Required event schema.
    pub event_schema: u16,
    /// Required targeted control schema.
    pub control_schema: u16,
    /// Whether a live recorder is composed; missed reasoning remains unrecoverable.
    pub live_fragments: bool,
    /// Exact snapshot route template.
    pub snapshot: String,
    /// Exact Agent tail route template.
    pub events: String,
    /// Exact targeted controls route template.
    pub controls: String,
    /// Exact command status route template.
    pub command_status: String,
}
/// Verified common-command binding present only on its exact turn-start row.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResponseBinding {
    /// Common command identity.
    pub command_id: String,
    /// Source response identity.
    pub response_id: String,
    /// Exact dispatch-bound turn sequence.
    pub turn_sequence: u64,
}
/// One complete durable projection, including withheld rows with a null kind.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentEventItem {
    /// Opaque resume coordinate.
    pub cursor: AgentCursor,
    /// Numeric chronology sequence, independent of Responses sequence numbers.
    pub sequence: u64,
    /// Original immutable append timestamp.
    pub appended_at: String,
    /// Safe visible event, or explicit null for a withheld source row.
    #[serde(deserialize_with = "required_nullable")]
    pub kind: Option<AgentEventKind>,
    /// Verified source response only for the exact common-command turn start.
    #[serde(deserialize_with = "required_nullable")]
    pub response_binding: Option<ResponseBinding>,
}
/// One bounded page through a frozen durable watermark.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentEventPage {
    /// Exact resource discriminator.
    pub object: EventPageObject,
    /// Event page schema, currently one.
    pub schema_version: u16,
    /// Requested Agent identity.
    pub agent_id: String,
    /// Incarnation bound into every coordinate.
    pub incarnation: String,
    /// Frozen watermark; acknowledge for tail startup only after all pages apply.
    pub through_cursor: AgentCursor,
    /// Consecutive immutable log projections, including withheld rows.
    pub items: Vec<AgentEventItem>,
    /// Next bounded page; null only when the frozen prefix is fully delivered.
    #[serde(deserialize_with = "required_nullable")]
    pub next_page_cursor: Option<AgentPageCursor>,
}
/// One durable SSE frame carrying explicit resource identity.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentEvent {
    /// Requested Agent identity.
    pub agent_id: String,
    /// Requested Agent incarnation.
    pub incarnation: String,
    /// Opaque resume coordinate matching the explicit SSE ID.
    pub cursor: AgentCursor,
    /// Numeric chronology sequence.
    pub sequence: u64,
    /// Original append timestamp.
    pub appended_at: String,
    /// Safe projection, or explicit null for a withheld source row.
    #[serde(deserialize_with = "required_nullable")]
    pub kind: Option<AgentEventKind>,
    /// Verified source response on its exact turn start.
    #[serde(deserialize_with = "required_nullable")]
    pub response_binding: Option<ResponseBinding>,
}
string_enum! { /// Ephemeral fragment class; neither class advances a cursor.
    pub enum LiveKind { TextDelta => "text_delta", ReasoningSummaryDelta => "reasoning_summary_delta" }
}
/// Ephemeral current-turn text or reasoning summary, never durable history.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentLive {
    /// Requested Agent identity.
    pub agent_id: String,
    /// Current Agent incarnation.
    pub incarnation: String,
    /// Exact active turn.
    pub turn_sequence: u64,
    /// Durable chronology anchor already delivered before this fragment.
    pub after_sequence: u64,
    /// Live message identity for committed-text reconciliation.
    pub message_id: String,
    /// Server live-fragment observation time; absent unless supported and requested.
    #[serde(default)]
    pub observed_at: Option<String>,
    /// Text or reasoning summary.
    pub kind: LiveKind,
    /// Ephemeral text, which must not enter durable reasoning storage.
    pub delta: String,
}
string_enum! { /// Observed cause of live-only loss.
    pub enum LiveGapReason { Lagged => "lagged", Reconnected => "reconnected" }
}
/// Safe live-only gap indication; reconnection loss has no invented frame count.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentLiveGap {
    /// Requested Agent identity.
    pub agent_id: String,
    /// Current incarnation.
    pub incarnation: String,
    /// Current active turn when known.
    #[serde(deserialize_with = "required_nullable")]
    pub turn_sequence: Option<u64>,
    /// Known bounded-hub lag count, or null for unknown reconnection loss.
    #[serde(deserialize_with = "required_nullable")]
    pub frames: Option<u64>,
    /// Observed cause.
    pub reason: LiveGapReason,
}
/// Authorized stream failure without private state or cursor acknowledgement.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentStreamError {
    /// Machine-readable safe error code.
    pub code: String,
    /// Safe public explanation.
    pub message: String,
}
/// One complete typed Agent frame; only `Event` is durable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentFrame {
    /// Consecutive durable history, including null-kind rows.
    Event(AgentEvent),
    /// Ephemeral text or reasoning summary.
    Live(AgentLive),
    /// Ephemeral loss observation.
    LiveGap(AgentLiveGap),
    /// Authorized observation failure; subsequent reading closes.
    Error(AgentStreamError),
}
