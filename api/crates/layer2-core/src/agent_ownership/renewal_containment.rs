//! Cancel one exact authority on store unavailability without replaying its write.
use super::*;

impl OwnershipCoordinator {
    pub(super) async fn renew_owner_or_cancel(
        &self,
        authority: &Arc<Authority>,
    ) -> Result<Option<OwnershipLease>, OwnershipError> {
        match bounded(self.store.renew(&authority.token)).await {
            Ok(lease) => Ok(Some(lease)),
            Err(error) => {
                // The store already retries definite aborts. Unavailable can also
                // mean an uncertain commit: never issue a replacement renewal or
                // infer an expiry from the previous lease. Cancel local effects
                // and retain this exact token for acknowledged cleanup/takeover.
                if matches!(error, OwnershipError::Unavailable(_))
                    && self.is_ready()
                    && authority.is_current()
                {
                    authority.cancel_receiver();
                    // Cancellation is monotonic and cannot relabel an authority
                    // whose deadline or independent revocation won the race.
                    if authority.receiver_cancelled() && self.is_ready() {
                        self.maintenance_error(
                            "owner_store_renew_contained",
                            Some(authority),
                            error,
                        );
                        return Ok(None);
                    }
                }
                Err(self.maintenance_error("owner_store_renew", Some(authority), error))
            }
        }
    }
}
