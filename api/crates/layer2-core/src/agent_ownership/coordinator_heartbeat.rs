use super::super::Membership;
use super::*;

type BoundedHeartbeat = Result<Result<Membership, OwnershipError>, tokio::time::error::Elapsed>;

impl OwnershipCoordinator {
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
        // Drain, process failure and local expiry keep their fatal result.
        let remaining = self
            .member_deadline
            .lock()
            .unwrap()
            .saturating_duration_since(Instant::now());
        if remaining.is_zero() || !self.is_ready() {
            return Err(self.maintenance_error("member_heartbeat", None, error));
        }
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
