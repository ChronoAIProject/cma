//! Publisher-owned drafts, immutable publications, placements, and public previews.

use crate::{
    Client, IdempotencyKey, PageOptions, ResourcePage, Result,
    http::Request,
    resources::{identity, positive},
};
use reqwest::{Method, StatusCode};
use serde::Deserialize;

mod badges;
mod types;
pub use badges::*;
pub use types::*;

/// Publisher-owned Trigger operations.
pub struct Triggers<'a> {
    pub(crate) client: &'a Client,
}

#[derive(Deserialize)]
pub(crate) struct CursorPage<T> {
    pub(crate) items: Vec<T>,
    pub(crate) next_cursor: Option<String>,
}

impl<T> From<CursorPage<T>> for ResourcePage<T> {
    fn from(page: CursorPage<T>) -> Self {
        Self {
            items: page.items,
            next: page.next_cursor,
        }
    }
}

impl Triggers<'_> {
    /// List owned Triggers using an opaque cursor.
    pub async fn list(&self, options: &PageOptions) -> Result<ResourcePage<Trigger>> {
        Ok(self
            .client
            .get::<CursorPage<Trigger>>(&["triggers"], &options.query())
            .await?
            .into())
    }
    /// Read one owned Trigger, verifying its identity.
    pub async fn read(&self, id: &str) -> Result<Trigger> {
        let result: Trigger = self.client.get(&["triggers", id], &[]).await?;
        identity(&result.id, id)?;
        Ok(result)
    }
    /// Create a draft; this neither publishes nor starts work.
    pub async fn create(&self, content: &TriggerContent, key: &IdempotencyKey) -> Result<Trigger> {
        validate_content(content)?;
        self.client
            .command(
                Method::POST,
                &["triggers"],
                Some(&serde_json::json!({"content":content})),
                key,
                StatusCode::CREATED,
            )
            .await
    }
    /// Replace an exact draft version.
    pub async fn save(
        &self,
        id: &str,
        draft_version: u64,
        content: &TriggerContent,
        key: &IdempotencyKey,
    ) -> Result<Trigger> {
        positive(draft_version)?;
        validate_content(content)?;
        let result: Trigger = self
            .client
            .command(
                Method::PUT,
                &["triggers", id],
                Some(&serde_json::json!({"draft_version":draft_version,"content":content})),
                key,
                StatusCode::OK,
            )
            .await?;
        identity(&result.id, id)?;
        Ok(result)
    }
    /// Delete an unpublished draft; published Triggers receive a typed refusal.
    pub async fn delete(&self, id: &str, key: &IdempotencyKey) -> Result<()> {
        self.client
            .empty(Request::command(
                Method::DELETE,
                &["triggers", id],
                None::<&()>,
                key,
                StatusCode::NO_CONTENT,
            )?)
            .await
    }
    /// Publish one exact draft version as an immutable revision.
    pub async fn publish(
        &self,
        id: &str,
        draft_version: u64,
        key: &IdempotencyKey,
    ) -> Result<PublishedTriggerRevision> {
        positive(draft_version)?;
        let result: PublishedTriggerRevision = self
            .client
            .command(
                Method::POST,
                &["triggers", id, "publish"],
                Some(&serde_json::json!({"draft_version":draft_version})),
                key,
                StatusCode::CREATED,
            )
            .await?;
        identity(&result.trigger_id, id)?;
        Ok(result)
    }
    /// Retire a Trigger without deleting historical revisions or Launches.
    pub async fn retire(&self, id: &str, key: &IdempotencyKey) -> Result<Trigger> {
        let result: Trigger = self
            .client
            .command(
                Method::POST,
                &["triggers", id, "retire"],
                None::<&()>,
                key,
                StatusCode::OK,
            )
            .await?;
        identity(&result.id, id)?;
        Ok(result)
    }
    /// Read immutable published revisions.
    pub async fn list_revisions(&self, id: &str) -> Result<Vec<TriggerRevision>> {
        self.client.get(&["triggers", id, "revisions"], &[]).await
    }
    /// List active placements, optionally retaining retired entries.
    pub async fn list_placements(&self, id: &str, include_retired: bool) -> Result<Vec<Placement>> {
        self.client
            .get(
                &["triggers", id, "placements"],
                &[("include_retired", include_retired.to_string())],
            )
            .await
    }
    /// Register one source URL and publisher label.
    pub async fn create_placement(
        &self,
        id: &str,
        source_url: &str,
        label: &str,
        key: &IdempotencyKey,
    ) -> Result<Placement> {
        self.client
            .command(
                Method::POST,
                &["triggers", id, "placements"],
                Some(&serde_json::json!({"source_url":source_url,"label":label})),
                key,
                StatusCode::CREATED,
            )
            .await
    }
    /// Retire one exact placement.
    pub async fn retire_placement(
        &self,
        id: &str,
        placement_id: &str,
        key: &IdempotencyKey,
    ) -> Result<Placement> {
        let result: Placement = self
            .client
            .command(
                Method::POST,
                &["triggers", id, "placements", placement_id, "retire"],
                None::<&()>,
                key,
                StatusCode::OK,
            )
            .await?;
        identity(&result.id, placement_id)?;
        Ok(result)
    }
    /// Fetch server-authored snippets for a registered placement.
    pub async fn embed(&self, id: &str, placement_id: &str) -> Result<EmbedDescriptor> {
        self.client
            .get(
                &["triggers", id, "embed"],
                &[("placement_id", placement_id.into())],
            )
            .await
    }
    /// Read aggregate and per-placement launch outcomes.
    pub async fn stats(&self, id: &str) -> Result<Statistics> {
        self.client.get(&["triggers", id, "stats"], &[]).await
    }
}

fn validate_content(content: &TriggerContent) -> Result<()> {
    if content.target.inline.is_some() {
        return Ok(());
    }
    for reference in [
        &content.target.sandbox_profile,
        &content.target.workspace_profile,
        &content.target.agent_profile,
    ] {
        positive(u64::from(reference.revision))?;
    }
    Ok(())
}

impl Client {
    /// Access owned Trigger drafts, publications, placements, and embeds.
    pub fn triggers(&self) -> Triggers<'_> {
        Triggers { client: self }
    }
    /// Read a public preview without sending the configured bearer credential.
    pub async fn read_trigger_preview(&self, public_id: &str) -> Result<TriggerPreview> {
        let segments = ["triggers", "public", public_id];
        let mut request = Request::get(&segments);
        request.anonymous = true;
        self.request(request).await
    }
}
