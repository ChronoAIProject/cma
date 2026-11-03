//! Agent REST commands and durable read projections, without live subscriptions.

use crate::{
    Client, EventOptions, IdempotencyKey, PageOptions, ResourcePage, Result,
    instances::AgentStatus,
    resources::{identity, positive},
};
use reqwest::{Method, StatusCode};

mod access_requests;
pub use access_requests::*;
mod artifacts;
mod attachments;
pub use attachments::*;
mod channel_usage;
mod deliverables;
mod events;
mod feedback_reasons;
mod lifecycle;
mod message_feedback;
mod models;
mod preferences;
mod provenance;
pub use preferences::*;
mod settings;
pub use feedback_reasons::*;
pub use message_feedback::*;
mod shared;
mod skills;
mod types;
pub use artifacts::*;
pub use channel_usage::*;
pub use deliverables::*;
pub use events::*;
pub use lifecycle::*;
pub use models::*;
pub use provenance::*;
pub use settings::*;
pub use shared::*;
pub use skills::*;
pub use types::*;

/// Owner Agent list filters and opaque pagination.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentListOptions {
    /// Optional Sandbox identifier filter.
    pub sandbox: Option<String>,
    /// Optional Workspace identifier filter.
    pub workspace: Option<String>,
    /// Exact status filter.
    pub status: Option<AgentStatus>,
    /// Opaque cursor and maximum page size.
    pub page: PageOptions,
}

/// Organization-scoped Agent filters; omission preserves personal inventory.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AccessibleAgentListOptions {
    /// Canonical organization ID to inspect under current membership.
    pub organization: Option<String>,
    /// Status, parent and cursor filters.
    pub filters: AgentListOptions,
}
impl AgentListOptions {
    fn query(&self) -> Vec<(&'static str, String)> {
        let mut query = self.page.query();
        if let Some(id) = &self.sandbox {
            query.push(("sandbox", id.clone()));
        }
        if let Some(id) = &self.workspace {
            query.push(("workspace", id.clone()));
        }
        if let Some(status) = self.status {
            query.push(("status", status.as_str().into()));
        }
        query
    }
}

/// Owner-facing Agent operations.
pub struct Agents<'a> {
    client: &'a Client,
}

