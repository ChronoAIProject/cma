//! Agent REST commands and durable read projections, without live subscriptions.

use crate::{
    Client, EventOptions, IdempotencyKey, PageOptions, ResourcePage, Result,
    instances::AgentStatus,
    resources::{identity, positive},
};
use reqwest::{Method, StatusCode};

mod events;
mod types;
pub use events::*;
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

/// Owner-facing Agent operations.
pub struct Agents<'a> {
    client: &'a Client,
}

impl Agents<'_> {
    /// List owned Agents.
    pub async fn list(&self, options: &AgentListOptions) -> Result<ResourcePage<AgentSummary>> {
        let mut query = options.page.query();
        if let Some(id) = &options.sandbox {
            query.push(("sandbox", id.clone()));
        }
        if let Some(id) = &options.workspace {
            query.push(("workspace", id.clone()));
        }
        if let Some(status) = options.status {
            query.push(("status", status.as_str().into()));
        }
        self.client.get(&["agents"], &query).await
    }
    /// Read an Agent and its full parent hierarchy.
    pub async fn read(&self, id: &str) -> Result<Agent> {
        let value: Agent = self.client.get(&["agents", id], &[]).await?;
        identity(&value.id, id)?;
        Ok(value)
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
