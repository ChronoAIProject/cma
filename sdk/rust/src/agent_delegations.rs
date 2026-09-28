//! Provider access to CMA resources through event-bound user consent.
use crate::{Client, Result, bot_setups::BotSetupIdentity, resource_access::ResourceOwner};
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};

/// Resolve the source actor from an admitted event or CMA callback continuation.
#[derive(Clone, Serialize, Deserialize)]
pub struct AgentDelegationIdentityRequest {
    /// Opaque capability; never decode or log it.
    pub event_ref: String,
}

/// An ordinary local CMA operation with the linked user's authority.
#[derive(Clone, Serialize, Deserialize)]
pub struct AgentDelegationOperationRequest {
    /// Current event or callback continuation capability.
    pub event_ref: String,
    /// Omit for Personal; organization membership is rechecked by CMA.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<ResourceOwner>,
    /// Ordinary operation's HTTP method; CMA enforces its route allowlist.
    pub method: String,
    /// Canonical local CMA API path with optional ordinary query parameters.
    pub path: String,
    /// Underlying route's request body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<serde_json::Value>,
    /// Required for mutations; retain unchanged when retrying an uncertain result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
}

/// A broker response can contain an ordinary resource refusal in its inner status.
#[derive(Clone, Serialize, Deserialize)]
pub struct AgentDelegationOperationResult {
    /// Underlying operation's HTTP status, including 202 for Responses admission.
    pub status: u16,
    /// Underlying result, problem, or null for an empty result.
    pub body: serde_json::Value,
}

/// Requires the configured provider binding's credential.
pub struct AgentDelegations<'a>(&'a Client);
impl Client {
    /// Access the event-bound user delegation broker.
    pub fn agent_delegations(&self) -> AgentDelegations<'_> {
        AgentDelegations(self)
    }
}
impl AgentDelegations<'_> {
    /// Return linked identity or the explicit browser-consent URL, never credentials.
    pub async fn identity(
        &self,
        body: &AgentDelegationIdentityRequest,
    ) -> Result<BotSetupIdentity> {
        self.0
            .operation(
                Method::POST,
                &["agent-delegations", "identity"],
                Some(body),
                StatusCode::OK,
            )
            .await
    }

    /// Dispatch an ordinary CMA operation; inspect the inner result's HTTP status.
    pub async fn operate(
        &self,
        body: &AgentDelegationOperationRequest,
    ) -> Result<AgentDelegationOperationResult> {
        self.0
            .operation(
                Method::POST,
                &["agent-delegations", "operations"],
                Some(body),
                StatusCode::OK,
            )
            .await
    }
}
