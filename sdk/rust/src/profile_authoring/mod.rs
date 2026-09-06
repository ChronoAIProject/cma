//! Capability-gated read-only metadata and repository verification for profile authors.

use crate::{Client, Error, Result};
use reqwest::{Method, StatusCode};

mod types;
mod validation;
pub use types::*;

/// Public authoring operations requiring the caller's profile-management capability.
pub struct ProfileAuthoring<'a> {
    client: &'a Client,
}

impl ProfileAuthoring<'_> {
    /// Read the deployment's immutable positive capacity bounds.
    pub async fn bounds(&self) -> Result<ProfileAuthoringBounds> {
        let result: ProfileAuthoringBounds = self
            .client
            .get(&["profile-authoring", "bounds"], &[])
            .await?;
        if result.min_cpu_millis == 0
            || result.min_cpu_millis > result.max_cpu_millis
            || result.min_memory_mib == 0
            || result.min_memory_mib > result.max_memory_mib
        {
            return Err(Error::protocol("invalid profile authoring capacity bounds"));
        }
        Ok(result)
    }

    /// Read safe service names for profile scopes, never connection credentials.
    pub async fn services(&self) -> Result<Vec<ProfileAuthoringService>> {
        let result: Vec<ProfileAuthoringService> = self
            .client
            .get(&["profile-authoring", "services"], &[])
            .await?;
        if !validation::services(&result) {
            return Err(Error::protocol("invalid profile authoring service catalog"));
        }
        Ok(result)
    }

    /// Search one bounded skill page, preserving exact versions and opaque cursors.
    pub async fn search_skills(
        &self,
        options: &ProfileAuthoringSkillSearch,
    ) -> Result<ProfileAuthoringSkillPage> {
        if !validation::query(options) {
            return Err(Error::validation(
                "skill search requires q <= 200 bytes, limit 1 to 50, and a nonempty cursor <= 2048 bytes without controls",
            ));
        }
        let mut query = Vec::new();
        if let Some(q) = &options.q {
            query.push(("q", q.clone()));
        }
        if let Some(limit) = options.limit {
            query.push(("limit", limit.to_string()));
        }
        if let Some(cursor) = &options.cursor {
            query.push(("cursor", cursor.clone()));
        }
        let result = self
            .client
            .get(&["profile-authoring", "skills"], &query)
            .await?;
        if !validation::skills(&result, options) {
            return Err(Error::protocol("invalid profile authoring skill page"));
        }
        Ok(result)
    }

    /// Verify a public repository without cloning, admitting work, or sending an idempotency key.
    pub async fn resolve_repository(
        &self,
        request: &ResolveRepositoryRequest,
    ) -> Result<ResolvedRepository> {
        let result = self
            .client
            .operation(
                Method::POST,
                &["profile-authoring", "repositories", "resolve"],
                Some(request),
                StatusCode::OK,
            )
            .await?;
        if !validation::repository(request, &result) {
            return Err(Error::protocol(
                "repository resolution identity or commit did not match its contract",
            ));
        }
        Ok(result)
    }
}

impl Client {
    /// Access public profile-authoring catalogs, deployment bounds, and repository verification.
    pub fn profile_authoring(&self) -> ProfileAuthoring<'_> {
        ProfileAuthoring { client: self }
    }
}
