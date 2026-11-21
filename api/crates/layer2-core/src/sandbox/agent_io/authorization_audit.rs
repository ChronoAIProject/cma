//! Append-only historical evidence stored alongside accepted command effects.
//! Existing receipts remain unchanged. This data is never execution authority
//! and is not part of recipient-visible status/history projections.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::ids::AgentCommandId;
use crate::resource_access::{ResourceAuthorizationAudit, ResourceTarget};
use crate::sandbox::store::{Page, PageRequest};

use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CommandAuthorizationEffect {
    Admission,
    Dispatch { dispatch: CommandDispatch },
    Revival { sandbox_revision: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandAuthorizationAudit {
    pub schema_version: u32,
    pub id: String,
    pub command_id: AgentCommandId,
    pub agent_id: crate::AgentId,
    pub actor: AgentActor,
    pub owner: AgentOwner,
    pub source: AgentCommandSource,
    pub command_kind: crate::sandbox::AgentOperationKind,
    /// Sign-in custody, independent of all resource policy/grant revisions.
    pub credential_authority: AgentAuthorizationRevision,
    pub command_revision: u64,
    pub recorded_at: String,
    pub effect: CommandAuthorizationEffect,
    pub resources: Vec<ResourceAuthorizationAudit>,
}

impl CommandAuthorizationAudit {
    pub fn new(
        command: &AdmittedAgentCommand,
        effect: CommandAuthorizationEffect,
        resources: Vec<ResourceAuthorizationAudit>,
    ) -> Result<Self, CommandLedgerError> {
        command.validate()?;
        let envelope = &command.receipt.envelope;
        let audit = Self {
            schema_version: 1,
            id: audit_id(&envelope.command_id, &effect)?,
            command_id: envelope.command_id.clone(),
            agent_id: envelope.agent_id.clone(),
            actor: envelope.actor.clone(),
            owner: envelope.owner.clone(),
            source: envelope.source.clone(),
            command_kind: envelope.command.kind(),
            credential_authority: envelope.authorization.clone(),
            command_revision: command.revision,
            recorded_at: crate::now_rfc3339(),
            effect,
            resources,
        };
        audit.validate()?;
        Ok(audit)
    }

    pub fn validate(&self) -> Result<(), CommandLedgerError> {
        let expected_count = match &self.effect {
            CommandAuthorizationEffect::Admission => 1,
            CommandAuthorizationEffect::Dispatch { dispatch } => {
                if dispatch.claim.command_id != self.command_id
                    || dispatch.claim.agent_id != self.agent_id
                    || dispatch.claim.fence == 0
                    || dispatch.operation_sequence == 0
                    || dispatch.turn_sequence == 0
                    || dispatch.runtime_generation == 0
                    || crate::seconds_from_rfc3339(&dispatch.fenced_at).is_none()
                {
                    return Err(CommandLedgerError::Corrupt);
                }
                1
            }
            CommandAuthorizationEffect::Revival { sandbox_revision } => {
                if *sandbox_revision == 0
                    || self.command_kind != crate::sandbox::AgentOperationKind::SendMessage
                {
                    return Err(CommandLedgerError::Corrupt);
                }
                2
            }
        };
        if self.schema_version != 1
            || self.id != audit_id(&self.command_id, &self.effect)?
            || self.command_revision == 0
            || !self.credential_authority.valid_replay_principal()
            || self.resources.len() != expected_count
            || crate::seconds_from_rfc3339(&self.recorded_at).is_none()
        {
            return Err(CommandLedgerError::Corrupt);
        }
        for (index, resource) in self.resources.iter().enumerate() {
            resource
                .validate()
                .map_err(|_| CommandLedgerError::Corrupt)?;
            let (target, capability) = if index == 0 {
                (
                    ResourceTarget::Agent {
                        agent_id: self.agent_id.clone(),
                    },
                    command_kind_capability(self.command_kind),
                )
            } else {
                (
                    ResourceTarget::Sandbox {
                        sandbox_id: self.resources[0].sandbox_id.clone(),
                    },
                    crate::resource_access::ResourceCapability::ManageSandbox,
                )
            };
            if resource.actor != self.actor
                || resource.owner != self.owner
                || resource.target != target
                || resource.capability != capability
                || resource.sandbox_id != self.resources[0].sandbox_id
                || resource.sandbox_created_at != self.resources[0].sandbox_created_at
                || resource.policy_revision != self.resources[0].policy_revision
            {
                return Err(CommandLedgerError::Corrupt);
            }
        }
        Ok(())
    }
}

fn command_kind_capability(
    kind: crate::sandbox::AgentOperationKind,
) -> crate::resource_access::ResourceCapability {
    use crate::resource_access::ResourceCapability;
    match kind {
        crate::sandbox::AgentOperationKind::SendMessage => ResourceCapability::Interact,
        crate::sandbox::AgentOperationKind::AnswerApproval => ResourceCapability::ResolveApproval,
        _ => ResourceCapability::OperateAgent,
    }
}

fn audit_id(
    id: &AgentCommandId,
    effect: &CommandAuthorizationEffect,
) -> Result<String, CommandLedgerError> {
    let bytes = serde_json::to_vec(&(id, effect)).map_err(|_| CommandLedgerError::Corrupt)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

#[async_trait::async_trait]
pub trait AgentCommandAuditStore: Send + Sync {
    /// Internal persistence only. Grant/membership evidence is sensitive resource
    /// management data; a command status read does not authorize its disclosure.
    /// Pages are ordered by immutable audit ID, not effect chronology.
    async fn command_authorization_audits(
        &self,
        command_id: &AgentCommandId,
        page: &PageRequest,
    ) -> Result<Page<CommandAuthorizationAudit>, CommandLedgerError>;
}
