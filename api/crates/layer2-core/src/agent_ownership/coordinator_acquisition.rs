use super::*;
use crate::agent_ownership::{
    authority::CleanupState,
    lease_operation::{LeaseWrite, WriteOutcome},
    Settlement, PROTOCOL_VERSION,
};

impl OwnershipCoordinator {
    pub(super) async fn acquire_contained(
        &self,
        id: &AgentId,
    ) -> Result<Option<Arc<Authority>>, OwnershipError> {
        if !self.is_ready() {
            return Err(OwnershipError::Stale);
        }
        if let Some(authority) = self.local(id).await {
            return Ok(Some(authority));
        }
        let Ok(_admission) = self.acquisition.try_lock() else {
            return Ok(None);
        };
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
        if let Some(old) = &old {
            if let Some(error) = old.settlement_error() {
                return Err(error);
            }
            if old.is_current() {
                return Ok(Some(old.clone()));
            }
            if !old.retirement_pending() {
                return Err(OwnershipError::Stale);
            }
        }
        let mut completion = match &old {
            Some(old) => match old.cleanup.try_lock() {
                Ok(state) => Some(state),
                Err(_) => return Ok(None),
            },
            None => None,
        };
        if let Some(operation) = existing {
            return self
                .join_acquisition(
                    id,
                    &operation,
                    completion.as_deref_mut(),
                    false,
                    Instant::now() + Duration::from_millis(STORE_DEADLINE_MS),
                )
                .await;
        }
        if self.skipped_heartbeats.load(Ordering::Acquire) > 0
            || completion
                .as_ref()
                .is_some_and(|state| !state.local_quiescent)
            || self.cleanup_pacing.deferred(id)
        {
            return Ok(None);
        }
        let Some(operation) = self.start_acquisition(id, old.as_ref()) else {
            return Ok(None);
        };
        self.join_acquisition(
            id,
            &operation,
            completion.as_deref_mut(),
            true,
            Instant::now() + Duration::from_millis(STORE_DEADLINE_MS),
        )
        .await
    }

    pub(super) fn start_acquisition(
        &self,
        id: &AgentId,
        old: Option<&Arc<Authority>>,
    ) -> Option<Arc<LeaseWrite>> {
        let mut owned = self.owned.lock().unwrap();
        let member_until = self.member_deadline.lock().unwrap();
        if Instant::now() >= *member_until
            || self.is_draining()
            || *self.failed.borrow()
            || self.skipped_heartbeats.load(Ordering::Acquire) > 0
        {
            return None;
        }
        if let Some(old) = old {
            if owned.get(id).is_none_or(|slot| !slot.owns(old)) {
                return None;
            }
        } else if !owned.contains_key(id) && owned.len() >= self.capacity {
            return None;
        }
        // Keep custody before the store task can issue a write.
        let slot = owned.entry(id.clone()).or_default();
        let operation =
            LeaseWrite::acquisition(self.store.clone(), id.clone(), self.instance_id.clone());
        slot.acquisition = Some(operation.clone());
        Some(operation)
    }

    pub(super) async fn join_acquisition(
        &self,
        id: &AgentId,
        operation: &Arc<LeaseWrite>,
        completion: Option<&mut CleanupState>,
        may_publish: bool,
        until: Instant,
    ) -> Result<Option<Arc<Authority>>, OwnershipError> {
        let outcome = match operation.join_until(until).await {
            Ok(outcome) => outcome,
            Err(OwnershipError::Unavailable(_)) if self.contains_failures() => return Ok(None),
            Err(error) => return Err(error),
        };
        // A waiter may run out of time without dropping the store task. Only
        // reconcile an uncertain result after that task has actually stopped.
        let outcome = self.reconcile_acquisition(id, outcome, until).await?;
        self.join_replaced_renewal(id, &outcome, until).await?;
        self.apply_acquisition(id, operation, outcome, completion, may_publish)
    }

    pub(super) async fn reconcile_acquisition(
        &self,
        id: &AgentId,
        outcome: WriteOutcome,
        until: Instant,
    ) -> Result<WriteOutcome, OwnershipError> {
        if matches!(outcome, WriteOutcome::Uncertain(_)) {
            let token = retirement::stage(
                until,
                self.store.reconcile_acquisition(id, &self.instance_id),
            )
            .await?;
            Ok(WriteOutcome::ReconciledAcquisition(token))
        } else {
            Ok(outcome)
        }
    }

