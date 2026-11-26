use super::super::{RenewalBatch, HEARTBEAT_INTERVAL_MS, MAX_RENEWAL_BATCH};
use super::heartbeat::RenewalHeartbeat;
use super::renewal::renewal_obsolete;
use super::*;
use futures_util::stream::FuturesUnordered;

enum ChunkCompletion {
    Finished,
    RenewIndividually,
}

impl OwnershipCoordinator {
    pub(super) async fn renew_in_batches(
        &self,
        owned: &[Arc<Authority>],
        heartbeat: &mut RenewalHeartbeat,
    ) -> Result<bool, OwnershipError> {
        let mut ordered: Vec<_> = owned
            .iter()
            .filter(|owner| !renewal_obsolete(owner))
            .collect();
        ordered.sort_by_key(|owner| owner.established_deadline());
        for chunk in ordered.chunks(MAX_RENEWAL_BATCH) {
            if !self.refresh_due_heartbeat(heartbeat).await? {
                return Ok(false);
            }
            let mut active = Vec::with_capacity(chunk.len());
            for &owner in chunk {
                if let Some(until) = self.live_renewal_deadline(owner, "owner_dispatch_deadline")? {
                    active.push((owner, until));
                }
            }
            if active.is_empty() {
                continue;
            }
            #[cfg(feature = "qualification-hooks")]
            if let Some(hook) = self.qualification_renewal.get() {
                // Keep unrelated owners on the ordinary batch client. Run the selected
                // renewal inside the same coordinator deadline and containment path.
                let split = active.iter().position(|(owner, _)| hook.selected(owner));
                if let Some(index) = split {
                    let (owner, _) = active.remove(index);
                    let selected = self.renew_one_before_member_deadline(owner, heartbeat);
                    if active.is_empty() {
                        selected.await?;
                        continue;
                    }
                    let owners: Vec<_> = active.iter().map(|(owner, _)| *owner).collect();
                    let earliest = active.iter().map(|(_, until)| *until).min().unwrap();
                    let ordinary = self.renew_chunk(&owners, earliest, heartbeat);
                    let (_, completion) = tokio::try_join!(selected, ordinary)?;
                    if matches!(completion, ChunkCompletion::RenewIndividually) {
                        for owner in owners {
                            if !self.refresh_due_heartbeat(heartbeat).await? {
                                return Ok(false);
                            }
                            self.renew_one_before_member_deadline(owner, heartbeat)
                                .await?;
                        }
                    }
                    continue;
                }
            }
            let owners: Vec<_> = active.iter().map(|(owner, _)| *owner).collect();
            let earliest = active.iter().map(|(_, until)| *until).min().unwrap();
            let result = self.renew_chunk(&owners, earliest, heartbeat).await?;
            if matches!(result, ChunkCompletion::RenewIndividually) {
                for owner in owners {
                    if !self.refresh_due_heartbeat(heartbeat).await? {
                        return Ok(false);
                    }
                    self.renew_one_before_member_deadline(owner, heartbeat)
                        .await?;
                }
            }
        }
        Ok(true)
    }

    async fn renew_one_before_member_deadline(
        &self,
        owner: &Arc<Authority>,
        heartbeat: &RenewalHeartbeat,
    ) -> Result<(), OwnershipError> {
        tokio::select! {
            biased;
            _ = tokio::time::sleep_until(heartbeat.member_deadline) => {
                Err(self.maintenance_error("member_batch_deadline", None, OwnershipError::Stale))
            }
            result = self.renew_owned(owner, heartbeat.started, heartbeat.membership.now_ms) => result,
        }
    }

    async fn refresh_due_heartbeat(
        &self,
        heartbeat: &mut RenewalHeartbeat,
    ) -> Result<bool, OwnershipError> {
        if heartbeat.started.elapsed() < Duration::from_millis(HEARTBEAT_INTERVAL_MS) {
            return Ok(true);
        }
        let Some(sample) = self.refresh_membership().await? else {
            return Ok(false);
        };
        *heartbeat = sample;
        Ok(true)
    }

