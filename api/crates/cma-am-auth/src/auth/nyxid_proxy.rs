//! NyxID's existing signed service identity authenticates the whole CMA API.
//!
//! NyxID checks the Agent Key, service grant and any action approval before
//! forwarding. CMA checks the signed service identity and its own resource
//! permissions. Accepted commands retain that admission's permission ceiling;
//! the owner's renewable credential is execution custody, never a replacement
//! for the proxy's authority. Ending a NyxBot turn does not cancel work CMA has
//! already accepted. Legacy ingress checks assertion expiry on each request.

use axum::http::HeaderMap;
use layer2_core::resource_access::CredentialSource;
use layer2_core::{Authorization, StoreError, SubjectId};
use std::sync::Arc;

use super::{cookie_authorization, refresh, AuthMode, AuthState, RequestAuth};

pub mod commands;
pub mod jwt;
pub use jwt::HEADER;

/// Record, without any claim or token content, why CMA did not accept a
/// proxied NyxID identity. The request still continues to the other
/// authentication lanes, so an unaccepted assertion otherwise leaves no trace
/// behind its eventual `401`.
fn note_unaccepted(stage: &'static str, reason: &'static str) {
    let context = layer2_core::diagnostic_context::DiagnosticContext::current();
    let span = tracing::warn_span!(
        target: crate::diagnostics::PROVIDER_IDENTITY_TARGET,
        parent: None,
        "identity_verification"
    );
    if span.is_disabled() {
        return;
    }
    let _entered = span.enter();
    tracing::warn!(
        target: crate::diagnostics::PROVIDER_IDENTITY_TARGET,
        parent: &span,
        event = "identity_verification_failure",
        request_hash = context.request_hash.as_ref(),
        endpoint = "identity_proxy",
        stage,
        reason,
        disposition = "rejected",
        "NyxID proxy identity was not accepted"
    );
}

/// A NyxID identity assertion reached a CMA whose proxy lane is disabled.
pub(super) fn note_disabled() {
    note_unaccepted("configuration", "proxy_disabled");
}

#[cfg(test)]
#[path = "nyxid_proxy/diagnostic_tests.rs"]
mod diagnostic_tests;

pub use crate::ProxyGrant;

impl AuthState {
    /// Refuse identity-only proxy ingress while retaining accepted-command custody.
    pub fn with_proxy_require_request_proof(mut self: Arc<Self>, enabled: bool) -> Arc<Self> {
        Arc::get_mut(&mut self)
            .expect("configure before sharing")
            .proxy_require_request_proof = enabled;
        self
    }

    pub(super) fn rejects_unverified_proxy_headers(&self, headers: &HeaderMap) -> bool {
        self.proxy_require_request_proof
            && (headers.contains_key(HEADER)
                || headers.contains_key(super::resource_request::HEADER))
    }

    pub fn with_identity_proxy(
        mut self: Arc<Self>,
        enabled: bool,
        issuer: Option<String>,
        jwks_url: Option<String>,
        audience: Option<String>,
    ) -> Result<Arc<Self>, String> {
        if !enabled {
            return Ok(self);
        }
        let AuthMode::Provider { public_url, .. } = &self.mode else {
            return Err("NyxID proxy authentication requires provider auth".into());
        };
        let config = jwt::Config {
            issuer: issuer.ok_or("CHRONO_NYXID_PROXY_ISSUER is required")?,
            jwks_url: jwks_url.ok_or("CHRONO_NYXID_PROXY_JWKS_URL is required")?,
            audience: audience.unwrap_or_else(|| public_url.trim_end_matches('/').to_owned()),
            service_id: self
                .agent_key_service_id
                .clone()
                .ok_or("CHRONO_NYXID_SERVICE_ID is required")?,
        };
        let verifier =
            jwt::Verifier::new(config).map_err(|_| "invalid NyxID proxy verifier configuration")?;
        Arc::get_mut(&mut self)
            .expect("configure before sharing")
            .identity_proxy = Some(verifier);
        Ok(self)
    }

    /// Record the stable replay principal on new proxy command admissions.
    ///
    /// Each proxied request carries a fresh signed assertion, so the exact
    /// grant (which includes the assertion id) cannot scope idempotency: an
    /// exact-key retry would otherwise admit a second command. Readers accept
    /// the principal already; enable this only after every running reader
    /// includes that floor.
    pub fn with_proxy_replay_principal(mut self: Arc<Self>, enabled: bool) -> Arc<Self> {
        Arc::get_mut(&mut self)
            .expect("configure before sharing")
            .proxy_replay_principal_enabled = enabled;
        self
    }

