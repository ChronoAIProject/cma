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
        let (old, existing) = self
            .owned
            .lock()
            .unwrap()
            .get(id)
            .map(|slot| (slot.authority.clone(), slot.acquisition.clone()))
            .unwrap_or_default();
        let cancelled = match old {
            Some(authority) if authority.is_current() => return Ok(Some(authority)),
            Some(authority) if authority.receiver_cancelled() => Some(authority),
            Some(_) => return Err(OwnershipError::Stale),
            None => None,
        };
        let until = Instant::now() + Duration::from_millis(STORE_DEADLINE_MS);
        let mut completion = match &cancelled {
            Some(old) => match old.cleanup.try_lock() {
                Ok(state) => Some(state),
                Err(_) => return Ok(None),
            },
            None => None,
        };
        let (operation, may_publish) = match existing {
            Some(operation) => (operation, false),
            None => match self.start_acquisition(id, cancelled.as_ref()) {
                Some(operation) => (operation, true),
                None => return Ok(None),
            },
        };
        let result = self
            .join_acquisition(
                id,
                &operation,
                completion.as_deref_mut(),
                may_publish,
                until,
            )
            .await?;
        if self.is_ready() {
            Ok(result)
        } else {
            Err(OwnershipError::Stale)
        }
    }
}
