//! Only overlapping requests share independent bearer verification work.

use std::collections::HashMap;
use std::sync::{Arc, Weak};
use std::time::Duration;

use tokio::sync::{Mutex, OnceCell};

use super::{
    identity::VerifiedSubject, persistence::SecretDigest,
    verification_error::ProviderVerificationError, AuthState, ProviderMetadata,
};

const PROVIDER_VERIFICATION_TIMEOUT: Duration = super::identity_retry::DEADLINE;
const MAX_VERIFICATION_FLIGHTS: usize = 256;
type VerificationFlight = OnceCell<Result<Arc<VerifiedSubject>, ProviderVerificationError>>;

#[derive(Default)]
pub(super) struct VerificationFlights {
    entries: Mutex<HashMap<SecretDigest, Weak<VerificationFlight>>>,
}

impl VerificationFlights {
    async fn acquire(&self, key: &SecretDigest) -> Arc<VerificationFlight> {
        let mut entries = self.entries.lock().await;
        entries.retain(|_, flight| flight.strong_count() > 0);
        if let Some(flight) = entries.get(key).and_then(Weak::upgrade) {
            if !flight.initialized() {
                return flight;
            }
        }
        let flight = Arc::new(OnceCell::new());
        if entries.len() < MAX_VERIFICATION_FLIGHTS || entries.contains_key(key) {
            entries.insert(key.clone(), Arc::downgrade(&flight));
        }
        flight
    }

    async fn release(&self, key: &SecretDigest, flight: &Arc<VerificationFlight>) {
        let mut entries = self.entries.lock().await;
        if entries
            .get(key)
            .is_some_and(|current| current.ptr_eq(&Arc::downgrade(flight)))
        {
            entries.remove(key);
        }
    }
}

pub(super) async fn verify_subject(
    auth: &AuthState,
    metadata: &ProviderMetadata,
    credential: &str,
) -> Result<Arc<VerifiedSubject>, ProviderVerificationError> {
    verify_with_timeout(auth, metadata, credential, PROVIDER_VERIFICATION_TIMEOUT).await
}

async fn verify_with_timeout(
    auth: &AuthState,
    metadata: &ProviderMetadata,
    credential: &str,
    timeout: Duration,
) -> Result<Arc<VerifiedSubject>, ProviderVerificationError> {
    verify_optional_metadata(auth, Some(metadata), credential, timeout).await
}

pub(super) async fn verify_request(
    auth: &AuthState,
    credential: &str,
) -> Result<Arc<VerifiedSubject>, ProviderVerificationError> {
    verify_optional_metadata(auth, None, credential, PROVIDER_VERIFICATION_TIMEOUT).await
}

async fn verify_optional_metadata(
    auth: &AuthState,
    metadata: Option<&ProviderMetadata>,
    credential: &str,
    timeout: Duration,
) -> Result<Arc<VerifiedSubject>, ProviderVerificationError> {
    let key = SecretDigest::identify("independent-bearer-verification", credential);
    let flight = auth.bearer_verifications.acquire(&key).await;
    let led = std::sync::atomic::AtomicBool::new(false);
    let verified = match tokio::time::timeout(
        timeout,
        flight.get_or_init(|| {
            led.store(true, std::sync::atomic::Ordering::Relaxed);
            super::identity_retry::verify(auth, metadata, credential, None, None, timeout)
        }),
    )
    .await
    {
        Ok(result) => result.clone(),
        Err(_) => {
            let error =
                ProviderVerificationError::Unavailable("identity caller deadline elapsed".into());
            use super::verification_diagnostics::{record, Endpoint, Reason, Stage};
            record(
                Endpoint::Verification,
                Stage::Deadline,
                Reason::CallerDeadline,
                None,
                &error,
            );
            Err(error)
        }
    };
    super::identity_observations::resolution(if led.load(std::sync::atomic::Ordering::Relaxed) {
        "led"
    } else {
        "shared"
    });
    if flight.initialized() {
        auth.bearer_verifications.release(&key, &flight).await;
    }
    verified.and_then(|evidence| {
        if !super::identity_lifetime::current(auth, &evidence) {
            return Err(ProviderVerificationError::Unavailable(
                "identity evidence changed before consumption".into(),
            ));
        }
        super::identity_lifetime::retain(&evidence);
        Ok(evidence)
    })
}

#[cfg(test)]
#[path = "bearer_authorization_tests.rs"]
mod tests;
