use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::Duration};

use crate::bridge::{allocation::AllocationRecord, BridgeRecord};
use crate::sandbox::agent_io::{
    AdmittedAgentCommand, AgentResponseStatus, ChannelCommandHold, ChannelCommandSidecar,
};
use crate::sandbox::{Agent, AgentCreationOrigin, AgentCreationProvenance, Sandbox, SandboxStatus};

use super::*;

/// Trusted runtime boundary input, never tool arguments or a callback payload.
/// The enqueue store must re-read/fence all these facts atomically. A capability
/// incarnation is server-selected and retained across turns of this session.
pub struct ChannelReplyTurnObservation<'a> {
    pub source: OutboxSource,
    pub command: &'a AdmittedAgentCommand,
    pub sidecar: &'a ChannelCommandSidecar,
    pub allocation: &'a AllocationRecord,
    pub agent: &'a Agent,
    pub sandbox: &'a Sandbox,
    pub provenance: &'a AgentCreationProvenance,
    pub bridge: &'a BridgeRecord,
    pub capability_incarnation: &'a str,
    pub deadline: tokio::time::Instant,
}

/// Opaque, nonserializable permission for explicit reply calls in one current
/// Channel turn of a Channel-created Agent. It is not ordinary history access.
pub struct ChannelReplyTurnAuthorization {
    pub(super) source: OutboxSource,
    pub(super) namespace: String,
    pub(super) capability_incarnation: String,
    pub(super) context_reference: String,
    pub(super) turn_sequence: u64,
    pub(super) runtime_generation: u64,
    pub(super) operation_sequence: u64,
    pub(super) agent_created_at: String,
    pub(super) creation_fingerprint: String,
    pub(super) deadline: tokio::time::Instant,
}
impl ChannelReplyTurnAuthorization {
    pub fn from_current(observed: ChannelReplyTurnObservation<'_>) -> Result<Self, OutboxError> {
        let ChannelReplyTurnObservation {
            source,
            command,
            sidecar,
            allocation,
            agent,
            sandbox,
            provenance,
            bridge,
            capability_incarnation,
            deadline,
        } = observed;
        source.validate()?;
        source.require_bridge(bridge)?;
        command
            .validate()
            .map_err(|_| OutboxError::SourceMismatch)?;
        allocation
            .validate()
            .map_err(|_| OutboxError::SourceMismatch)?;
        if deadline <= tokio::time::Instant::now()
            || !super::id(capability_incarnation)
            || command.status != AgentResponseStatus::InProgress
            || super::hash(&command.receipt)? != source.admission_sha256
            || command.receipt.envelope.command_id != source.command_id
            || command.receipt.response_id.as_ref() != Some(&source.response_id)
            || sidecar.schema_version != 1
            || sidecar.command_id != source.command_id
            || sidecar.response_id != source.response_id
            || sidecar.inbox_id != source.inbox_id
            || sidecar.allocation_id != source.allocation_id
            || sidecar.body_sha256 != source.inbox_sha256
            || sidecar.request_fingerprint != command.receipt.envelope.request_fingerprint
            || allocation.id != source.allocation_id
            || allocation.owner != source.owner
            || allocation.placement.target() != &source.target
        {
            return Err(OutboxError::ToolUnavailable);
        }
        // Only the current Channel worker may release the durable input hold.
        if sidecar.execution_hold != ChannelCommandHold::Released {
            return Err(OutboxError::ToolUnavailable);
        }
        if !matches!(&provenance.creation_origin, AgentCreationOrigin::ChannelBridge { bridge_id, .. }
            if bridge_id == source.pin.bridge_id.as_str())
            || provenance.owner != source.owner
            || provenance.agent_id != source.target.agent_id
            || agent.id != source.target.agent_id
            || sandbox.id != source.target.sandbox_id
        {
            return Err(OutboxError::ToolUnavailable);
        }
        // No manual/Trigger/legacy origin is upgraded by later Bridge use.
        // The creation provenance must match the separately authorized creator.
        provenance
            .validate_against(agent, sandbox)
            .map_err(|_| OutboxError::ToolUnavailable)?;
        if matches!(
            allocation.placement,
            crate::bridge::allocation::AllocationPlacement::NewSandbox { .. }
        ) {
            let basis = crate::bridge::invoke::ChannelInvokeBasis {
                allocation: allocation.clone(),
                creation: provenance.clone(),
            };
            basis
                .require_bridge(bridge, &source.pin, &source.config)
                .map_err(|_| OutboxError::ToolUnavailable)?;
        }
        let dispatch = command
            .work
            .dispatch
            .as_ref()
            .ok_or(OutboxError::ToolUnavailable)?;
        if agent.conversation.active_turn != Some(dispatch.turn_sequence)
            || agent.conversation.runtime_generation != Some(dispatch.runtime_generation)
            || agent.requested_stop_reason.is_some()
            || !agent.status.is_live()
            || sandbox.lifecycle.checkpoint_capture.is_some()
            || !matches!(
                sandbox.status,
                SandboxStatus::Active | SandboxStatus::CredentialRotating
            )
            || !agent
                .conversation
                .operation
                .as_ref()
                .is_some_and(|operation| {
                    operation.sequence == dispatch.operation_sequence
                        && operation.turn_sequence == dispatch.turn_sequence
                        && operation.runtime_generation == dispatch.runtime_generation
                })
        {
            return Err(OutboxError::ToolUnavailable);
        }
        let namespace = super::session::namespace(provenance, capability_incarnation)?;
        let context_reference = format!(
            "brctx_{}",
            super::hash(&(
                "cma.bridge.reply-context.v1",
                &namespace,
                source.fingerprint()?,
                dispatch.turn_sequence,
                dispatch.runtime_generation
            ))?
        );
        Ok(Self {
            source,
            namespace,
            capability_incarnation: capability_incarnation.into(),
            context_reference,
            turn_sequence: dispatch.turn_sequence,
            runtime_generation: dispatch.runtime_generation,
            operation_sequence: dispatch.operation_sequence,
            agent_created_at: agent.created_at.clone(),
            creation_fingerprint: agent.creation_fingerprint.clone(),
            deadline: deadline.min(tokio::time::Instant::now() + Duration::from_secs(15)),
        })
    }
    pub fn source(&self) -> &OutboxSource {
        &self.source
    }
    pub fn namespace(&self) -> &str {
        &self.namespace
    }
    pub fn capability_incarnation(&self) -> &str {
        &self.capability_incarnation
    }
    pub fn context_reference(&self) -> &str {
        &self.context_reference
    }
    pub fn turn_sequence(&self) -> u64 {
        self.turn_sequence
    }
    pub fn runtime_generation(&self) -> u64 {
        self.runtime_generation
    }
    pub fn operation_sequence(&self) -> u64 {
        self.operation_sequence
    }
    pub fn agent_created_at(&self) -> &str {
        &self.agent_created_at
    }
    pub fn creation_fingerprint(&self) -> &str {
        &self.creation_fingerprint
    }
    pub fn deadline(&self) -> tokio::time::Instant {
        self.deadline
    }
    pub fn require_current(&self) -> Result<(), OutboxError> {
        if self.deadline <= tokio::time::Instant::now() {
            return Err(OutboxError::Expired);
        }
        Ok(())
    }
    /// Reconstruct the same source/scope from a single transactional snapshot.
    /// This rejects replacement Agents, changed turns, sidecars and provenance.
    pub fn require_snapshot(
        &self,
        observed: ChannelReplyTurnObservation<'_>,
    ) -> Result<(), OutboxError> {
        self.require_current()?;
        let fresh = Self::from_current(observed)?;
        if fresh.source != self.source
            || fresh.namespace != self.namespace
            || fresh.context_reference != self.context_reference
            || fresh.turn_sequence != self.turn_sequence
            || fresh.operation_sequence != self.operation_sequence
            || fresh.runtime_generation != self.runtime_generation
            || fresh.agent_created_at != self.agent_created_at
            || fresh.creation_fingerprint != self.creation_fingerprint
        {
            return Err(OutboxError::SourceMismatch);
        }
        Ok(())
    }
    pub fn selected_key(
        &self,
        validation: &crate::bridge::BridgeValidation,
    ) -> Result<DeliveryCredentialRef, OutboxError> {
        self.require_current()?;
        validation
            .require(&self.source.owner, self.source.config())
            .map_err(|_| OutboxError::AuthorityUnavailable)?;
        let pin = validation.custody_pin();
        Ok(DeliveryCredentialRef::RetainedKey {
            reference: pin.reference().clone(),
            revision: pin.revision(),
            key_state_version: pin.key_state_version(),
        })
    }
}

