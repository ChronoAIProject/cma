//! Console sessions, held only in memory. A pod restart signs everyone out;
//! the next GET shows the sign-in page.
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnerSelection {
    Personal,
    Organization(String),
}

/// A one-time notice shown on the next page.
#[derive(Debug, Clone, PartialEq, Eq)]
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

    /// Start a session for `subject` and return its cookie value, which is
    /// the only place the session id exists; the store keeps its SHA-256.
    /// The subject's oldest sessions beyond its cap end first, then the least
    /// recently seen sessions beyond the global cap.
    pub fn create(&self, subject: String, tokens: TokenSet) -> Option<(Secret, Arc<SessionCell>)> {
        let mut id = [0_u8; SESSION_ID_BYTES];
        let mut csrf_key = [0_u8; 32];
        let random = SystemRandom::new();
        if random.fill(&mut id).is_err() || random.fill(&mut csrf_key).is_err() {
            tracing::error!("secure randomness unavailable; console session not created");
            return None;
        }
        let sid = URL_SAFE_NO_PAD.encode(id);
        let key = digest(&sid);
        let now = self.clock.now();
        let subject_hash = subject_hash_prefix(&subject);
        let cell = Arc::new(SessionCell {
            key,
            meta: Mutex::new(SessionMeta {
                subject,
                selected: OwnerSelection::Personal,
                created_at: now,
                last_seen: now,
                csrf_key,
                flash: None,
                orgs: None,
            }),
            auth: tokio::sync::Mutex::new(SessionAuth {
                access: tokens.access,
                refresh: tokens.refresh,
                access_expires_at: now + tokens.expires_in,
                refresh_retry_at: None,
            }),
            ended: AtomicBool::new(false),
        });
        self.sessions.insert(key, cell.clone());
        self.enforce_subject_cap(&cell);
        self.enforce_global_cap();
        self.record_size();
        tracing::info!(subject_hash_prefix = %subject_hash, outcome = "created", "console session");
        Some((Secret::new(sid), cell))
    }

    /// The live session named by a cookie value, after its idle and absolute
    /// checks; seeing it counts as activity.
    pub fn find(&self, sid: &str) -> Option<Arc<SessionCell>> {
        if sid.len() != 43 || URL_SAFE_NO_PAD.decode(sid).is_err() {
            return None;
        }
        // The guard is a temporary of this statement: dropped before any use.
        let cell = self
            .sessions
            .get(&digest(sid))
            .map(|entry| entry.value().clone())?;
        let now = self.clock.now();
        let expired = {
            let mut meta = cell.meta();
            let expired = self.expiry(&meta, now);
            if expired.is_none() {
                meta.last_seen = now;
            }
            expired
        };
        match expired {
            Some(reason) => {
                self.end(&cell, reason);
                None
            }
            None => Some(cell),
        }
    }

    /// True when `cell` has ended, ending it first if it is idle or past its
    /// absolute lifetime. Authority runs it under the `auth` lock, so a
    /// request that waited behind another never serves an expired session.
    pub fn end_if_expired(&self, cell: &Arc<SessionCell>) -> bool {
        if cell.is_ended() {
            return true;
        }
        let expired = self.expiry(&cell.meta(), self.clock.now());
        match expired {
            Some(reason) => {
                self.end(cell, reason);
                true
            }
            None => false,
        }
    }

    /// Remove `cell` and revoke its refresh token, best effort. Idempotent.
    pub fn end(&self, cell: &Arc<SessionCell>, reason: EndReason) {
        if cell.ended.swap(true, Ordering::AcqRel) {
            return;
        }
        self.sessions.remove(&cell.key);
        if reason.eviction() {
            metrics::counter!(names::CONSOLE_SESSION_EVICTIONS, names::REASON => reason.label())
                .increment(1);
        }
        let subject_hash = subject_hash_prefix(&cell.meta().subject);
        tracing::info!(subject_hash_prefix = %subject_hash, outcome = "ended", reason = reason.label(), "console session");
        self.revoker.revoke(cell.clone());
        self.record_size();
    }

    /// Revoke a refresh token that never became a session, best effort.
    pub(crate) fn revoke_unused(&self, refresh: Secret) {
        self.revoker.revoke_token(refresh);
    }

    /// End every idle or expired session; the console runs it every minute.
    pub fn sweep(&self) {
        let now = self.clock.now();
        let expired: Vec<(Arc<SessionCell>, EndReason)> = self
            .sessions
            .iter()
            .filter_map(|entry| {
                let cell = entry.value().clone();
                let reason = self.expiry(&cell.meta(), now)?;
                Some((cell, reason))
            })
            .collect();
        for (cell, reason) in expired {
            self.end(&cell, reason);
        }
    }

    /// Refresh the access token only when it expires within 30 s (Amendment
    /// R6 §R6.4); the caller holds the `auth` lock, so concurrent requests of
    /// one session make one refresh. The rotated refresh token replaces the
    /// old one before the lock is released.
    pub async fn refresh(&self, cell: &Arc<SessionCell>, auth: &mut SessionAuth) -> Refresh {
        let now = self.clock.now();
        if auth.access_usable(now) {
            return Refresh::NotDue;
        }
        // A NyxID outage costs at most one token request per session per
        // 10 s.
        if auth.refresh_retry_at.is_some_and(|at| now < at) {
            return Refresh::Transient;
        }
        let Some(endpoints) = self.endpoints.get() else {
            return Refresh::Transient;
        };
        match self.tokens.refresh(&endpoints.token, &auth.refresh).await {
            Ok(tokens) => {
                auth.rotate(tokens, now);
                Refresh::Rotated
            }
            Err(TokenFailure::Definitive) => {
                self.end(cell, EndReason::Refused);
                Refresh::Ended
            }
            Err(TokenFailure::Transient) => {
                // A rotation hidden by a lost response makes the next refresh
                // reuse the old token; NyxID then refuses it with
                // invalid_grant and the session ends, failing closed.
                auth.refresh_retry_at = Some(now + REFRESH_RETRY);
                Refresh::Transient
            }
        }
    }

    fn expiry(&self, meta: &SessionMeta, now: Instant) -> Option<EndReason> {
        if now.duration_since(meta.created_at) >= self.limits.ttl {
            Some(EndReason::Absolute)
        } else if now.duration_since(meta.last_seen) >= self.limits.idle {
            Some(EndReason::Idle)
        } else {
            None
        }
    }

    fn enforce_subject_cap(&self, created: &Arc<SessionCell>) {
        let subject = created.meta().subject.clone();
        let mut own: Vec<(Instant, Arc<SessionCell>)> = self
            .sessions
            .iter()
            .filter_map(|entry| {
                let cell = entry.value().clone();
                let meta = cell.meta();
                (meta.subject == subject).then(|| (meta.created_at, cell.clone()))
            })
            .collect();
        if own.len() <= self.limits.per_subject {
            return;
        }
        own.sort_by_key(|(created_at, _)| *created_at);
        let excess = own.len() - self.limits.per_subject;
        for (_, cell) in own
            .into_iter()
            .filter(|(_, cell)| !Arc::ptr_eq(cell, created))
            .take(excess)
        {
            self.end(&cell, EndReason::SubjectCap);
        }
    }

    fn enforce_global_cap(&self) {
        let excess = self.sessions.len().saturating_sub(self.limits.max);
        if excess == 0 {
            return;
        }
        let mut all: Vec<(Instant, Arc<SessionCell>)> = self
            .sessions
            .iter()
            .map(|entry| {
                let cell = entry.value().clone();
                let last_seen = cell.meta().last_seen;
                (last_seen, cell)
            })
            .collect();
        all.sort_by_key(|(last_seen, _)| *last_seen);
        for (_, cell) in all.into_iter().take(excess) {
            self.end(&cell, EndReason::GlobalCap);
        }
    }

    fn record_size(&self) {
        metrics::gauge!(names::CONSOLE_SESSIONS).set(self.sessions.len() as f64);
    }
}

fn digest(sid: &str) -> [u8; 32] {
    Sha256::digest(sid.as_bytes()).into()
}
