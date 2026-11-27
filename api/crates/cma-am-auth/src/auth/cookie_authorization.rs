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
    verification_error::{ProviderVerificationError, VerificationCause, VerificationFailure},
    AuthState, RequestAuth,
};

pub const PROVIDER_VERIFICATION_TIMEOUT: Duration = super::identity_retry::DEADLINE;
pub const MAX_VERIFICATION_FLIGHTS: usize = 256;
pub type VerificationFlight = OnceCell<Result<Arc<identity::VerifiedSubject>, VerificationFailure>>;

pub(super) struct BrowserVerification {
    pub(super) key: VerificationKey,
    pub(super) evidence: Arc<identity::VerifiedSubject>,
}

#[derive(Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerificationKey {
    subject: SubjectId,
    incarnation: String,
    pub generation: u64,
    access: [u8; 32],
    pub(super) expires_at: SystemTime,
}

impl VerificationKey {
    pub fn new(state: &SubjectAuthState, tokens: &DurableSubjectTokens) -> Self {
        Self {
            subject: state.subject.clone(),
            incarnation: state.incarnation.clone(),
            generation: state.token_generation,
            access: *SecretDigest::identify("cookie-verification", tokens.access_token.expose())
                .as_bytes(),
            expires_at: tokens.expires_at,
        }
    }
}

#[derive(Default)]
pub struct VerificationFlights {
    entries: Mutex<HashMap<VerificationKey, Weak<VerificationFlight>>>,
}

impl VerificationFlights {
    #[cfg(feature = "test-support")]
    pub async fn fixture_entries(
        &self,
    ) -> tokio::sync::MutexGuard<'_, HashMap<VerificationKey, Weak<VerificationFlight>>> {
        self.entries.lock().await
    }

    pub async fn acquire(&self, key: &VerificationKey) -> Arc<VerificationFlight> {
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

    pub async fn release(&self, key: &VerificationKey, flight: &Arc<VerificationFlight>) {
        let mut entries = self.entries.lock().await;
        if entries
            .get(key)
            .is_some_and(|current| current.ptr_eq(&Arc::downgrade(flight)))
        {
            entries.remove(key);
        }
    }
}

pub async fn request_for_cookie(
    auth: &AuthState,
    owner: &SubjectId,
    state: SubjectAuthState,
) -> Result<RequestAuth, StoreError> {
    request_with_timeout(auth, owner, state, PROVIDER_VERIFICATION_TIMEOUT).await
}

/// Live cookie verification with explicit refusal and no fallback. Browser
/// callers require sign-in on refusal; command/portal callers map it to revocation.
pub async fn request_for_browser_cookie(
    auth: &AuthState,
    owner: &SubjectId,
    state: SubjectAuthState,
) -> Result<Option<RequestAuth>, StoreError> {
    verify_with_timeout(auth, owner, state, PROVIDER_VERIFICATION_TIMEOUT).await
}

pub(super) async fn request_for_browser_cookie_with_receipt(
    auth: &AuthState,
    owner: &SubjectId,
    state: SubjectAuthState,
    receipt: Option<&mut Option<BrowserVerification>>,
) -> Result<Option<RequestAuth>, StoreError> {
    verify(auth, owner, state, PROVIDER_VERIFICATION_TIMEOUT, receipt).await
}

pub async fn request_with_timeout(
    auth: &AuthState,
    owner: &SubjectId,
    state: SubjectAuthState,
    timeout: Duration,
) -> Result<RequestAuth, StoreError> {
    verify_with_timeout(auth, owner, state, timeout)
        .await?
        .ok_or_else(unavailable)
}

pub async fn verify_with_timeout(
    auth: &AuthState,
    owner: &SubjectId,
    state: SubjectAuthState,
    timeout: Duration,
) -> Result<Option<RequestAuth>, StoreError> {
    verify(auth, owner, state, timeout, None).await
}

