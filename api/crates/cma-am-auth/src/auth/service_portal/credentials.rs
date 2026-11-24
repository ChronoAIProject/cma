//! Resolve one browser login and fence its exact refreshed credential custody.
use super::super::{
    cookie_authorization,
    persistence::{SecretDigest, SubjectAuthState},
    refresh, AuthState, AUTH_COOKIE, AUTH_COOKIE_DOMAIN,
};
use super::PortalError;
use axum::http::{header, HeaderMap, StatusCode};
use layer2_core::SubjectId;
use std::time::SystemTime;

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
            .ok_or(StatusCode::UNAUTHORIZED)?;
        if login.subject != self.state.subject || login.expires_at <= SystemTime::now() {
            return Err(StatusCode::UNAUTHORIZED.into());
        }
        require_available(auth, &login.subject).await?;
        cookie_authorization::require_current_custody(auth, &self.state)
            .await
            .map_err(|_| StatusCode::UNAUTHORIZED)?;
        Ok(())
    }
}

pub(super) async fn resolve(auth: &AuthState, headers: &HeaderMap) -> Result<Custody, PortalError> {
    // The portal is deliberately cookie-only. API bearers use the Gateway's
    // public API directly; combining browser and bearer identities is ambiguous.
    if headers.contains_key(header::AUTHORIZATION) {
        return Err(StatusCode::UNAUTHORIZED.into());
    }
    let mut values = Vec::new();
    for header in headers.get_all(header::COOKIE) {
        let header = header.to_str().map_err(|_| StatusCode::UNAUTHORIZED)?;
        values.extend(
            header
                .split(';')
                .filter_map(|p| p.trim().split_once('='))
                .filter(|(name, _)| *name == AUTH_COOKIE)
                .map(|(_, value)| value),
        );
    }
    if values.len() != 1 || values[0].is_empty() || values[0].len() > 512 {
        return Err(StatusCode::UNAUTHORIZED.into());
    }
    let login_id = SecretDigest::identify(AUTH_COOKIE_DOMAIN, values[0]);
    let login = auth
        .store
        .browser_login(&login_id)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    if login.expires_at <= SystemTime::now() {
        return Err(StatusCode::UNAUTHORIZED.into());
    }
    require_available(auth, &login.subject).await?;
    let mut state = auth
        .store
        .subject(&login.subject)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    refresh::refresh_if_needed(auth, &mut state)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let resolved =
        cookie_authorization::request_for_browser_cookie(auth, &login.subject, state.clone())
            .await
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
            .ok_or(StatusCode::UNAUTHORIZED)?;
    if resolved.delegated_credential.is_none() {
        return Err(StatusCode::UNAUTHORIZED.into());
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
