//! Recovery invokes only the dedicated retained-conversation adapter entry.
//! Production backends without native proof return Unsupported; no ordinary
//! reconnect, settings/context replay or PublishConnection fallback exists here.
use std::sync::Arc;

use async_trait::async_trait;
use layer2_core::{
    sandbox::{
        agent_capacity_recovery::{
            activation::RecoveryActivation,
            ports::{CapacityRecoveryRuntime, CompletedAttachment, RecoveryError},
            state::{
                ConfirmedNativeState, RecoveryClaim, RecoveryConfirmation, RecoveryPhase,
                RecoveryWork,
            },
        },
        Agent, AgentUpdateRecorder,
    },
    RuntimeCapabilities,
};
use layer3_adapter::{
    agent_capacity_recovery::{
        CapacityRecoveryError, CapacityRecoveryRequest, ConfirmedAgentState, RetainedAgentTarget,
    },
    ConversationChannel, ConversationUpdate, RuntimeHandle, SandboxAdapter,
};
use tokio::sync::mpsc;

use crate::sandbox::live_agents::{LiveAgentRegistry, LiveAgentTarget};

pub(crate) struct RestrictedRecoveryRuntime {
    adapter: Arc<dyn SandboxAdapter>,
    registry: Arc<LiveAgentRegistry>,
    recorder: Arc<dyn AgentUpdateRecorder>,
}

impl RestrictedRecoveryRuntime {
    pub(crate) fn new(
        adapter: Arc<dyn SandboxAdapter>,
        registry: Arc<LiveAgentRegistry>,
        recorder: Arc<dyn AgentUpdateRecorder>,
    ) -> Self {
        Self {
            adapter,
            registry,
            recorder,
        }
    }
}

pub(crate) struct RetainedAttachment {
    channel: ConversationChannel,
    updates: mpsc::Receiver<ConversationUpdate>,
    work: RecoveryWork,
    authority_fence: u64,
    capabilities: RuntimeCapabilities,
}

#[async_trait]
impl CapacityRecoveryRuntime for RestrictedRecoveryRuntime {
    type Attachment = RetainedAttachment;

    async fn reattach_existing(
        &self,
        claim: &RecoveryClaim,
    ) -> Result<(Self::Attachment, RecoveryConfirmation), RecoveryError> {
        claim.work.validate()?;
        let fence = claim
            .work
            .lease
            .as_ref()
            .ok_or(RecoveryError::Conflict)?
            .fence;
        let attached = self.connect(&claim.work, &claim.agent, fence).await?;
        attached.1.validate_for(claim)?;
        Ok(attached)
    }

    async fn reattach_completed(
        &self,
        activation: &RecoveryActivation,
    ) -> Result<CompletedAttachment<Self::Attachment>, RecoveryError> {
        let target = &activation.completed().target;
        if self.registry.recovery_attachment_is_active(
            &LiveAgentTarget {
                agent_id: target.agent_id.clone(),
                conversation_id: target.conversation_id.clone(),
                runtime_generation: target.conversation_generation,
            },
            activation.writer_fence(),
        ) {
            // The service rechecks its exact receipt and current authority after
            // this observation too. No staged entry is promoted or native
            // confirmation/acknowledgement invented for an existing connection.
            return Ok(CompletedAttachment::AlreadyActivated);
        }
        let attached = self
            .connect(
                activation.completed(),
                activation.agent(),
                activation.writer_fence(),
            )
            .await?;
        activation.require_confirmation(&attached.1)?;
        Ok(CompletedAttachment::Reattached {
            attachment: attached.0,
            confirmation: Box::new(attached.1),
        })
    }

    async fn acknowledge(
        &self,
        attachment: Self::Attachment,
        completed: &RecoveryWork,
    ) -> Result<(), RecoveryError> {
        completed.validate()?;
        let mut expected = attachment.work.clone();
        let fence = attachment.authority_fence;
        if expected.phase == RecoveryPhase::Attaching {
            expected.phase = RecoveryPhase::Completed;
            expected.revision = expected
                .revision
                .checked_add(1)
                .ok_or(RecoveryError::InvalidEvidence)?;
            expected.lease = None;
            expected.completed_at = completed.completed_at.clone();
        }
        if completed != &expected {
            return Err(RecoveryError::InvalidEvidence);
        }
        let staged = self
            .registry
            .attach(
                completed.request.agent_id.clone(),
                completed.target.conversation_id.clone(),
                completed.target.conversation_generation,
                fence,
                attachment.capabilities,
                attachment.channel.input,
                attachment.updates,
                self.recorder.clone(),
            )
            .map_err(|_| RecoveryError::Conflict)?;
        if self.registry.publish_and_activate(&staged).is_err() {
            self.registry.rollback(&staged);
            return Err(RecoveryError::Unavailable);
        }
        // The existing recorder ACKs each native frame only after its own exact
        // durable output receipt. No blanket output cursor is acknowledged here.
        Ok(())
    }
}

