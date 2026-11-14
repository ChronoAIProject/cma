//! Process-local Console session operations.
use super::*;

impl SessionStore {
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
