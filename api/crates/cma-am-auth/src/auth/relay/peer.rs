//! Short-lived ingress attestation, authenticated by the Manager relay.
use super::*;
use crate::auth::RequestAuth;
use layer2_core::{Authorization, SubjectId};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::time::Instant;

// Relay signatures carry integer Unix seconds. A nine-second ceiling stays
// inside the existing ten-second envelope even after rounding.
const MAX_AGE: Duration = Duration::from_secs(9);

/// The relay must authenticate the sender, audience and exact request before
/// supplying these claims to the browser authentication middleware.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeerBrowserClaim {
    version: u8,
    subject: SubjectId,
    authorization: Authorization,
    login: [u8; 32],
    key: cookie_authorization::VerificationKey,
    target: [u8; 32],
    expires_at: SystemTime,
}

/// A peer claim that this Manager has checked against current durable custody.
#[derive(Clone)]
pub struct PeerBrowserReceipt {
    claim: Arc<PeerBrowserClaim>,
    deadline: Instant,
    credential: SecretDigest,
}

impl BrowserRelayProof {
    pub async fn peer_claim(
        &self,
        auth: &AuthState,
        method: &Method,
        uri: &Uri,
    ) -> Result<PeerBrowserClaim, StatusCode> {
        if !self.matches(method, uri) {
            return Err(StatusCode::UNAUTHORIZED);
        }
        let subject = &self.verification.subject;
        let login_expiry = login_expiry(auth, &self.login, subject).await?;
        let now = SystemTime::now();
        let expiry = identity_lifetime::relay_expiry(&self.verification, now)
            .filter(|_| identity_lifetime::current(auth, &self.verification))
            .ok_or(StatusCode::UNAUTHORIZED)?;
        let expires_at = expiry
            .min(login_expiry)
            .min(self.key.expires_at)
            .min(now + MAX_AGE);
        if expires_at <= now {
            return Err(StatusCode::UNAUTHORIZED);
        }
        let super::super::permissions::PermissionEvidence::Active(permissions) =
            &self.verification.permissions
        else {
            return Err(StatusCode::UNAUTHORIZED);
        };
        Ok(PeerBrowserClaim {
            version: 1,
            subject: subject.clone(),
            authorization: crate::permissions::capabilities_from_permissions(
                permissions.iter().cloned(),
            ),
            login: *self.login.as_bytes(),
            key: self.key.clone(),
            target: *BrowserRelayProof::target_digest(uri).as_bytes(),
            expires_at,
        })
    }
}

impl PeerBrowserClaim {
    pub(in crate::auth) async fn resolve(
        self,
        auth: &AuthState,
        headers: &HeaderMap,
        method: &Method,
        uri: &Uri,
    ) -> Result<(RequestAuth, PeerBrowserReceipt), StatusCode> {
        if self.version != 1
            || !matches!(auth.mode, AuthMode::Provider { .. })
            || !self.matches(method, uri)
            || !session_routes::request_cookie(headers, super::super::AUTH_COOKIE).is_some_and(
                |cookie| {
                    SecretDigest::identify(super::super::AUTH_COOKIE_DOMAIN, cookie).as_bytes()
                        == &self.login
                },
            )
        {
            return Err(StatusCode::UNAUTHORIZED);
        }
        let remaining = self
            .expires_at
            .duration_since(SystemTime::now())
            .ok()
            .filter(|remaining| !remaining.is_zero() && *remaining <= MAX_AGE)
            .ok_or(StatusCode::UNAUTHORIZED)?;
        let deadline = Instant::now()
            .checked_add(remaining)
            .ok_or(StatusCode::UNAUTHORIZED)?;
        let state = self.subject_custody(auth).await?;
        login_expiry(auth, &SecretDigest::from_bytes(self.login), &self.subject).await?;
        let tokens = state.tokens.as_ref().ok_or(StatusCode::UNAUTHORIZED)?;
        let credential = tokens.access_token.expose();
        let resolved = RequestAuth {
            binding: None,
            credential_source: CredentialSource::Retained,
            subject: self.subject.clone(),
            authorization: self.authorization.clone(),
            profile: state.profile,
            delegated_credential: Some(credential.to_owned()),
        };
        let receipt = PeerBrowserReceipt {
            claim: Arc::new(self),
            deadline,
            credential: SecretDigest::identify("peer-browser-credential", credential),
        };
        receipt.require_time()?;
        Ok((resolved, receipt))
    }

    fn matches(&self, method: &Method, uri: &Uri) -> bool {
        *method == Method::GET && BrowserRelayProof::target_digest(uri).as_bytes() == &self.target
    }

    async fn subject_custody(&self, auth: &AuthState) -> Result<SubjectAuthState, StatusCode> {
        let state = auth
            .store
            .subject(&self.subject)
            .await
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
            .ok_or(StatusCode::UNAUTHORIZED)?;
        if state.subject != self.subject
            || !state
                .usable_tokens(SystemTime::now())
                .is_some_and(|tokens| {
                    cookie_authorization::VerificationKey::new(&state, tokens) == self.key
                })
        {
            return Err(StatusCode::UNAUTHORIZED);
        }
        Ok(state)
    }
}

impl PeerBrowserReceipt {
    pub fn matches(&self, method: &Method, uri: &Uri) -> bool {
        self.claim.matches(method, uri)
    }

    pub(super) fn matches_caller(&self, context: &CallerContext, credential: Option<&str>) -> bool {
        self.require_time().is_ok()
            && context.credential_source == CredentialSource::Retained
            && self.claim.subject == context.caller.subject
            && self.claim.authorization == context.caller.authorization
            && credential.is_some_and(|value| {
                SecretDigest::identify("peer-browser-credential", value) == self.credential
            })
    }

    pub(super) async fn current(&self, auth: &AuthState) -> Result<(), StatusCode> {
        self.require_time()?;
        self.claim.subject_custody(auth).await?;
        login_expiry(
            auth,
            &SecretDigest::from_bytes(self.claim.login),
            &self.claim.subject,
        )
        .await?;
        self.require_time()
    }

    pub(in crate::auth) fn require_time(&self) -> Result<(), StatusCode> {
        if self.deadline <= Instant::now() || self.claim.expires_at <= SystemTime::now() {
            return Err(StatusCode::UNAUTHORIZED);
        }
        Ok(())
    }

    pub(in crate::auth) fn deadline(&self) -> Instant {
        self.deadline
    }
}
