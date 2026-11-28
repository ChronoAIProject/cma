//! Fresh exact-caller proof before public owner lookup. No completed-result cache.
use super::{
    cookie_authorization, identity::VerifiedSubject, identity_lifetime, persistence::*,
    session_routes, AuthMode, AuthState,
};
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use layer2_core::resource_access::{CallerContext, CredentialSource};
use std::{sync::Arc, time::SystemTime};

mod peer;
pub use peer::{PeerBrowserClaim, PeerBrowserReceipt};

pub struct RelayCaller {
    auth: Arc<AuthState>,
    context: CallerContext,
    browser: Option<(SecretDigest, SubjectAuthState)>,
    verification: Option<Arc<VerifiedSubject>>,
    peer: Option<PeerBrowserReceipt>,
}

#[derive(Clone, Copy)]
pub struct ReuseFilePreviewIdentity;

/// A live browser verification captured by the ambient middleware for this request.
#[derive(Clone)]
pub struct BrowserRelayProof {
    login: SecretDigest,
    key: cookie_authorization::VerificationKey,
    verification: Arc<VerifiedSubject>,
    target: Option<SecretDigest>,
}

impl BrowserRelayProof {
    pub(super) fn capture(
        login: SecretDigest,
        receipt: cookie_authorization::BrowserVerification,
    ) -> Self {
        Self {
            login,
            key: receipt.key,
            verification: receipt.evidence,
            target: None,
        }
    }

    pub(super) fn bind(mut self, uri: &Uri) -> Self {
        self.target = Some(Self::target_digest(uri));
        self
    }

    pub fn matches(&self, method: &Method, uri: &Uri) -> bool {
        *method == Method::GET && self.target.as_ref() == Some(&Self::target_digest(uri))
    }

    fn target_digest(uri: &Uri) -> SecretDigest {
        SecretDigest::identify("file-preview-relay-target", &uri.to_string())
    }
}

