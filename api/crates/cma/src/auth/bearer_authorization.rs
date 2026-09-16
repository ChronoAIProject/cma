//! Only overlapping requests share independent bearer verification work.

use std::collections::HashMap;
use std::sync::{Arc, Weak};
use std::time::Duration;

use tokio::sync::{Mutex, OnceCell};

use super::{
    identity::{self, VerifiedSubject},
    persistence::SecretDigest,
    verification_error::ProviderVerificationError,
    AuthState, ProviderMetadata,
};

const PROVIDER_VERIFICATION_TIMEOUT: Duration = Duration::from_secs(15);
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
    // AuthState fixes the provider/client scope. The key contains no raw bearer;
    // distinct credentials must never share identity, permissions or expiry.
    let key = SecretDigest::identify("independent-bearer-verification", credential);
    let flight = auth.bearer_verifications.acquire(&key).await;
    // Completed evidence is not reused by later requests. Every caller retains
    // its own custody, expiry, resource policy and command fences after this IO.
    // OnceCell keeps verification attached to a caller: cancellation never
    // leaves a detached provider task running or a permanently pending entry.
    let verified = match tokio::time::timeout(
        timeout,
        flight.get_or_init(|| async {
            identity::verify_subject(auth, metadata, credential)
                .await
                .map(Arc::new)
        }),
    )
    .await
    {
        Ok(result) => result.clone(),
        Err(_) => {
            identity::verification_failure("bearer", "deadline", None);
            Err(ProviderVerificationError::Unavailable(
                "bearer authorization verification deadline exceeded".into(),
            ))
        }
    };
    if flight.initialized() {
        auth.bearer_verifications.release(&key, &flight).await;
    }
    verified
}

#[cfg(test)]
#[path = "bearer_authorization_tests.rs"]
mod tests;
