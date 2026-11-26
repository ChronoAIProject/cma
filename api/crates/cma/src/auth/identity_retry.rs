//! One bounded retry sequence inside the existing verification single-flight.

use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};
use tokio::time::{timeout, Instant};

#[cfg(test)]
use super::permissions::PermissionEvidence;
use super::{
    identity::{self, VerifiedSubject},
    verification_error::{
        ProviderVerificationError as Error, VerificationCause, VerificationFailure,
    },
    AuthState, ProviderMetadata,
};

pub(super) const DEADLINE: Duration = Duration::from_millis(2500);
const ATTEMPT: Duration = Duration::from_millis(750);
const ATTEMPTS: u32 = 3;

#[cfg(test)]
pub(super) async fn verify(
    auth: &AuthState,
    metadata: Option<&ProviderMetadata>,
    token: &str,
    expiry: Option<SystemTime>,
    expected_subject: Option<&str>,
    budget: Duration,
) -> Result<Arc<VerifiedSubject>, Error> {
    verify_detailed(auth, metadata, token, expiry, expected_subject, budget)
        .await
        .map_err(|failure| failure.error)
}

pub(super) async fn verify_detailed(
    auth: &AuthState,
    metadata: Option<&ProviderMetadata>,
    token: &str,
    expiry: Option<SystemTime>,
    expected_subject: Option<&str>,
    budget: Duration,
) -> Result<Arc<VerifiedSubject>, VerificationFailure> {
    // No queue of distinct-token verifications accumulates during an outage.
    // Local overload is not evidence of provider unavailability and cannot fall back.
    use super::verification_diagnostics::{record, Endpoint, Reason, Stage};
    #[cfg(test)]
    let cache = auth.identity_store.positive_cache();
    #[cfg(test)]
    if let Some(cache) = cache {
        let live_key = auth.identity_store.key(token);
        let discovered = auth.metadata.try_read().ok();
        let metadata = metadata.or_else(|| discovered.as_deref()?.as_ref());
        if let Some(evidence) = metadata.and_then(|metadata| {
            cache.cached_positive(&live_key, metadata, expiry, expected_subject)
        }) {
            return Ok(evidence);
        }
    }
    let _slot = auth
        .verification_slots
        .try_acquire()
        .map_err(|_| Error::Unavailable("identity verification capacity exhausted".into()))
        .inspect_err(|error| {
            record(
                Endpoint::Verification,
                Stage::Admission,
                Reason::VerificationCapacity,
                None,
                error,
            )
        })
        .map_err(|error| VerificationFailure::new(error, VerificationCause::Capacity))?;
    let live_key = auth.identity_store.key(token);
    let token_epoch = auth.identity_store.token_epoch(&live_key).ok_or_else(|| {
        let error = Error::Unavailable("identity verification fence capacity exhausted".into());
        record(
            Endpoint::Verification,
            Stage::Admission,
            Reason::FenceCapacity,
            None,
            &error,
        );
        VerificationFailure::new(error, VerificationCause::FenceCapacity)
    })?;
    let stop = Instant::now() + budget.min(DEADLINE);
    let result = timeout(budget.min(DEADLINE), async {
        let mut last = Error::Transient("deadline");
        for attempt in 0..ATTEMPTS {
            if attempt > 0 {
                let jitter = u64::from(uuid::Uuid::new_v4().as_bytes()[0]) % 31;
                let backoff = Duration::from_millis(40 * (1 << (attempt - 1)) + jitter);
                let delay = match &last {
                    Error::RateLimited(hint) => hint.remaining().max(backoff),
                    _ => backoff,
                };
                // Reserve a complete attempt inside the caller budget. Starting
                // a partial attempt at its edge can lose the typed throttle to
                // the outer caller deadline before projecting Retry-After.
                if delay.saturating_add(ATTEMPT) >= stop.saturating_duration_since(Instant::now()) {
                    return Err(last);
                }
                tokio::time::sleep(delay).await;
            }
            let started = Instant::now();
            let deadline = started + ATTEMPT;
            let result = async {
                let discovered;
                let metadata = match metadata {
                    Some(metadata) => metadata,
                    None => {
                        discovered = auth.verification_metadata_until(deadline).await?;
                        &discovered
                    }
                };
                identity::verify_subject_expected(auth, metadata, token, expected_subject, deadline)
                    .await
                    .map(|evidence| (evidence, metadata.clone()))
            }
            .await;
            match result {
                Err(error @ (Error::Transient(_) | Error::RateLimited(_))) => {
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
    // An outer cancellation has no trustworthy endpoint phase. Only the
    // provider reader can classify a pre-response timeout as transient.
    .map(|result| result.map_err(VerificationFailure::from))
    .unwrap_or_else(|_| {
        let error = Error::Unavailable("identity verification deadline elapsed".into());
        record(
            Endpoint::Verification,
            Stage::Deadline,
            Reason::VerificationDeadline,
            None,
            &error,
        );
        Err(VerificationFailure::new(
            error,
            VerificationCause::VerificationDeadline,
        ))
    });
    let result = match result {
        Ok((mut evidence, metadata)) => {
            let expiry = [expiry, evidence.token_expires_at, evidence.api_expires_at]
                .into_iter()
                .flatten()
                .min();
            // Reject expired or unrepresentable evidence before publication.
            // Equivalent publications never borrow or refresh another lifetime.
            let lifetime = super::identity_lifetime::VerificationLifetime::new(
                live_key,
                token_epoch.clone(),
                expiry,
            )?;
            #[cfg(test)]
            let mut lifetime = lifetime;
            #[cfg(test)]
            let cache =
                cache.filter(|_| matches!(&evidence.permissions, PermissionEvidence::Active(_)));
            #[cfg(test)]
            if cache.is_some() {
                lifetime.cap_for_cache();
            }
            let stamp = auth.identity_store.evidence_stamp(
                &live_key,
                super::identity_evidence::EvidenceScope::new(auth, &metadata, token),
                &evidence,
            )?;
            let completed = auth
                .identity_store
                .finish_verification(&live_key, &token_epoch, &stamp)
                .ok_or_else(|| {
                    VerificationFailure::new(
                        Error::Unavailable(
                            "identity evidence invalidated during verification".into(),
                        ),
                        VerificationCause::PublicationEpochChanged,
                    )
                })?;
            #[cfg(test)]
            let cache_epoch = completed.clone();
            evidence.verification = Some(lifetime.published(completed));
            let evidence = Arc::new(evidence);
            #[cfg(test)]
            if let Some(cache) = cache {
                cache.remember_positive(&live_key, &cache_epoch, metadata, evidence.clone());
            }
            Ok(evidence)
        }
        Err(error) => Err(error),
    };
    if result
        .as_ref()
        .err()
        .is_some_and(|failure| matches!(failure.error, Error::Rejected(_) | Error::Unavailable(_)))
    {
        auth.identity_store.invalidate_token(token);
    }
    result
}

pub(super) fn unavailable() -> layer2_core::StoreError {
    layer2_core::StoreError::Unavailable("identity_provider_unavailable".into())
}
