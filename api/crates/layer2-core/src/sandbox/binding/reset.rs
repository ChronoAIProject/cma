//! Owner-requested replacement keeps transport identity and all historical mappings.
use super::{
    BindingConversation, BindingError, BindingState, BindingWork, ProviderBinding, SessionAffinity,
};
use crate::{
    resource_access::ResourceOwner,
    sandbox::{
        Agent, AgentStatus, AgentWorkStep, Sandbox, SandboxStatus, SandboxWorkStep, Workspace,
        WorkspaceStatus, WorkspaceWorkStep,
    },
    AgentId, SubjectId,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversationResetRequest {
    pub binding_id: String,
    /// Stable manager-owned scope shared by all consumers of one channel.
    pub idempotency_binding_id: String,
    pub conversation_id: String,
    pub expected_agent_id: AgentId,
    pub owner: ResourceOwner,
    pub manager: SubjectId,
    pub operation_id: String,
    /// Includes the complete public request, including its channel version.
    pub request_digest: String,
}
impl ConversationResetRequest {
    pub fn validate(&self) -> Result<(), BindingError> {
        if !super::valid_binding_id(&self.binding_id)
            || !super::valid_binding_id(&self.idempotency_binding_id)
            || !super::model::valid_id(&self.conversation_id, 8)
            || self.expected_agent_id.as_str().is_empty()
            || self.expected_agent_id.as_str().len() > 128
            || self.manager.as_str().is_empty()
            || [&self.operation_id, &self.request_digest]
                .into_iter()
                .any(|v| {
                    v.len() != 64
                        || !v
                            .bytes()
                            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                })
        {
            return Err(BindingError::InvalidRequest(
                "invalid conversation reset".into(),
            ));
        }
        Ok(())
    }
    pub fn authorize(
        &self,
        binding: &ProviderBinding,
        scope: &ProviderBinding,
    ) -> Result<(), BindingError> {
        if binding.binding_id != self.binding_id
            || scope.binding_id != self.idempotency_binding_id
            || binding.manager != self.manager
            || scope.manager != self.manager
            || binding.resource_owner() != self.owner
            || scope.resource_owner() != self.owner
        {
            return Err(BindingError::NotFound);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversationHead {
    pub transport_conversation_id: String,
    pub incarnation: u64,
    /// A reservation only. Workers discover it only after ensure materializes it.
    pub successor: BindingConversation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversationResetReceipt {
    pub request: ConversationResetRequest,
    pub previous: BindingConversation,
    pub head: ConversationHead,
}

/// Stored outside the strict binding record; ordinary binding updates preserve it.
/// Bounded, permanent receipts use the existing binding primary-key index.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversationResetState {
    pub heads: BTreeMap<String, ConversationHead>,
    pub receipts: BTreeMap<String, ConversationResetReceipt>,
}
impl ConversationResetState {
    pub fn key(generation: u64, conversation: &str) -> String {
        crate::profiles::content_digest(&(generation, conversation)).expect("string tuple")
    }
    pub fn head(&self, generation: u64, conversation: &str) -> Option<&ConversationHead> {
        self.heads.get(&Self::key(generation, conversation))
    }
    pub fn replay(
        &self,
        request: &ConversationResetRequest,
    ) -> Result<Option<ConversationResetReceipt>, BindingError> {
        self.receipts
            .get(&request.operation_id)
            .map(|receipt| {
                if receipt.request != *request {
                    Err(BindingError::ResetRefused("partition_reset_key_conflict"))
                } else {
                    Ok(receipt.clone())
                }
            })
            .transpose()
    }
    pub fn validate(&self, binding: &str) -> Result<(), String> {
        if self.heads.len() > 256 || self.receipts.len() > 512 {
            return Err("conversation reset state exceeds bounds".into());
        }
        for (key, head) in &self.heads {
            head.successor.validate()?;
            if head.incarnation == 0
                || head.successor.binding_id != binding
                || *key != Self::key(head.successor.generation, &head.transport_conversation_id)
                || !super::model::valid_id(&head.transport_conversation_id, 8)
                || head.successor.conversation_id == head.transport_conversation_id
            {
                return Err("invalid conversation incarnation head".into());
            }
        }
        for (key, receipt) in &self.receipts {
            receipt.request.validate().map_err(|e| e.to_string())?;
            receipt.previous.validate()?;
            receipt.head.successor.validate()?;
            if key != &receipt.request.operation_id
                || receipt.request.idempotency_binding_id != binding
                || receipt.request.binding_id != receipt.previous.binding_id
                || receipt.previous.binding_id != receipt.head.successor.binding_id
                || receipt.previous.generation != receipt.head.successor.generation
                || receipt.previous.agent_id != receipt.request.expected_agent_id
                || receipt.head.transport_conversation_id != receipt.request.conversation_id
                || receipt.head.incarnation == 0
            {
                return Err("invalid conversation reset receipt".into());
            }
        }
        Ok(())
    }
}

pub fn reset_binding_eligibility(binding: &ProviderBinding) -> Result<(), BindingError> {
    let refuse = BindingError::ResetRefused;
    if binding.existing_agent.is_some() {
        return Err(refuse("partition_reset_existing_agent"));
    }
    if binding.profile.session_affinity != SessionAffinity::Conversation {
        return Err(refuse("partition_reset_affinity_unsupported"));
    }
    if binding.state != BindingState::Ready
        || binding.work != BindingWork::Idle
        || binding.previous.is_some()
    {
        return Err(refuse("partition_reset_binding_not_ready"));
    }
    Ok(())
}

pub fn reset_eligibility(
    binding: &ProviderBinding,
    conversation: &BindingConversation,
    agent: Option<&Agent>,
    sandbox: Option<&Sandbox>,
    workspace: Option<&Workspace>,
) -> Result<(), BindingError> {
    let refuse = BindingError::ResetRefused;
    reset_binding_eligibility(binding)?;
    if conversation.borrowed {
        return Err(refuse("partition_reset_existing_agent"));
    }
    if binding.profile.session_affinity != SessionAffinity::Conversation {
        return Err(refuse("partition_reset_affinity_unsupported"));
    }
    if binding.state != BindingState::Ready
        || binding.work != BindingWork::Idle
        || binding.previous.is_some()
    {
        return Err(refuse("partition_reset_binding_not_ready"));
    }
    if conversation.binding_id != binding.binding_id
        || conversation.generation != binding.generation
        || conversation.owner != binding.owner
        || conversation.sandbox_id != binding.sandbox_id
        || conversation.workspace_id != binding.workspace_id
    {
        return Err(BindingError::Corrupt(
            "reset mapping lineage differs".into(),
        ));
    }
    if matches!(
        conversation.state,
        super::BindingConversationState::Retiring | super::BindingConversationState::Retired
    ) {
        return Err(refuse("partition_reset_mapping_retired"));
    }
    let agent = agent.ok_or(refuse("partition_reset_agent_missing"))?;
    if agent.id != conversation.agent_id
        || agent.owner != conversation.owner
        || agent.sandbox_id != conversation.sandbox_id
        || agent.workspace_id != conversation.workspace_id
        || agent.worktree_id != conversation.worktree_id
    {
        return Err(BindingError::Corrupt("reset Agent lineage differs".into()));
    }
    if agent.status != AgentStatus::Deleted {
        return Err(refuse("partition_reset_agent_not_deleted"));
    }
    if agent.work.step != AgentWorkStep::None
        || agent.work.lease.is_some()
        || !agent.work.cleanup_debt.is_empty()
        || agent.requested_stop_reason.is_some()
    {
        return Err(refuse("partition_reset_cleanup_pending"));
    }
    let (Some(sandbox), Some(workspace)) = (sandbox, workspace) else {
        return Err(refuse("partition_reset_parents_unavailable"));
    };
    if sandbox.id != binding.sandbox_id
        || sandbox.owner != binding.owner
        || workspace.id != binding.workspace_id
        || workspace.owner != binding.owner
        || workspace.sandbox_id != sandbox.id
        || !matches!(
            sandbox.status,
            SandboxStatus::Active | SandboxStatus::Paused
        )
        || sandbox.work.step != SandboxWorkStep::None
        || sandbox.work.lease.is_some()
        || workspace.status != WorkspaceStatus::Ready
        || workspace.work.step != WorkspaceWorkStep::None
        || workspace.work.lease.is_some()
    {
        return Err(refuse("partition_reset_parents_unavailable"));
    }
    Ok(())
}

impl super::BindingService {
    pub async fn reset_conversation(
        &self,
        caller: &crate::Caller,
        mut request: ConversationResetRequest,
        enabled: bool,
    ) -> Result<ConversationResetReceipt, BindingError> {
        super::service::require(caller)?;
        request.manager = caller.subject.clone();
        request.validate()?;
        self.store
            .reset_binding_conversation(request, enabled)
            .await
    }
}
