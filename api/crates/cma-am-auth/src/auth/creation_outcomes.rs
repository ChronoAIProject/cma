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
    cookie_authorization,
    nyxid_proxy::{self, jwt},
    persistence::SubjectAuthState,
    AuthMode, AuthState, AUTH_COOKIE, AUTH_COOKIE_DOMAIN,
};

struct Authority {
    owner: SubjectAuthState,
    ceiling: Option<Authorization>,
    expires_at: SystemTime,
    session: Option<super::persistence::SecretDigest>,
}

impl AuthState {
    pub fn with_creation_outcomes_enabled(mut self: Arc<Self>, enabled: bool) -> Arc<Self> {
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
                .filter(|owner| owner.subject == subject)
                .ok_or(StatusCode::FORBIDDEN)?;
            let asserted = crate::permissions::capabilities_from_permissions(identity.permissions);
            return Ok(Authority {
                owner,
                ceiling: Some(asserted),
                expires_at: identity.expires_at,
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
        if owner.subject != login.subject {
            return Err(UNAVAILABLE);
        }
        Ok(Authority {
            owner,
            ceiling: None,
            expires_at: login.expires_at,
            session: Some(id),
        })
    }

    async fn check_creation_standing(&self, authority: &Authority) -> Result<(), StatusCode> {
        super::identity_lifetime::scope(
            self,
            async {
                self.verify_creation_standing(authority).await?;
                Ok(Some(()))
            },
            || StatusCode::SERVICE_UNAVAILABLE,
        )
        .await
        .map(|_| ())
    }

    async fn verify_creation_standing(&self, authority: &Authority) -> Result<(), StatusCode> {
        let unavailable = |_| StatusCode::SERVICE_UNAVAILABLE;
        let owner = &authority.owner;
        if authority.expires_at <= SystemTime::now()
            || authority
                .ceiling
                .as_ref()
                .is_some_and(|ceiling| !ceiling.allows(Capability::UseOwnSandboxes))
        {
            return Err(StatusCode::FORBIDDEN);
        }
        if owner
            .tokens
            .as_ref()
            .is_none_or(|tokens| tokens.expires_at <= SystemTime::now())
        {
            return Err(StatusCode::SERVICE_UNAVAILABLE);
        }
        let verified =
            cookie_authorization::request_for_browser_cookie(self, &owner.subject, owner.clone())
                .await
                .map_err(unavailable)?
                .ok_or(StatusCode::FORBIDDEN)?;
        let effective = authority
            .ceiling
            .as_ref()
            .map_or(verified.authorization.clone(), |ceiling| {
                nyxid_proxy::intersect(ceiling, &verified.authorization)
            });
        if !effective.allows(Capability::UseOwnSandboxes)
            || self.owner_session_authority(&verified).is_none()
        {
            return Err(StatusCode::FORBIDDEN);
        }
        if let Some(session) = &authority.session {
            let current = self
                .store
                .browser_login(session)
                .await
                .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
            if !current.is_some_and(|login| {
                login.subject == owner.subject && login.expires_at > SystemTime::now()
            }) {
                return Err(StatusCode::FORBIDDEN);
            }
        }
        if self
            .standing
            .authoritative(&owner.subject)
            .await
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
        {
            return Err(StatusCode::FORBIDDEN);
        }
        cookie_authorization::require_current_custody(self, owner)
            .await
            .map_err(unavailable)?;
        if authority.expires_at <= SystemTime::now() {
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
        crate::request_audit::bind(crate::request_audit::VerifiedActor {
            subject: authority.owner.subject.clone(),
            channel: layer2_core::ActivityChannel::Web,
            credential: crate::request_audit::Credential::WebSession,
            scope: "personal",
        })
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
        request
            .extensions_mut()
            .insert(AuthenticatedSubject(authority.owner.subject.clone()));
        request
            .extensions_mut()
            .insert(AuthorizedLookup(authority.owner.subject.clone()));
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
