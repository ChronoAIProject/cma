use super::*;
use crate::agent_ownership::{
    MaintenanceStep, Membership, HEARTBEAT_INTERVAL_MS, PROTOCOL_VERSION,
};

impl OwnershipCoordinator {
    /// One heartbeat call at most. The episode and its final-attempt latch live
    /// in the coordinator, independently of the caller's returned wake hint.
    pub async fn maintain_step(&self) -> Result<MaintenanceStep, OwnershipError> {
        if !self.contains_failures() {
            return self
                .maintain_legacy()
                .await
                .map(|hints| MaintenanceStep::Confirmed {
                    hints,
                    next_at: Instant::now() + Duration::from_millis(HEARTBEAT_INTERVAL_MS),
                });
        }
        let result = self.retry_step().await;
        if result.is_err() {
            tracing::warn!(parent: None, stage = "ownership_maintenance",
                outcome = if Instant::now() >= *self.member_deadline.lock().unwrap() { "confirmed_lease_expired" } else { "fatal" },
                "Contained ownership maintenance requires shutdown");
            self.fail().await;
        }
        result
    }

    async fn retry_step(&self) -> Result<MaintenanceStep, OwnershipError> {
        let mut lifecycle = self.lifecycle.subscribe();
        let mut retry = tokio::select! {
            biased;
            _ = lifecycle.wait_for(|state| *state != super::Lifecycle::Serving) => return self.stopped_step(),
            retry = self.retry.lock() => retry,
        };
        if self.is_draining() {
            return Ok(MaintenanceStep::Draining);
        }
        if !self.is_ready() {
            return Err(OwnershipError::Stale);
        }
        let now = Instant::now();
        if now < retry.next_at {
            return Ok(MaintenanceStep::RetryAt(retry.next_at));
        }
        let old_until = *self.member_deadline.lock().unwrap();
        let episode = retry.begin(old_until)?;
        let Some(call_until) = episode.attempt_until(now) else {
            return Ok(MaintenanceStep::RetryAt(episode.confirmed_until));
        };
        // Keep adoption blocked through reply validation and owner normalization.
        self.skipped_heartbeats.store(1, Ordering::Release);
        for authority in self
            .owned
            .lock()
            .unwrap()
            .values()
            .filter_map(|slot| slot.authority.as_ref())
        {
            authority.mark_heartbeat(episode.generation());
        }
        let result = tokio::select! {
            biased;
            _ = lifecycle.wait_for(|state| *state != super::Lifecycle::Serving) => return self.stopped_step(),
            result = tokio::time::timeout_at(call_until, self.store.heartbeat(&self.instance_id, false)) => result,
        };
        let membership = match result {
            Ok(Ok(membership)) => membership,
            Err(_) | Ok(Err(OwnershipError::Unavailable(_))) => {
                if !self.is_ready() {
                    return Err(OwnershipError::Stale);
                }
                let next_at = episode.unavailable(Instant::now());
                if episode.failures == 1 {
                    tracing::info!(parent: None, stage = "member_heartbeat", outcome = "retry_started",
                        remaining_ms = old_until.saturating_duration_since(Instant::now()).as_millis() as u64,
                        "Membership heartbeat entered bounded retry");
                }
                tracing::debug!(parent: None,
                    stage = "member_heartbeat",
                    outcome = "retry",
                    failures = episode.failures,
                    elapsed_ms = episode.started.elapsed().as_millis() as u64,
                    remaining_ms = old_until
                        .saturating_duration_since(Instant::now())
                        .as_millis() as u64,
                    "Membership heartbeat retry remains within its confirmed lease"
                );
                retry.next_at = next_at;
                return Ok(MaintenanceStep::RetryAt(retry.next_at));
            }
            Ok(Err(error)) => return Err(error),
        };
        let proof = episode.proof();
        let until = self.validate_heartbeat(now, &membership)?;
        {
            let mut deadline = self.member_deadline.lock().unwrap();
            if *deadline <= Instant::now()
                || *deadline != old_until
                || self.is_draining()
                || *self.failed.borrow()
            {
                return Err(OwnershipError::Stale);
            }
            *deadline = until;
        }
        let owned = self.owned().await;
        let renewals = stream::iter(owned.clone())
            .map(|authority| async move {
                self.renew_owned_with_episode(&authority, now, membership.now_ms, proof)
                    .await
            })
            .buffer_unordered(4)
            .try_collect::<Vec<_>>();
        tokio::select! {
            biased;
            _ = lifecycle.wait_for(|state| *state != super::Lifecycle::Serving) => return self.stopped_step(),
            result = tokio::time::timeout_at(until, renewals) => {
                result.map_err(|_| OwnershipError::Stale)??;
            }
        }
        let hints = self.handoff_hints(owned, &membership)?;
        {
            let deadline = self.member_deadline.lock().unwrap();
            if *deadline <= Instant::now() || self.is_draining() || *self.failed.borrow() {
                return Err(OwnershipError::Stale);
            }
            self.skipped_heartbeats.store(0, Ordering::Release);
        }
        if proof.is_some() {
            tracing::info!(parent: None, stage = "member_heartbeat", outcome = "recovered",
                "Membership heartbeat and exact owners recovered");
        }
        retry.confirmed(now);
        Ok(MaintenanceStep::Confirmed {
            hints,
            next_at: retry.next_at,
        })
    }

    fn stopped_step(&self) -> Result<MaintenanceStep, OwnershipError> {
        if self.is_draining() && !*self.failed.borrow() {
            Ok(MaintenanceStep::Draining)
        } else {
            Err(OwnershipError::Stale)
        }
    }

    fn validate_heartbeat(
        &self,
        started: Instant,
        membership: &Membership,
    ) -> Result<Instant, OwnershipError> {
        membership.require_compatible()?;
        let mut matches = membership
            .members
            .iter()
            .filter(|member| member.instance_id == self.instance_id);
        let member = matches.next().ok_or(OwnershipError::Stale)?;
        if matches.next().is_some() || member.protocol != PROTOCOL_VERSION {
            return Err(OwnershipError::IncompatibleProtocol);
        }
        if member.draining {
            return Err(OwnershipError::Stale);
        }
        deadline(started, membership.now_ms, member.expires_at_ms)
    }

    fn handoff_hints(
        &self,
        owned: Vec<Arc<Authority>>,
        membership: &Membership,
    ) -> Result<Vec<Arc<Authority>>, OwnershipError> {
        let mut hints = Vec::new();
        for authority in owned {
            if !authority.is_current() {
                continue;
            }
            if candidates(
                &authority.token.agent_id,
                membership,
                Some(&self.instance_id),
            )?
            .first()
            .is_some_and(|member| member.instance_id != self.instance_id)
            {
                hints.push(authority);
                if hints.len() == 4 {
                    break;
                }
            }
        }
        Ok(hints)
    }
}
