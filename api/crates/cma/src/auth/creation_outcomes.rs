//! Read authority for receipt inspection. This path never refreshes credentials
//! or records a delegated command grant.

use std::{sync::Arc, time::SystemTime};

use axum::{
    extract::Request,
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::Response,
};
use layer1_public::{
    rest::creation_outcomes::{problem, AuthorizedLookup},
    AuthenticatedSubject,
};
use layer2_core::{Authorization, Capability, SubjectId};

use super::{
    nyxid_proxy::{self, jwt},
    AuthMode, AuthState, AUTH_COOKIE, AUTH_COOKIE_DOMAIN,
};

struct Authority {
    subject: SubjectId,
    incarnation: String,
    authorization: Authorization,
    expires_at: Option<SystemTime>,
    session: Option<super::persistence::SecretDigest>,
}

impl AuthState {
    pub(crate) fn with_creation_outcomes_enabled(mut self: Arc<Self>, enabled: bool) -> Arc<Self> {
        Arc::get_mut(&mut self)
            .expect("configure auth before sharing")
            .creation_outcomes_enabled = enabled;
        self
    }

    async fn creation_reader(&self, headers: &HeaderMap) -> Result<Authority, StatusCode> {
        const UNAVAILABLE: StatusCode = StatusCode::SERVICE_UNAVAILABLE;
        if headers.contains_key(nyxid_proxy::HEADER) {
            let token = jwt::identity_token(headers)
                .ok()
                .flatten()
                .ok_or(StatusCode::UNAUTHORIZED)?;
            let verifier = self
                .identity_proxy
                .as_ref()
                .ok_or(StatusCode::UNAUTHORIZED)?;
            let identity = verifier.verify(token).await.map_err(|error| match error {
                jwt::VerifyError::Rejected(_) => StatusCode::UNAUTHORIZED,
                jwt::VerifyError::Unavailable(_) => UNAVAILABLE,
            })?;
            let subject = SubjectId::new(identity.subject);
            let owner = self
                .store
                .subject(&subject)
                .await
                .map_err(|_| UNAVAILABLE)?
                .ok_or(StatusCode::FORBIDDEN)?;
            if owner.subject != subject
                || owner
                    .tokens
                    .as_ref()
                    .is_none_or(|tokens| tokens.expires_at <= SystemTime::now())
            {
                return Err(UNAVAILABLE);
            }
            let asserted = crate::permissions::capabilities_from_permissions(identity.permissions);
            return Ok(Authority {
                subject,
                incarnation: owner.incarnation,
                authorization: nyxid_proxy::intersect(&asserted, &owner.authorization),
                expires_at: Some(identity.expires_at),
                session: None,
            });
        }
        if super::bearer::bearer_token(headers).is_some() {
            return Err(StatusCode::FORBIDDEN);
        }
        if matches!(self.mode, AuthMode::Trusted { .. }) {
            return Err(StatusCode::UNAUTHORIZED);
        }
        let cookie = super::session_routes::request_cookie(headers, AUTH_COOKIE)
            .ok_or(StatusCode::UNAUTHORIZED)?;
        let id = super::persistence::SecretDigest::identify(AUTH_COOKIE_DOMAIN, cookie);
        let login = self
            .store
            .browser_login(&id)
            .await
            .map_err(|_| UNAVAILABLE)?
            .filter(|login| login.expires_at > SystemTime::now())
            .ok_or(StatusCode::UNAUTHORIZED)?;
        let owner = self
            .store
            .subject(&login.subject)
            .await
            .map_err(|_| UNAVAILABLE)?
            .ok_or(StatusCode::UNAUTHORIZED)?;
        if owner.subject != login.subject
            || owner
                .tokens
                .as_ref()
                .is_none_or(|tokens| tokens.expires_at <= SystemTime::now())
        {
            return Err(UNAVAILABLE);
        }
        Ok(Authority {
            subject: owner.subject,
            incarnation: owner.incarnation,
            authorization: owner.authorization,
            expires_at: Some(login.expires_at),
            session: Some(id),
        })
    }

    async fn check_creation_standing(&self, authority: &Authority) -> Result<(), StatusCode> {
        if !authority.authorization.allows(Capability::UseOwnSandboxes)
            || authority
                .expires_at
                .is_some_and(|expiry| expiry <= SystemTime::now())
        {
            return Err(StatusCode::FORBIDDEN);
        }
        let current = self
            .store
            .subject(&authority.subject)
            .await
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
            .filter(|owner| {
                owner.subject == authority.subject
                    && owner.incarnation == authority.incarnation
                    && owner.authorization.allows(Capability::UseOwnSandboxes)
            })
            .ok_or(StatusCode::FORBIDDEN)?;
        if current
            .tokens
            .is_none_or(|tokens| tokens.expires_at <= SystemTime::now())
        {
            return Err(StatusCode::SERVICE_UNAVAILABLE);
        }
        if let Some(session) = &authority.session {
            let current = self
                .store
                .browser_login(session)
                .await
                .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
            if !current.is_some_and(|login| {
                login.subject == authority.subject && login.expires_at > SystemTime::now()
            }) {
                return Err(StatusCode::FORBIDDEN);
            }
        }
        if self
            .standing
            .authoritative(&authority.subject)
            .await
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
        {
            return Err(StatusCode::FORBIDDEN);
        }
        Ok(())
    }
}

pub(super) async fn attach(auth: &Arc<AuthState>, mut request: Request, next: Next) -> Response {
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        if request
            .headers()
            .contains_key(super::resource_request::HEADER)
        {
            return Err(StatusCode::FORBIDDEN);
        }
        let authority = auth.creation_reader(request.headers()).await?;
        auth.check_creation_standing(&authority).await?;
        request
            .extensions_mut()
            .insert(AuthenticatedSubject(authority.subject.clone()));
        request
            .extensions_mut()
            .insert(AuthorizedLookup(authority.subject.clone()));
        let response = next.run(request).await;
        if response.status().is_success() || response.status() == StatusCode::CONFLICT {
            auth.check_creation_standing(&authority).await?;
        }
        Ok(response)
    })
    .await;
    match outcome {
        Ok(Ok(response)) => response,
        Ok(Err(status)) => problem(status),
        Err(_) => problem(StatusCode::SERVICE_UNAVAILABLE),
    }
}

#[cfg(test)]
#[path = "creation_outcomes_tests.rs"]
mod tests;
