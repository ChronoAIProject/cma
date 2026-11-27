use super::*;
use crate::agent_ownership::lease_operation::unresolved;

impl OwnershipCoordinator {
    pub async fn poll_acquisitions(&self) -> Result<(), OwnershipError> {
        if self.is_draining() {
            return Ok(());
        }
        let Ok(_admission) = self.acquisition.try_lock() else {
            return Ok(());
        };
        let until = Instant::now() + Duration::from_secs(2);
        match self.finish_acquisitions(until).await {
            Ok(()) | Err(OwnershipError::Unavailable(_)) => {}
            Err(error) => return Err(error),
        }
        if !self.contains_failures() {
            // These tokens were never published and have no attached work.
            let unpublished = self
                .owned()
                .await
                .into_iter()
                .filter(|authority| authority.is_unpublished());
            let releases =
                stream::iter(unpublished).for_each_concurrent(4, |authority| async move {
                    let _ = self
                        .handoff(&authority, authority.revoke_transports())
                        .await;
                });
            let _ = tokio::time::timeout_at(until, releases).await;
        }
        Ok(())
    }
    /// Close publication before waiting for admitted operations. The caller
    /// takes its final authority snapshot only after joining lifecycle work.
    pub async fn prepare_drain(&self, until: Instant) -> Result<(), OwnershipError> {
        self.stop_admission();
        // Keep time for local work and known exact-token releases even when
        // an acquisition's store reply is still unresolved.
        let custody_until =
            until.min(Instant::now() + Duration::from_millis(3 * STORE_DEADLINE_MS));
        let _maintenance =
            retirement::within(custody_until, async { Ok(self.retry.lock().await) }).await?;
        let _admission =
            retirement::within(custody_until, async { Ok(self.acquisition.lock().await) }).await?;
        let acquisitions = self.finish_acquisitions(custody_until).await;
        let renewals = stream::iter(self.owned().await)
            .map(|authority| async move { authority.join_renewal_until(custody_until).await })
            .buffer_unordered(4)
            .collect::<Vec<_>>()
            .await;
        // Advertising failure cannot reopen local admission or erase custody.
        if self.is_registered() {
            let _ = retirement::stage(until, self.store.heartbeat(&self.instance_id, true)).await;
        }
        let mut error = None;
        for result in std::iter::once(acquisitions).chain(renewals) {
            if let Err(reason) = result {
                retain_error(&mut error, reason);
            }
        }
        error.map_or(Ok(()), Err)
    }

    async fn finish_acquisitions(&self, until: Instant) -> Result<(), OwnershipError> {
        let operations: Vec<_> = self
            .owned
            .lock()
            .unwrap()
            .iter()
            .filter_map(|(id, slot)| {
                slot.acquisition
                    .as_ref()
                    .map(|operation| (id.clone(), slot.authority.clone(), operation.clone()))
            })
            .collect();
        let results = stream::iter(operations)
            .map(|(id, authority, operation)| async move {
                let result = async {
                    let outcome = operation.join_until(until).await?;
                    self.reconcile_acquisition(&id, outcome, until).await
                }
                .await;
                (id, authority, operation, result)
            })
            .buffer_unordered(4)
            .collect::<Vec<_>>()
            .await;
        let mut error = None;
        for (id, authority, operation, result) in results {
            let outcome = match result {
                Ok(outcome) => outcome,
                Err(reason) => {
                    retain_error(&mut error, reason);
                    continue;
                }
            };
            let mut completion = match &authority {
                Some(authority) => {
                    match retirement::within(until, async { Ok(authority.cleanup.lock().await) })
                        .await
                    {
                        Ok(state) => Some(state),
                        Err(reason) => {
                            retain_error(&mut error, reason);
                            continue;
                        }
                    }
                }
                None => None,
            };
            if let Err(reason) = self.join_replaced_renewal(&id, &outcome, until).await {
                retain_error(&mut error, reason);
                continue;
            }
            if let Err(reason) =
                self.apply_acquisition(&id, &operation, outcome, completion.as_deref_mut(), false)
            {
                retain_error(&mut error, reason);
            }
        }
        if self
            .owned
            .lock()
            .unwrap()
            .values()
            .any(|slot| slot.acquisition.is_some())
        {
            error.get_or_insert_with(unresolved);
        }
        error.map_or(Ok(()), Err)
    }

    /// Observe termination of every introduced write task even after drain was
    /// incomplete. This never converts an uncertain durable result to success.
    pub async fn abort_operations_until(&self, until: Instant) -> Result<(), OwnershipError> {
        let operations: Vec<_> = self
            .owned
            .lock()
            .unwrap()
            .values()
            .filter_map(|slot| slot.acquisition.clone())
            .collect();
        let acquisitions = stream::iter(operations)
            .map(|operation| async move { operation.abort_until(until).await })
            .buffer_unordered(4)
            .collect::<Vec<_>>()
            .await;
        let renewals = stream::iter(self.owned().await)
            .map(|authority| async move { authority.abort_renewal_until(until).await })
            .buffer_unordered(4)
            .collect::<Vec<_>>()
            .await;
        let mut error = None;
        for result in acquisitions.into_iter().chain(renewals) {
            if let Err(reason) = result {
                retain_error(&mut error, reason);
            }
        }
        error.map_or(Ok(()), Err)
    }
}

fn retain_error(first: &mut Option<OwnershipError>, next: OwnershipError) {
    if first.is_none()
        || (matches!(first, Some(OwnershipError::Unavailable(_)))
            && !matches!(next, OwnershipError::Unavailable(_)))
    {
        *first = Some(next);
    }
}
