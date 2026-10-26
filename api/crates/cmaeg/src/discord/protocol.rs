use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;
use url::Url;

// GUILDS, GUILD_MESSAGES, DIRECT_MESSAGES and privileged MESSAGE_CONTENT.
pub const MESSAGE_INTENTS: u64 = 1 | (1 << 9) | (1 << 12) | (1 << 15);
pub(super) const FRAME_LIMIT: usize = 1024 * 1024;
pub(super) const QUEUE_LIMIT: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GatewayError {
    InvalidConfiguration,
    MissingMessageContentIntent,
    Authority,
    Unavailable,
    Protocol,
}

/// Resume authority is private durable state, never logs or a public response.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Checkpoint {
    pub session_id: String,
    pub resume_url: String,
    pub sequence: u64,
}

/// Raw events came from the authenticated Discord WebSocket, never from NyxID.
/// MESSAGE_CREATE idempotency must use (native source record, data.id), because
/// a fresh Discord session can redeliver the same message under a new sequence.
pub struct Dispatch {
    pub name: String,
    pub sequence: u64,
    pub data: Value,
}
impl Dispatch {
    pub fn message_id(&self) -> Option<&str> {
        (self.name == "MESSAGE_CREATE")
            .then(|| snowflake(&self.data["id"]))
            .flatten()
    }
}

/// A native source implements this against its sealed records. No process-local
/// implementation is suitable for production. Every method must fence current
/// source ownership and credential revision. Implementations must be cancellable.
pub trait NativeDiscordJournal: Send + Sync {
    /// Validate current source ownership and credential revision before opening
    /// any socket. For a fresh connection, also reserve IDENTIFY durably against
    /// /gateway/bot session_start_limit and max_concurrency buckets (at least 5 s
    /// between the same bucket). Failed connections conservatively consume that
    /// reservation. Resume never silently falls back to an unreserved IDENTIFY.
    fn authorize_connection<'a>(
        &'a self,
        resume: Option<&'a Checkpoint>,
    ) -> BoxFuture<'a, Result<(), GatewayError>>;
    /// Persist the exact event/admission and checkpoint atomically (or advance
    /// the checkpoint only after verified admission). Known duplicate native
    /// message IDs restore the original admission, never create another turn.
    /// Other dispatches must preserve sequence order even if they launch no work.
    fn admit<'a>(
        &'a self,
        checkpoint: &'a Checkpoint,
        event: &'a Dispatch,
    ) -> BoxFuture<'a, Result<(), GatewayError>>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndReason {
    Stopped,
    Transport,
    Reconnect,
    InvalidSession,
    Backpressure,
    CommitUnavailable,
    HeartbeatLost,
    RateLimited,
    AuthenticationFailed,
    AuthorityRevoked,
    DisallowedIntents,
    InvalidIntents,
    ShardingRequired,
    UnsupportedVersion,
    Protocol,
}
pub struct GatewayEnd {
    pub reason: EndReason,
    pub checkpoint: Option<Checkpoint>,
    pub retry_after: Duration,
}
impl GatewayEnd {
    pub fn retryable(&self) -> bool {
        !matches!(
            self.reason,
            EndReason::Stopped
                | EndReason::AuthenticationFailed
                | EndReason::AuthorityRevoked
                | EndReason::DisallowedIntents
                | EndReason::InvalidIntents
                | EndReason::ShardingRequired
                | EndReason::UnsupportedVersion
                | EndReason::Protocol
        )
    }
}

pub(super) fn endpoint(raw: &str) -> Result<Url, GatewayError> {
    let mut url = Url::parse(raw).map_err(|_| GatewayError::Protocol)?;
    let host = url.host_str().ok_or(GatewayError::Protocol)?;
    if url.scheme() != "wss"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some_and(|port| port != 443)
        || !matches!(url.path(), "" | "/")
        || url.fragment().is_some()
        || !(host == "gateway.discord.gg"
            || (host.starts_with("gateway-") && host.ends_with(".discord.gg")))
    {
        return Err(GatewayError::Protocol);
    }
    url.set_query(Some("v=10&encoding=json"));
    Ok(url)
}
pub(super) fn snowflake(value: &Value) -> Option<&str> {
    value
        .as_str()
        .filter(|id| !id.is_empty() && id.len() <= 32 && id.bytes().all(|b| b.is_ascii_digit()))
}
pub(super) fn checkpoint(
    previous: Option<&Checkpoint>,
    event: &Dispatch,
    bot_id: &str,
) -> Result<Checkpoint, GatewayError> {
    if event.name == "READY" {
        let session = event.data["session_id"]
            .as_str()
            .filter(|s| !s.is_empty() && s.len() <= 256 && s.bytes().all(|b| b.is_ascii_graphic()))
            .ok_or(GatewayError::Protocol)?;
        if snowflake(&event.data["user"]["id"]) != Some(bot_id) || event.data["user"]["bot"] != true
        {
            return Err(GatewayError::Authority);
        }
        let resume = endpoint(
            event.data["resume_gateway_url"]
                .as_str()
                .ok_or(GatewayError::Protocol)?,
        )?;
        return Ok(Checkpoint {
            session_id: session.into(),
            resume_url: resume.to_string(),
            sequence: event.sequence,
        });
    }
    let mut next = previous.cloned().ok_or(GatewayError::Protocol)?;
    next.sequence = event.sequence;
    Ok(next)
}
pub(super) fn close(code: u16) -> (EndReason, bool) {
    match code {
        4004 => (EndReason::AuthenticationFailed, true),
        4013 => (EndReason::InvalidIntents, true),
        4014 => (EndReason::DisallowedIntents, true),
        4010 | 4011 => (EndReason::ShardingRequired, true),
        4012 => (EndReason::UnsupportedVersion, true),
        1000 | 1001 | 4007 | 4009 => (EndReason::InvalidSession, true),
        4008 => (EndReason::RateLimited, false),
        _ => (EndReason::Reconnect, false),
    }
}
