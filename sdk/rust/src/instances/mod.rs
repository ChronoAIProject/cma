//! Owner-scoped Sandbox, Workspace, and Worktree resources.

use reqwest::{Method, StatusCode};

use crate::{
    Client, EventOptions, IdempotencyKey, PageOptions, ResourceList, ResourcePage, Result,
    resources::{identity, positive},
};

mod commands;
mod events;
pub use commands::*;
mod lifecycle;
mod runtime_update;
mod shared;
pub use runtime_update::*;
mod types;
pub use events::*;
pub use lifecycle::*;
pub use shared::*;
pub use types::*;

/// Pagination and optional status filter for owned Sandboxes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SandboxListOptions {
    /// Opaque cursor and maximum page size.
    pub page: PageOptions,
    /// Exact lifecycle status filter.
    pub status: Option<SandboxStatus>,
    /// Exclude retired history before pagination; defaults to false.
    pub exclude_retired: bool,
}

/// Personal or explicit current organization Sandbox inventory.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AccessibleSandboxListOptions {
    /// Canonical organization ID; omission selects personal resources.
    pub organization: Option<String>,
    /// Explicit incoming whole-Sandbox grants, mutually exclusive with organization.
    pub shared: bool,
    /// Pagination and status filters.
    pub filters: SandboxListOptions,
}

/// Owner-facing Sandbox resources and lifecycle commands.
pub struct Sandboxes<'a> {
    client: &'a Client,
}

impl Sandboxes<'_> {
    /// List personally owned Sandboxes or current organization-visible metadata.
    pub async fn list_accessible(
        &self,
        options: &AccessibleSandboxListOptions,
    ) -> Result<ResourcePage<AccessibleSandboxSummary>> {
        if options.shared && options.organization.is_some() {
            return Err(crate::Error::validation(
                "choose organization or shared inventory",
            ));
        }
        let mut query = options.filters.page.query();
        if let Some(status) = options.filters.status {
            query.push(("status", status.as_str().into()));
        }
        if options.filters.exclude_retired {
            query.push(("exclude_retired", "true".into()));
        }
        if let Some(organization) = &options.organization {
            query.push(("organization", organization.clone()));
        }
        if options.shared {
            query.push(("shared", "true".into()));
        }
        self.client.get(&["sandboxes"], &query).await
    }

    /// Read personal detail or shared metadata and authorized child counts.
    pub async fn read_accessible(&self, id: &str) -> Result<AccessibleSandboxDetail> {
        let value: AccessibleSandboxDetail = self.client.get(&["sandboxes", id], &[]).await?;
        identity(value.id(), id)?;
        Ok(value)
    }
    /// List owned Sandboxes with hierarchy counts.
    pub async fn list(&self, options: &SandboxListOptions) -> Result<ResourcePage<SandboxSummary>> {
        let mut query = options.page.query();
        if let Some(status) = options.status {
            query.push(("status", status.as_str().into()));
        }
        if options.exclude_retired {
            query.push(("exclude_retired", "true".into()));
        }
        self.client.get(&["sandboxes"], &query).await
    }
    /// Read one Sandbox and its Workspace summaries.
    pub async fn read(&self, id: &str) -> Result<SandboxDetail> {
        let value: SandboxDetail = self.client.get(&["sandboxes", id], &[]).await?;
        identity(&value.summary.sandbox.id, id)?;
        Ok(value)
    }
    /// Begin provisioning. HTTP 201 does not mean the Sandbox is Active.
    pub async fn create(
        &self,
        body: &CreateSandboxRequest,
        key: &IdempotencyKey,
    ) -> Result<Sandbox> {
        positive(u64::from(body.sandbox_profile.revision))?;
        self.client
            .command(
                Method::POST,
                &["sandboxes"],
                Some(body),
                key,
                StatusCode::CREATED,
            )
            .await
    }
    /// Begin retirement, returning the accepted lifecycle state.
    pub async fn delete(&self, id: &str, key: &IdempotencyKey) -> Result<Sandbox> {
        self.lifecycle(id, &["sandboxes", id], Method::DELETE, key)
            .await
    }
    /// Begin a pause without changing Agent identity or history.
    pub async fn pause(&self, id: &str, key: &IdempotencyKey) -> Result<Sandbox> {
        self.lifecycle(id, &["sandboxes", id, "pause"], Method::POST, key)
            .await
    }
    /// Begin revival from durable Sandbox state.
    pub async fn revive(&self, id: &str, key: &IdempotencyKey) -> Result<Sandbox> {
        self.lifecycle(id, &["sandboxes", id, "revive"], Method::POST, key)
            .await
    }
    /// Add explicit service access without changing the pinned profile or starting work.
    pub async fn grant_service_access(
        &self,
        id: &str,
        body: &GrantSandboxServiceAccessRequest,
        key: &IdempotencyKey,
    ) -> Result<Sandbox> {
        let value: Sandbox = self
            .client
            .command(
                Method::POST,
                &["sandboxes", id, "service-access"],
                Some(body),
                key,
                StatusCode::OK,
            )
            .await?;
        identity(&value.id, id)?;
        Ok(value)
    }
    /// Read a bounded array of durable Sandbox events, not a live stream.
    pub async fn events(&self, id: &str, options: EventOptions) -> Result<Vec<SandboxEvent>> {
        let events: Vec<SandboxEvent> = self
            .client
            .get(&["sandboxes", id, "events"], &options.query())
            .await?;
        for event in &events {
            identity(&event.sandbox_id, id)?;
        }
        Ok(events)
    }
    async fn lifecycle(
        &self,
        id: &str,
        path: &[&str],
        method: Method,
        key: &IdempotencyKey,
    ) -> Result<Sandbox> {
        let value: Sandbox = self
            .client
            .command(method, path, None::<&()>, key, StatusCode::ACCEPTED)
            .await?;
        identity(&value.id, id)?;
        Ok(value)
    }
}