impl RelayCaller {
    pub async fn verify(
        auth: Arc<AuthState>,
        headers: &HeaderMap,
        context: &CallerContext,
        credential: Option<&str>,
        browser_proof: Option<&BrowserRelayProof>,
        peer: Option<&PeerBrowserReceipt>,
    ) -> Result<Self, StatusCode> {
        valid(context)?;
        if let Some(peer) = peer {
            if browser_proof.is_some() || !peer.matches_caller(context, credential) {
                return Err(StatusCode::UNAUTHORIZED);
            }
            return Ok(Self {
                auth,
                context: context.clone(),
                browser: None,
                verification: None,
                peer: Some(peer.clone()),
            });
        }
        if matches!(
            context.credential_source,
            CredentialSource::RequestBoundRead { .. }
        ) {
            if browser_proof.is_some() {
                return Err(StatusCode::UNAUTHORIZED);
            }
            // Ordinary middleware already verified the exact method/path/query proof.
            // Its deadline remains attached; ambient credentials never replace it.
            return Ok(Self {
                auth,
                context: context.clone(),
                browser: None,
                verification: None,
                peer: None,
            });
        }
        let browser = if matches!(auth.mode, AuthMode::Provider { .. })
            && context.credential_source == CredentialSource::Retained
        {
            let cookie = session_routes::request_cookie(headers, super::AUTH_COOKIE)
                .ok_or(StatusCode::UNAUTHORIZED)?;
            let login = SecretDigest::identify(super::AUTH_COOKIE_DOMAIN, cookie);
            require_login(&auth, &login, context).await?;
            let state = auth
                .store
                .subject(&context.caller.subject)
                .await
                .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
                .ok_or(StatusCode::UNAUTHORIZED)?;
            if !state.tokens.as_ref().is_some_and(|tokens| {
                Some(tokens.access_token.expose()) == credential
                    && tokens.expires_at > SystemTime::now()
            }) {
                return Err(StatusCode::UNAUTHORIZED);
            }
            Some((login, state))
        } else {
            None
        };
        if let Some(proof) = browser_proof {
            let (login, state) = browser.as_ref().ok_or(StatusCode::UNAUTHORIZED)?;
            let tokens = state.tokens.as_ref().ok_or(StatusCode::UNAUTHORIZED)?;
            let super::permissions::PermissionEvidence::Active(permissions) =
                &proof.verification.permissions
            else {
                return Err(StatusCode::UNAUTHORIZED);
            };
            if login != &proof.login
                || cookie_authorization::VerificationKey::new(state, tokens) != proof.key
                || proof.verification.subject != context.caller.subject
                || crate::permissions::capabilities_from_permissions(permissions.iter().cloned())
                    != context.caller.authorization
                || !identity_lifetime::current(&auth, &proof.verification)
            {
                return Err(StatusCode::UNAUTHORIZED);
            }
            let caller = Self {
                auth,
                context: context.clone(),
                browser,
                verification: Some(proof.verification.clone()),
                peer: None,
            };
            caller.current().await?;
            tracing::debug!(
                event = "file_preview_ingress_identity_reused",
                "File preview reused its live ingress identity verification"
            );
            return Ok(caller);
        }
        let resolved = match &browser {
            Some((_, state)) => {
                cookie_authorization::request_for_browser_cookie(
                    &auth,
                    &context.caller.subject,
                    state.clone(),
                )
                .await
            }
            None => auth.resolve_request(headers).await,
        }
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
        .ok_or(StatusCode::UNAUTHORIZED)?;
        if resolved.subject != context.caller.subject
            || resolved.authorization != context.caller.authorization
            || resolved.credential_source != context.credential_source
            || resolved.binding.is_some()
            || resolved.delegated_credential.as_deref() != credential
        {
            return Err(StatusCode::UNAUTHORIZED);
        }
        let proof = Self {
            auth,
            context: context.clone(),
            browser,
            verification: None,
            peer: None,
        };
        proof.current().await?;
        Ok(proof)
    }

    /// Policy IO may overlap logout, credential replacement or expiry.
    pub async fn current(&self) -> Result<(), StatusCode> {
        valid(&self.context)?;
        if let Some(peer) = &self.peer {
            return peer.current(&self.auth).await;
        }
        if let Some((login, state)) = &self.browser {
            cookie_authorization::current_custody(&self.auth, state)
                .await
                .map_err(|error| match error {
                    cookie_authorization::CustodyError::Changed => StatusCode::UNAUTHORIZED,
                    cookie_authorization::CustodyError::Store(_) => StatusCode::SERVICE_UNAVAILABLE,
                })?;
            require_login(&self.auth, login, &self.context).await?;
        }
        if self
            .verification
            .as_ref()
            .is_some_and(|receipt| !identity_lifetime::current(&self.auth, receipt))
        {
            return Err(StatusCode::UNAUTHORIZED);
        }
        valid(&self.context)
    }
}

fn valid(context: &CallerContext) -> Result<(), StatusCode> {
    if !context.validate_principal()
        || context
            .credential_deadline()
            .is_some_and(|deadline| deadline <= tokio::time::Instant::now())
    {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(())
}

async fn require_login(
    auth: &AuthState,
    login: &SecretDigest,
    context: &CallerContext,
) -> Result<(), StatusCode> {
    login_expiry(auth, login, &context.caller.subject)
        .await
        .map(|_| ())
}

async fn login_expiry(
    auth: &AuthState,
    login: &SecretDigest,
    subject: &layer2_core::SubjectId,
) -> Result<SystemTime, StatusCode> {
    auth.store
        .browser_login(login)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
        .filter(|current| {
            &current.id == login
                && &current.subject == subject
                && current.expires_at > SystemTime::now()
        })
        .map(|current| current.expires_at)
        .ok_or(StatusCode::UNAUTHORIZED)
}

#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub mod tests;
