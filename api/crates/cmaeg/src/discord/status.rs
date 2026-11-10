//! Public observations contain only fixed vocabulary, booleans and timestamps.
use serde::Serialize;
use utoipa::ToSchema;

#[path = "status_attempt.rs"]
mod attempt;
#[path = "status_registry.rs"]
mod registry;
pub(crate) use attempt::StatusAttempt;
pub(crate) use registry::StatusRegistry;

macro_rules! vocabulary {
    ($name:ident { $($variant:ident => $wire:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
        #[serde(rename_all = "snake_case")]
        pub enum $name { $(#[serde(rename = $wire)] $variant),+ }
        impl $name {
            pub(crate) fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $wire),+ }
            }
        }
    };
}
vocabulary!(NativeDiscordConnectionStage {
    Unobserved => "unobserved", Resolve => "resolve", Discovery => "discovery",
    Lease => "lease", Authorize => "authorize", Socket => "socket", Hello => "hello",
    Identify => "identify", Resume => "resume", Ready => "ready", Renew => "renew",
    EventCommit => "event_commit", Stopped => "stopped"
});
vocabulary!(NativeDiscordConnectionOutcome {
    Unknown => "unknown", Pending => "pending", Ok => "ok", Retrying => "retrying",
    Blocked => "blocked", Stopped => "stopped"
});
vocabulary!(NativeDiscordConnectionFailureCode {
    Unavailable => "unavailable", AuthorityRefused => "authority_refused",
    InvalidConfiguration => "invalid_configuration", MissingMessageContentIntent => "missing_message_content_intent",
    RateLimited => "rate_limited", Protocol => "protocol",
    DiscoveryUnauthorized => "discovery_unauthorized", BotIdentityMismatch => "bot_identity_mismatch",
    ShardingUnsupported => "sharding_unsupported", SessionLimitInvalid => "session_limit_invalid",
    LeaseContended => "lease_contended", LeaseOwnerMismatch => "lease_owner_mismatch",
    LeaseSourceMismatch => "lease_source_mismatch", LeaseInvalid => "lease_invalid",
    IdentifyBudgetExhausted => "identify_budget_exhausted", IdentifyBucketWait => "identify_bucket_wait",
    HelloTimeout => "hello_timeout", HelloInvalid => "hello_invalid",
    AuthenticationFailed => "authentication_failed", DisallowedIntents => "disallowed_intents",
    InvalidIntents => "invalid_intents", ShardingRequired => "sharding_required",
    UnsupportedVersion => "unsupported_version", Backpressure => "backpressure",
    CommitUnavailable => "commit_unavailable", HeartbeatLost => "heartbeat_lost",
    RenewFailed => "renew_failed", RenewTimeout => "renew_timeout",
    AuthorityRevoked => "authority_revoked", InvalidSession => "invalid_session",
    Reconnect => "reconnect", Transport => "transport"
});

pub(crate) use NativeDiscordConnectionFailureCode as FailureCode;
pub(crate) use NativeDiscordConnectionOutcome as Outcome;
pub(crate) use NativeDiscordConnectionStage as Stage;

/// This serving process's observation, never a cluster-wide handover authority.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct NativeDiscordConnectionStatus {
    /// Always `serving_instance`; another replica may own the connection.
    pub scope: String,
    pub stage: Stage,
    pub outcome: Outcome,
    /// Null for an unobserved, stale or different source revision.
    pub connected: Option<bool>,
    pub authenticated: Option<bool>,
    pub ready: Option<bool>,
    /// unknown, acquired, contended, lost or released.
    pub lease: String,
    /// not_attempted, reserved, sent, resuming or resume_sent.
    pub identify: String,
    pub observed_at: Option<String>,
    pub valid_until: Option<String>,
    pub stale: bool,
    pub last_ready_at: Option<String>,
    /// Last authenticated MESSAGE_CREATE received, before durable commit.
    pub last_event_at: Option<String>,
    pub last_event_committed_at: Option<String>,
    pub last_dispatch_at: Option<String>,
    /// unknown, admitted, pending, refused, expired or invalid; not reply delivery.
    pub last_dispatch_outcome: String,
    pub last_failure_stage: Option<Stage>,
    pub last_failure_code: Option<FailureCode>,
    pub last_failure_at: Option<String>,
    pub retry_at: Option<String>,
}
impl Default for NativeDiscordConnectionStatus {
    fn default() -> Self {
        Self {
            scope: "serving_instance".into(),
            stage: Stage::Unobserved,
            outcome: Outcome::Unknown,
            connected: None,
            authenticated: None,
            ready: None,
            lease: "unknown".into(),
            identify: "not_attempted".into(),
            observed_at: None,
            valid_until: None,
            stale: false,
            last_ready_at: None,
            last_event_at: None,
            last_event_committed_at: None,
            last_dispatch_at: None,
            last_dispatch_outcome: "unknown".into(),
            last_failure_stage: None,
            last_failure_code: None,
            last_failure_at: None,
            retry_at: None,
        }
    }
}
impl From<super::GatewayError> for FailureCode {
    fn from(error: super::GatewayError) -> Self {
        use super::GatewayError as E;
        match error {
            E::Authority => Self::AuthorityRefused,
            E::Unavailable => Self::Unavailable,
            E::InvalidConfiguration => Self::InvalidConfiguration,
            E::MissingMessageContentIntent => Self::MissingMessageContentIntent,
            E::RateLimited => Self::RateLimited,
            E::Protocol => Self::Protocol,
        }
    }
}
impl From<super::EndReason> for FailureCode {
    fn from(reason: super::EndReason) -> Self {
        use super::EndReason as E;
        match reason {
            E::Stopped | E::AuthorityRevoked => Self::AuthorityRevoked,
            E::Transport => Self::Transport,
            E::Reconnect => Self::Reconnect,
            E::InvalidSession => Self::InvalidSession,
            E::Backpressure => Self::Backpressure,
            E::CommitUnavailable => Self::CommitUnavailable,
            E::HeartbeatLost => Self::HeartbeatLost,
            E::RateLimited => Self::RateLimited,
            E::AuthenticationFailed => Self::AuthenticationFailed,
            E::DisallowedIntents => Self::DisallowedIntents,
            E::InvalidIntents => Self::InvalidIntents,
            E::ShardingRequired => Self::ShardingRequired,
            E::UnsupportedVersion => Self::UnsupportedVersion,
            E::Protocol => Self::Protocol,
        }
    }
}
#[cfg(test)]
#[path = "status_tests.rs"]
mod tests;