    async fn renew_chunk(
        &self,
        owners: &[&Arc<Authority>],
        earliest: Instant,
        heartbeat: &RenewalHeartbeat,
    ) -> Result<ChunkCompletion, OwnershipError> {
        let started = Instant::now();
        let until = earliest
            .min(heartbeat.member_deadline)
            .min(started + Duration::from_millis(STORE_DEADLINE_MS));
        let tokens: Vec<_> = owners.iter().map(|owner| owner.token.clone()).collect();
        let call = tokio::time::timeout_at(until, self.store.renew_batch(&tokens));
        tokio::pin!(call);
        let mut revocations: FuturesUnordered<_> = owners
            .iter()
            .map(|owner| async move {
                owner.revoked().await;
                *owner
            })
            .collect();
        let result = loop {
            tokio::select! {
                biased;
                Some(owner) = revocations.next() => {
                    if !renewal_obsolete(owner) {
                        return Err(self.maintenance_error("owner_active_deadline", Some(owner), OwnershipError::Stale));
                    }
                }
                result = &mut call => break result,
            }
        };
        let outcome = match &result {
            Ok(Ok(RenewalBatch::Committed(_))) => "committed",
            Ok(Ok(RenewalBatch::PartialMatchAborted)) => "partial_match_aborted",
            Ok(Err(OwnershipError::Unavailable(_))) => "unavailable",
            Ok(Err(_)) => "rejected",
            Err(_) => "timeout",
        };
        tracing::debug!(
            count = owners.len(),
            outcome,
            elapsed_ms = started.elapsed().as_millis() as u64,
            lease_remaining_ms = earliest
                .saturating_duration_since(Instant::now())
                .as_millis() as u64,
            "Agent ownership renewal chunk completed"
        );

        if !self.is_ready() {
            return Err(self.maintenance_error(
                "member_batch_deadline",
                None,
                OwnershipError::Stale,
            ));
        }
        match result {
            Ok(Ok(RenewalBatch::Committed(leases))) => {
                self.apply_chunk(owners, &leases, heartbeat)?;
                Ok(ChunkCompletion::Finished)
            }
            Ok(Ok(RenewalBatch::PartialMatchAborted)) => Ok(ChunkCompletion::RenewIndividually),
            Ok(Err(error)) if !matches!(error, OwnershipError::Unavailable(_)) => {
                Err(self.maintenance_error("owner_batch_store", None, error))
            }
            failure => {
                let reason = if failure.is_err() {
                    "renewal_call_timeout"
                } else {
                    "renewal_store_unavailable"
                };
                for owner in owners {
                    self.contain_unavailable_renewal(owner, reason)?;
                }
                Ok(ChunkCompletion::Finished)
            }
        }
    }

    fn apply_chunk(
        &self,
        owners: &[&Arc<Authority>],
        leases: &[OwnershipLease],
        heartbeat: &RenewalHeartbeat,
    ) -> Result<(), OwnershipError> {
        let by_agent: BTreeMap<_, _> = leases
            .iter()
            .map(|lease| (&lease.token.agent_id, lease))
            .collect();
        let invalid = || {
            self.maintenance_error(
                "owner_batch_result",
                None,
                OwnershipError::Invalid("batch renewal did not return the exact tokens".into()),
            )
        };
        if by_agent.len() != owners.len() || leases.len() != owners.len() {
            return Err(invalid());
        }
        // Validate the whole response before extending any captured authority.
        for owner in owners {
            let lease = by_agent.get(&owner.token.agent_id).ok_or_else(invalid)?;
            require_same_token(&owner.token, lease).map_err(|_| invalid())?;
            deadline(
                heartbeat.started,
                heartbeat.membership.now_ms,
                lease.expires_at_ms,
            )
            .map_err(|error| self.maintenance_error("owner_store_deadline", Some(owner), error))?;
        }
        for owner in owners {
            self.apply_renewal(
                owner,
                heartbeat.started,
                heartbeat.membership.now_ms,
                by_agent[&owner.token.agent_id],
            )?;
        }
        Ok(())
    }
}