impl Agents<'_> {
    /// List safe creation facts, including retained archived Agents. A filtered
    /// page may be empty; keep following `next` until it is absent.
    pub async fn provenance_inventory(
        &self,
        options: &AgentProvenanceInventoryOptions,
    ) -> Result<ResourcePage<AgentProvenanceInventoryItem>> {
        let mut query = options.scope.filters.query();
        if options.shared {
            query.push(("shared", "true".into()));
        }
        if let Some(id) = &options.scope.organization {
            query.push(("organization", id.clone()));
        }
        if let Some(category) = options.creation_origin {
            query.push(("creation_origin", category.as_str().into()));
        }
        if let Some(status) = options.task_status {
            query.push(("task_status", status.as_str().into()));
        }
        if let Some(active) = options.channel_active {
            query.push(("channel_active", active.to_string()));
        }
        if let Some(history) = options.channel_history {
            query.push(("channel_history", history.to_string()));
        }
        self.client.get(&["agents", "provenance"], &query).await
    }
    /// List owned Agents.
    pub async fn list(&self, options: &AgentListOptions) -> Result<ResourcePage<AgentSummary>> {
        let query = options.query();
        self.client.get(&["agents"], &query).await
    }
    /// List personal Agents or Agents accessible in an explicit organization.
    pub async fn list_accessible(
        &self,
        options: &AccessibleAgentListOptions,
    ) -> Result<ResourcePage<AccessibleAgentSummary>> {
        let mut query = options.filters.query();
        if let Some(id) = &options.organization {
            query.push(("organization", id.clone()));
        }
        self.client.get(&["agents"], &query).await
    }
    /// Read an Agent and its full parent hierarchy.
    pub async fn read(&self, id: &str) -> Result<Agent> {
        let value: Agent = self.client.get(&["agents", id], &[]).await?;
        identity(&value.id, id)?;
        Ok(value)
    }
    /// Read personal full detail or the safe Agent-only shared projection.
    pub async fn read_accessible(&self, id: &str) -> Result<AccessibleAgent> {
        let value: AccessibleAgent = self.client.get(&["agents", id], &[]).await?;
        identity(value.id(), id)?;
        Ok(value)
    }
    /// Read immutable creation facts; details require current resource management.
    pub async fn provenance(&self, id: &str) -> Result<AgentProvenance> {
        let value: AgentProvenance = self.client.get(&["agents", id, "provenance"], &[]).await?;
        identity(&value.agent_id, id)?;
        Ok(value)
    }
    /// Read creation provenance and current published Trigger shortcuts.
    pub async fn source(&self, id: &str) -> Result<AgentSource> {
        match self.source_accessible(id).await? {
            AccessibleAgentSource::Personal(value) => Ok(value),
            AccessibleAgentSource::Shared(_) => Err(crate::Error::protocol(
                "shared Agent source requires source_accessible",
            )),
        }
    }
    /// Read personal source details or a safe shared creation category.
    pub async fn source_accessible(&self, id: &str) -> Result<AccessibleAgentSource> {
        let value: AccessibleAgentSource = self.client.get(&["agents", id, "source"], &[]).await?;
        identity(value.agent_id(), id)?;
        Ok(value)
    }
    /// Read the named unsupported-terminal refusal; this release has no terminal plane.
    pub async fn terminal(&self, id: &str) -> Result<()> {
        self.client
            .refusal(crate::http::Request::get(&["agents", id, "terminal"]))
            .await
    }
    /// Create an Agent inside an existing Workspace; readiness is asynchronous.
    pub async fn create(
        &self,
        workspace_id: &str,
        body: &CreateAgentRequest,
        key: &IdempotencyKey,
    ) -> Result<Agent> {
        positive(u64::from(body.agent_profile.revision))?;
        self.client
            .command(
                Method::POST,
                &["workspaces", workspace_id, "agents"],
                Some(body),
                key,
                StatusCode::CREATED,
            )
            .await
    }
    /// Submit an idempotent owner message and return its queue acknowledgement.
    pub async fn input(
        &self,
        id: &str,
        body: &AgentTextRequest,
        key: &IdempotencyKey,
    ) -> Result<InputAccepted> {
        self.client
            .command(
                Method::POST,
                &["agents", id, "input"],
                Some(body),
                key,
                StatusCode::ACCEPTED,
            )
            .await
    }
    /// Submit steering input for the active turn.
    pub async fn steer(
        &self,
        id: &str,
        body: &AgentTextRequest,
        key: &IdempotencyKey,
    ) -> Result<()> {
        self.client
            .command_void(Method::POST, &["agents", id, "steer"], Some(body), key)
            .await
    }
    /// Request interruption of the active turn.
    pub async fn interrupt(&self, id: &str, key: &IdempotencyKey) -> Result<()> {
        self.client
            .command_void(Method::POST, &["agents", id, "interrupt"], None::<&()>, key)
            .await
    }
    /// Resolve one exact approval identifier.
    pub async fn answer_approval(
        &self,
        id: &str,
        request_id: &str,
        body: &ApprovalDecisionRequest,
        key: &IdempotencyKey,
    ) -> Result<()> {
        self.client
            .command_void(
                Method::POST,
                &["agents", id, "approvals", request_id],
                Some(body),
                key,
            )
            .await
    }
    /// Schedule an owner-requested stop.
    pub async fn stop(
        &self,
        id: &str,
        body: &StopAgentRequest,
        key: &IdempotencyKey,
    ) -> Result<()> {
        self.client
            .command_void(Method::POST, &["agents", id, "stop"], Some(body), key)
            .await
    }
    /// Schedule deletion of an owned Agent.
    pub async fn delete(&self, id: &str, key: &IdempotencyKey) -> Result<()> {
        self.client
            .command_void(Method::DELETE, &["agents", id], None::<&()>, key)
            .await
    }
    /// Read a bounded page of durable events, not a live stream.
    pub async fn events(
        &self,
        id: &str,
        options: EventOptions,
    ) -> Result<ResourcePage<AgentEvent, u64>> {
        self.client
            .get(&["agents", id, "events"], &options.query())
            .await
    }
    /// Read the durable transcript through its returned sequence.
    pub async fn transcript(&self, id: &str) -> Result<AgentTranscript> {
        let value: AgentTranscript = self.client.get(&["agents", id, "transcript"], &[]).await?;
        identity(&value.agent_id, id)?;
        Ok(value)
    }
    #[doc = "Read durable events including bounded reported tool output when supported."]
    pub async fn events_with_output(
        &self,
        id: &str,
        options: EventOptions,
    ) -> Result<ResourcePage<AgentEvent, u64>> {
        let mut query = options.query();
        query.push(("tool_output", "true".into()));
        self.client.get(&["agents", id, "events"], &query).await
    }
    /// Read durably recorded model token usage.
    pub async fn usage(&self, id: &str) -> Result<AgentUsage> {
        let value: AgentUsage = self.client.get(&["agents", id, "usage"], &[]).await?;
        identity(&value.agent_id, id)?;
        Ok(value)
    }
    /// Read runtime counters attributed to the Agent's Sandbox.
    pub async fn resources(&self, id: &str) -> Result<AgentResources> {
        let value: AgentResources = self.client.get(&["agents", id, "resources"], &[]).await?;
        identity(&value.agent_id, id)?;
        Ok(value)
    }
}

impl Client {
    /// Access Agent REST resources and commands.
    pub fn agents(&self) -> Agents<'_> {
        Agents { client: self }
    }
}
