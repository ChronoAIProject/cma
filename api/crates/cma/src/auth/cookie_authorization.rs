//! Request-local cookie authority, fenced against concurrent token replacement.

use std::collections::HashMap;
use std::sync::{Arc, Weak};
use std::time::{Duration, SystemTime};

use layer2_core::{Authorization, StoreError, SubjectId};
use tokio::sync::{Mutex, OnceCell};

use super::{
    identity,
    permissions::PermissionEvidence,
    persistence::{DurableSubjectTokens, SecretDigest, SubjectAuthState},
    verification_error::ProviderVerificationError,
    AuthState, RequestAuth,
};

const PROVIDER_VERIFICATION_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_VERIFICATION_FLIGHTS: usize = 256;
type VerificationFlight =
    OnceCell<Result<Arc<identity::VerifiedSubject>, ProviderVerificationError>>;

#[derive(Clone, PartialEq, Eq, Hash)]
struct VerificationKey {
    subject: SubjectId,
    incarnation: String,
    generation: u64,
    access: SecretDigest,
    expires_at: SystemTime,
}

impl VerificationKey {
    fn new(state: &SubjectAuthState, tokens: &DurableSubjectTokens) -> Self {
        Self {
            subject: state.subject.clone(),
            incarnation: state.incarnation.clone(),
            generation: state.token_generation,
            access: SecretDigest::identify("cookie-verification", tokens.access_token.expose()),
            expires_at: tokens.expires_at,
        }
    }
}

#[derive(Default)]
pub(super) struct VerificationFlights {
    entries: Mutex<HashMap<VerificationKey, Weak<VerificationFlight>>>,
}

impl VerificationFlights {
    async fn acquire(&self, key: &VerificationKey) -> Arc<VerificationFlight> {
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

    async fn release(&self, key: &VerificationKey, flight: &Arc<VerificationFlight>) {
        let mut entries = self.entries.lock().await;
        if entries
            .get(key)
            .is_some_and(|current| current.ptr_eq(&Arc::downgrade(flight)))
        {
            entries.remove(key);
        }
    }
}

pub(super) async fn request_for_cookie(
    auth: &AuthState,
    owner: &SubjectId,
    state: SubjectAuthState,
) -> Result<RequestAuth, StoreError> {
    request_with_timeout(auth, owner, state, PROVIDER_VERIFICATION_TIMEOUT).await
}

/// A definitive rejection of the current browser credential requires sign-in.
/// Retained Agent work uses `request_for_cookie` and keeps its retryable outcome.
pub(super) async fn request_for_browser_cookie(
    auth: &AuthState,
    owner: &SubjectId,
    state: SubjectAuthState,
) -> Result<Option<RequestAuth>, StoreError> {
    verify_with_timeout(auth, owner, state, PROVIDER_VERIFICATION_TIMEOUT).await
}

async fn request_with_timeout(
    auth: &AuthState,
    owner: &SubjectId,
    state: SubjectAuthState,
    timeout: Duration,
) -> Result<RequestAuth, StoreError> {
    verify_with_timeout(auth, owner, state, timeout)
        .await?
        .ok_or_else(unavailable)
}

async fn verify_with_timeout(
    auth: &AuthState,
    owner: &SubjectId,
    state: SubjectAuthState,
    timeout: Duration,
) -> Result<Option<RequestAuth>, StoreError> {
    if &state.subject != owner {
        return Err(unavailable());
    }
    let Some(tokens) = state.tokens.as_ref().filter(|tokens| {
        tokens.expires_at > SystemTime::now() && !tokens.access_token.expose().trim().is_empty()
    }) else {
        return Ok(Some(RequestAuth {
            binding: None,
            credential_source: layer2_core::resource_access::CredentialSource::Retained,
            subject: state.subject,
            authorization: Authorization::none(),
            profile: state.profile,
            delegated_credential: None,
        }));
    };
    let key = VerificationKey::new(&state, tokens);
    let flight = auth.cookie_verifications.acquire(&key).await;
    // Only overlapping requests share provider work. No completed evidence is cached,
    // and dropping every caller cancels the provider future rather than detaching it.
    let verified = match tokio::time::timeout(
        timeout,
        flight.get_or_init(|| async {
            async {
                let metadata = auth
                    .metadata()
                    .await
                    .map_err(ProviderVerificationError::Unavailable)?;
                identity::verify_subject(auth, &metadata, tokens.access_token.expose()).await
            }
            .await
            .map(Arc::new)
        }),
    )
    .await
    {
        Ok(result) => result.clone(),
        Err(_) => {
            identity::verification_failure("cookie", "deadline", None);
            Err(ProviderVerificationError::Unavailable(
                "cookie verification deadline elapsed".into(),
            ))
        }
    };
    if flight.initialized() {
        auth.cookie_verifications.release(&key, &flight).await;
    }
    let verified = match verified {
        Ok(verified) => verified,
        Err(ProviderVerificationError::Rejected(_)) => {
            // A refusal of a replaced token says nothing about the current login.
            // Keep races retryable and never remove shared durable credentials.
            require_current_custody(auth, &state).await?;
            return Ok(None);
        }
        Err(ProviderVerificationError::Unavailable(_)) => return Err(unavailable()),
    };
    if &verified.subject != owner {
        return Err(unavailable());
    }

    // Provider evidence describes exactly this token generation, not a later login.
    require_current_custody(auth, &state).await?;
    let (authorization, delegated_credential) = match &verified.permissions {
        PermissionEvidence::Active(permissions) => (
            crate::permissions::capabilities_from_permissions(permissions.iter().cloned()),
            Some(tokens.access_token.expose().to_owned()),
        ),
        PermissionEvidence::Inactive => (Authorization::none(), None),
    };
    Ok(Some(RequestAuth {
        binding: None,
        credential_source: layer2_core::resource_access::CredentialSource::Retained,
        subject: verified.subject.clone(),
        authorization,
        profile: verified.profile.clone(),
        delegated_credential,
    }))
}

/// Recheck the exact selected credential after any subsequent authorization IO.
/// An unchanged sign-in alone is insufficient: replacement or removal of the
/// verified access token must also invalidate evidence obtained using it.
pub(super) async fn require_current_custody(
    auth: &AuthState,
    state: &SubjectAuthState,
) -> Result<(), StoreError> {
    let tokens = state.tokens.as_ref().ok_or_else(unavailable)?;
    let current = auth
        .store
        .subject(&state.subject)
        .await?
        .ok_or_else(unavailable)?;
    let unchanged = current.subject == state.subject
        && current.incarnation == state.incarnation
        && current.token_generation == state.token_generation
        && current.tokens.as_ref().is_some_and(|current_tokens| {
            current_tokens.access_token == tokens.access_token
                && current_tokens.expires_at > SystemTime::now()
        });
    if !unchanged {
        identity::verification_failure("cookie", "authority_changed", None);
        return Err(unavailable());
    }
    Ok(())
}

fn unavailable() -> StoreError {
    StoreError::Unavailable("cookie authorization could not be verified".into())
}

#[cfg(test)]
#[path = "cookie_authorization_tests.rs"]
mod tests;
