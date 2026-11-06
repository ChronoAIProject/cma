//! One bounded retry sequence inside the existing verification single-flight.

use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};
use tokio::time::{timeout, Instant};

use super::{
    identity::{self, VerifiedSubject},
    permissions::PermissionEvidence,
    verification_error::ProviderVerificationError as Error,
    AuthState, ProviderMetadata,
};

pub(super) const DEADLINE: Duration = Duration::from_millis(2500);
const ATTEMPT: Duration = Duration::from_millis(750);
const ATTEMPTS: u32 = 3;

pub(super) async fn verify(
    auth: &AuthState,
    metadata: Option<&ProviderMetadata>,
    token: &str,
    expiry: Option<SystemTime>,
    expected_subject: Option<&str>,
    budget: Duration,
) -> Result<Arc<VerifiedSubject>, Error> {
    // No queue of distinct-token verifications accumulates during an outage.
    // Local overload is not evidence of provider unavailability and cannot fall back.
    let _slot = auth
        .verification_slots
        .try_acquire()
        .map_err(|_| Error::Unavailable("identity verification capacity exhausted".into()))?;
    let key = auth.identity_store.key(token);
    let epoch = auth.identity_store.epoch();
    let result = timeout(budget.min(DEADLINE), async {
        let mut last = Error::Transient("deadline");
        for attempt in 0..ATTEMPTS {
            if attempt > 0 {
                let jitter = u64::from(uuid::Uuid::new_v4().as_bytes()[0]) % 31;
                tokio::time::sleep(Duration::from_millis(40 * (1 << (attempt - 1)) + jitter)).await;
            }
            let started = Instant::now();
            let result = timeout(ATTEMPT, async {
                let discovered;
                let metadata = match metadata {
                    Some(metadata) => metadata,
                    None => {
                        discovered = auth.verification_metadata().await?;
                        &discovered
                    }
                };
                identity::verify_subject_expected(auth, metadata, token, expected_subject).await
            })
            .await
            .unwrap_or(Err(Error::Transient("timeout")));
            match result {
                Err(error @ Error::Transient(_)) => {
                    tracing::warn!(
                        event = "identity_verification_retry",
                        attempt = attempt + 1,
                        elapsed_ms = started.elapsed().as_millis() as u64,
                        reason = error.reason(),
                        "Identity verification attempt unavailable"
                    );
                    last = error;
                }
                result => return result,
            }
        }
        Err(last)
    })
    .await
    .unwrap_or(Err(Error::Transient("deadline")))
    .map(Arc::new);
    match &result {
        Ok(evidence) if matches!(evidence.permissions, PermissionEvidence::Active(_)) => {
            let expiry = match (expiry, evidence.token_expires_at) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
            if let Some(expiry) = expiry {
                auth.identity_store
                    .remember(key, &epoch, evidence.clone(), expiry);
            } else {
                auth.identity_store.invalidate(&key);
            }
        }
        Err(Error::Transient(_)) => {}
        _ => auth.identity_store.invalidate(&key),
    }
    result
}

pub(super) fn unavailable() -> layer2_core::StoreError {
    layer2_core::StoreError::Unavailable("identity_provider_unavailable".into())
}
