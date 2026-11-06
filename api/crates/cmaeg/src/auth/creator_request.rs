//! Method permissions and current organization scope for creator requests.
use super::{bearer, service_error, Creator, CreatorAuthError, CreatorAuthState};
use axum::{extract::FromRequestParts, http::request::Parts};

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
            axum::http::Method::POST => "cma-eg:channels:create",
            _ => "cma-eg:channels:update",
        };
        if bearer(&parts.headers).is_some_and(cma_service_auth::is_service_key) {
            return auth
                .authenticate_owner_key(&parts.headers, permission)
                .await;
        }
        if let Some(service) = &auth.service {
            let principal = service
                .authenticate(&parts.headers)
                .await
                .map_err(service_error)?;
            principal.require(permission).map_err(service_error)?;
            return auth
                .service_creator(principal.subject, &parts.headers)
                .await;
        }
        auth.authenticate(&parts.headers).await
    }
}
