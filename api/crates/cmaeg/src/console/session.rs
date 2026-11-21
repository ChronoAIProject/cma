//! Console session snapshots with an optional shared durable authority.
//! Local compatibility mode remains available until the owner activates migration.
//!
//! Locking rule: a request clones its cell's `Arc` and drops the map guard
//! before any await. `meta` is a std mutex held only for short synchronous
//! reads and writes; `auth` is an async mutex that makes refresh single-flight
//! and serializes one session's NyxID checks. No `DashMap` guard and no std
//! mutex guard is ever held across an await. The session keeps no authority
//! verdict: REST's creator cache is the only cache (Amendment R6 §R6.4).
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, MutexGuard,
    },
    time::{Duration, Instant},
};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use cmaeg_core::channel::Secret;
use dashmap::DashMap;
use ring::rand::{SecureRandom, SystemRandom};
use sha2::{Digest, Sha256};

use super::{clock::Clock, oidc::EndpointCell, revoke::Revoker, subject_hash_prefix, token::*};
use crate::{telemetry::names, ConsoleConfig};

mod codec;
mod local;
mod refresh;
mod shared;
use super::session_backend::{SessionBackend, StoreError};
use cmaeg_core::seal::MasterKeyRing;
use codec::Snapshot;
use shared::SharedSessions;

/// A transient refresh failure is retried after this long.
const REFRESH_RETRY: Duration = Duration::from_secs(10);
/// An access token this close to expiry is refreshed, and is not served.
const EXPIRY_MARGIN: Duration = Duration::from_secs(30);
const SESSION_ID_BYTES: usize = 32;

/// The session bounds, from the console configuration.
#[derive(Debug, Clone)]
pub struct SessionLimits {
    pub ttl: Duration,
    pub idle: Duration,
    pub max: usize,
    pub per_subject: usize,
}

impl From<&ConsoleConfig> for SessionLimits {
    fn from(config: &ConsoleConfig) -> Self {
        Self {
            ttl: config.session_ttl,
            idle: config.session_idle,
            max: config.sessions_max,
            per_subject: config.sessions_per_subject,
        }
    }
}

/// Whose channels the session looks at: a selection, never a grant.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum OwnerSelection {
    Personal,
    Organization(String),
}

/// A one-time notice shown on the next page.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Flash {
    AdminEnded { organization: String },
    OwnerRefused,
}

/// An organization offered by the owner switcher; display only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrgChoice {
    pub id: String,
    pub name: String,
    pub admin: bool,
}

/// What a session shows and selects. Short synchronous access only.
pub struct SessionMeta {
    pub subject: String,
    pub selected: OwnerSelection,
    pub created_at: Instant,
    pub last_seen: Instant,
    pub csrf_key: [u8; 32],
    pub flash: Option<Flash>,
    /// The switcher's list and when it was fetched.
    pub orgs: Option<(Vec<OrgChoice>, Instant)>,
}

/// A session's NyxID tokens. Reached only through the `auth` async mutex.
pub struct SessionAuth {
    access: Secret,
    refresh: Secret,
    access_expires_at: Instant,
    refresh_retry_at: Option<Instant>,
}

impl SessionAuth {
    pub fn access(&self) -> &Secret {
        &self.access
    }

    pub(crate) fn refresh(&self) -> &Secret {
        &self.refresh
    }

    /// True while the access token is valid for more than 30 s.
    pub fn access_usable(&self, now: Instant) -> bool {
        self.access_expires_at > now + EXPIRY_MARGIN
    }

    fn rotate(&mut self, tokens: TokenSet, issued_at: Instant) {
        self.access = tokens.access;
        self.refresh = tokens.refresh;
        self.access_expires_at = issued_at + tokens.expires_in;
        self.refresh_retry_at = None;
    }
}

pub struct SessionCell {
    key: [u8; 32],
    meta: Mutex<SessionMeta>,
    auth: tokio::sync::Mutex<SessionAuth>,
    ended: AtomicBool,
    snapshot: Mutex<Option<Snapshot>>,
}

impl SessionCell {
    pub fn meta(&self) -> MutexGuard<'_, SessionMeta> {
        self.meta
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub async fn lock_auth(&self) -> tokio::sync::MutexGuard<'_, SessionAuth> {
        self.auth.lock().await
    }

    /// True once the session was removed; a request still holding the cell
    /// must not use it.
    pub fn is_ended(&self) -> bool {
        self.ended.load(Ordering::Acquire)
    }
}

/// Why a session ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndReason {
    SubjectCap,
    GlobalCap,
    Idle,
    Absolute,
    Logout,
    /// NyxID refused the refresh token or the identity.
    Refused,
}

impl EndReason {
    fn label(self) -> &'static str {
        match self {
            Self::SubjectCap => "subject_cap",
            Self::GlobalCap => "global_cap",
            Self::Idle => "idle",
            Self::Absolute => "absolute",
            Self::Logout => "logout",
            Self::Refused => "refused",
        }
    }

    fn eviction(self) -> bool {
        matches!(
            self,
            Self::SubjectCap | Self::GlobalCap | Self::Idle | Self::Absolute
        )
    }
}

/// The outcome of a refresh attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refresh {
    NotDue,
    Rotated,
    /// NyxID could not answer now. A refresh is due only within 30 s of
    /// expiry, so the current access token is no longer served either.
    Transient,
    /// NyxID refused the refresh token; the session has ended.
    Ended,
}

pub struct SessionStore {
    sessions: DashMap<[u8; 32], Arc<SessionCell>>,
    limits: SessionLimits,
    clock: Arc<dyn Clock>,
    tokens: TokenClient,
    endpoints: Arc<EndpointCell>,
    revoker: Revoker,
    shared: Option<SharedSessions>,
}

impl SessionStore {
    pub fn new(
        limits: SessionLimits,
        clock: Arc<dyn Clock>,
        tokens: TokenClient,
        endpoints: Arc<EndpointCell>,
    ) -> Self {
        Self {
            sessions: DashMap::new(),
            shared: None,
            limits,
            revoker: Revoker::new(tokens.clone(), endpoints.clone()),
            clock,
            tokens,
            endpoints,
        }
    }

    pub fn len(&self) -> usize {
        self.sessions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }

    pub fn now(&self) -> Instant {
        self.clock.now()
    }
}

fn digest(sid: &str) -> [u8; 32] {
    Sha256::digest(sid.as_bytes()).into()
}
