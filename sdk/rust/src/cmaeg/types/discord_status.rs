//! Native Discord status is local observation, never handover authority.
use serde::{Deserialize, Serialize};

/// Additive connection status. Unknown future vocabulary is retained as strings.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeDiscordConnectionStatus {
    /// Always serving_instance in this version; other replicas may own the lease.
    pub scope: String,
    /// Latest locally observed connection stage.
    pub stage: String,
    /// unknown, pending, ok, retrying, blocked, or stopped.
    pub outcome: String,
    /// Null when local current-revision evidence is absent or stale.
    pub connected: Option<bool>,
    /// True only after validated READY/RESUMED and durable journal commit.
    pub authenticated: Option<bool>,
    /// Same freshness and authority limits as authenticated.
    pub ready: Option<bool>,
    /// Local lease observation: unknown, acquired, contended, lost, released.
    pub lease: String,
    /// not_attempted, reserved, sent, resuming, or resume_sent.
    pub identify: String,
    /// RFC3339 timestamp of the latest observation.
    pub observed_at: Option<String>,
    /// Display deadline; monotonic lease and heartbeat deadlines enforce freshness.
    pub valid_until: Option<String>,
    /// True when the observation's local evidence has expired.
    pub stale: bool,
    /// Latest validated, durably committed READY or RESUMED.
    pub last_ready_at: Option<String>,
    /// Latest authenticated MESSAGE_CREATE reception; not proof of commit.
    pub last_event_at: Option<String>,
    /// Latest message's durable journal commit.
    pub last_event_committed_at: Option<String>,
    /// Latest native routed admission observation; not reply delivery.
    pub last_dispatch_at: Option<String>,
    /// unknown, admitted, pending, refused, expired, or invalid.
    pub last_dispatch_outcome: String,
    /// Stage of the most recent failure, retained across attempts of that revision.
    pub last_failure_stage: Option<String>,
    /// Fixed reason only; no upstream error, identity or secret data.
    pub last_failure_code: Option<String>,
    /// RFC3339 timestamp of the most recent failure.
    pub last_failure_at: Option<String>,
    /// Next retry estimate; null for terminal failures or no pending retry.
    pub retry_at: Option<String>,
}

/// Fixed server vocabulary, represented as text for forward compatibility.
pub type NativeDiscordConnectionStage = String;

/// Fixed server vocabulary, represented as text for forward compatibility.
pub type NativeDiscordConnectionOutcome = String;

/// Fixed server vocabulary, represented as text for forward compatibility.
pub type NativeDiscordConnectionFailureCode = String;
