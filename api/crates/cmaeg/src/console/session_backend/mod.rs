//! Shared Console authority. Callers encrypt the payload and give every
//! revision a unique nonce; a successful compare-exchange never recreates a row.
use futures_util::future::BoxFuture;

use super::session::SessionLimits;

mod create;
mod mongo;
pub mod schema;
pub use mongo::MongoSessionBackend;

/// A snapshot taken at Mongo's clock, never at a process-local `Instant`.
#[derive(Clone)]
pub struct Stored {
    pub payload: Vec<u8>,
    pub subject_hash: String,
    pub now_ms: i64,
    pub created_ms: i64,
    pub last_seen_ms: i64,
    /// Absolute deadline. Idle expiry is also enforced by every backend read.
    pub expires_ms: i64,
    pub access_expires_ms: i64,
    /// Best-effort provider cleanup after a proved create commit. Bounded to
    /// four victims; authority is removed for every evicted row regardless.
    pub evicted: Vec<Evicted>,
}

#[derive(Clone)]
pub struct Evicted {
    pub id: String,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreError {
    Unavailable,
    Invalid,
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Unavailable => "console session storage unavailable",
            Self::Invalid => "invalid console session storage input",
        })
    }
}
impl std::error::Error for StoreError {}

/// A completed load is an authority observation, including for cached handles.
/// Errors never permit fallback to local state. An uncertain CAS must be
/// reconciled by loading the exact payload; it is not permission to repeat an
/// external refresh. `revoke` succeeds only after durable deletion or proved
/// absence. Session identifiers are never reused.
pub trait SessionBackend: Send + Sync {
    fn create<'a>(
        &'a self,
        id: &'a str,
        subject_hash: &'a str,
        payload: Vec<u8>,
        access_ttl_ms: i64,
        limits: &'a SessionLimits,
    ) -> BoxFuture<'a, Result<Stored, StoreError>>;
    fn load<'a>(
        &'a self,
        id: &'a str,
        touch: bool,
    ) -> BoxFuture<'a, Result<Option<Stored>, StoreError>>;
    fn compare_exchange<'a>(
        &'a self,
        id: &'a str,
        expected: &'a [u8],
        next: Vec<u8>,
        access_expires_ms: Option<i64>,
    ) -> BoxFuture<'a, Result<Option<Stored>, StoreError>>;
    /// Final admission of the exact token/owner snapshot checked by the caller.
    /// The database must still show a useful access-token lifetime at this fence.
    fn admit<'a>(
        &'a self,
        id: &'a str,
        expected: &'a [u8],
    ) -> BoxFuture<'a, Result<bool, StoreError>>;
    /// The acknowledged deletion returns its exact encrypted payload for
    /// bounded provider cleanup. Proved absence without a receipt returns None.
    fn revoke<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<Option<Vec<u8>>, StoreError>>;
}

const MAX_PAYLOAD: usize = 65_536;
const MAX_SESSIONS: usize = 100_000;
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);
const SERVER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

fn identifier(value: &str) -> bool {
    (value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit()))
        || (value.len() == 43
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b)))
}

fn payload_valid(payload: &[u8]) -> bool {
    !payload.is_empty() && payload.len() <= MAX_PAYLOAD
}

#[cfg(test)]
mod mongo_tests;
#[cfg(test)]
mod tests;