impl RestrictedRecoveryRuntime {
    async fn connect(
        &self,
        work: &RecoveryWork,
        agent: &Agent,
        fence: u64,
    ) -> Result<(RetainedAttachment, RecoveryConfirmation), RecoveryError> {
        let request = native_request(work, agent, fence)?;
        let (updates, receiver) = mpsc::channel(128);
        let connection = self
            .adapter
            .reconnect_agent_for_capacity_recovery(&request, updates)
            .await
            .map_err(|error| match error {
                CapacityRecoveryError::Unsupported | CapacityRecoveryError::Unavailable => {
                    RecoveryError::Unavailable
                }
                CapacityRecoveryError::IdentityConflict => RecoveryError::Conflict,
                CapacityRecoveryError::StateUnconfirmed
                | CapacityRecoveryError::OutputUnconfirmed => RecoveryError::InvalidEvidence,
            })?;
        connection
            .confirmation()
            .validate_for(&request)
            .map_err(|_| RecoveryError::InvalidEvidence)?;
        let native_state = match &connection.confirmation().native_state {
            ConfirmedAgentState::Idle => ConfirmedNativeState::Idle,
            ConfirmedAgentState::Working { .. } => ConfirmedNativeState::Working,
            ConfirmedAgentState::WaitingApproval { .. } => ConfirmedNativeState::WaitingApproval,
        };
        let confirmation = RecoveryConfirmation {
            operation_key: request.operation_key,
            target: work.target.clone(),
            authority_fence: request.writer_fence,
            native_state,
        };
        let (channel, _) = connection.into_parts();
        // Hold the bounded receiver here; neither the recorder nor any input
        // sender is published until the exact durable completion is established.
        Ok((
            RetainedAttachment {
                channel,
                updates: receiver,
                work: work.clone(),
                authority_fence: fence,
                capabilities: agent.capabilities.clone(),
            },
            confirmation,
        ))
    }
}

fn native_request(
    work: &RecoveryWork,
    agent: &Agent,
    fence: u64,
) -> Result<CapacityRecoveryRequest, RecoveryError> {
    work.validate()?;
    let target = &work.target;
    if agent.id != target.agent_id
        || agent.creation_key != target.creation_key
        || agent.creation_fingerprint != target.creation_fingerprint
        || agent.conversation.conversation_id.as_ref() != Some(&target.conversation_id)
        || agent.conversation.runtime_generation != Some(target.conversation_generation)
        || agent.work.next_fence != work.next_fence
        || fence.checked_add(1) != Some(work.next_fence)
    {
        return Err(RecoveryError::Conflict);
    }
    let request = CapacityRecoveryRequest {
        operation_key: work.request.key()?,
        writer_fence: fence,
        challenge: uuid::Uuid::new_v4().simple().to_string(),
        target: RetainedAgentTarget {
            sandbox_id: agent.sandbox_id.to_string(),
            agent_id: target.agent_id.to_string(),
            runtime: RuntimeHandle::new(&target.runtime.runtime_id),
            boot_id: target
                .runtime
                .boot_id
                .clone()
                .ok_or(RecoveryError::InvalidEvidence)?,
            provision_generation: target
                .runtime
                .provision_generation
                .ok_or(RecoveryError::InvalidEvidence)?,
            home_identity: target.home_identity.clone(),
            conversation_id: target.conversation_id.clone(),
            conversation_generation: target.conversation_generation,
            original_open_operation: target.original_open.operation_id.clone(),
            original_open_sha256: target.original_open.request_sha256.clone(),
            active_turn: target.active_turn,
        },
    };
    request
        .validate()
        .map_err(|_| RecoveryError::InvalidEvidence)?;
    Ok(request)
}
