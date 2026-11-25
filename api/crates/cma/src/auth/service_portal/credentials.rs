//! Resolve one browser login and fence its exact refreshed credential custody.
use super::super::{
    cookie_authorization,
    persistence::{SecretDigest, SubjectAuthState},
    refresh, AuthState, AUTH_COOKIE, AUTH_COOKIE_DOMAIN,
};
use super::PortalError;
use axum::http::{header, HeaderMap, StatusCode};
use layer2_core::SubjectId;
use std::fmt;
use std::time::SystemTime;

/// Why the portal refused a browser credential. Each value is a closed,
/// content-free reason for the 401, so a refusal can be attributed without
/// logging cookies, subjects or tokens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Refusal {
    /// An explicit `Authorization` header; the portal is cookie-only.
    BearerPresented,
    CookieMissing,
    /// More than one `cma_session` value; none is chosen.
    CookieAmbiguous,
    CookieInvalid,
    LoginUnknown,
    LoginExpired,
    /// The login existed when resolved and has since been removed.
    LoginRevoked,
    SubjectUnknown,
    /// No current provider authorization for the browser login.
    AuthorizationMissing,
    DelegatedCredentialMissing,
    /// The resolved login or token custody changed during the request.
    CustodyChanged,
}

impl Refusal {
    pub(super) fn code(self) -> &'static str {
        match self {
            Self::BearerPresented => "portal_bearer_presented",
            Self::CookieMissing => "portal_cookie_missing",
            Self::CookieAmbiguous => "portal_cookie_ambiguous",
            Self::CookieInvalid => "portal_cookie_invalid",
            Self::LoginUnknown => "portal_login_unknown",
            Self::LoginExpired => "portal_login_expired",
            Self::LoginRevoked => "portal_login_revoked",
            Self::SubjectUnknown => "portal_subject_unknown",
            Self::AuthorizationMissing => "portal_authorization_missing",
            Self::DelegatedCredentialMissing => "portal_delegated_credential_missing",
            Self::CustodyChanged => "portal_custody_changed",
        }
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

pub(super) struct Custody {
    login: SecretDigest,
    state: SubjectAuthState,
}
impl Custody {
    pub(super) fn token(&self) -> &str {
        self.state
            .tokens
            .as_ref()
            .expect("verified token")
            .access_token
            .expose()
    }
    pub(super) async fn current(&self, auth: &AuthState) -> Result<(), PortalError> {
        let login = auth
            .store
            .browser_login(&self.login)
            .await
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
            .ok_or(Refusal::LoginRevoked)?;
        if login.subject != self.state.subject {
            return Err(Refusal::CustodyChanged.into());
        }
        if login.expires_at <= SystemTime::now() {
            return Err(Refusal::LoginExpired.into());
        }
        require_available(auth, &login.subject).await?;
        cookie_authorization::require_current_custody(auth, &self.state)
            .await
            .map_err(|_| Refusal::CustodyChanged)?;
        Ok(())
    }
}

pub(super) async fn resolve(auth: &AuthState, headers: &HeaderMap) -> Result<Custody, PortalError> {
    // The portal is deliberately cookie-only. API bearers use the Gateway's
    // public API directly; combining browser and bearer identities is ambiguous.
    if headers.contains_key(header::AUTHORIZATION) {
        return Err(Refusal::BearerPresented.into());
    }
    let mut values = Vec::new();
    for header in headers.get_all(header::COOKIE) {
        let header = header.to_str().map_err(|_| Refusal::CookieInvalid)?;
        values.extend(
            header
                .split(';')
                .filter_map(|p| p.trim().split_once('='))
                .filter(|(name, _)| *name == AUTH_COOKIE)
                .map(|(_, value)| value),
        );
    }
    match values.as_slice() {
        [] => return Err(Refusal::CookieMissing.into()),
        [value] if value.is_empty() || value.len() > 512 => {
            return Err(Refusal::CookieInvalid.into())
        }
        [_] => {}
        _ => return Err(Refusal::CookieAmbiguous.into()),
    }
    let login_id = SecretDigest::identify(AUTH_COOKIE_DOMAIN, values[0]);
    let login = auth
        .store
        .browser_login(&login_id)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
        .ok_or(Refusal::LoginUnknown)?;
    if login.expires_at <= SystemTime::now() {
        return Err(Refusal::LoginExpired.into());
    }
    require_available(auth, &login.subject).await?;
    let mut state = auth
        .store
        .subject(&login.subject)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
        .ok_or(Refusal::SubjectUnknown)?;
    refresh::refresh_if_needed(auth, &mut state)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let resolved =
        cookie_authorization::request_for_browser_cookie(auth, &login.subject, state.clone())
            .await
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
            .ok_or(Refusal::AuthorizationMissing)?;
    if resolved.delegated_credential.is_none() {
        return Err(Refusal::DelegatedCredentialMissing.into());
    }
    let custody = Custody {
        login: login_id,
        state,
    };
    custody.current(auth).await?;
    Ok(custody)
}

async fn require_available(auth: &AuthState, subject: &SubjectId) -> Result<(), PortalError> {
    if auth
        .standing
        .erasing(subject)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
    {
        return Err(PortalError::AccountDeletionInProgress);
    }
    Ok(())
}
