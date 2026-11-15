//! Cookie-only inert reads. No refresh, sign-in, credential enrolment or writes.
use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};

use axum::{
    extract::{Request, State},
    http::{header, HeaderMap, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use layer1_public::AuthenticatedSubject;

use super::{
    identity::VerifiedSubject,
    persistence::{SecretDigest, SubjectAuthState},
    AuthMode, AuthState,
};

/// Install only around the explicitly selected finite JSON read routes.
pub(crate) async fn attach(
    State(auth): State<Arc<AuthState>>,
    mut request: Request,
    next: Next,
) -> Response {
    let result = async {
        if !matches!(*request.method(), Method::GET | Method::HEAD) {
            return Err(StatusCode::SERVICE_UNAVAILABLE);
        }
        let proof = authenticate(auth.clone(), request.headers()).await?;
        let super::permissions::PermissionEvidence::Active(permissions) =
            &proof.verified.permissions
        else {
            return Err(StatusCode::UNAUTHORIZED);
        };
        request
            .extensions_mut()
            .insert(crate::permissions::capabilities_from_permissions(
                permissions.iter().cloned(),
            ));
        request
            .extensions_mut()
            .insert(AuthenticatedSubject(proof.state.subject.clone()));
        // These routes finish serialization before returning; no streaming body
        // can outlive the final login, token generation and expiry checks.
        let response = next.run(request).await;
        proof.current(&auth).await?;
        Ok(response)
    }
    .await;
    let mut response = match result {
        Ok(response) => response,
        Err(status) => status.into_response(),
    };
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response
}

pub(crate) struct ReadProof {
    auth: Arc<AuthState>,
    login: SecretDigest,
    state: SubjectAuthState,
    verified: Arc<VerifiedSubject>,
}

impl ReadProof {
    pub(crate) fn caller(&self) -> layer2_core::Caller {
        let authorization = match &self.verified.permissions {
            super::permissions::PermissionEvidence::Active(permissions) => {
                crate::permissions::capabilities_from_permissions(permissions.iter().cloned())
            }
            super::permissions::PermissionEvidence::Inactive => layer2_core::Authorization::none(),
        };
        layer2_core::Caller::new(self.state.subject.clone(), authorization)
    }

    /// CLI dispatch reacquires live provider permission and the exact durable
    /// credential custody. A changed credential or permission refuses dispatch.
    pub(crate) async fn recheck(&self) -> Result<layer2_core::Caller, StatusCode> {
        self.current(&self.auth).await?;
        let tokens = self.state.tokens.as_ref().ok_or(StatusCode::UNAUTHORIZED)?;
        let fresh = super::identity_retry::verify(
            &self.auth,
            None,
            tokens.access_token.expose(),
            Some(tokens.expires_at),
            Some(self.state.subject.as_str()),
            super::identity_retry::DEADLINE,
        )
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
        if fresh.subject != self.state.subject
            || fresh.permissions != self.verified.permissions
            || !super::identity_lifetime::current(&self.auth, &fresh)
        {
            return Err(StatusCode::UNAUTHORIZED);
        }
        self.current(&self.auth).await?;
        if !super::identity_lifetime::current(&self.auth, &fresh) {
            return Err(StatusCode::UNAUTHORIZED);
        }
        Ok(self.caller())
    }

    async fn current(&self, auth: &AuthState) -> Result<(), StatusCode> {
        if !super::identity_lifetime::current(auth, &self.verified) {
            return Err(StatusCode::UNAUTHORIZED);
        }
        let current = auth
            .store
            .browser_session(&self.login)
            .await
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
            .ok_or(StatusCode::UNAUTHORIZED)?;
        let Some(state) = current.1 else {
            return Err(StatusCode::UNAUTHORIZED);
        };
        if current.0.subject != self.state.subject
            || current.0.expires_at <= SystemTime::now()
            || state.subject != self.state.subject
            || state.incarnation != self.state.incarnation
            || state.token_generation != self.state.token_generation
            || state.tokens != self.state.tokens
            || state.refresh_lease.is_some()
            || !state
                .tokens
                .as_ref()
                .is_some_and(|t| t.expires_at > SystemTime::now())
            || !super::identity_lifetime::current(auth, &self.verified)
        {
            return Err(StatusCode::UNAUTHORIZED);
        }
        Ok(())
    }
}

pub(crate) async fn authenticate(
    auth: Arc<AuthState>,
    headers: &HeaderMap,
) -> Result<ReadProof, StatusCode> {
    if !matches!(auth.mode, AuthMode::Provider { .. }) {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    let id = cookie(headers)?;
    let (login, state) = auth
        .store
        .browser_session(&id)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let state = state.ok_or(StatusCode::UNAUTHORIZED)?;
    state
        .validate()
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    if state.subject != login.subject
        || login.expires_at <= SystemTime::now()
        || state.refresh_lease.is_some()
    {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let tokens = state.tokens.as_ref().ok_or(StatusCode::UNAUTHORIZED)?;
    // A credential needing ordinary renewal is explicitly unavailable here.
    if tokens.access_token.expose().is_empty()
        || tokens
            .expires_at
            .duration_since(SystemTime::now())
            .unwrap_or_default()
            <= Duration::from_secs(60)
    {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let verified = super::identity_retry::verify(
        &auth,
        None,
        tokens.access_token.expose(),
        Some(tokens.expires_at),
        Some(state.subject.as_str()),
        super::identity_retry::DEADLINE,
    )
    .await
    .map_err(|error| match error {
        super::verification_error::ProviderVerificationError::Rejected(_) => {
            StatusCode::UNAUTHORIZED
        }
        _ => StatusCode::SERVICE_UNAVAILABLE,
    })?;
    if verified.subject != state.subject
        || !matches!(
            verified.permissions,
            super::permissions::PermissionEvidence::Active(_)
        )
    {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let proof = ReadProof {
        auth: auth.clone(),
        login: id,
        state,
        verified,
    };
    proof.current(&auth).await?;
    Ok(proof)
}

fn cookie(headers: &HeaderMap) -> Result<SecretDigest, StatusCode> {
    if headers.contains_key(header::AUTHORIZATION)
        || headers.contains_key(super::nyxid_proxy::HEADER)
        || headers.contains_key(super::resource_request::HEADER)
    {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let mut selected = None;
    for value in headers.get_all(header::COOKIE) {
        for item in value
            .to_str()
            .map_err(|_| StatusCode::UNAUTHORIZED)?
            .split(';')
        {
            if let Some((name, value)) = item.trim().split_once('=') {
                if name == super::AUTH_COOKIE {
                    if selected.is_some() || value.is_empty() || value.len() > 512 {
                        return Err(StatusCode::UNAUTHORIZED);
                    }
                    selected = Some(value);
                }
            }
        }
    }
    selected
        .map(|value| SecretDigest::identify(super::AUTH_COOKIE_DOMAIN, value))
        .ok_or(StatusCode::UNAUTHORIZED)
}

#[cfg(test)]
mod tests;
