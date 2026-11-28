//! Method permissions and current organization scope for creator requests.
use super::{
    bearer, selected_owner, service_error, Creator, CreatorAuthError, CreatorAuthState,
    CreatorAuthenticator,
};
use axum::{
    extract::FromRequestParts,
    http::{request::Parts, HeaderMap},
};
use cmaeg_core::request_audit::Credential;

impl<S> FromRequestParts<S> for Creator
where
    S: CreatorAuthState + Send + Sync,
{
    type Rejection = CreatorAuthError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let auth = state.creator_auth();
        let outbound = parts.uri.path().split('/').nth(4) == Some("outbound-messages");
        let permission = match parts.method {
            axum::http::Method::GET | axum::http::Method::HEAD => "cma-eg:channels:read",
            _ if outbound => "cma-eg:channels:update",
            axum::http::Method::DELETE => "cma-eg:channels:delete",
            axum::http::Method::POST if parts.uri.path().ends_with("/test") => {
                "cma-eg:channels:test"
            }
            axum::http::Method::POST
                if matches!(
                    parts.uri.path().split('/').collect::<Vec<_>>().as_slice(),
                    ["", "v1", "channels", _, "partition", "resets"]
                ) =>
            {
                "cma-eg:channels:update"
            }
            axum::http::Method::POST => "cma-eg:channels:create",
            _ => "cma-eg:channels:update",
        };
        if bearer(&parts.headers).is_some_and(cma_service_auth::is_service_key) {
            let creator = auth
                .authenticate_owner_key(&parts.headers, permission)
                .await?;
            crate::request_audit::bind(&creator.subject, None, Credential::OwnerKey).await?;
            return Ok(creator);
        }
        if let Some(service) = &auth.service {
            let principal = service
                .authenticate(&parts.headers)
                .await
                .map_err(service_error)?;
            require_permission(&principal, permission).await?;
            return auth
                .service_creator(&principal.subject, &parts.headers)
                .await;
        }
        auth.authenticate(&parts.headers).await
    }
}

impl CreatorAuthenticator {
    pub async fn authenticate(&self, headers: &HeaderMap) -> Result<Creator, CreatorAuthError> {
        if bearer(headers).is_some_and(cma_service_auth::is_service_key) {
            let creator = self
                .authenticate_owner_key(headers, "cma-eg:channels:read")
                .await?;
            crate::request_audit::bind(&creator.subject, None, Credential::OwnerKey).await?;
            return Ok(creator);
        }
        if let Some(service) = &self.service {
            let principal = service.authenticate(headers).await.map_err(service_error)?;
            require_permission(&principal, "cma-eg:channels:read").await?;
            return self.service_creator(&principal.subject, headers).await;
        }
        let token = bearer(headers).ok_or(CreatorAuthError::NotAuthenticated)?;
        let owner = selected_owner(headers)?;
        let creator = self.authenticate_token(token, owner).await?;
        if crate::request_audit::active() {
            let actor = if owner.is_some() {
                self.authenticate_token(token, None).await?.subject
            } else {
                creator.subject.clone()
            };
            crate::request_audit::bind(
                &actor,
                creator.organization.then_some(creator.subject.as_str()),
                Credential::Bearer,
            )
            .await?;
        }
        Ok(creator)
    }
}

pub(super) async fn require_permission(
    principal: &cma_service_auth::Principal,
    permission: &str,
) -> Result<(), CreatorAuthError> {
    if let Err(error) = principal.require(permission) {
        crate::request_audit::bind(&principal.subject, None, Credential::Bearer).await?;
        return Err(service_error(error));
    }
    Ok(())
}
