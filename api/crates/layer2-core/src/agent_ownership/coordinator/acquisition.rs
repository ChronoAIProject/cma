//! Keep committed or uncertain acquisition custody across cancellation and drain.
use super::*;

impl OwnershipCoordinator {
    pub async fn acquire(&self, id: &AgentId) -> Result<Option<Arc<Authority>>, OwnershipError> {
        if !self.can_acquire() {
            return Err(OwnershipError::Stale);
        }
        let _acquisition = self.acquisition.lock().await;
        if !self.can_acquire() {
            return Err(OwnershipError::Stale);
        }
        let cached = {
            let owned = self.owned.lock().await;
            if !self.can_acquire() {
                return Err(OwnershipError::Stale);
            }
            owned.get(id).cloned()
        };
        let cancelled = match cached {
            Some(authority) if authority.is_current() => return Ok(Some(authority)),
            Some(authority) if authority.receiver_cancelled() => Some(authority),
            Some(_) => return Err(OwnershipError::Stale),
            None => None,
        };
        let started = Instant::now();
        let membership = bounded(self.store.membership()).await?;
        membership.require_compatible()?;
        if !self.can_acquire() {
            return Err(OwnershipError::Stale);
        }
        // Record before polling the write. Both an error and dropping the future
        // can hide a committed lease. Only a confirmed result discharges this.
        let previously_unconfirmed = {
            let mut pending = self.pending_acquisitions.lock().unwrap();
            let previous = pending.contains(id);
            if !previous && pending.len() >= self.max_pending_acquisitions {
                return Err(OwnershipError::Unavailable(
                    "unconfirmed ownership acquisition capacity is full".into(),
                ));
            }
            pending.insert(id.clone());
            previous
        };
        let Some(lease) = bounded(self.store.acquire(id, &self.instance_id)).await? else {
            // A later refusal cannot prove an earlier response-lost claim was
            // absent. Keep that custody until a confirmed exact token accounts
            // for it; the existing store CAS remains the only acquisition path.
            if !previously_unconfirmed {
                self.pending_acquisitions.lock().unwrap().remove(id);
            }
            return Ok(None);
        };
        if lease.token.instance_id != self.instance_id || lease.token.agent_id != *id {
            return Err(OwnershipError::Invalid(
                "acquisition returned another owner's token".into(),
            ));
        }
        if cancelled
            .as_ref()
            .is_some_and(|old| lease.token.epoch <= old.token.epoch)
        {
            self.pending_acquisitions.lock().unwrap().remove(id);
            return Ok(None);
        }
        let until = deadline(started, membership.now_ms, lease.expires_at_ms);
        let authority = Authority::with_store(
            lease.token,
            until.as_ref().copied().unwrap_or_else(|_| Instant::now()),
            self.store.clone(),
        );
        let mut owned = self.owned.lock().await;
        let admitted = until.is_ok() && self.can_acquire();
        if !admitted {
            authority.retiring.store(true, Ordering::Release);
            authority.revoke();
        }
        // Even a late or expired confirmed token remains visible to exact-token
        // cleanup. It was never returned to a caller as execution authority.
        owned.insert(id.clone(), authority.clone());
        self.pending_acquisitions.lock().unwrap().remove(id);
        drop(owned);
        if admitted {
            Ok(Some(authority))
        } else {
            if !self.is_draining() || !self.member_is_live() {
                self.fail().await;
            }
            Err(OwnershipError::Stale)
        }
    }
}