/// Workspaces always belong to a Sandbox.
pub struct Workspaces<'a> {
    client: &'a Client,
}

impl Workspaces<'_> {
    /// Prepare a Workspace from an inline source; readiness is asynchronous.
    pub async fn create_inline(
        &self,
        sandbox_id: &str,
        body: &CreateInlineWorkspaceRequest,
        key: &IdempotencyKey,
    ) -> Result<Workspace> {
        self.client
            .command(
                Method::POST,
                &["sandboxes", sandbox_id, "workspaces", "inline"],
                Some(body),
                key,
                StatusCode::CREATED,
            )
            .await
    }
    /// Retry original preparation only for a failed, never-ready Workspace.
    /// The same key replays; use a new key only after a terminal failed attempt.
    pub async fn recover_prepare(&self, id: &str, key: &IdempotencyKey) -> Result<Workspace> {
        let value: Workspace = self
            .client
            .command::<_, ()>(
                Method::POST,
                &["workspaces", id, "recover-prepare"],
                None,
                key,
                StatusCode::ACCEPTED,
            )
            .await?;
        identity(&value.id, id)?;
        Ok(value)
    }

    /// List personal/shared summaries under current parent Sandbox access.
    pub async fn list_accessible(
        &self,
        sandbox_id: &str,
    ) -> Result<ResourceList<AccessibleWorkspaceSummary>> {
        self.client
            .get(&["sandboxes", sandbox_id, "workspaces"], &[])
            .await
    }
    /// Read personal detail or a safe shared Workspace hierarchy.
    pub async fn read_accessible(&self, id: &str) -> Result<AccessibleWorkspaceDetail> {
        let value: AccessibleWorkspaceDetail = self.client.get(&["workspaces", id], &[]).await?;
        identity(value.id(), id)?;
        Ok(value)
    }
    /// List Workspace summaries in an owned Sandbox.
    pub async fn list(&self, sandbox_id: &str) -> Result<ResourceList<WorkspaceSummary>> {
        self.client
            .get(&["sandboxes", sandbox_id, "workspaces"], &[])
            .await
    }
    /// Read one Workspace with its Worktrees and Agents.
    pub async fn read(&self, id: &str) -> Result<WorkspaceDetail> {
        let value: WorkspaceDetail = self.client.get(&["workspaces", id], &[]).await?;
        identity(&value.workspace.id, id)?;
        Ok(value)
    }
    /// Begin preparing a Workspace; HTTP 201 is not a readiness acknowledgement.
    pub async fn create(
        &self,
        sandbox_id: &str,
        body: &CreateWorkspaceRequest,
        key: &IdempotencyKey,
    ) -> Result<Workspace> {
        positive(u64::from(body.workspace_profile.revision))?;
        self.client
            .command(
                Method::POST,
                &["sandboxes", sandbox_id, "workspaces"],
                Some(body),
                key,
                StatusCode::CREATED,
            )
            .await
    }
    /// Schedule Workspace removal.
    pub async fn delete(&self, id: &str, key: &IdempotencyKey) -> Result<Workspace> {
        let value: Workspace = self
            .client
            .command(
                Method::DELETE,
                &["workspaces", id],
                None::<&()>,
                key,
                StatusCode::ACCEPTED,
            )
            .await?;
        identity(&value.id, id)?;
        Ok(value)
    }
}

/// Read-only Worktree resources; creation and deletion follow Agent lifecycle.
pub struct Worktrees<'a> {
    client: &'a Client,
}

impl Worktrees<'_> {
    /// List personal/shared metadata under current parent Workspace access.
    pub async fn list_accessible(
        &self,
        workspace_id: &str,
    ) -> Result<ResourceList<AccessibleWorktree>> {
        self.client
            .get(&["workspaces", workspace_id, "worktrees"], &[])
            .await
    }
    /// Read a personal Worktree or safe shared metadata.
    pub async fn read_accessible(&self, id: &str) -> Result<AccessibleWorktree> {
        let value: AccessibleWorktree = self.client.get(&["worktrees", id], &[]).await?;
        identity(value.id(), id)?;
        Ok(value)
    }
    /// List Worktrees allocated within an owned Workspace.
    pub async fn list(&self, workspace_id: &str) -> Result<ResourceList<Worktree>> {
        self.client
            .get(&["workspaces", workspace_id, "worktrees"], &[])
            .await
    }
    /// Read one owned Worktree.
    pub async fn read(&self, id: &str) -> Result<Worktree> {
        let value: Worktree = self.client.get(&["worktrees", id], &[]).await?;
        identity(&value.id, id)?;
        Ok(value)
    }
}

impl Client {
    /// Access Sandbox resources and lifecycle commands.
    pub fn sandboxes(&self) -> Sandboxes<'_> {
        Sandboxes { client: self }
    }
    /// Access Workspace resources.
    pub fn workspaces(&self) -> Workspaces<'_> {
        Workspaces { client: self }
    }
    /// Access read-only Worktree resources.
    pub fn worktrees(&self) -> Worktrees<'_> {
        Worktrees { client: self }
    }
}
