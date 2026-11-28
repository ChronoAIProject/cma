use serde::{Deserialize, Serialize};

use crate::ids::{AgentCommandId, AgentId, AgentResponseId, LaunchId};
use crate::profiles::content_digest;
use crate::resource_access::{ResourceOwner, ResourcePolicyFence};
use crate::sandbox::store::{AgentAdmission, SandboxMutationGuard};
use crate::sandbox::{Agent, AgentOrigin, FirstMessageDelivery, FirstMessagePurpose, Sandbox};
use crate::trigger_domain::{Launch, LaunchInputProtocol};

use super::super::*;

// Onboarding follows the Agent startup lifecycle. The shared receipt schema
// requires a timestamp, so use its maximum RFC3339 value for this source.
const ONBOARDING_QUEUE_DEADLINE: &str = "9999-12-31T23:59:59Z";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Onboarding {
    operation_id: String,
    purpose: FirstMessagePurpose,
    desktop: bool,
}

/// Permanent execution identity, stored outside the legacy Agent wire record.
/// Integrity alone is not execution authority; the store proves this receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TriggerFirstMessageBinding {
    schema_version: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    launch_id: Option<LaunchId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    protocol: Option<LaunchInputProtocol>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    onboarding: Option<Onboarding>,
    creation_key: String,
    creation_fingerprint: String,
    agent_created_at: String,
    sandbox_created_at: String,
    profile_digest: String,
    workspace_id: crate::WorkspaceId,
    worktree_id: crate::WorktreeId,
    sandbox_id: crate::SandboxId,
    origin_key: String,
    payload_sha256: String,
    queued_at: String,
    receipt: AdmissionReceipt,
    key_sha256: String,
    queue_deadline: String,
}

impl TriggerFirstMessageBinding {
    /// Called by trusted atomic Agent admission after loading the accepted Launch.
    pub fn prepare(
        launch: &Launch,
        admission: &AgentAdmission,
        agent: &Agent,
        parent: &Sandbox,
        policy: ResourcePolicyFence,
        owner: ResourceOwner,
    ) -> Result<(Self, CommandReservation), CommandLedgerError> {
        let protocol = launch
            .input_protocol
            .clone()
            .ok_or(CommandLedgerError::Invalid)?;
        protocol
            .require_launch(launch)
            .map_err(|_| CommandLedgerError::Corrupt)?;
        launch
            .initiation
            .as_ref()
            .ok_or(CommandLedgerError::Corrupt)?
            .require_agent_admission(admission)
            .map_err(|_| CommandLedgerError::NotAdmissible)?;
        if launch.owner != agent.owner {
            return Err(CommandLedgerError::NotAdmissible);
        }
        let (mut binding, request) = Self::prepare_common(
            admission,
            agent,
            parent,
            policy,
            owner,
            AgentActor::Trigger {
                subject: launch.owner.clone(),
                launch_id: launch.id.clone(),
            },
            AgentCommandSource {
                kind: AgentSourceKind::Trigger,
                reference: launch.id.to_string(),
            },
            format!("trigger-first:{}", launch.id),
        )?;
        binding.launch_id = Some(launch.id.clone());
        binding.protocol = Some(protocol);
        binding.require_agent(agent)?;
        Ok((binding, request))
    }

