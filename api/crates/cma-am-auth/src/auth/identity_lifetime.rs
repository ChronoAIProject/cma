//! Exact live verification receipts held only by their admitted consumers.
use std::{cell::RefCell, future::Future, sync::Arc, time::SystemTime};

use tokio::time::Instant;

use super::{
    identity::VerifiedSubject,
    identity_flight::{Epoch, TokenHash},
    verification_error::{
        ProviderVerificationError as Error, VerificationCause, VerificationFailure,
    },
    AuthState,
};

pub(super) struct VerificationLifetime {
    key: TokenHash,
    epoch: Epoch,
    expires_at: Option<SystemTime>,
    deadline: Option<Instant>,
}

impl VerificationLifetime {
    pub(super) fn new(
        key: TokenHash,
        epoch: Epoch,
        expiry: Option<SystemTime>,
    ) -> Result<Self, VerificationFailure> {
        let deadline = expiry
            .map(|expiry| {
                let remaining = expiry.duration_since(SystemTime::now()).map_err(|_| {
                    VerificationFailure::new(
                        Error::Rejected("identity evidence expired during verification".into()),
                        VerificationCause::ReceiptExpired,
                    )
                })?;
                Instant::now().checked_add(remaining).ok_or_else(|| {
                    VerificationFailure::new(
                        Error::Unavailable("identity expiry exceeds monotonic clock bounds".into()),
                        VerificationCause::ExpiryClockBounds,
                    )
                })
            })
            .transpose()?;
        Ok(Self {
            key,
            epoch,
            expires_at: expiry,
            deadline,
        })
    }

    pub(super) fn published(mut self, epoch: Epoch) -> Self {
        // Publication changes only the fence, never the validated deadline.
        self.epoch = epoch;
        self
    }

    fn failure(&self, auth: &AuthState) -> Option<VerificationCause> {
        if self
            .expires_at
            .is_some_and(|expiry| expiry <= SystemTime::now())
            || self
                .deadline
                .is_some_and(|deadline| Instant::now() >= deadline)
        {
            Some(VerificationCause::ConsumptionExpired)
        } else if !auth.identity_store.token_current(&self.key, &self.epoch) {
            Some(VerificationCause::ConsumptionEpochChanged)
        } else {
            None
        }
    }
}

pub(super) fn current(auth: &AuthState, evidence: &VerifiedSubject) -> bool {
    failure(auth, evidence).is_none()
}

/// Project the original monotonic deadline to a wall time without extending it.
pub(super) fn relay_expiry(evidence: &VerifiedSubject, now: SystemTime) -> Option<SystemTime> {
    let lifetime = evidence.verification.as_ref()?;
    let wall = lifetime.expires_at?;
    let monotonic = lifetime.deadline?;
    let remaining = monotonic.checked_duration_since(Instant::now())?;
    Some(now.checked_add(remaining)?.min(wall))
}

pub(super) fn failure(auth: &AuthState, evidence: &VerifiedSubject) -> Option<VerificationCause> {
    if evidence
        .token_expires_at
        .is_some_and(|expiry| expiry <= SystemTime::now())
    {
        Some(VerificationCause::ConsumptionExpired)
    } else {
        evidence
            .verification
            .as_ref()
            .and_then(|proof| proof.failure(auth))
    }
}

tokio::task_local! { static CURRENT: RefCell<Vec<Arc<VerifiedSubject>>>; }

pub(super) fn retain(evidence: &Arc<VerifiedSubject>) {
    // Only an already-admitted request owns this receipt. It is never entered
    // into a lookup table for a future request to discover or borrow.
    let _ = CURRENT.try_with(|receipts| receipts.borrow_mut().push(evidence.clone()));
}

pub(super) async fn scope<T, E>(
    auth: &AuthState,
    future: impl Future<Output = Result<Option<T>, E>>,
    expired: impl FnOnce() -> E,
) -> Result<Option<T>, E> {
    CURRENT
        .scope(RefCell::new(Vec::new()), async {
            let Some(result) = future.await? else {
                return Ok(None);
            };
            if CURRENT.with(|receipts| {
                receipts
                    .borrow()
                    .iter()
                    .all(|receipt| current(auth, receipt))
            }) {
                Ok(Some(result))
            } else {
                tracing::debug!(parent: None, event = "identity_evidence_changed",
                "Live identity evidence changed or expired during authorization");
                Err(expired())
            }
        })
        .await
}
