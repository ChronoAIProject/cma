use super::super::authority::ReceiverCancellation;
use super::*;

impl OwnershipCoordinator {
    pub(super) async fn renew_owned(
        &self,
        authority: &Arc<Authority>,
        started: Instant,
        now_ms: i64,
    ) -> Result<(), OwnershipError> {
        let obsolete =
            || authority.retiring.load(Ordering::Acquire) || authority.receiver_cancelled();
        let fatal = |stage, error| self.maintenance_error(stage, Some(authority), error);
        if obsolete() {
            return Ok(());
        }
        if !self.is_ready() || !authority.is_current() {
            return Err(fatal("owner_dispatch_deadline", OwnershipError::Stale));
        }
        let renewal = tokio::time::timeout(
            Duration::from_millis(STORE_DEADLINE_MS),
            self.store.renew(&authority.token),
        );
        tokio::pin!(renewal);
        let result = tokio::select! {
            biased;
            _ = authority.revoked() => {
                if !obsolete() {
                    return Err(fatal("owner_active_deadline", OwnershipError::Stale));
                }
                // A deliberate handoff/cancellation must not revive authority.
                // Still classify a returned protocol/record failure as fatal.
                renewal.await
            }
            result = &mut renewal => result,
        };
        let reason = match result {
            Err(_) => "renewal_call_timeout",
            Ok(Err(OwnershipError::Unavailable(_))) => "renewal_store_unavailable",
            Ok(Err(OwnershipError::Stale)) if obsolete() => return Ok(()),
            Ok(Err(error)) => return Err(fatal("owner_store_renew", error)),
            Ok(Ok(lease)) => {
                // A different token is an invariant failure, not a late success
                // for this captured authority, even if it has since retired.
                require_same_token(&authority.token, &lease)
                    .map_err(|error| fatal("owner_same_token", error))?;
                let until = deadline(started, now_ms, lease.expires_at_ms)
                    .map_err(|error| fatal("owner_store_deadline", error))?;
                if obsolete() {
                    return Ok(());
                }
                if !self.is_ready() || !authority.is_current() {
                    return Err(fatal("owner_result_deadline", OwnershipError::Stale));
                }
                return match authority.renew_until(until) {
                    Ok(()) => Ok(()),
                    Err(_) if obsolete() => Ok(()),
                    Err(error) => Err(fatal("owner_local_renew", error)),
                };
            }
        };
        if obsolete() {
            return Ok(());
        }
        if !self.is_ready() {
            return Err(fatal("member_before_containment", OwnershipError::Stale));
        }
        // Validity and cancellation share the authority's deadline lock. A
        // racing expiry or generic revocation can never become containment.
        let outcome = authority.cancel_receiver();
        if !self.is_ready() {
            return Err(fatal("member_after_containment", OwnershipError::Stale));
        }
        match outcome {
            ReceiverCancellation::CancelledWhileLive => {
                use sha2::{Digest, Sha256};
                let agent_sha256 = format!(
                    "{:x}",
                    Sha256::digest(authority.token.agent_id.as_str().as_bytes())
                );
                let instance_sha256 = format!("{:x}", Sha256::digest(self.instance_id.as_bytes()));
                tracing::warn!(scope = "agent", stage = "owner_store_renew", reason,
                    outcome = "cancelled_while_live", %agent_sha256, %instance_sha256,
                    epoch = authority.token.epoch, "Unavailable renewal cancelled one Agent authority");
                Ok(())
            }
            ReceiverCancellation::AlreadySafelyCancelled => Ok(()),
            // A handoff can win between the obsolete check and cancellation.
            // Its result is obsolete; do not relabel the revoked authority.
            ReceiverCancellation::NotLive if authority.retiring.load(Ordering::Acquire) => Ok(()),
            ReceiverCancellation::NotLive => {
                Err(fatal("owner_containment_deadline", OwnershipError::Stale))
            }
        }
    }
}

pub(super) fn error_reason(error: &OwnershipError) -> &'static str {
    match error {
        OwnershipError::Stale => "stale",
        OwnershipError::IncompatibleProtocol => "incompatible_protocol",
        OwnershipError::Invalid(_) => "invalid",
        OwnershipError::Unavailable(_) => "unavailable",
    }
}
