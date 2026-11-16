//! Irreversible queue dispositions require explicit service-scoped authority.
use super::{bearer, service_error, Creator, CreatorAuthError, CreatorAuthState};
use axum::{extract::FromRequestParts, http::request::Parts};

pub const RELEASE_PERMISSION: &str = "cma-eg:partitions:release";

pub struct PartitionReleaseActor {
    pub creator: Creator,
    pub actor: String,
}

impl<S: CreatorAuthState + Send + Sync> FromRequestParts<S> for PartitionReleaseActor {
    type Rejection = CreatorAuthError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let auth = state.creator_auth();
        let token = bearer(&parts.headers).ok_or(CreatorAuthError::NotAuthenticated)?;
        if cma_service_auth::is_service_key(token) {
            let creator = auth
                .authenticate_owner_key(&parts.headers, RELEASE_PERMISSION)
                .await?;
            return Ok(Self {
                actor: creator.subject.clone(),
                creator,
            });
        }
        // Legacy identity-only authentication proves an owner, not a recovery
        // grant. Never fall through to it when scoped introspection is absent.
        let service = auth.service.as_ref().ok_or(CreatorAuthError::Forbidden)?;
        let principal = service
            .authenticate(&parts.headers)
            .await
            .map_err(service_error)?;
        principal
            .require(RELEASE_PERMISSION)
            .map_err(service_error)?;
        Ok(Self {
            actor: principal.subject.clone(),
            creator: auth
                .service_creator(principal.subject, &parts.headers)
                .await?,
        })
    }
}
