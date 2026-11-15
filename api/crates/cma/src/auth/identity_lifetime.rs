//! Exact live verification receipts held only by their admitted consumers.
use std::{cell::RefCell, future::Future, sync::Arc, time::SystemTime};

use layer2_core::StoreError;
use tokio::time::Instant;

use super::{
    identity::VerifiedSubject,
    identity_flight::{Epoch, TokenHash},
    verification_error::ProviderVerificationError as Error,
    AuthState, RequestAuth,
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
    ) -> Result<Self, Error> {
        let deadline = expiry
            .map(|expiry| {
                let remaining = expiry.duration_since(SystemTime::now()).map_err(|_| {
                    Error::Rejected("identity evidence expired during verification".into())
                })?;
                Instant::now().checked_add(remaining).ok_or_else(|| {
                    Error::Unavailable("identity expiry exceeds monotonic clock bounds".into())
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

    fn current(&self, auth: &AuthState) -> bool {
        self.expires_at
            .is_none_or(|expiry| expiry > SystemTime::now())
            && self
                .deadline
                .is_none_or(|deadline| Instant::now() < deadline)
            && auth.identity_store.token_current(&self.key, &self.epoch)
    }
}

pub(super) fn current(auth: &AuthState, evidence: &VerifiedSubject) -> bool {
    evidence
        .token_expires_at
        .is_none_or(|expiry| expiry > SystemTime::now())
        && evidence
            .verification
            .as_ref()
            .is_none_or(|proof| proof.current(auth))
}

tokio::task_local! { static CURRENT: RefCell<Vec<Arc<VerifiedSubject>>>; }

pub(super) fn retain(evidence: &Arc<VerifiedSubject>) {
    // Only an already-admitted request owns this receipt. It is never entered
    // into a lookup table for a future request to discover or borrow.
    let _ = CURRENT.try_with(|receipts| receipts.borrow_mut().push(evidence.clone()));
}

pub(super) async fn scope(
    auth: &AuthState,
    future: impl Future<Output = Result<Option<RequestAuth>, StoreError>>,
) -> Result<Option<RequestAuth>, StoreError> {
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
                Err(super::identity_retry::unavailable())
            }
        })
        .await
}
