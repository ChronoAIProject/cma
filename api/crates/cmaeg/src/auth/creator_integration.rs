use super::*;
use axum::{extract::FromRequestParts, http::request::Parts};

pub struct IntegrationOwner {
    pub subject: String,
}

impl<S> FromRequestParts<S> for IntegrationOwner
where
    S: CreatorAuthState + Send + Sync,
{
    type Rejection = CreatorAuthError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let creator = Creator::from_request_parts(parts, state).await?;
        Ok(Self {
            subject: creator.subject,
        })
    }
}

pub struct Administrator {
    pub creator: Creator,
    pub actor: String,
}

impl<S> FromRequestParts<S> for Administrator
where
    S: CreatorAuthState + Send + Sync,
{
    type Rejection = CreatorAuthError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        bearer(&parts.headers).ok_or(CreatorAuthError::NotAuthenticated)?;
        let auth = state.creator_auth();
        let service = auth.service.as_ref().ok_or(CreatorAuthError::Forbidden)?;
        let principal = service
            .authenticate(&parts.headers)
            .await
            .map_err(service_error)?;
        super::request::require_permission(&principal, "cma-eg:admin").await?;
        let creator = auth
            .service_creator(&principal.subject, &parts.headers)
            .await?;
        Ok(Self {
            actor: principal.subject,
            creator,
        })
    }
}

impl CreatorAuthenticator {
    pub(super) async fn service_creator(
        &self,
        subject: &str,
        headers: &HeaderMap,
    ) -> Result<Creator, CreatorAuthError> {
        let result = self.resolve_service_creator(subject, headers).await;
        let organization = result
            .as_ref()
            .ok()
            .filter(|creator| creator.organization)
            .map(|creator| creator.subject.as_str());
        crate::request_audit::bind(
            subject,
            organization,
            cmaeg_core::request_audit::Credential::Bearer,
        )
        .await?;
        result
    }

    async fn resolve_service_creator(
        &self,
        subject: &str,
        headers: &HeaderMap,
    ) -> Result<Creator, CreatorAuthError> {
        let token = bearer(headers).ok_or(CreatorAuthError::NotAuthenticated)?;
        let owner = selected_owner(headers)?;
        if let Some(owner) = owner {
            tokio::time::timeout(Duration::from_secs(3), async {
                let (identity, organization) = self.fetch_subject(token).await?;
                if identity != subject {
                    return Err(CreatorAuthError::NotAuthenticated);
                }
                if organization {
                    if owner != subject {
                        return Err(CreatorAuthError::NotAuthenticated);
                    }
                } else if !self.verify_organization(token, owner).await? {
                    return Err(CreatorAuthError::NotAuthenticated);
                }
                Ok(())
            })
            .await
            .map_err(|_| CreatorAuthError::ProviderUnavailable)??;
        }
        Ok(Creator {
            subject: owner.unwrap_or(subject).to_owned(),
            organization: owner.is_some(),
            token_hash: token_hash(token),
        })
    }
}
