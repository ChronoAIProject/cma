use super::super::Membership;
use super::*;

type BoundedHeartbeat = Result<Result<Membership, OwnershipError>, tokio::time::error::Elapsed>;

pub(super) struct RenewalHeartbeat {
    pub(super) started: Instant,
    pub(super) member_deadline: Instant,
    pub(super) membership: Membership,
}

impl OwnershipCoordinator {
    pub(super) async fn refresh_membership(
        &self,
    ) -> Result<Option<RenewalHeartbeat>, OwnershipError> {
        if !self.is_ready() {
            return Err(self.maintenance_error("member_readiness", None, OwnershipError::Stale));
        }
        let started = Instant::now();
        let established_deadline = *self.member_deadline.lock().unwrap();
        let heartbeat = tokio::time::timeout(
            Duration::from_millis(STORE_DEADLINE_MS),
            self.store.heartbeat(&self.instance_id, false),
        );
        let heartbeat = tokio::time::timeout_at(established_deadline, heartbeat)
            .await
            .map_err(|_| {
                self.maintenance_error("member_local_deadline", None, OwnershipError::Stale)
            })?;
        // Neither membership nor any authority is extended by a skipped tick.
        let Some(membership) = self.heartbeat_membership(heartbeat)? else {
            return Ok(None);
        };
        membership
            .require_compatible()
            .map_err(|error| self.maintenance_error("member_protocol", None, error))?;
        let member = membership
            .members
            .iter()
            .find(|member| member.instance_id == self.instance_id)
            .ok_or_else(|| self.maintenance_error("member_lookup", None, OwnershipError::Stale))?;
        if member.draining {
            return Err(self.maintenance_error("member_drain", None, OwnershipError::Stale));
        }
        let member_deadline = deadline(started, membership.now_ms, member.expires_at_ms)
            .map_err(|error| self.maintenance_error("member_store_deadline", None, error))?;
        {
            let mut current = self.member_deadline.lock().unwrap();
            if *current <= Instant::now() {
                return Err(self.maintenance_error(
                    "member_local_deadline",
                    None,
                    OwnershipError::Stale,
                ));
            }
            *current = member_deadline;
        }
        Ok(Some(RenewalHeartbeat {
            started,
            member_deadline,
            membership,
        }))
    }

    /// Returns None to skip one tick after a transient heartbeat failure.
    ///
    /// Skipping extends neither the member deadline nor any Agent authority:
    /// each keeps its existing store-derived deadline and simply runs out if
    /// the store stays unavailable. Takeover still requires durable expiry.
    pub(super) fn heartbeat_membership(
        &self,
        heartbeat: BoundedHeartbeat,
    ) -> Result<Option<Membership>, OwnershipError> {
        let (reason, error) = match heartbeat {
            Ok(Ok(membership)) => {
                self.skipped_heartbeats.store(0, Ordering::Release);
                return Ok(Some(membership));
            }
            Err(_) => ("heartbeat_call_timeout", deadline_exceeded()),
            Ok(Err(error @ OwnershipError::Unavailable(_))) => {
                ("heartbeat_store_unavailable", error)
            }
            Ok(Err(error)) => return Err(self.maintenance_error("member_heartbeat", None, error)),
        };
        // Only established, serving membership may wait for the next tick.
        // Drain, process failure and local expiry keep their fatal result;
        // is_ready() already refuses at or after the member deadline.
        if !self.is_ready() {
            let member_deadline = *self.member_deadline.lock().unwrap();
            let observed_at = Instant::now();
            let draining = self.draining.load(Ordering::Acquire);
            let failed = *self.failed.borrow();
            let prior_skipped_ticks = self.skipped_heartbeats.load(Ordering::Acquire);
            let remaining = member_deadline.saturating_duration_since(observed_at);
            tracing::error!(
                stage = "member_heartbeat",
                scope = "process",
                outcome = "fatal",
                reason,
                draining,
                failed,
                prior_skipped_ticks,
                member_remaining_ms = u64::try_from(remaining.as_millis()).unwrap_or(u64::MAX),
                member_deadline_elapsed = observed_at >= member_deadline,
                "Unavailable member heartbeat cannot skip maintenance"
            );
            return Err(self.maintenance_error("member_heartbeat", None, error));
        }
        let remaining = self
            .member_deadline
            .lock()
            .unwrap()
            .saturating_duration_since(Instant::now());
        let consecutive = self.skipped_heartbeats.fetch_add(1, Ordering::AcqRel) + 1;
        metrics::counter!("cma_agent_ownership_heartbeat_skips_total", "reason" => reason)
            .increment(1);
        use sha2::{Digest, Sha256};
        let instance_sha256 = format!("{:x}", Sha256::digest(self.instance_id.as_bytes()));
        tracing::warn!(stage = "member_heartbeat", scope = "process", reason,
            outcome = "skipped_tick", consecutive,
            member_remaining_ms = u64::try_from(remaining.as_millis()).unwrap_or(u64::MAX),
            %instance_sha256, "Unavailable member heartbeat skipped one maintenance tick");
        Ok(None)
    }
}
