//! Layer 2 — core session model and events.
//!
//! The single tunnel every Layer 1 interface talks to. Protocol-agnostic and
//! kernel-agnostic: no protocol name and no kernel name may appear here.

use serde::{Deserialize, Serialize};

/// Lifecycle state of a session. `Idle` means the runtime was reclaimed while
/// the session itself remains valid and revivable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    /// Runtime is being provisioned.
    Provisioning,
    /// Runtime is up and accepting work.
    Ready,
    /// A turn is in progress.
    Running,
    /// Runtime reclaimed; session intact and revivable.
    Idle,
    /// Terminal state.
    Terminated,
}

/// Identifier of a session. Stable across an idle to revive cycle.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(pub String);

/// A durable, ordered event on a session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    /// Session this event belongs to.
    pub session_id: SessionId,
    /// Monotonic sequence, per session.
    pub seq: u64,
    /// Event discriminator.
    pub kind: EventKind,
}

/// The semantic event vocabulary. Superset of what any single interface projects.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum EventKind {
    /// Session status changed.
    StatusChanged {
        /// The new status.
        status: SessionStatus,
    },
    /// A turn began.
    TurnStarted,
    /// A turn completed.
    TurnCompleted,
    /// The runtime is blocked awaiting a decision from the client.
    ApprovalRequested {
        /// Correlates with the resolving decision.
        request_id: String,
    },
    /// A pending approval was resolved.
    ApprovalResolved {
        /// The request that was resolved.
        request_id: String,
        /// Whether the action was permitted.
        allowed: bool,
    },
}

/// Errors surfaced by the core.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    /// No such session.
    #[error("session not found")]
    SessionNotFound,
    /// The operation is not valid in the session's current state.
    #[error("invalid state for operation")]
    InvalidState,
}