    pub(super) async fn proxy_request(
        &self,
        headers: &HeaderMap,
        personal_inventory: bool,
    ) -> Result<Option<RequestAuth>, StoreError> {
        let token = match jwt::identity_token(headers) {
            Ok(Some(token)) => token,
            Ok(None) => return Ok(None),
            Err(jwt::VerifyError::Rejected(reason)) => {
                note_unaccepted("header", reason);
                return Ok(None);
            }
            Err(jwt::VerifyError::Unavailable(_)) => {
                note_unaccepted("header", "unreadable");
                return Ok(None);
            }
        };
        let Some(verifier) = &self.identity_proxy else {
            return Ok(None);
        };
        let identity = match verifier.verify(token).await {
            Ok(identity) => identity,
            Err(jwt::VerifyError::Rejected(reason)) => {
                note_unaccepted("assertion", reason);
                return Ok(None);
            }
            Err(jwt::VerifyError::Unavailable(_)) => {
                return Err(StoreError::Unavailable(
                    "NyxID signing keys unavailable".into(),
                ))
            }
        };
        let subject = SubjectId::new(identity.subject.clone());
        let authorization =
            crate::permissions::capabilities_from_permissions(identity.permissions.iter().cloned());
        let assertion_only = || RequestAuth {
            binding: None,
            credential_source: CredentialSource::Independent {
                expires_at: Some(identity.expires_at),
            },
            subject: subject.clone(),
            authorization: authorization.clone(),
            profile: identity.profile,
            delegated_credential: None,
        };
        let Some(mut owner) = self.store.subject(&subject).await? else {
            // Renewable CMA credentials are optional for synchronous resources.
            return Ok(Some(assertion_only()));
        };
        if owner.subject != subject {
            return Err(StoreError::Malformed("proxy owner mismatch".into()));
        }
        refresh::refresh_if_needed(self, &mut owner).await?;
        let owner_incarnation = owner.incarnation.clone();
        let mut request = if personal_inventory {
            let retained = cookie_authorization::verify_with_timeout(
                self,
                &subject,
                owner,
                cookie_authorization::PROVIDER_VERIFICATION_TIMEOUT,
            )
            .await?;
            match retained {
                Some(request) if request.delegated_credential.is_some() => request,
                _ => {
                    // The signed subject can read its own inventory when retained custody
                    // is absent or explicitly rejected. Dependency errors still propagate.
                    tracing::debug!(
                        "Using signed identity for personal inventory without retained custody"
                    );
                    return Ok(Some(assertion_only()));
                }
            }
        } else {
            cookie_authorization::request_for_cookie(self, &subject, owner).await?
        };
        request.authorization = intersect(&authorization, &request.authorization);
        let grant = ProxyGrant {
            id: ProxyGrant::identity(
                &identity.issuer,
                &identity.service_id,
                &subject,
                &identity.jti,
            ),
            issuer: identity.issuer,
            service_id: identity.service_id,
            assertion_id: identity.jti,
            subject,
            authorization: request.authorization.clone(),
            owner_incarnation,
        };
        grant.validate()?;
        // Persist the ceiling once; a replay cannot replace its owner incarnation
        // or gain permissions which were absent at the original admission.
        self.store.insert_proxy_grant(grant.clone()).await?;
        let saved = self
            .store
            .proxy_grant(&grant.id)
            .await?
            .ok_or_else(|| StoreError::Unavailable("proxy admission unavailable".into()))?;
        if saved.subject != grant.subject
            || saved.owner_incarnation != grant.owner_incarnation
            || saved.issuer != grant.issuer
            || saved.service_id != grant.service_id
        {
            return Ok(None);
        }
        request.authorization = intersect(&request.authorization, &saved.authorization);
        request.credential_source = CredentialSource::IdentityProxy {
            grant_digest: grant.id,
        };
        Ok(Some(request))
    }
}

pub(super) fn intersect(left: &Authorization, right: &Authorization) -> Authorization {
    Authorization::new(left.iter().filter(|capability| right.allows(*capability)))
}