    pub(super) async fn join_replaced_renewal(
        &self,
        id: &AgentId,
        outcome: &WriteOutcome,
        until: Instant,
    ) -> Result<(), OwnershipError> {
        let token = match outcome {
            WriteOutcome::Acquired {
                lease: Some(lease),
                started,
                now_ms,
            } => {
                self.acquired_deadline(id, lease, *started, *now_ms)?;
                &lease.token
            }
            WriteOutcome::ReconciledAcquisition(Some(token)) => {
                self.validate_acquisition_token(id, token)?;
                token
            }
            _ => return Ok(()),
        };
        let old = self
            .owned
            .lock()
            .unwrap()
            .get(id)
            .and_then(|slot| slot.authority.clone());
        if let Some(old) = old.filter(|old| token.epoch > old.token.epoch) {
            // The validated higher epoch fences any old write. Observe the
            // actual task's termination before removing its custody slot.
            old.abort_renewal_until(until).await?;
        }
        Ok(())
    }

    fn acquired_deadline(
        &self,
        id: &AgentId,
        lease: &OwnershipLease,
        started: Instant,
        now_ms: i64,
    ) -> Result<Instant, OwnershipError> {
        self.validate_acquisition_token(id, &lease.token)?;
        let duration = lease
            .expires_at_ms
            .checked_sub(now_ms)
            .filter(|duration| *duration > 0)
            .ok_or_else(|| {
                OwnershipError::Invalid("acquisition returned an invalid duration".into())
            })?;
        started
            .checked_add(Duration::from_millis(duration as u64))
            .ok_or_else(|| OwnershipError::Invalid("acquisition duration overflow".into()))
    }

    fn validate_acquisition_token(
        &self,
        id: &AgentId,
        token: &OwnershipToken,
    ) -> Result<(), OwnershipError> {
        if token.agent_id != *id
            || token.instance_id != self.instance_id
            || token.protocol != PROTOCOL_VERSION
            || token.epoch == 0
        {
            Err(OwnershipError::Invalid(
                "acquisition returned an invalid token".into(),
            ))
        } else {
            Ok(())
        }
    }

    pub(super) fn apply_acquisition(
        &self,
        id: &AgentId,
        operation: &Arc<LeaseWrite>,
        outcome: WriteOutcome,
        completion: Option<&mut CleanupState>,
        may_publish: bool,
    ) -> Result<Option<Arc<Authority>>, OwnershipError> {
        let mut owned = self.owned.lock().unwrap();
        let slot = owned
            .get_mut(id)
            .filter(|slot| {
                slot.acquisition
                    .as_ref()
                    .is_some_and(|current| Arc::ptr_eq(current, operation))
            })
            .ok_or(OwnershipError::Stale)?;
        let acquisition = match outcome {
            WriteOutcome::Acquired {
                started,
                now_ms,
                lease,
            } => lease
                .map(|lease| {
                    let until = self.acquired_deadline(id, &lease, started, now_ms)?;
                    Ok((lease.token, Some(until)))
                })
                .transpose()?,
            WriteOutcome::ReconciledAcquisition(token) => token.map(|token| (token, None)),
            WriteOutcome::NotIssued(error) => {
                slot.acquisition = None;
                if slot.authority.is_none() {
                    owned.remove(id);
                }
                return match error {
                    OwnershipError::Unavailable(_) if self.contains_failures() => Ok(None),
                    error => Err(error),
                };
            }
            WriteOutcome::Uncertain(error) => return Err(error),
            WriteOutcome::Renewed(_) | WriteOutcome::RenewalRefused(_) => {
                return Err(OwnershipError::Invalid(
                    "acquisition returned another operation".into(),
                ))
            }
        };
        let Some((token, until)) = acquisition else {
            slot.acquisition = None;
            if slot.authority.is_none()
                || completion
                    .as_ref()
                    .is_some_and(|state| state.settlement.is_some())
            {
                owned.remove(id);
            }
            return Ok(None);
        };
        if slot
            .authority
            .as_ref()
            .is_some_and(|old| token.epoch <= old.token.epoch)
        {
            slot.acquisition = None;
            return Ok(None);
        }
        if completion.as_ref().is_some_and(|state| matches!(state.settlement, Some(Settlement::Superseded { newer_epoch }) if newer_epoch > token.epoch)) {
            owned.remove(id);
            return Ok(None);
        }
        let member_until = self.member_deadline.lock().unwrap();
        let publish_until = until.filter(|until| {
            may_publish
                && Instant::now() < *until
                && Instant::now() < *member_until
                && !self.is_draining()
                && !*self.failed.borrow()
                && self.skipped_heartbeats.load(Ordering::Acquire) == 0
        });
        let epoch = token.epoch;
        let authority = if let Some(until) = publish_until {
            Authority::with_store_policy(token, until, self.store.clone(), self.policy)
        } else {
            Authority::unpublished(token, self.store.clone(), self.policy)
        };
        if let Some(state) = completion {
            if state.settlement.is_none() {
                state.settlement = Some(Settlement::Superseded { newer_epoch: epoch });
            }
            self.cleanup_pacing.receiver_cleanup_completed(id);
        }
        slot.authority = Some(authority.clone());
        slot.acquisition = None;
        Ok(publish_until.map(|_| authority))
    }
}

#[cfg(test)]
#[path = "coordinator_acquisition_tests.rs"]
mod tests;
