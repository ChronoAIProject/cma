//! Request-local cookie authority, fenced against concurrent token replacement.

use std::collections::HashMap;
use std::sync::{Arc, Weak};
use std::time::{Duration, SystemTime};

use layer2_core::{Authorization, StoreError, SubjectId};
use tokio::sync::{Mutex, OnceCell};

use super::{
    identity,
    identity_fallback::CredentialPurpose,
    permissions::PermissionEvidence,
    persistence::{DurableSubjectTokens, SecretDigest, SubjectAuthState},
    verification_error::ProviderVerificationError,
    AuthState, RequestAuth,
};

const PROVIDER_VERIFICATION_TIMEOUT: Duration = super::identity_retry::DEADLINE;
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

/// Live cookie verification with explicit refusal and no fallback. Browser
/// callers require sign-in on refusal; command/portal callers map it to revocation.
pub(super) async fn request_for_browser_cookie(
    auth: &AuthState,
    owner: &SubjectId,
    state: SubjectAuthState,
) -> Result<Option<RequestAuth>, StoreError> {
    verify_with_timeout(auth, owner, state, PROVIDER_VERIFICATION_TIMEOUT, false).await
}

pub(super) async fn request_for_session(
    auth: &AuthState,
    owner: &SubjectId,
    state: SubjectAuthState,
) -> Result<Option<RequestAuth>, StoreError> {
    verify_with_timeout(auth, owner, state, PROVIDER_VERIFICATION_TIMEOUT, true).await
}

async fn request_with_timeout(
    auth: &AuthState,
    owner: &SubjectId,
    state: SubjectAuthState,
    timeout: Duration,
) -> Result<RequestAuth, StoreError> {
    verify_with_timeout(auth, owner, state, timeout, false)
        .await?
        .ok_or_else(unavailable)
}

async fn verify_with_timeout(
    auth: &AuthState,
    owner: &SubjectId,
    state: SubjectAuthState,
    timeout: Duration,
    allow_fallback: bool,
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
    let token_hash = auth.identity_store.key(
        CredentialPurpose::BrowserCookie,
        tokens.access_token.expose(),
    );
    let epoch = auth.identity_store.epoch();
    // The whole retry sequence and its deadline belong to the shared flight.
    // Completed verification is never used to skip the next provider check.
    let led = std::sync::atomic::AtomicBool::new(false);
    let verified = match tokio::time::timeout(
        timeout,
        flight.get_or_init(|| {
            led.store(true, std::sync::atomic::Ordering::Relaxed);
            super::identity_retry::verify(
                auth,
                None,
                tokens.access_token.expose(),
                CredentialPurpose::BrowserCookie,
                Some(tokens.expires_at),
                Some(owner.as_str()),
                timeout,
            )
        }),
    )
    .await
    {
        Ok(result) => result.clone(),
        Err(_) => {
            auth.identity_store
                .invalidate_token(tokens.access_token.expose());
            Err(ProviderVerificationError::Unavailable(
                "identity caller deadline elapsed".into(),
            ))
        }
    };
    let source = if led.load(std::sync::atomic::Ordering::Relaxed) {
        "led"
    } else {
        "shared"
    };
    metrics::counter!("cma_cookie_identity_resolutions_total", "source" => source).increment(1);
    tracing::debug!(parent: None, event = "cookie_identity_resolution", source, count = 1u64,
        "Cookie identity verification resolved");
    if flight.initialized() {
        auth.cookie_verifications.release(&key, &flight).await;
    }
    let mut fallback_reason = None;
    let mut verified = match verified {
        Ok(verified) => verified,
        Err(ProviderVerificationError::Rejected(_)) => {
            // A refusal of a replaced token says nothing about the current login.
            // Keep races retryable and never remove shared durable credentials.
            require_current_custody(auth, &state).await?;
            return Ok(None);
        }
        Err(ProviderVerificationError::Transient(reason)) => {
            let cached = allow_fallback
                .then(|| auth.identity_store.get(&token_hash, &epoch))
                .flatten();
            let Some(verified) = cached else {
                ProviderVerificationError::Transient(reason).record_unavailable();
                return Err(super::identity_retry::unavailable());
            };
            fallback_reason = Some(reason);
            verified
        }
        Err(error @ ProviderVerificationError::Unavailable(_)) => {
            error.record_unavailable();
            return Err(unavailable());
        }
    };
    if &verified.subject != owner {
        auth.identity_store.invalidate(&token_hash);
        return Err(unavailable());
    }

    // Provider evidence describes exactly this token generation, not a later login.
    require_current_custody(auth, &state).await?;
    if let Some(reason) = fallback_reason {
        // Custody IO may overlap logout or another request's rejection.
        verified = auth
            .identity_store
            .get(&token_hash, &epoch)
            .ok_or_else(super::identity_retry::unavailable)?;
        if &verified.subject != owner {
            auth.identity_store.invalidate(&token_hash);
            return Err(unavailable());
        }
        super::identity_fallback::report(&token_hash, reason);
    }
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
