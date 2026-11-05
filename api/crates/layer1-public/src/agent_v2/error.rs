use axum::{http::StatusCode, response::Response};
use layer2_core::resource_access::{
    AccessError, MembershipError, ResourceAccessError, ResourceAccessStoreError,
};
use layer2_core::sandbox::AgentReadError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Error {
    Query,
    Cursor,
    Incarnation,
    BeyondHead,
    History,
    Unavailable,
    Missing,
    StateChanged,
}
impl Error {
    pub fn code(self) -> &'static str {
        match self {
            Self::Query => "invalid_agent_events_query",
            Self::Cursor => "invalid_agent_cursor",
            Self::Incarnation => "agent_incarnation_changed",
            Self::BeyondHead => "agent_cursor_beyond_head",
            Self::History => "agent_history_unavailable",
            Self::Unavailable => "agent_read_unavailable",
            Self::Missing => "agent_not_found",
            Self::StateChanged => "agent_state_changed",
        }
    }
    pub fn message(self) -> &'static str {
        match self {
            Self::Query => "Supply the documented Agent event query exactly once per field.",
            Self::Cursor => "Supply a valid cursor from this Agent event API.",
            Self::Incarnation => "The cursor does not belong to this Agent incarnation.",
            Self::BeyondHead => "The cursor exceeds this Agent's durable history head.",
            Self::History => "The requested Agent history cannot be verified.",
            Self::Unavailable => "Agent history is temporarily unavailable.",
            Self::Missing => "The Agent is unavailable to this caller.",
            Self::StateChanged => {
                "The Agent state changed. Restart approval pagination from the current head."
            }
        }
    }
    pub fn response(self) -> Response {
        let status = match self {
            Self::Query | Self::Cursor | Self::BeyondHead => StatusCode::BAD_REQUEST,
            Self::Incarnation | Self::StateChanged => StatusCode::CONFLICT,
            Self::Missing => StatusCode::NOT_FOUND,
            Self::History | Self::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        };
        crate::conversation::failure(status, self.code(), self.message())
    }
}
impl From<AgentReadError> for Error {
    fn from(value: AgentReadError) -> Self {
        match value {
            AgentReadError::NotFound
            | AgentReadError::Forbidden
            | AgentReadError::Access(ResourceAccessError::Access(
                AccessError::NotFound
                | AccessError::Forbidden
                | AccessError::UnsafeInteractiveShare,
            ))
            | AgentReadError::Access(ResourceAccessError::Store(
                ResourceAccessStoreError::NotFound,
            ))
            | AgentReadError::Access(ResourceAccessError::Membership(MembershipError::NotMember)) => {
                Self::Missing
            }
            AgentReadError::Corrupt(_) => Self::History,
            AgentReadError::InvalidPage => Self::BeyondHead,
            _ => Self::Unavailable,
        }
    }
}
