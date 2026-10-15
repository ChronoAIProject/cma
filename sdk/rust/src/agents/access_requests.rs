//! Durable owner-approved Agent service access.
use super::Agents;
use crate::{Error, IdempotencyKey, Result};
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

string_enum! { /// Durable progress; only succeeded means runtime access was verified.
    pub enum AgentAccessStatus {
        Pending => "pending", Connecting => "connecting", Applying => "applying",
        Succeeded => "succeeded", Denied => "denied", Failed => "failed"
    }
}
/// Owner-bound connection target for an approved access request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentAccessConnectionTarget {
    /// Existing durable access request.
    AgentAccessRequest {
        /// Request identity.
        id: String,
    },
}
/// Connection recovery is separate from successful runtime verification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentAccessConnection {
    /// Verified owner target for service connection flow.
    pub target: AgentAccessConnectionTarget,
}
/// Durable request and its current progress.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentAccessRequest {
    /// Durable access-request identity.
    pub id: String,
    /// Exact originating Agent.
    pub agent_id: String,
    /// Current durable state.
    pub status: AgentAccessStatus,
    /// Requested or approved service identities.
    pub services: Vec<String>,
    /// Explanation supplied by the originating Agent.
    pub reason: String,
    /// Failed predecessor retained when this new consent attempt was requested.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_of: Option<String>,
    /// Connection recovery while an approved request is connecting or applying.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection: Option<AgentAccessConnection>,
    /// Safe terminal failure detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
/// Owner-visible requests for one Agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentAccessRequestList {
    /// Distinct durable requests.
    pub items: Vec<AgentAccessRequest>,
}
/// Exact service selection explicitly approved by the owner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApproveAgentAccessRequest {
    /// Between 1 and 32 ASCII service identities, each at most 128 bytes.
    pub services: Vec<String>,
}
/// Owner-approved replacement of shared Sandbox service access.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateAgentServiceAccessRequest {
    /// Exact desired services, including required baseline services.
    pub services: Vec<String>,
    /// Previously observed services; a stale selection is refused without changes.
    pub expected_services: Vec<String>,
}
fn valid_services(services: &[String]) -> bool {
    !services.is_empty()
        && services.len() <= 32
        && services.iter().all(|service| {
            !service.is_empty()
                && service.len() <= 128
                && service
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
        })
}
impl AgentAccessRequest {
    fn validate(&self, agent_id: &str, request_id: Option<&str>) -> Result<()> {
        if self.id.is_empty()
            || self.retry_of.as_ref().is_some_and(|id| {
                id == &self.id
                    || id.len() != 68
                    || !id.starts_with("aar_")
                    || !id[4..]
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            })
            || self.agent_id != agent_id
            || request_id.is_some_and(|id| id != self.id)
            || !valid_services(&self.services)
            || (self.status == AgentAccessStatus::Connecting && self.connection.is_none())
            || self
                .connection
                .as_ref()
                .is_some_and(|connection| match &connection.target {
                    AgentAccessConnectionTarget::AgentAccessRequest { id } => id != &self.id,
                })
        {
            return Err(Error::protocol(
                "access request identity or connection did not match the request",
            ));
        }
        Ok(())
    }
}
impl Agents<'_> {
    /// Update shared access in place; preserve the exact body and key after an uncertain reply.
    pub async fn update_service_access(
        &self,
        agent_id: &str,
        body: &UpdateAgentServiceAccessRequest,
        key: &IdempotencyKey,
    ) -> Result<AgentAccessRequest> {
        if !valid_services(&body.services) || !valid_services(&body.expected_services) {
            return Err(Error::validation(
                "expected 1 to 32 bounded service identities",
            ));
        }
        let result: AgentAccessRequest = self
            .client
            .command(
                Method::PUT,
                &["agents", agent_id, "service-access"],
                Some(body),
                key,
                StatusCode::ACCEPTED,
            )
            .await?;
        result.validate(agent_id, None)?;
        Ok(result)
    }
    /// Read durable requests without granting access or resuming an Agent.
    pub async fn access_requests(&self, agent_id: &str) -> Result<AgentAccessRequestList> {
        let page: AgentAccessRequestList = self
            .client
            .get(&["agents", agent_id, "access-requests"], &[])
            .await?;
        let mut ids = HashSet::new();
        for item in &page.items {
            item.validate(agent_id, None)?;
            if !ids.insert(&item.id) {
                return Err(Error::protocol(
                    "access requests contained repeated identities",
                ));
            }
        }
        Ok(page)
    }
    /// Approve exact services; retry uncertain replies with the original body and key.
    pub async fn approve_access_request(
        &self,
        agent_id: &str,
        request_id: &str,
        body: &ApproveAgentAccessRequest,
        key: &IdempotencyKey,
    ) -> Result<AgentAccessRequest> {
        if !valid_services(&body.services) {
            return Err(Error::validation(
                "expected 1 to 32 bounded service identities",
            ));
        }
        let result: AgentAccessRequest = self
            .client
            .command(
                Method::POST,
                &["agents", agent_id, "access-requests", request_id, "approve"],
                Some(body),
                key,
                StatusCode::OK,
            )
            .await?;
        result.validate(agent_id, Some(request_id))?;
        Ok(result)
    }
    /// Explicitly deny a request; preserves the provided command identity.
    pub async fn deny_access_request(
        &self,
        agent_id: &str,
        request_id: &str,
        key: &IdempotencyKey,
    ) -> Result<AgentAccessRequest> {
        let result: AgentAccessRequest = self
            .client
            .command(
                Method::POST,
                &["agents", agent_id, "access-requests", request_id, "deny"],
                Some(&serde_json::json!({})),
                key,
                StatusCode::OK,
            )
            .await?;
        result.validate(agent_id, Some(request_id))?;
        Ok(result)
    }
}
