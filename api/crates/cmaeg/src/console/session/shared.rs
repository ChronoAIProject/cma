//! Durable operations never consult the compatibility map for authority.
use super::codec::{self, Data};
use super::*;

pub(super) struct SharedSessions {
    pub backend: Arc<dyn SessionBackend>,
    pub keys: Arc<MasterKeyRing>,
    pub requests: Arc<tokio::sync::Semaphore>,
    pub refreshes: tokio::sync::Semaphore,
}

impl SessionStore {
    pub fn with_backend(
        mut self,
        backend: Arc<dyn SessionBackend>,
        keys: Arc<MasterKeyRing>,
    ) -> Self {
        self.shared = Some(SharedSessions {
            backend,
            keys,
            requests: Arc::new(tokio::sync::Semaphore::new(64)),
            refreshes: tokio::sync::Semaphore::new(16),
        });
        self
    }

    /// Refuse excess work immediately; independent snapshots must not create
    /// an unbounded queue of durable-authority reads or refresh waiters.
    pub(crate) fn try_request(
        &self,
    ) -> Result<Option<tokio::sync::OwnedSemaphorePermit>, StoreError> {
        self.shared
            .as_ref()
            .map(|shared| {
                shared
                    .requests
                    .clone()
                    .try_acquire_owned()
                    .map_err(|_| StoreError::Unavailable)
            })
            .transpose()
    }

    pub async fn create_session(
        &self,
        subject: String,
        tokens: TokenSet,
    ) -> Result<(Secret, Arc<SessionCell>), StoreError> {
        let Some(shared) = &self.shared else {
            return self.create(subject, tokens).ok_or(StoreError::Unavailable);
        };
        let sid = codec::random()?;
        let key = digest(&sid);
        let id = codec::id(&key);
        let mut csrf = [0; 32];
        SystemRandom::new()
            .fill(&mut csrf)
            .map_err(|_| StoreError::Unavailable)?;
        let data = Data {
            subject: subject.clone(),
            selected: OwnerSelection::Personal,
            csrf,
            flash: None,
            access: tokens.access.as_str().into(),
            refresh: tokens.refresh.as_str().into(),
            revision: codec::random()?,
            token_revision: codec::random()?,
            refreshing: false,
        };
        // The backend bounds creation to three seconds. Reserve that entire
        // interval so quota/network latency cannot extend a provider deadline.
        let lifetime = tokens.expires_in.saturating_sub(Duration::from_secs(3));
        if lifetime <= EXPIRY_MARGIN {
            self.revoke_unused(tokens.refresh);
            return Err(StoreError::Unavailable);
        }
        let payload = shared.seal(&id, &data)?;
        let created_before = self.now();
        let mut stored = match shared
            .backend
            .create(
                &id,
                &codec::id(&digest(&subject)),
                payload,
                millis(lifetime),
                &self.limits,
            )
            .await
        {
            Ok(stored) => stored,
            Err(error) => {
                self.revoke_unused(tokens.refresh);
                return Err(error);
            }
        };
        for victim in std::mem::take(&mut stored.evicted) {
            match shared.open_data(&victim.id, &victim.payload) {
                Ok(data) => self.revoke_unused(Secret::new(data.refresh)),
                Err(_) => {
                    tracing::warn!(stage = "eviction", "sealed console victim cannot be opened")
                }
            }
        }
        let cell = self.cell(key, shared.open(&id, stored)?, created_before);
        tracing::info!(outcome = "created", mode = "shared", "console session");
        Ok((Secret::new(sid), cell))
    }

    pub async fn find_session(&self, sid: &str) -> Result<Option<Arc<SessionCell>>, StoreError> {
        let Some(shared) = &self.shared else {
            return Ok(self.find(sid));
        };
        if sid.len() != 43 || URL_SAFE_NO_PAD.decode(sid).is_err() {
            return Ok(None);
        }
        let key = digest(sid);
        let id = codec::id(&key);
        let observed_before = self.now();
        shared
            .backend
            .load(&id, true)
            .await?
            .map(|stored| {
                shared
                    .open(&id, stored)
                    .map(|snapshot| self.cell(key, snapshot, observed_before))
            })
            .transpose()
    }

