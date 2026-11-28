//! Incremental service authorization for an existing personal browser session.

use std::{
    collections::BTreeSet,
    future::Future,
    sync::Arc,
    time::{Duration, SystemTime},
};

use axum::{
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use layer2_core::{
    resource_access::CredentialSource, sandbox::RetirementAuthorityVersion, ServiceId, StoreError,
    SubjectId,
};
use serde::{Deserialize, Serialize};
use url::Url;

use super::{
    flow::{code_challenge, return_target::app_return_target},
    persistence::{LoginAttempt, SecretDigest, SecretValue},
    session_routes::request_cookie,
    token::random_token,
    AuthMode, AuthState, AUTH_COOKIE, AUTH_COOKIE_DOMAIN, LOGIN_ATTEMPT_LIFETIME,
};

mod nyxid;
mod routes;
pub use routes::router;
pub const CALLBACK_PATH: &str = "/auth/service-authorization/callback";
pub const ATTEMPT_DOMAIN: &str = "service-authorization-attempt";
const PROVIDER_TIMEOUT: Duration = Duration::from_secs(10);

pub use crate::RecoveryIntent;

pub struct VerifiedAccountGrant {
    pub subject: SubjectId,
    pub status: GrantStatus,
    pub management_credential_unsupported: bool,
    pub management_url: Option<Url>,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GrantStatus {
    Unknown,
    Authorized,
    Partial,
    Expired,
    NotGranted,
    #[cfg(any(test, feature = "test-support"))]
    Revoked,
}

pub struct ConsentIdentity {
    pub subject: SubjectId,
    pub issuer: String,
    pub missing: BTreeSet<ServiceId>,
}

#[async_trait::async_trait]
pub trait RetirementWake: Send + Sync {
    async fn wake_retirement(&self, owner: &SubjectId) -> Result<(), StoreError>;
}

/// Only a reviewed provider integration may implement a grant read or issue consent.
/// None means the provider could not attest to the requested scope. It carries
/// no information about the presence or absence of a grant.
#[async_trait::async_trait]
pub trait GrantAuthority: Send + Sync {
    fn issuer(&self) -> Option<&Url>;

    fn authorization_origin(&self) -> Option<&Url> {
        self.issuer()
    }

    /// Browser management is a configured provider destination, independent
    /// of whether the current credential can read a grant.
    fn management_url(&self) -> Option<Url> {
        None
    }

    async fn account(
        &self,
        auth: &AuthState,
        subject: &SubjectId,
    ) -> Result<Option<VerifiedAccountGrant>, StoreError>;

    async fn retirement_authority_version(
        &self,
        _auth: &AuthState,
        _token: &SecretValue,
        _required: &BTreeSet<ServiceId>,
    ) -> Result<Option<RetirementAuthorityVersion>, StoreError> {
        Ok(None)
    }

    /// Authoritative, complete missing subset for this issuer, subject and set.
    async fn missing(
        &self,
        auth: &AuthState,
        intent: &RecoveryIntent,
    ) -> Result<Option<BTreeSet<ServiceId>>, StoreError>;

    async fn consent_url(
        &self,
        auth: &AuthState,
        intent: &RecoveryIntent,
        state: &str,
        s256_challenge: &str,
    ) -> Result<Option<Url>, StoreError>;

    /// Verify the code, issuer, subject, redirect URI and S256 proof. Recovery
    /// never replaces identity credentials belonging to the original session.
    async fn redeem(
        &self,
        auth: &AuthState,
        intent: &RecoveryIntent,
        code: &str,
        verifier: &SecretValue,
    ) -> Result<Option<ConsentIdentity>, StoreError>;
}

struct Unsupported;

#[async_trait::async_trait]
impl GrantAuthority for Unsupported {
    fn issuer(&self) -> Option<&Url> {
        None
    }

    async fn account(
        &self,
        _: &AuthState,
        _: &SubjectId,
    ) -> Result<Option<VerifiedAccountGrant>, StoreError> {
        Ok(None)
    }

    async fn missing(
        &self,
        _: &AuthState,
        _: &RecoveryIntent,
    ) -> Result<Option<BTreeSet<ServiceId>>, StoreError> {
        Ok(None)
    }

    async fn consent_url(
        &self,
        _: &AuthState,
        _: &RecoveryIntent,
        _: &str,
        _: &str,
    ) -> Result<Option<Url>, StoreError> {
        Ok(None)
    }

    async fn redeem(
        &self,
        _: &AuthState,
        _: &RecoveryIntent,
        _: &str,
        _: &SecretValue,
    ) -> Result<Option<ConsentIdentity>, StoreError> {
        Ok(None)
    }
}

impl AuthState {
    pub fn with_retirement_wake(mut self: Arc<Self>, wake: Arc<dyn RetirementWake>) -> Arc<Self> {
        Arc::get_mut(&mut self)
            .expect("configure before sharing")
            .retirement_wake = Some(wake);
        self
    }

    pub(super) async fn wake_retirement(&self, owner: &SubjectId) {
        let Some(wake) = &self.retirement_wake else {
            return;
        };
        if !matches!(
            tokio::time::timeout(Duration::from_secs(3), wake.wake_retirement(owner)).await,
            Ok(Ok(()))
        ) {
            tracing::warn!("Could not wake parked retirement after an owner authorization update");
        }
    }

    pub fn with_service_authorization(
        mut self: Arc<Self>,
        enabled: bool,
        config: Option<&crate::service_config::ServiceAuthorizationConfig>,
    ) -> Arc<Self> {
        if enabled && matches!(self.mode, AuthMode::Provider { .. }) {
            let provider: Arc<dyn GrantAuthority> = config
                .and_then(|config| nyxid::Nyxid::new(&self, config))
                .map(|provider| Arc::new(provider) as Arc<dyn GrantAuthority>)
                .unwrap_or_else(|| Arc::new(Unsupported));
            Arc::get_mut(&mut self)
                .expect("configure before sharing")
                .service_authorization = Some(provider);
        }
        self
    }

    /// Bind the provider's consent evidence to the retained credential supplied by the worker.
    pub async fn retirement_authority_version(
        &self,
        owner: &SubjectId,
        token: &SecretValue,
        required: &BTreeSet<ServiceId>,
    ) -> Result<Option<RetirementAuthorityVersion>, StoreError> {
        let Some(provider) = self.service_authorization.as_ref() else {
            return Ok(None);
        };
        let matches = |state: &super::persistence::SubjectAuthState| {
            &state.subject == owner
                && state
                    .usable_tokens(SystemTime::now())
                    .is_some_and(|current| current.access_token.expose() == token.expose())
        };
        if self
            .standing
            .authoritative(owner)
            .await
            .map_err(|_| StoreError::Unavailable("owner account standing is unavailable".into()))?
        {
            return Ok(None);
        }
        let Some(before) = self.store.subject(owner).await?.filter(matches) else {
            return Ok(None);
        };
        let observed = provider
            .retirement_authority_version(self, token, required)
            .await?;
        if !self
            .store
            .subject(owner)
            .await?
            .as_ref()
            .is_some_and(|after| {
                matches(after)
                    && after.incarnation == before.incarnation
                    && after.token_generation == before.token_generation
            })
        {
            return Ok(None);
        }
        Ok(observed.map(|version| RetirementAuthorityVersion {
            token_generation: Some(before.token_generation),
            ..version
        }))
    }

    pub async fn retirement_token_generation(&self, owner: &SubjectId) -> Result<u64, StoreError> {
        Ok(self
            .store
            .subject(owner)
            .await?
            .map_or(0, |state| state.token_generation))
    }

    pub fn service_authorization_enabled(&self) -> bool {
        self.service_authorization.is_some()
    }

    pub fn service_authorization_recovery_available(&self) -> bool {
        self.service_authorization
            .as_ref()
            .and_then(|grant| grant.issuer())
            .is_some()
    }
}

#[derive(Debug, Clone, Copy)]
pub enum RecoveryError {
    Disabled,
    Forbidden,
    Invalid,
    Changed,
    Unknown,
}

impl IntoResponse for RecoveryError {
    fn into_response(self) -> Response {
        let (status, code) = match self {
            Self::Disabled => (StatusCode::NOT_FOUND, "disabled"),
            Self::Forbidden => (StatusCode::FORBIDDEN, "account_changed"),
            Self::Invalid => (StatusCode::BAD_REQUEST, "invalid_request"),
            Self::Changed => (StatusCode::CONFLICT, "authorization_changed"),
            Self::Unknown => (StatusCode::SERVICE_UNAVAILABLE, "authorization_unknown"),
        };
        tracing::debug!(code, "Service authorization request did not complete");
        (
            status,
            [(header::CACHE_CONTROL, "private, no-store")],
            Json(serde_json::json!({"code": code})),
        )
            .into_response()
    }
}

fn browser_key(headers: &HeaderMap) -> Result<SecretDigest, RecoveryError> {
    let value = request_cookie(headers, AUTH_COOKIE).ok_or(RecoveryError::Forbidden)?;
    Ok(SecretDigest::identify(AUTH_COOKIE_DOMAIN, value))
}

async fn live_session(
    auth: &AuthState,
    key: &SecretDigest,
    subject: &SubjectId,
) -> Result<(), RecoveryError> {
    let login = auth
        .store
        .browser_login(key)
        .await
        .map_err(|_| RecoveryError::Unknown)?;
    if !login.is_some_and(|login| &login.subject == subject && login.expires_at > SystemTime::now())
        || auth
            .standing
            .erasing(subject)
            .await
            .map_err(|_| RecoveryError::Unknown)?
    {
        return Err(RecoveryError::Forbidden);
    }
    Ok(())
}

pub fn start_allowed(
    auth: &AuthState,
    headers: &HeaderMap,
    source: CredentialSource,
) -> Result<(), RecoveryError> {
    if !auth.service_authorization_enabled() {
        return Err(RecoveryError::Disabled);
    }
    if source != CredentialSource::Retained || !auth.permits_browser_origin(headers) {
        return Err(RecoveryError::Forbidden);
    }
    Ok(())
}

fn return_target(value: &str) -> Result<String, RecoveryError> {
    app_return_target(value)
        .ok()
        .filter(|path| !path.starts_with("/auth/"))
        .ok_or(RecoveryError::Invalid)
}

fn login_path(return_to: &str) -> String {
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("return_to", return_to)
        .finish();
    format!("/auth/login?{query}")
}

fn provider_url(auth: &AuthState, url: &Url) -> bool {
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && Url::parse(&auth.nyxid_public_url).is_ok_and(|origin| origin.origin() == url.origin())
}

async fn provider_evidence<T>(
    operation: &'static str,
    request: impl Future<Output = Result<Option<T>, StoreError>>,
) -> Option<T> {
    match tokio::time::timeout(PROVIDER_TIMEOUT, request).await {
        Ok(Ok(evidence)) => evidence,
        Ok(Err(_)) => {
            tracing::warn!(
                operation,
                "Service authorization authority could not supply evidence"
            );
            None
        }
        Err(_) => {
            tracing::warn!(operation, "Service authorization authority timed out");
            None
        }
    }
}

pub async fn begin(
    auth: &AuthState,
    headers: &HeaderMap,
    subject: &SubjectId,
    required: BTreeSet<ServiceId>,
    return_to: &str,
) -> Result<Response, RecoveryError> {
    let provider = auth
        .service_authorization
        .as_ref()
        .ok_or(RecoveryError::Disabled)?;
    let issuer = provider.issuer().ok_or(RecoveryError::Unknown)?;
    let return_to = return_target(return_to)?;
    let key = browser_key(headers)?;
    live_session(auth, &key, subject).await?;

    // The allowed Origin owns the host-only cookie, including on a configured alias.
    let origin = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .ok_or(RecoveryError::Forbidden)?;
    let mut intent = RecoveryIntent {
        subject: subject.clone(),
        issuer: issuer.as_str().into(),
        required,
        requested: BTreeSet::new(),
        redirect_uri: format!("{origin}{CALLBACK_PATH}"),
        return_to,
    };
    if intent.required.is_empty() {
        return Err(RecoveryError::Changed);
    }
    let missing = provider_evidence("required_grant", provider.missing(auth, &intent))
        .await
        .filter(|missing| missing.is_subset(&intent.required))
        .ok_or(RecoveryError::Unknown)?;
    if missing.is_empty() {
        return Err(RecoveryError::Changed);
    }
    intent.requested = missing;
    let state = random_token();
    let verifier = random_token();
    let challenge = code_challenge(&verifier);
    let url = provider_evidence(
        "incremental_consent",
        provider.consent_url(auth, &intent, &state, &challenge),
    )
    .await
    .filter(|url| {
        url.scheme() == "https"
            && url.username().is_empty()
            && url.password().is_none()
            && provider
                .authorization_origin()
                .is_some_and(|origin| origin.origin() == url.origin())
    })
    .ok_or(RecoveryError::Unknown)?;
    live_session(auth, &key, subject).await?;
    let now = SystemTime::now();
    auth.store
        .put_login_attempt(LoginAttempt {
            id: SecretDigest::identify(ATTEMPT_DOMAIN, &state),
            browser_binding: key,
            verifier: SecretValue::new(verifier),
            return_to: None,
            actor_link_intent: None,
            service_authorization: Some(intent),
            silent: false,
            consent_requested: false,
            custody_reserved: false,
            started_at: Some(now),
            expires_at: now + LOGIN_ATTEMPT_LIFETIME,
        })
        .await
        .map_err(|_| RecoveryError::Unknown)?;
    tracing::debug!("Service authorization attempt opened");
    Ok((
        [(header::CACHE_CONTROL, "private, no-store")],
        Json(serde_json::json!({
            "owner": subject.as_str(),
            "hosted_url": url.as_str(),
        })),
    )
        .into_response())
}