    pub fn prepare_onboarding(
        admission: &AgentAdmission,
        agent: &Agent,
        parent: &Sandbox,
        policy: ResourcePolicyFence,
        owner: ResourceOwner,
    ) -> Result<(Self, CommandReservation), CommandLedgerError> {
        let AgentOrigin::Lifecycle { operation_id } = &admission.origin else {
            return Err(CommandLedgerError::Invalid);
        };
        let actor = admission
            .created_by
            .clone()
            .ok_or(CommandLedgerError::Invalid)?;
        let first = agent
            .conversation
            .first_message
            .as_ref()
            .ok_or(CommandLedgerError::Invalid)?;
        let mut binding_and_request = Self::prepare_common(
            admission,
            agent,
            parent,
            policy,
            owner,
            actor,
            AgentCommandSource {
                kind: AgentSourceKind::Onboarding,
                reference: operation_id.clone(),
            },
            format!("onboarding-first:{}", agent.creation_key),
        )?;
        binding_and_request.0.onboarding = Some(Onboarding {
            operation_id: operation_id.clone(),
            purpose: first.purpose.ok_or(CommandLedgerError::Invalid)?,
            desktop: first.desktop,
        });
        binding_and_request.0.require_agent(agent)?;
        Ok(binding_and_request)
    }