async fn verify(
    auth: &AuthState,
    owner: &SubjectId,
    state: SubjectAuthState,
    timeout: Duration,
    receipt: Option<&mut Option<BrowserVerification>>,
) -> Result<Option<RequestAuth>, StoreError> {
    if &state.subject != owner {
        return Err(unavailable());
    }
    let Some(tokens) = state.usable_tokens(SystemTime::now()) else {
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
    // The whole retry sequence and its deadline belong to the shared flight.
    // Only callers admitted while it is unfinished can share this exact result.
    let led = std::sync::atomic::AtomicBool::new(false);
    let verified = match tokio::time::timeout(
        timeout,
        flight.get_or_init(|| {
            led.store(true, std::sync::atomic::Ordering::Relaxed);
            super::identity_retry::verify_detailed(
                auth,
                None,
                tokens.access_token.expose(),
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
            Err(VerificationFailure::new(
                error,
                VerificationCause::CallerDeadline,
            ))
        }
    };
    let source = if led.load(std::sync::atomic::Ordering::Relaxed) {
        "led"
    } else {
        "shared"
    };
    metrics::counter!("cma_cookie_identity_resolutions_total", "source" => source).increment(1);
    super::identity_observations::resolution(source);
    tracing::debug!(parent: None, event = "cookie_identity_resolution", source, count = 1u64,
        "Cookie identity verification resolved");
    if flight.initialized() {
        auth.cookie_verifications.release(&key, &flight).await;
    }
    let verified = match verified
        .inspect_err(VerificationFailure::record_local_cause)
        .map_err(|failure| failure.error)
    {
        Ok(verified) => verified,
        Err(ProviderVerificationError::Rejected(_)) => {
            // A refusal of a replaced token says nothing about the current login.
            // Keep races retryable and never remove shared durable credentials.
            require_current_custody(auth, &state).await?;
            return Ok(None);
        }
        Err(
            error @ (ProviderVerificationError::RateLimited(_)
            | ProviderVerificationError::Transient(_)),
        ) => {
            error.record_unavailable();
            return Err(super::identity_retry::unavailable());
        }
        Err(error @ ProviderVerificationError::Unavailable(_)) => {
            error.record_unavailable();
            return Err(unavailable());
        }
    };
    if &verified.subject != owner {
        auth.identity_store
            .invalidate_token(tokens.access_token.expose());
        return Err(unavailable());
    }

    // Provider evidence describes exactly this token generation, not a later login.
    require_current_custody(auth, &state).await?;
    if !super::identity_lifetime::current(auth, &verified) {
        return Err(unavailable());
    }
    super::identity_lifetime::retain(&verified);
    let (authorization, delegated_credential) = match &verified.permissions {
        PermissionEvidence::Active(permissions) => (
            crate::permissions::capabilities_from_permissions(permissions.iter().cloned()),
            Some(tokens.access_token.expose().to_owned()),
        ),
        PermissionEvidence::Inactive => (Authorization::none(), None),
    };
    if let Some(receipt) = receipt.filter(|_| delegated_credential.is_some()) {
        *receipt = Some(BrowserVerification {
            key,
            evidence: verified.clone(),
        });
    }
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
    current_custody(auth, state)
        .await
        .map_err(|error| match error {
            CustodyError::Changed => unavailable(),
            CustodyError::Store(error) => error,
        })
}

pub(super) enum CustodyError {
    Changed,
    Store(StoreError),
}

/// Keep confirmed credential changes distinct from an unreadable auth store.
pub(super) async fn current_custody(
    auth: &AuthState,
    state: &SubjectAuthState,
) -> Result<(), CustodyError> {
    let tokens = state.tokens.as_ref().ok_or(CustodyError::Changed)?;
    let current = auth
        .store
        .subject(&state.subject)
        .await
        .map_err(CustodyError::Store)?
        .ok_or(CustodyError::Changed)?;
    let unchanged = current.subject == state.subject
        && current.incarnation == state.incarnation
        && current.token_generation == state.token_generation
        && current.tokens.as_ref().is_some_and(|current_tokens| {
            current_tokens.access_token == tokens.access_token
                && current_tokens.expires_at > SystemTime::now()
        });
    if !unchanged {
        identity::verification_failure("cookie", "authority_changed", None);
        return Err(CustodyError::Changed);
    }
    Ok(())
}

fn unavailable() -> StoreError {
    StoreError::Unavailable("cookie authorization could not be verified".into())
}
