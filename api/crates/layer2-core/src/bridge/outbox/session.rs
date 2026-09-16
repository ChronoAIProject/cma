use std::time::Duration;

use crate::resource_access::ResourceOwner;
use crate::sandbox::{Agent, AgentCreationOrigin, AgentCreationProvenance, Sandbox};
use crate::AgentId;

use super::{ChannelReplyToolRequest, OutboxError};

/// Only the trusted runtime capability store supplies this observation after
/// authenticating its opaque bearer and exact opened runtime incarnation.
pub struct ChannelReplySessionObservation<'a> {
    pub agent: &'a Agent,
    pub sandbox: &'a Sandbox,
    pub provenance: &'a AgentCreationProvenance,
    pub capability_incarnation: &'a str,
    pub runtime_generation: u64,
    pub deadline: tokio::time::Instant,
}

/// Session authentication permits recovery of a prior invocation, including
/// after the originating turn finishes. It never permits a new reply by itself.
pub struct ChannelReplySessionAuthorization {
    agent_id: AgentId,
    owner: ResourceOwner,
    bridge_id: String,
    agent_created_at: String,
    creation_fingerprint: String,
    capability_incarnation: String,
    runtime_generation: u64,
    namespace: String,
    deadline: tokio::time::Instant,
}

impl ChannelReplySessionAuthorization {
    pub fn from_current(observed: ChannelReplySessionObservation<'_>) -> Result<Self, OutboxError> {
        let ChannelReplySessionObservation {
            agent,
            sandbox,
            provenance,
            capability_incarnation,
            runtime_generation,
            deadline,
        } = observed;
        if deadline <= tokio::time::Instant::now()
            || !super::id(capability_incarnation)
            || runtime_generation == 0
            || agent.conversation.runtime_generation != Some(runtime_generation)
        {
            return Err(OutboxError::ToolUnavailable);
        }
        let AgentCreationOrigin::ChannelBridge { bridge_id, .. } = &provenance.creation_origin
        else {
            return Err(OutboxError::ToolUnavailable);
        };
        provenance
            .validate_against(agent, sandbox)
            .map_err(|_| OutboxError::ToolUnavailable)?;
        Ok(Self {
            agent_id: agent.id.clone(),
            owner: provenance.owner.clone(),
            bridge_id: bridge_id.clone(),
            agent_created_at: agent.created_at.clone(),
            creation_fingerprint: agent.creation_fingerprint.clone(),
            capability_incarnation: capability_incarnation.into(),
            runtime_generation,
            namespace: namespace(provenance, capability_incarnation)?,
            deadline: deadline.min(tokio::time::Instant::now() + Duration::from_secs(15)),
        })
    }
    pub fn agent_id(&self) -> &AgentId {
        &self.agent_id
    }
    pub fn owner(&self) -> &ResourceOwner {
        &self.owner
    }
    pub fn bridge_id(&self) -> &str {
        &self.bridge_id
    }
    pub fn agent_created_at(&self) -> &str {
        &self.agent_created_at
    }
    pub fn creation_fingerprint(&self) -> &str {
        &self.creation_fingerprint
    }
    pub fn capability_incarnation(&self) -> &str {
        &self.capability_incarnation
    }
    pub fn runtime_generation(&self) -> u64 {
        self.runtime_generation
    }
    pub fn namespace(&self) -> &str {
        &self.namespace
    }
    pub fn deadline(&self) -> tokio::time::Instant {
        self.deadline
    }
    pub fn require_current(&self) -> Result<(), OutboxError> {
        if tokio::time::Instant::now() >= self.deadline {
            return Err(OutboxError::Expired);
        }
        Ok(())
    }
    pub fn require_snapshot(
        &self,
        observed: ChannelReplySessionObservation<'_>,
    ) -> Result<(), OutboxError> {
        self.require_current()?;
        let fresh = Self::from_current(observed)?;
        if self.namespace != fresh.namespace
            || self.agent_id != fresh.agent_id
            || self.owner != fresh.owner
            || self.bridge_id != fresh.bridge_id
            || self.runtime_generation != fresh.runtime_generation
        {
            return Err(OutboxError::SourceMismatch);
        }
        Ok(())
    }
    /// The lookup ID carries no authority; the store must revalidate this proof
    /// before exposing the durable receipt, and compare the original text hash.
    pub fn request_identity(
        &self,
        request: &ChannelReplyToolRequest,
    ) -> Result<(String, String), OutboxError> {
        self.require_current()?;
        request_identity(&self.namespace, request)
    }
}

pub(super) fn namespace(
    provenance: &AgentCreationProvenance,
    capability_incarnation: &str,
) -> Result<String, OutboxError> {
    super::hash(&(
        "cma.bridge.reply-tool.namespace.v1",
        &provenance.agent_id,
        &provenance.agent_created_at,
        &provenance.creation_fingerprint,
        capability_incarnation,
    ))
}

pub(super) fn request_identity(
    namespace: &str,
    request: &ChannelReplyToolRequest,
) -> Result<(String, String), OutboxError> {
    if !super::id(&request.request_key)
        || request.text.trim().is_empty()
        || request.text.len() > 1_048_576
    {
        return Err(OutboxError::Bounds);
    }
    Ok((
        invocation_id(namespace, &request.request_key)?,
        super::hash(&request.text)?,
    ))
}

pub(super) fn invocation_id(namespace: &str, request_key: &str) -> Result<String, OutboxError> {
    if !super::id(request_key) {
        return Err(OutboxError::Bounds);
    }
    Ok(format!(
        "brti_{}",
        super::hash(&("cma.bridge.reply-tool.v1", namespace, request_key))?
    ))
}