    fn cell(&self, key: [u8; 32], snapshot: Snapshot, now: Instant) -> Arc<SessionCell> {
        Arc::new(SessionCell {
            key,
            meta: Mutex::new(SessionMeta {
                subject: snapshot.data.subject.clone(),
                selected: snapshot.data.selected.clone(),
                created_at: now,
                last_seen: now,
                csrf_key: snapshot.data.csrf,
                flash: snapshot.data.flash.clone(),
                orgs: None,
            }),
            auth: tokio::sync::Mutex::new(SessionAuth {
                access: Secret::new(snapshot.data.access.clone()),
                refresh: Secret::new(snapshot.data.refresh.clone()),
                access_expires_at: now
                    + Duration::from_millis(
                        snapshot
                            .stored
                            .access_expires_ms
                            .saturating_sub(snapshot.stored.now_ms)
                            .max(0) as u64,
                    ),
                refresh_retry_at: None,
            }),
            ended: AtomicBool::new(false),
            snapshot: Mutex::new(Some(snapshot)),
        })
    }

    /// Re-read after acquiring the local lock: another replica may have rotated
    /// tokens, changed the selection or revoked this request's old snapshot.
    pub async fn synchronize(
        &self,
        cell: &Arc<SessionCell>,
        auth: &mut SessionAuth,
    ) -> Result<bool, StoreError> {
        let Some(shared) = &self.shared else {
            return Ok(!self.end_if_expired(cell));
        };
        let id = codec::id(&cell.key);
        let observed_before = self.now();
        match shared.backend.load(&id, false).await? {
            Some(stored) => {
                cell.apply(shared.open(&id, stored)?, auth, observed_before);
                Ok(true)
            }
            None => {
                cell.ended.store(true, Ordering::Release);
                Ok(false)
            }
        }
    }

    /// The final admission point follows every awaited identity/refresh check.
    /// A changed snapshot requires a new request and a fresh owner check.
    pub async fn admit(&self, cell: &Arc<SessionCell>) -> Result<bool, StoreError> {
        let Some(shared) = &self.shared else {
            return Ok(!self.end_if_expired(cell));
        };
        let expected = cell.snapshot()?;
        if expected.data.refreshing {
            return Err(StoreError::Unavailable);
        }
        if shared
            .backend
            .admit(&codec::id(&cell.key), &expected.stored.payload)
            .await?
        {
            return Ok(true);
        }
        match shared.backend.load(&codec::id(&cell.key), false).await? {
            Some(_) => Err(StoreError::Unavailable),
            None => {
                cell.ended.store(true, Ordering::Release);
                Ok(false)
            }
        }
    }

    pub async fn end_session(
        &self,
        cell: &Arc<SessionCell>,
        reason: EndReason,
    ) -> Result<(), StoreError> {
        let Some(shared) = &self.shared else {
            self.end(cell, reason);
            return Ok(());
        };
        let id = codec::id(&cell.key);
        let deleted = shared.backend.revoke(&id).await?;
        cell.ended.store(true, Ordering::Release);
        if let Some(deleted) = deleted {
            match shared.open_data(&id, &deleted) {
                Ok(data) => self.revoke_unused(Secret::new(data.refresh)),
                Err(_) => tracing::warn!(
                    stage = "logout",
                    "sealed console cleanup token cannot be opened"
                ),
            }
        }
        tracing::info!(
            outcome = "ended",
            reason = reason.label(),
            mode = "shared",
            "console session"
        );
        Ok(())
    }

    pub async fn selection(
        &self,
        cell: &Arc<SessionCell>,
        selected: OwnerSelection,
        flash: Option<Flash>,
    ) -> Result<(), StoreError> {
        self.change_meta(cell, Some(selected), Some(flash)).await
    }

    pub async fn set_flash(
        &self,
        cell: &Arc<SessionCell>,
        flash: Option<Flash>,
    ) -> Result<(), StoreError> {
        self.change_meta(cell, None, Some(flash)).await
    }

    async fn change_meta(
        &self,
        cell: &Arc<SessionCell>,
        selected: Option<OwnerSelection>,
        flash: Option<Option<Flash>>,
    ) -> Result<(), StoreError> {
        if let Some(shared) = &self.shared {
            let mut snapshot = cell.snapshot()?;
            if snapshot.data.refreshing {
                return Err(StoreError::Unavailable);
            }
            if let Some(selected) = &selected {
                snapshot.data.selected = selected.clone();
            }
            if let Some(flash) = &flash {
                snapshot.data.flash = flash.clone();
            }
            snapshot.data.revision = codec::random()?;
            let id = codec::id(&cell.key);
            let next = shared.seal(&id, &snapshot.data)?;
            let stored = shared
                .backend
                .compare_exchange(&id, &snapshot.stored.payload, next, None)
                .await?
                .ok_or(StoreError::Unavailable)?;
            snapshot.stored = stored;
            *cell
                .snapshot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(snapshot);
        }
        let mut meta = cell.meta();
        if let Some(selected) = selected {
            meta.selected = selected;
        }
        if let Some(flash) = flash {
            meta.flash = flash;
        }
        Ok(())
    }
}

pub(super) fn millis(duration: Duration) -> i64 {
    i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
}
