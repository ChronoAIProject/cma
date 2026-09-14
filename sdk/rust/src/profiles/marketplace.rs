//! Public immutable profile marketplace projections.

use reqwest::{Method, StatusCode};
use serde::Deserialize;
use std::marker::PhantomData;

use super::{
    AgentProfileContent, ProfileContent, ProfileDocument, ProfileKind, ProfileRevision,
    SandboxProfileContent, WorkspaceProfileContent,
};
use crate::{
    Client, Error, IdempotencyKey, PageOptions, ResourcePage, Result,
    resources::{identity, positive},
};

/// Search and pagination for one marketplace profile kind.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MarketplaceListOptions {
    /// Optional search text.
    pub q: Option<String>,
    /// Opaque cursor and result bound.
    pub page: PageOptions,
}

/// Search-result metadata for one published profile.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct MarketplaceSummary {
    /// Profile kind.
    pub kind: ProfileKind,
    /// Profile identifier.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Description.
    pub description: String,
    /// Public owner display name.
    pub owner_display: String,
    /// Newest immutable revision number.
    #[serde(deserialize_with = "crate::resources::deserialize_positive")]
    pub latest_revision: u32,
    /// Publication timestamp.
    pub published_at: String,
    /// Recorded use count.
    pub uses_count: u64,
}

/// A referenced profile name resolved at its exact immutable revision.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ResolvedProfileName {
    /// Referenced profile kind.
    pub kind: ProfileKind,
    /// Referenced profile identifier.
    pub id: String,
    /// Exact referenced revision.
    #[serde(deserialize_with = "crate::resources::deserialize_positive")]
    pub revision: u32,
    /// Display name.
    pub name: String,
}

/// Immutable dependency content accompanying a marketplace entry.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ResolvedProfileRevision {
    /// Agent profile dependency.
    Agent {
        /// Dependency display name.
        name: String,
        /// Immutable Agent revision.
        revision: ProfileRevision<AgentProfileContent>,
    },
    /// Workspace profile dependency.
    Workspace {
        /// Dependency display name.
        name: String,
        /// Immutable Workspace revision.
        revision: ProfileRevision<WorkspaceProfileContent>,
    },
    /// Sandbox profile dependency.
    Sandbox {
        /// Dependency display name.
        name: String,
        /// Immutable Sandbox revision.
        revision: ProfileRevision<SandboxProfileContent>,
    },
}

/// Explicit trust requirement for a published Sandbox profile.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SandboxProfileTrust {
    /// Whether creation requires confirmation of the commands below.
    pub requires_confirmation: bool,
    /// Digest to echo with an explicit trust-confirmed creation request.
    pub commands_digest: String,
}

/// Published content with resolved dependencies and, for Sandboxes, trust metadata.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct MarketplaceEntry<C> {
    /// Search and identity metadata.
    pub summary: MarketplaceSummary,
    /// Current immutable revision.
    pub revision: ProfileRevision<C>,
    /// Names of all referenced revisions.
    pub resolved_profiles: Vec<ResolvedProfileName>,
    /// Exact content of referenced revisions.
    pub resolved_revisions: Vec<ResolvedProfileRevision>,
    /// Always present for Sandbox entries; absent for other kinds.
    pub trust: Option<SandboxProfileTrust>,
}

/// Public marketplace operations for one strongly typed profile kind.
pub struct Marketplace<'a, C> {
    client: &'a Client,
    content: PhantomData<C>,
}

impl<'a, C: ProfileContent> Marketplace<'a, C> {
    pub(super) fn new(client: &'a Client) -> Self {
        Self {
            client,
            content: PhantomData,
        }
    }

    /// Search published entries of this kind.
    pub async fn list(
        &self,
        options: &MarketplaceListOptions,
    ) -> Result<ResourcePage<MarketplaceSummary>> {
        let mut query = options.page.query();
        if let Some(q) = &options.q {
            query.push(("q", q.clone()));
        }
        let page: ResourcePage<MarketplaceSummary> = self
            .client
            .get(&["marketplace", C::COLLECTION], &query)
            .await?;
        if page.items.iter().any(|summary| summary.kind != C::KIND) {
            return Err(Error::protocol(
                "marketplace kind did not match the request",
            ));
        }
        Ok(page)
    }

    /// Read one entry and verify its kind, revision, and requested identity.
    pub async fn read(&self, id: &str) -> Result<MarketplaceEntry<C>> {
        let entry: MarketplaceEntry<C> = self
            .client
            .get(&["marketplace", C::COLLECTION, id], &[])
            .await?;
        identity(&entry.summary.id, id)?;
        identity(&entry.revision.profile_id, id)?;
        if entry.summary.kind != C::KIND
            || entry.summary.latest_revision != entry.revision.summary.revision
            || (C::KIND == ProfileKind::Sandbox && entry.trust.is_none())
        {
            return Err(Error::protocol(
                "marketplace revision or trust metadata did not match the request",
            ));
        }
        Ok(entry)
    }

    /// Fork an explicitly pinned revision into a new owned private draft.
    pub async fn fork(
        &self,
        id: &str,
        revision: u32,
        key: &IdempotencyKey,
    ) -> Result<ProfileDocument<C>> {
        positive(u64::from(revision))?;
        self.client
            .command(
                Method::POST,
                &["marketplace", C::COLLECTION, id, "fork"],
                Some(&serde_json::json!({"revision": revision})),
                key,
                StatusCode::CREATED,
            )
            .await
    }
}
