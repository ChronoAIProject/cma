//! Profile draft authoring, publication, and visibility.

use reqwest::{Method, StatusCode};
use serde::{Serialize, de::DeserializeOwned};
use std::marker::PhantomData;

use crate::{
    Client, IdempotencyKey, PageOptions, ResourcePage, Result,
    resources::{identity, positive},
};

mod marketplace;
mod types;
pub use marketplace::*;
pub use types::*;

mod sealed {
    pub trait Sealed {
        fn validate(&self) -> crate::Result<()>;
    }
}

/// A supported profile content type, sealed to the three REST resource kinds.
pub trait ProfileContent: sealed::Sealed + Serialize + DeserializeOwned {
    /// Kind represented by this content.
    const KIND: ProfileKind;
    /// Exact REST resource collection segment.
    const COLLECTION: &'static str;
}

macro_rules! content_kind {
    ($content:ty, $kind:ident, $collection:literal) => {
        impl ProfileContent for $content {
            const KIND: ProfileKind = ProfileKind::$kind;
            const COLLECTION: &'static str = $collection;
        }
    };
}
content_kind!(AgentProfileContent, Agent, "agent-profiles");
content_kind!(WorkspaceProfileContent, Workspace, "workspace-profiles");
content_kind!(SandboxProfileContent, Sandbox, "sandbox-profiles");

impl sealed::Sealed for AgentProfileContent {
    fn validate(&self) -> Result<()> {
        Ok(())
    }
}

impl sealed::Sealed for WorkspaceProfileContent {
    fn validate(&self) -> Result<()> {
        for reference in &self.agent_profiles {
            positive(u64::from(reference.revision))?;
        }
        Ok(())
    }
}

impl sealed::Sealed for SandboxProfileContent {
    fn validate(&self) -> Result<()> {
        if self.cpu_millis == 0
            || self.memory_mib == 0
            || self.max_workspaces == 0
            || self.max_agents_per_workspace == 0
        {
            return Err(crate::Error::validation(
                "Sandbox resource allocations and limits must be positive",
            ));
        }
        for reference in &self.workspace_profiles {
            positive(u64::from(reference.revision))?;
        }
        Ok(())
    }
}

/// Owner-facing operations for one strongly typed profile kind.
pub struct Profiles<'a, C> {
    client: &'a Client,
    content: PhantomData<C>,
}

impl<C: ProfileContent> Profiles<'_, C> {
    /// List owned profiles using an opaque exclusive cursor.
    pub async fn list(&self, options: &PageOptions) -> Result<ResourcePage<ProfileDocument<C>>> {
        self.client.get(&[C::COLLECTION], &options.query()).await
    }

    /// Read an owned profile and its draft.
    pub async fn read(&self, id: &str) -> Result<ProfileDocument<C>> {
        let profile: ProfileDocument<C> = self.client.get(&[C::COLLECTION, id], &[]).await?;
        identity(&profile.id, id)?;
        Ok(profile)
    }

    /// Create an owned draft using an explicit mutation key.
    pub async fn create(
        &self,
        body: &ProfileDraft<C>,
        key: &IdempotencyKey,
    ) -> Result<ProfileDocument<C>> {
        body.content.validate()?;
        self.client
            .command(
                Method::POST,
                &[C::COLLECTION],
                Some(body),
                key,
                StatusCode::CREATED,
            )
            .await
    }

    /// Replace content at an exact draft version, optionally updating metadata.
    pub async fn save(
        &self,
        id: &str,
        body: &UpdateProfile<C>,
        key: &IdempotencyKey,
    ) -> Result<ProfileDocument<C>> {
        positive(body.draft_version)?;
        body.content.validate()?;
        let profile: ProfileDocument<C> = self
            .client
            .command(
                Method::PUT,
                &[C::COLLECTION, id],
                Some(body),
                key,
                StatusCode::OK,
            )
            .await?;
        identity(&profile.id, id)?;
        Ok(profile)
    }

    /// Publish one exact draft version as an immutable revision.
    pub async fn publish(
        &self,
        id: &str,
        draft_version: u64,
        key: &IdempotencyKey,
    ) -> Result<ProfileRevision<C>> {
        positive(draft_version)?;
        let revision: ProfileRevision<C> = self
            .client
            .command(
                Method::POST,
                &[C::COLLECTION, id, "revisions"],
                Some(&serde_json::json!({"draft_version": draft_version})),
                key,
                StatusCode::CREATED,
            )
            .await?;
        identity(&revision.profile_id, id)?;
        Ok(revision)
    }

    /// Page through immutable revisions of one profile.
    pub async fn list_revisions(
        &self,
        id: &str,
        options: &PageOptions,
    ) -> Result<ResourcePage<ProfileRevision<C>>> {
        let page: ResourcePage<ProfileRevision<C>> = self
            .client
            .get(&[C::COLLECTION, id, "revisions"], &options.query())
            .await?;
        for revision in &page.items {
            identity(&revision.profile_id, id)?;
        }
        Ok(page)
    }

    /// Read an exact immutable revision and verify its returned identity.
    pub async fn read_revision(&self, id: &str, number: u32) -> Result<ProfileRevision<C>> {
        positive(u64::from(number))?;
        let revision: ProfileRevision<C> = self
            .client
            .get(&[C::COLLECTION, id, "revisions", &number.to_string()], &[])
            .await?;
        identity(&revision.profile_id, id)?;
        if revision.summary.revision != number {
            return Err(crate::Error::protocol(
                "profile revision did not match the request",
            ));
        }
        Ok(revision)
    }

    /// Set the visibility of published revisions.
    pub async fn set_visibility(
        &self,
        id: &str,
        visibility: ProfileVisibility,
        key: &IdempotencyKey,
    ) -> Result<ProfileDocument<C>> {
        let profile: ProfileDocument<C> = self
            .client
            .command(
                Method::PUT,
                &[C::COLLECTION, id, "visibility"],
                Some(&serde_json::json!({"visibility": visibility})),
                key,
                StatusCode::OK,
            )
            .await?;
        identity(&profile.id, id)?;
        Ok(profile)
    }

    /// Retire a profile without removing already-published revisions.
    pub async fn retire(&self, id: &str, key: &IdempotencyKey) -> Result<ProfileDocument<C>> {
        let profile: ProfileDocument<C> = self
            .client
            .command(
                Method::POST,
                &[C::COLLECTION, id, "retire"],
                None::<&()>,
                key,
                StatusCode::OK,
            )
            .await?;
        identity(&profile.id, id)?;
        Ok(profile)
    }
}

impl Client {
    /// Access Agent profile drafts and revisions.
    pub fn agent_profiles(&self) -> Profiles<'_, AgentProfileContent> {
        Profiles {
            client: self,
            content: PhantomData,
        }
    }
    /// Access Workspace profile drafts and revisions.
    pub fn workspace_profiles(&self) -> Profiles<'_, WorkspaceProfileContent> {
        Profiles {
            client: self,
            content: PhantomData,
        }
    }
    /// Access Sandbox profile drafts and revisions.
    pub fn sandbox_profiles(&self) -> Profiles<'_, SandboxProfileContent> {
        Profiles {
            client: self,
            content: PhantomData,
        }
    }
    /// Browse or fork marketplace entries of the explicitly chosen profile kind.
    pub fn marketplace<C: ProfileContent>(&self) -> Marketplace<'_, C> {
        Marketplace::new(self)
    }
}
