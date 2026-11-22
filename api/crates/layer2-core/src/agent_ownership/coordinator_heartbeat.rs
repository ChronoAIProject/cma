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
        let error = match heartbeat {
            Ok(Ok(membership)) => return Ok(Some(membership)),
            Err(_) => deadline_exceeded(),
            Ok(Err(error @ OwnershipError::Unavailable(_))) => error,
            Ok(Err(error)) => return Err(self.maintenance_error("member_heartbeat", None, error)),
        };
        // Only established, serving membership may wait for the next tick.
        // Drain, process failure and local expiry keep their fatal result.
        if !self.is_ready() {
            return Err(self.maintenance_error("member_heartbeat", None, error));
        }
        Ok(None)
    }
}
