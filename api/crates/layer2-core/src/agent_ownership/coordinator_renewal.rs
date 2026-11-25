use super::super::authority::ReceiverCancellation;
use super::*;

#[cfg(test)]
type BeforeRenewalDeadline = Box<dyn Fn(&Authority, &'static str) + Send>;

#[cfg(test)]
tokio::task_local! {
    pub(super) static BEFORE_RENEWAL_DEADLINE: BeforeRenewalDeadline;
}

impl OwnershipCoordinator {
    pub(super) async fn renew_owned(
        &self,
        authority: &Arc<Authority>,
        started: Instant,
        now_ms: i64,
    ) -> Result<(), OwnershipError> {
        self.renew_owned_with_episode(authority, started, now_ms, None)
            .await
    }

    pub(super) async fn renew_owned_with_episode(
        &self,
        authority: &Arc<Authority>,
        started: Instant,
        now_ms: i64,
        episode: Option<super::super::heartbeat_retry::TransientEpisode>,
    ) -> Result<(), OwnershipError> {
        if self.contains_failures() {
            if let Some(error) = authority.settlement_error() {
                return Err(error);
            }
        }
        let normalize = || {
            self.contains_failures()
                && self.is_ready()
                && episode.is_some_and(|episode| authority.retire_retry_expiry(episode))
        };
        let fatal = |stage, error| self.maintenance_error(stage, Some(authority), error);
        if self.contains_failures() && !authority.is_current() && normalize() {
            return Ok(());
        }
        if self
            .live_renewal_deadline(authority, "owner_dispatch_deadline")?
            .is_none()
        {
            return Ok(());
        }
        let renewal = tokio::time::timeout(Duration::from_millis(STORE_DEADLINE_MS), async {
            #[cfg(feature = "qualification-hooks")]
            if let Some(hook) = self.qualification_renewal.get() {
                let window = super::qualification::RenewalWindow { started, now_ms };
                if self.contains_failures() {
                    return authority
                        .renew_store_qualified(window, || hook.take_renewal(authority, window))
                        .await;
                }
                if let Some(attempt) = hook.take_renewal(authority, window) {
                    let result = attempt.write.await;
                    return attempt
                        .waiter
                        .deliver(result, window, &authority.token)
                        .await;
                }
            }
            if self.contains_failures() {
                authority.renew_store_owned().await
            } else {
                self.store.renew(&authority.token).await
            }
        });
        tokio::pin!(renewal);
        let result = tokio::select! {
            biased;
            _ = authority.revoked() => {
                if !renewal_obsolete(authority) && !normalize() {
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
            Ok(Err(OwnershipError::Stale)) if renewal_obsolete(authority) => return Ok(()),
            Ok(Err(error)) => return Err(fatal("owner_store_renew", error)),
            Ok(Ok(lease)) => {
                // A different token is an invariant failure, not a late success
                // for this captured authority, even if it has since retired.
                require_same_token(&authority.token, &lease)
                    .map_err(|error| fatal("owner_same_token", error))?;
                let until = deadline(started, now_ms, lease.expires_at_ms)
                    .map_err(|error| fatal("owner_store_deadline", error))?;
                if self.contains_failures() && !authority.is_current() && normalize() {
                    return Ok(());
                }
                if self
                    .live_renewal_deadline(authority, "owner_result_deadline")?
                    .is_none()
                {
                    return Ok(());
                }
                return match authority.renew_until(until) {
                    Ok(()) => Ok(()),
                    Err(_) if renewal_obsolete(authority) => Ok(()),
                    Err(error) => Err(fatal("owner_local_renew", error)),
                };
            }
        };
        if renewal_obsolete(authority) {
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

    pub(super) fn apply_renewal(
        &self,
        authority: &Authority,
        started: Instant,
        now_ms: i64,
        lease: &OwnershipLease,
    ) -> Result<(), OwnershipError> {
        let fatal = |stage, error| self.maintenance_error(stage, Some(authority), error);
        require_same_token(&authority.token, lease)
            .map_err(|error| fatal("owner_same_token", error))?;
        let until = deadline(started, now_ms, lease.expires_at_ms)
            .map_err(|error| fatal("owner_store_deadline", error))?;
        if self
            .live_renewal_deadline(authority, "owner_result_deadline")?
            .is_none()
        {
            return Ok(());
        }
        match authority.renew_until(until) {
            Ok(()) => Ok(()),
            Err(_) if self.is_ready() && renewal_obsolete(authority) => Ok(()),
            Err(error) => Err(fatal("owner_local_renew", error)),
        }
    }

    pub(super) fn live_renewal_deadline(
        &self,
        authority: &Authority,
        stage: &'static str,
    ) -> Result<Option<Instant>, OwnershipError> {
        if renewal_obsolete(authority) {
            return Ok(None);
        }
        #[cfg(test)]
        let _ = BEFORE_RENEWAL_DEADLINE.try_with(|hook| hook(authority, stage));
        let deadline = authority
            .established_deadline()
            .filter(|until| *until > Instant::now());
        let stale = || self.maintenance_error(stage, Some(authority), OwnershipError::Stale);
        if !self.is_ready() {
            return Err(stale());
        }
        if renewal_obsolete(authority) {
            return Ok(None);
        }
        deadline.map(Some).ok_or_else(stale)
    }

    pub(super) fn contain_unavailable_renewal(
        &self,
        authority: &Authority,
        reason: &'static str,
    ) -> Result<(), OwnershipError> {
        let fatal = |stage, error| self.maintenance_error(stage, Some(authority), error);
        if renewal_obsolete(authority) {
            return Ok(());
        }
        if !self.is_ready() {
            return Err(fatal("member_before_containment", OwnershipError::Stale));
        }
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
            ReceiverCancellation::NotLive if authority.retiring.load(Ordering::Acquire) => Ok(()),
            ReceiverCancellation::NotLive => {
                Err(fatal("owner_containment_deadline", OwnershipError::Stale))
            }
        }
    }
}

pub(super) fn renewal_obsolete(authority: &Authority) -> bool {
    authority.retiring.load(Ordering::Acquire) || authority.receiver_cancelled()
}

pub(super) fn error_reason(error: &OwnershipError) -> &'static str {
    match error {
        OwnershipError::Stale => "stale",
        OwnershipError::IncompatibleProtocol => "incompatible_protocol",
        OwnershipError::Invalid(_) => "invalid",
        OwnershipError::Unavailable(_) => "unavailable",
    }
}
