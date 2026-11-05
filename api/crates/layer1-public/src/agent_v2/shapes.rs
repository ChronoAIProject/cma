use layer2_core::sandbox::agent_io::AgentEventKind;
use serde::Serialize;

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub(super) struct EventItem {
    pub cursor: String,
    pub sequence: u64,
    pub appended_at: String,
    #[schema(value_type = Option<serde_json::Value>)]
    pub kind: Option<AgentEventKind>,
    pub response_binding: Option<ResponseBinding>,
    /// Present only for verified steering input when Prefer: cma-message-bindings is requested.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_command_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub(super) struct ResponseBinding {
    pub command_id: String,
    pub response_id: String,
    pub turn_sequence: u64,
}

impl From<layer2_core::sandbox::AgentResponseBinding> for ResponseBinding {
    fn from(value: layer2_core::sandbox::AgentResponseBinding) -> Self {
        Self {
            command_id: value.command_id.to_string(),
            response_id: value.response_id.to_string(),
            turn_sequence: value.turn_sequence,
        }
    }
}

#[derive(Serialize, utoipa::ToSchema)]
pub(super) struct EventHead {
    pub object: &'static str,
    pub schema_version: u16,
    pub agent_id: String,
    pub incarnation: String,
    pub through_cursor: String,
    pub active_turn: Option<EventItem>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub(super) struct EventPage {
    pub object: &'static str,
    pub schema_version: u16,
    pub agent_id: String,
    pub incarnation: String,
    pub through_cursor: String,
    pub items: Vec<EventItem>,
    pub next_page_cursor: Option<String>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub(super) struct StatePage {
    pub object: &'static str,
    pub schema_version: u16,
    pub agent_id: String,
    pub incarnation: String,
    pub through_cursor: String,
    pub context: Vec<EventItem>,
    pub approvals: Vec<EventItem>,
    pub next_approval_cursor: Option<String>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub(super) struct HistoryWindow {
    pub object: &'static str,
    pub schema_version: u16,
    pub agent_id: String,
    pub incarnation: String,
    /// Exclusive lower boundary; context does not acknowledge earlier events.
    pub start_cursor: String,
    pub through_cursor: String,
    pub context: Option<EventItem>,
    /// Original first loss event in that turn before the lower boundary.
    pub gap: Option<EventItem>,
    pub items: Vec<EventItem>,
    /// Pass as through_cursor to read the immediately preceding window.
    pub previous_cursor: Option<String>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub(super) struct DurableEvent {
    pub agent_id: String,
    pub incarnation: String,
    #[serde(flatten)]
    pub item: EventItem,
}
