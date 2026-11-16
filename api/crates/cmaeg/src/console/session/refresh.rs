//! An external rotating-token attempt is permanently consumed before sending.
//! A timeout or dead process must never reopen the old token for another send.
use super::*;
use super::{codec, shared::millis};

impl SessionStore {
    pub(super) async fn refresh_shared(
        &self,
        cell: &Arc<SessionCell>,
        auth: &mut SessionAuth,
    ) -> Refresh {
        match self.refresh_durable(cell, auth).await {
            Ok(result) => result,
            Err(_) => {
                tracing::warn!(
                    outcome = "unavailable",
                    stage = "refresh",
                    "console session store"
                );
                Refresh::Transient
            }
        }
    }

    async fn refresh_durable(
        &self,
        cell: &Arc<SessionCell>,
        auth: &mut SessionAuth,
    ) -> Result<Refresh, StoreError> {
        let shared = self.shared.as_ref().ok_or(StoreError::Invalid)?;
        let mut permit = None;
        let id = codec::id(&cell.key);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
        loop {
            if !self.synchronize(cell, auth).await? {
                return Ok(Refresh::Ended);
            }
            let mut snapshot = cell.snapshot()?;
            if !snapshot.data.refreshing && auth.access_usable(self.now()) {
                return Ok(Refresh::NotDue);
            }
            if permit.is_none() {
                permit = Some(
                    shared
                        .refreshes
                        .try_acquire()
                        .map_err(|_| StoreError::Unavailable)?,
                );
            }
            if snapshot.data.refreshing {
                if tokio::time::Instant::now() >= deadline {
                    // The attempt's outcome is unknown. Deleting the authority
                    // is safe; sending its possibly consumed token again is not.
                    self.end_session(cell, EndReason::Refused).await?;
                    return Ok(Refresh::Ended);
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
            let Some(endpoints) = self.endpoints.get() else {
                return Ok(Refresh::Transient);
            };
            snapshot.data.refreshing = true;
            snapshot.data.revision = codec::random()?;
            let claimed = shared.seal(&id, &snapshot.data)?;
            let claimed_before = self.now();
            let Some(stored) = shared
                .backend
                .compare_exchange(&id, &snapshot.stored.payload, claimed.clone(), None)
                .await?
            else {
                if tokio::time::Instant::now() >= deadline {
                    return Err(StoreError::Unavailable);
                }
                continue;
            };
            // Only an acknowledged exact claim permits the provider call.
            // An uncertain claim returns an error and remains non-replayable.
            let claimed_at = stored.now_ms;
            cell.apply(shared.open(&id, stored)?, auth, claimed_before);
            let tokens = match self.tokens.refresh(&endpoints.token, &auth.refresh).await {
                Ok(tokens) => tokens,
                Err(_) => {
                    self.end_session(cell, EndReason::Refused).await?;
                    return Ok(Refresh::Ended);
                }
            };
            snapshot.data.access = tokens.access.as_str().into();
            snapshot.data.refresh = tokens.refresh.as_str().into();
            snapshot.data.refreshing = false;
            snapshot.data.token_revision = snapshot.data.revision.clone();
            snapshot.data.revision = codec::random()?;
            let next = shared.seal(&id, &snapshot.data)?;
            let completed_before = self.now();
            let completion = shared
                .backend
                .compare_exchange(
                    &id,
                    &claimed,
                    next.clone(),
                    Some(claimed_at.saturating_add(millis(tokens.expires_in))),
                )
                .await;
            match completion {
                Ok(Some(stored)) => {
                    cell.apply(shared.open(&id, stored)?, auth, completed_before);
                    return Ok(Refresh::Rotated);
                }
                Ok(None) | Err(_) => {
                    let read_before = self.now();
                    match shared.backend.load(&id, false).await? {
                        Some(stored) => {
                            let loaded = shared.open(&id, stored)?;
                            if !loaded.data.refreshing
                                && loaded.data.token_revision == snapshot.data.token_revision
                                && loaded.data.access == snapshot.data.access
                                && loaded.data.refresh == snapshot.data.refresh
                            {
                                // Cosmetic/owner revisions may have followed publication.
                                // The caller rechecks this loaded owner and exact snapshot.
                                cell.apply(loaded, auth, read_before);
                                return Ok(Refresh::Rotated);
                            }
                            // Unknown publication or a newer token generation does not
                            // establish that our successor was never published.
                            return Err(StoreError::Unavailable);
                        }
                        None => {
                            // A missing/expired observation alone is not cleanup authority.
                            // Prove durable deletion/absence before revoking this result.
                            self.end_session(cell, EndReason::Refused).await?;
                            self.revoke_unused(tokens.refresh);
                            return Ok(Refresh::Ended);
                        }
                    }
                }
            }
        }
    }
}
