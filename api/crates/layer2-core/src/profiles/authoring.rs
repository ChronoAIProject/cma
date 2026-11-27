use async_trait::async_trait;
use std::sync::Arc;

use crate::{Caller, Capability, ServiceId, SkillId, SubjectId};

use super::{ProfileCapacityBounds, ProfileError, ProfilePage, WorkspaceLocation};

mod credential;
mod validation;

pub use credential::AuthoringCredential;

pub const DEFAULT_SKILL_PAGE_SIZE: u16 = 20;
pub const MAX_SKILL_PAGE_SIZE: u16 = 50;
pub const MAX_SKILL_VERSIONS: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthoringServiceEntry {
    pub id: ServiceId,
    pub catalog_id: Option<String>,
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthoringSkillEntry {
    pub id: SkillId,
    pub name: String,
    pub description: Option<String>,
    pub versions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillCatalogQuery {
    pub query: String,
    pub limit: u16,
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryRequest {
    pub url: String,
    pub reference: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryResolution {
    pub url: String,
    pub reference: Option<String>,
    pub default_branch: String,
    pub commit_sha: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AuthoringSourceError {
    #[error("authoring source unavailable: {0}")]
    Unavailable(String),
    #[error("authoring source returned invalid data: {0}")]
    InvalidResponse(String),
    #[error("authoring query is invalid: {0}")]
    InvalidQuery(String),
    #[error("skill catalog access requires service authorization")]
    SkillsAuthorizationRequired,
    #[error("repository is absent or inaccessible")]
    RepositoryNotFound,
    #[error("repository reference is absent")]
    ReferenceNotFound,
}

/// Read-only authoring sources; owner authority never appears in returned values.
#[async_trait]
pub trait ProfileAuthoringSource: Send + Sync {
    async fn services(
        &self,
        owner: &SubjectId,
        credential: Option<&AuthoringCredential>,
    ) -> Result<Vec<AuthoringServiceEntry>, AuthoringSourceError>;

    async fn skills(
        &self,
        owner: &SubjectId,
        query: &SkillCatalogQuery,
        credential: Option<&AuthoringCredential>,
    ) -> Result<ProfilePage<AuthoringSkillEntry>, AuthoringSourceError>;

    async fn resolve_repository(
        &self,
        request: &RepositoryRequest,
    ) -> Result<RepositoryResolution, AuthoringSourceError>;
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProfileAuthoringError {
    #[error("profile authoring is forbidden")]
    Forbidden,
    #[error("owner catalog access is forbidden")]
    CatalogForbidden,
    #[error("profile authoring query is invalid")]
    InvalidQuery,
    #[error(transparent)]
    InvalidRepository(#[from] ProfileError),
    #[error("repository is absent or inaccessible")]
    RepositoryNotFound,
    #[error("repository reference is absent")]
    ReferenceNotFound,
    #[error("profile service catalog is unavailable")]
    ServicesUnavailable,
    #[error("profile skill catalog is unavailable")]
    SkillsUnavailable,
    #[error("skill catalog access requires service authorization")]
    SkillsAuthorizationRequired,
    #[error("repository verification is unavailable")]
    RepositoryUnavailable,
}

pub struct ProfileAuthoringService {
    bounds: ProfileCapacityBounds,
    source: Arc<dyn ProfileAuthoringSource>,
}

impl ProfileAuthoringService {
    pub fn new(
        bounds: ProfileCapacityBounds,
        source: Arc<dyn ProfileAuthoringSource>,
    ) -> Arc<Self> {
        assert!(
            bounds.min_cpu_millis > 0
                && bounds.min_cpu_millis <= bounds.max_cpu_millis
                && bounds.min_memory_mib > 0
                && bounds.min_memory_mib <= bounds.max_memory_mib,
            "profile authoring requires valid deployment capacity bounds"
        );
        Arc::new(Self { bounds, source })
    }

    pub fn authorize(&self, caller: &Caller) -> Result<(), ProfileAuthoringError> {
        caller
            .require(Capability::ManageOwnProfiles)
            .map_err(|_| ProfileAuthoringError::Forbidden)
    }

    pub fn authorize_catalog(&self, caller: &Caller) -> Result<(), ProfileAuthoringError> {
        caller
            .require(Capability::UseOwnSandboxes)
            .or_else(|_| caller.require(Capability::ManageOwnProfiles))
            .map_err(|_| ProfileAuthoringError::CatalogForbidden)
    }

    pub fn bounds(&self, caller: &Caller) -> Result<ProfileCapacityBounds, ProfileAuthoringError> {
        self.authorize(caller)?;
        Ok(self.bounds)
    }

    pub async fn services(
        &self,
        caller: &Caller,
        credential: Option<&AuthoringCredential>,
    ) -> Result<Vec<AuthoringServiceEntry>, ProfileAuthoringError> {
        self.authorize_catalog(caller)?;
        let mut entries = self
            .source
            .services(&caller.subject, credential)
            .await
            .map_err(|_| ProfileAuthoringError::ServicesUnavailable)?;
        if !validation::services(&entries) {
            return Err(ProfileAuthoringError::ServicesUnavailable);
        }
        entries.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(entries)
    }

    pub async fn skills(
        &self,
        caller: &Caller,
        query: SkillCatalogQuery,
        credential: Option<&AuthoringCredential>,
    ) -> Result<ProfilePage<AuthoringSkillEntry>, ProfileAuthoringError> {
        self.authorize_catalog(caller)?;
        if !validation::query(&query) {
            return Err(ProfileAuthoringError::InvalidQuery);
        }
        let page = self
            .source
            .skills(&caller.subject, &query, credential)
            .await
            .map_err(|error| match error {
                AuthoringSourceError::InvalidQuery(_) => ProfileAuthoringError::InvalidQuery,
                AuthoringSourceError::SkillsAuthorizationRequired => {
                    ProfileAuthoringError::SkillsAuthorizationRequired
                }
                _ => ProfileAuthoringError::SkillsUnavailable,
            })?;
        if !validation::skills(&page, &query) {
            return Err(ProfileAuthoringError::SkillsUnavailable);
        }
        Ok(page)
    }

    pub async fn resolve_repository(
        &self,
        caller: &Caller,
        request: RepositoryRequest,
    ) -> Result<RepositoryResolution, ProfileAuthoringError> {
        self.authorize(caller)?;
        WorkspaceLocation::GitRepository {
            url: request.url.clone(),
            reference: request.reference.clone(),
        }
        .validate()?;
        if request.url.len() > 2048 || validation::repository_identity(&request.url).is_none() {
            return Err(ProfileError::new("invalid_repository", "location.url").into());
        }
        let resolved =
            self.source
                .resolve_repository(&request)
                .await
                .map_err(|error| match error {
                    AuthoringSourceError::RepositoryNotFound => {
                        ProfileAuthoringError::RepositoryNotFound
                    }
                    AuthoringSourceError::ReferenceNotFound => {
                        ProfileAuthoringError::ReferenceNotFound
                    }
                    _ => ProfileAuthoringError::RepositoryUnavailable,
                })?;
        if !validation::repository(&request, &resolved) {
            return Err(ProfileAuthoringError::RepositoryUnavailable);
        }
        Ok(resolved)
    }
}

#[cfg(test)]
#[path = "authoring_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "catalog_authorization_tests.rs"]
mod catalog_authorization_tests;