    #[allow(clippy::too_many_arguments)]
    fn prepare_common(
        admission: &AgentAdmission,
        agent: &Agent,
        parent: &Sandbox,
        policy: ResourcePolicyFence,
        owner: ResourceOwner,
        actor: AgentActor,
        source: AgentCommandSource,
        authority_id: String,
    ) -> Result<(Self, CommandReservation), CommandLedgerError> {
        let first = agent
            .conversation
            .first_message
            .as_ref()
            .ok_or(CommandLedgerError::Invalid)?;
        if first.state != FirstMessageDelivery::Queued
            || first.attempt != 0
            || first.recovery_generation != 0
            || first.common_binding.is_some()
            || first.origin_key != agent.creation_key
            || agent.conversation.active_turn.is_some()
            || agent.conversation.operation.is_some()
            || !agent.conversation.queued_messages.is_empty()
            || admission.workspace_id != agent.workspace_id
            || admission.profile != agent.profile
            || parent.id != agent.sandbox_id
            || owner
                != (ResourceOwner::Person {
                    subject: agent.owner.clone(),
                })
        {
            return Err(CommandLedgerError::NotAdmissible);
        }
        let command = AgentCommand::message(first.text.clone());
        let binding_key = if source.kind == AgentSourceKind::Onboarding {
            content_digest(&("onboarding-first-v1", &source.reference, &agent.owner))
        } else {
            content_digest(&(
                source.reference.as_str(),
                agent.id.as_str(),
                agent.created_at.as_str(),
                first.origin_key.as_str(),
                first.payload_sha256.as_str(),
            ))
        }
        .map_err(|_| CommandLedgerError::Corrupt)?;
        let queue_deadline = if source.kind == AgentSourceKind::Onboarding {
            ONBOARDING_QUEUE_DEADLINE.to_owned()
        } else {
            let admitted_at = crate::seconds_from_rfc3339(&admission.event_at)
                .ok_or(CommandLedgerError::Invalid)?;
            crate::rfc3339_at(
                std::time::SystemTime::UNIX_EPOCH
                    + std::time::Duration::from_secs(
                        admitted_at
                            .checked_add(300)
                            .ok_or(CommandLedgerError::Invalid)?,
                    ),
            )
        };
        let request = CommandReservation {
            binding: None,
            envelope: AgentCommandEnvelope {
                continuation: None,
                schema_version: AGENT_IO_SCHEMA_VERSION,
                command_id: AgentCommandId::generate(),
                agent_id: agent.id.clone(),
                owner,
                actor,
                source,
                correlation_id: binding_key.clone(),
                request_fingerprint: command
                    .fingerprint()
                    .map_err(|_| CommandLedgerError::Invalid)?,
                authorization: AgentAuthorizationRevision {
                    replay_principal: None,
                    authority_id,
                    policy_revision: 1,
                    grant_revision: None,
                },
                command,
            },
            key_sha256: command_key_digest(&binding_key)?,
            response_id: Some(AgentResponseId::generate()),
            admitted_at: admission.event_at.clone(),
            queue_deadline,
            queue_capacity: MAX_COMMAND_QUEUE_CAPACITY,
            expected_agent_revision: agent.revision,
            parent: SandboxMutationGuard::capture(parent),
            policy,
            authority_deadline: None,
            admission_audit: None,
        };
        request.validate()?;
        let binding = Self {
            schema_version: 1,
            launch_id: None,
            protocol: None,
            onboarding: None,
            creation_key: admission.creation_key.clone(),
            creation_fingerprint: admission
                .fingerprint()
                .map_err(|_| CommandLedgerError::Corrupt)?,
            agent_created_at: agent.created_at.clone(),
            sandbox_created_at: parent.created_at.clone(),
            profile_digest: agent.profile.digest.clone(),
            workspace_id: agent.workspace_id.clone(),
            worktree_id: agent.worktree_id.clone(),
            sandbox_id: agent.sandbox_id.clone(),
            origin_key: first.origin_key.clone(),
            payload_sha256: first.payload_sha256.clone(),
            queued_at: first.queued_at.clone(),
            receipt: request
                .record(1, agent.conversation.last_seq, None)?
                .receipt,
            key_sha256: request.key_sha256.clone(),
            queue_deadline: request.queue_deadline.clone(),
        };
        Ok((binding, request))
    }
    pub fn bind_receipt(
        &mut self,
        command: &AdmittedAgentCommand,
    ) -> Result<(), CommandLedgerError> {
        if self.receipt.envelope != command.receipt.envelope
            || self.receipt.response_id != command.receipt.response_id
            || self.receipt.admitted_at != command.receipt.admitted_at
            || self.receipt.event_cursor != command.receipt.event_cursor
            || command.receipt.agent_order != 1
        {
            return Err(CommandLedgerError::Conflict);
        }
        self.receipt = command.receipt.clone();
        self.require_command(command)
    }
    pub fn digest(&self) -> Result<String, CommandLedgerError> {
        content_digest(self).map_err(|_| CommandLedgerError::Corrupt)
    }
    pub fn launch_id(&self) -> Option<&LaunchId> {
        self.launch_id.as_ref()
    }
    pub fn is_onboarding(&self) -> bool {
        self.onboarding.is_some()
    }
    pub(in crate::sandbox) fn is_onboarding_setup(&self) -> bool {
        self.onboarding
            .as_ref()
            .is_some_and(|source| source.purpose == FirstMessagePurpose::Setup)
    }
    pub fn document_key(&self) -> String {
        self.launch_id.as_ref().map_or_else(
            || format!("onboarding:{}", self.agent_id()),
            ToString::to_string,
        )
    }
    pub fn agent_id(&self) -> &AgentId {
        &self.receipt.envelope.agent_id
    }
    pub fn receipt(&self) -> &AdmissionReceipt {
        &self.receipt
    }
    pub fn sandbox_id(&self) -> &crate::SandboxId {
        &self.sandbox_id
    }
    pub fn require_launch(&self, launch: &Launch) -> Result<(), CommandLedgerError> {
        self.protocol
            .as_ref()
            .ok_or(CommandLedgerError::Corrupt)?
            .require_launch(launch)
            .map_err(|_| CommandLedgerError::Corrupt)?;
        if launch.input_binding_sha256.as_ref() != Some(&self.digest()?)
            || self.onboarding.is_some()
            || self.launch_id.as_ref() != Some(&launch.id)
            || launch.input_protocol != self.protocol
            || self.receipt.envelope.actor
                != (AgentActor::Trigger {
                    subject: launch.owner.clone(),
                    launch_id: launch.id.clone(),
                })
            || launch.resolution.workspace.id() != Some(&self.workspace_id)
            || launch.resolution.sandbox.id() != Some(&self.sandbox_id)
            || launch
                .resolution
                .agent
                .id()
                .is_some_and(|agent| agent != self.agent_id())
        {
            return Err(CommandLedgerError::Corrupt);
        }
        Ok(())
    }
    pub fn require_admission(&self, admission: &AgentAdmission) -> Result<(), CommandLedgerError> {
        if self.creation_key != admission.creation_key
            || self.creation_fingerprint
                != admission
                    .fingerprint()
                    .map_err(|_| CommandLedgerError::Corrupt)?
        {
            return Err(CommandLedgerError::IdempotencyConflict);
        }
        Ok(())
    }
    pub fn require_agent(&self, agent: &Agent) -> Result<(), CommandLedgerError> {
        self.receipt
            .validate_integrity()
            .map_err(|_| CommandLedgerError::Corrupt)?;
        let first = agent
            .conversation
            .first_message
            .as_ref()
            .ok_or(CommandLedgerError::Corrupt)?;
        let envelope = &self.receipt.envelope;
        let valid_source = match (&self.launch_id, &self.protocol, &self.onboarding) {
            (Some(launch_id), Some(_), None) => {
                envelope.source.kind == AgentSourceKind::Trigger
                    && envelope.source.reference == launch_id.as_str()
                    && envelope.actor
                        == (AgentActor::Trigger {
                            subject: agent.owner.clone(),
                            launch_id: launch_id.clone(),
                        })
            }
            (None, None, Some(onboarding)) => {
                envelope.source.kind == AgentSourceKind::Onboarding
                    && envelope.source.reference == onboarding.operation_id
                    && envelope.actor.ordinary_subject() == Some(&agent.owner)
                    && agent.origin
                        == (AgentOrigin::Lifecycle {
                            operation_id: onboarding.operation_id.clone(),
                        })
                    && self.creation_fingerprint == agent.creation_fingerprint
                    && first.purpose == Some(onboarding.purpose)
                    && first.desktop == onboarding.desktop
            }
            _ => false,
        };
        if self.schema_version != 1
            || !valid_source
            || self.agent_id() != &agent.id
            || self.agent_created_at != agent.created_at
            || self.workspace_id != agent.workspace_id
            || &self.worktree_id
                != if self.is_onboarding() {
                    agent.creation_worktree_id()
                } else {
                    &agent.worktree_id
                }
            || self.sandbox_id != agent.sandbox_id
            || self.profile_digest != agent.profile.digest
            || self.origin_key != agent.creation_key
            || self.origin_key != first.origin_key
            || self.payload_sha256 != first.payload_sha256
            || self.queued_at != first.queued_at
            || self.receipt.envelope.command != AgentCommand::message(first.text.clone())
            || first.recovery_generation != 0
            || first.attempt > 1
        {
            return Err(CommandLedgerError::Corrupt);
        }
        Ok(())
    }
    pub fn require_parent(
        &self,
        parent: &Sandbox,
        owner: &ResourceOwner,
    ) -> Result<(), CommandLedgerError> {
        if parent.id != self.sandbox_id
            || parent.created_at != self.sandbox_created_at
            || owner != &self.receipt.envelope.owner
            || owner
                != &(ResourceOwner::Person {
                    subject: parent.owner.clone(),
                })
        {
            return Err(CommandLedgerError::NotAdmissible);
        }
        Ok(())
    }
    pub fn require_command(
        &self,
        command: &AdmittedAgentCommand,
    ) -> Result<(), CommandLedgerError> {
        command.validate()?;
        if self.receipt != command.receipt
            || self.key_sha256 != command.key_sha256
            || self.queue_deadline != command.queue_deadline
            || command.target_turn.is_some()
        {
            return Err(CommandLedgerError::Corrupt);
        }
        Ok(())
    }
}

pub fn is_bound_trigger_first(command: &AdmittedAgentCommand, agent: &Agent) -> bool {
    agent
        .conversation
        .first_message
        .as_ref()
        .is_some_and(|first| {
            first.state == FirstMessageDelivery::Queued
                && first.common_binding.as_ref().is_some_and(|binding| {
                    binding.require_agent(agent).is_ok() && binding.require_command(command).is_ok()
                })
        })
}