/// Tool arguments after the transport boundary resolves reply_context to exact
/// server-side scope. JSON-RPC IDs are deliberately not idempotency keys.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelReplyToolRequest {
    pub request_key: String,
    pub text: String,
}

pub struct ChannelReplyToolIntent {
    pub(super) scope: ChannelReplyTurnAuthorization,
    pub(super) invocation_id: String,
    pub(super) request_sha256: String,
    pub(super) text: String,
}
impl ChannelReplyToolIntent {
    pub fn prepare(
        scope: ChannelReplyTurnAuthorization,
        request: ChannelReplyToolRequest,
    ) -> Result<Self, OutboxError> {
        scope.require_current()?;
        // Permanent in this eligible Agent creation/capability incarnation.
        // A later turn reusing this key recovers the ORIGINAL admitted source.
        let (invocation_id, request_sha256) =
            super::session::request_identity(&scope.namespace, &request)?;
        Ok(Self {
            scope,
            invocation_id,
            request_sha256,
            text: request.text,
        })
    }
    pub fn scope(&self) -> &ChannelReplyTurnAuthorization {
        &self.scope
    }
    pub fn id(&self) -> &str {
        &self.invocation_id
    }
    pub fn request_sha256(&self) -> &str {
        &self.request_sha256
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplyToolStatus {
    Queued,
    Sent,
    Rejected,
    Unknown,
    Abandoned,
}

/// Safe tool response: queued is never rendered as delivered. No destination,
/// credential, provider response body or source attribution enters the result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplyToolReceipt {
    pub receipt_id: String,
    pub status: ReplyToolStatus,
    pub duplicate_risk: bool,
    pub failure: Option<DeliveryFailure>,
}

#[async_trait]
pub trait BridgeReplyToolStore: Send + Sync {
    /// First replay the durable namespace+request_key receipt: equal content
    /// returns its ORIGINAL source/status; changed content conflicts. For new
    /// admission re-read/fence creation provenance, exact active command/turn,
    /// Channel sidecar, inbox/allocation and Bridge grant/custody. Append this
    /// explicit call and all chunks atomically to the per-response order, with
    /// a durable invocation receipt. Never wait for response/turn completion.
    /// Multiple calls require a deliberately retained matching key; reserve
    /// any initial reply token once across ALL items for that original inbox.
    async fn enqueue(
        &self,
        intent: ChannelReplyToolIntent,
    ) -> Result<ReplyToolReceipt, OutboxError>;
}

pub struct BridgeReplyToolService {
    store: Arc<dyn BridgeReplyToolStore>,
}
impl BridgeReplyToolService {
    pub fn new(store: Arc<dyn BridgeReplyToolStore>) -> Self {
        Self { store }
    }
    pub async fn enqueue(
        &self,
        scope: ChannelReplyTurnAuthorization,
        request: ChannelReplyToolRequest,
    ) -> Result<ReplyToolReceipt, OutboxError> {
        self.store
            .enqueue(ChannelReplyToolIntent::prepare(scope, request)?)
            .await
    }
}
