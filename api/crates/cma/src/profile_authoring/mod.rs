//! Owner-scoped production dependencies for public profile authoring.

use std::sync::Arc;
use std::time::Duration;

use layer2_core::profiles::authoring::{
    AuthoringCredential, AuthoringServiceEntry, AuthoringSkillEntry, AuthoringSourceError,
    ProfileAuthoringSource, RepositoryRequest, RepositoryResolution, SkillCatalogQuery,
};
use layer2_core::profiles::ProfilePage;
use layer2_core::SubjectId;

use crate::auth::{keys::KeyIssuer, persistence::SecretValue, AuthMode, AuthState};
use crate::config::Config;
use crate::sandbox::OwnerTokenSource;

mod http;
mod repositories;
mod skills;

pub(crate) async fn resolve_public_snapshot(
    repository: &RepositoryRequest,
) -> Result<RepositoryResolution, AuthoringSourceError> {
    repositories::resolve_public_snapshot(repository).await
}

pub(crate) struct CmaProfileAuthoringSource {
    issuer: Option<Arc<KeyIssuer>>,
    tokens: Option<Arc<dyn OwnerTokenSource>>,
    skills: skills::SkillCatalog,
    repositories: repositories::RepositoryResolver,
}

impl CmaProfileAuthoringSource {
    pub(crate) fn production(config: &Config, auth: Arc<AuthState>) -> Result<Arc<Self>, String> {
        let http = http::client()?;
        let (issuer, tokens) = match &config.auth_mode {
            AuthMode::Provider { base_url, .. } => (
                Some(Arc::new(KeyIssuer::new(
                    http.clone(),
                    base_url,
                    &config.deployment_id,
                ))),
                Some(auth as Arc<dyn OwnerTokenSource>),
            ),
            AuthMode::Trusted { .. } => (None, None),
        };
        Ok(Arc::new(Self {
            issuer,
            tokens,
            skills: skills::SkillCatalog {
                http: http.clone(),
                origin: reqwest::Url::parse(&config.runtime.platform_base_url)
                    .map_err(|_| "profile skill origin is invalid".to_string())?,
            },
            repositories: repositories::RepositoryResolver {
                http,
                origin: reqwest::Url::parse("https://api.github.com")
                    .expect("the fixed repository API origin is valid"),
            },
        }))
    }

    async fn bearer(
        &self,
        owner: &SubjectId,
        credential: Option<&AuthoringCredential>,
    ) -> Result<Option<SecretValue>, AuthoringSourceError> {
        if let Some(credential) = credential {
            return Ok(Some(SecretValue::new(credential.value().to_owned())));
        }
        let Some(tokens) = &self.tokens else {
            return Ok(None);
        };
        tokens
            .token_for(owner)
            .await
            .map_err(|_| http::unavailable("owner credential lookup failed"))?
            .map(Some)
            .ok_or_else(|| http::unavailable("owner credential is unavailable"))
    }
}

#[async_trait::async_trait]
impl ProfileAuthoringSource for CmaProfileAuthoringSource {
    async fn services(
        &self,
        owner: &SubjectId,
        credential: Option<&AuthoringCredential>,
    ) -> Result<Vec<AuthoringServiceEntry>, AuthoringSourceError> {
        let issuer = self
            .issuer
            .as_ref()
            .ok_or_else(|| http::unavailable("service catalogue is not composed"))?;
        let token = self
            .bearer(owner, credential)
            .await?
            .ok_or_else(|| http::unavailable("owner credential is unavailable"))?;
        let services = issuer
            .services_of(token.expose())
            .await
            .map_err(|_| http::unavailable("service catalogue request failed"))?;
        Ok(services
            .into_iter()
            .map(|service| AuthoringServiceEntry {
                id: service.id,
                catalog_id: Some(service.catalog_id),
                name: service.name,
                description: service.description,
            })
            .collect())
    }

    async fn skills(
        &self,
        owner: &SubjectId,
        query: &SkillCatalogQuery,
        credential: Option<&AuthoringCredential>,
    ) -> Result<ProfilePage<AuthoringSkillEntry>, AuthoringSourceError> {
        let resolution = async {
            let token = self.bearer(owner, credential).await?;
            self.skills
                .search(token.as_ref().map(SecretValue::expose), query)
                .await
        };
        let result = tokio::time::timeout(Duration::from_secs(30), resolution)
            .await
            .map_err(|_| http::unavailable("skill catalogue deadline elapsed"))
            .and_then(|result| result);
        if let Err(error) = &result {
            // Every cause becomes one public 503; name it here (#2270). Details
            // are fixed stage labels and status codes, never upstream text.
            if !matches!(error, AuthoringSourceError::InvalidQuery(_)) {
                tracing::warn!(target: "cma_profile_authoring", error = %error,
                    "Ornn skill catalogue read failed");
            }
        }
        result
    }

    async fn resolve_repository(
        &self,
        request: &RepositoryRequest,
    ) -> Result<RepositoryResolution, AuthoringSourceError> {
        tokio::time::timeout(Duration::from_secs(30), self.repositories.resolve(request))
            .await
            .map_err(|_| http::unavailable("repository verification deadline elapsed"))?
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod dependency_contract_tests;
