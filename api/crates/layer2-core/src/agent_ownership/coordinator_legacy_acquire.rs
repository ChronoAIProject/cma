use super::*;

impl OwnershipCoordinator {
    pub(super) async fn acquire_legacy(
        &self,
        id: &AgentId,
    ) -> Result<Option<Arc<Authority>>, OwnershipError> {
        if !self.is_ready() {
            return Err(OwnershipError::Stale);
        }
        // Serialize local acquisitions, including uncertain retries, without
        // holding the owned-map lock across IO needed by renewal and drain.
        // Only the store CAS grants authority between processes.
        let _acquisition = self.acquisition.lock().await;
        if !self.is_ready() {
            return Err(OwnershipError::Stale);
        }
        let cancelled = match self
            .owned
            .lock()
            .unwrap()
            .get(id)
            .and_then(|slot| slot.authority.clone())
        {
            Some(authority) if authority.is_current() => return Ok(Some(authority)),
            Some(authority) if authority.receiver_cancelled() => Some(authority),
            Some(_) => return Err(OwnershipError::Stale),
            None => None,
        };
        // A skipped heartbeat may begin an outage that ends in failure: adopt
        // nothing new until a heartbeat succeeds. Existing authority stays.
        if self.skipped_heartbeats.load(Ordering::Acquire) > 0 {
            return Ok(None);
        }
        let started = Instant::now();
        let membership = bounded(self.store.membership()).await?;
        let Some(lease) = bounded(self.store.acquire(id, &self.instance_id)).await? else {
            return Ok(None);
        };
        if lease.token.instance_id != self.instance_id || lease.token.agent_id != *id {
            return Err(OwnershipError::Invalid(
                "acquisition returned another owner's token".into(),
            ));
        }
        // An uncertain late renewal may leave the cancelled exact token live.
        // A new caller can receive authority only for a strictly newer epoch.
        if cancelled
            .as_ref()
            .is_some_and(|old| lease.token.epoch <= old.token.epoch)
        {
            return Ok(None);
        }
        let until = deadline(started, membership.now_ms, lease.expires_at_ms)?;
        let authority = Authority::with_store(lease.token, until, self.store.clone());
        let mut owned = self.owned.lock().unwrap();
        if !self.is_ready() {
            authority.revoke();
            return Err(OwnershipError::Stale);
        }
        owned.insert(
            id.clone(),
            slot::Slot {
                authority: Some(authority.clone()),
                acquisition: None,
            },
        );
        Ok(Some(authority))
    }
}
