//! Persisted routed-execution phases and provider recovery capabilities.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutionError {
    Abandoned,
    IdempotencyConflict,
    OperationLimit,
    Invalid,
    Unavailable,
    Uncertain,
    Corrupt,
    LostLease,
    Busy,
    Expired,
    AuthorityChanged,
    Unsupported,
}

/// Capabilities proved against the configured provider, not inferred from a slug.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RecoverySupport {
    pub exact_body_idempotency: bool,
    pub response_replay: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state", deny_unknown_fields)]
pub enum Phase {
    Pending,
    Prepared,
    Starting,
    Unknown,
    Accepted {
        response_id: String,
        durable_cursor: Option<u64>,
    },
    Completed {
        response_id: String,
        durable_cursor: Option<u64>,
    },
    Refused {
        code: String,
    },
}
impl Phase {
    pub fn terminal(&self) -> bool {
        matches!(self, Self::Completed { .. } | Self::Refused { .. })
    }
}
