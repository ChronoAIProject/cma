use super::model::{
    BindingConversation, BindingConversationState, BindingState, BindingWork, ProviderBinding,
};
use crate::sandbox::store::SandboxStoreError;

/// Binding and conversation writes use compare-and-swap. Conversation admission
/// additionally fences the exact parent revision in the same transaction.
#[async_trait::async_trait]
pub trait BindingStore: Send + Sync {
    async fn transfer_profile_conversation(
        &self,
        _request: super::ProfileTransferRequest,
        _enabled: bool,
    ) -> Result<super::ProfileTransferReceipt, super::BindingError> {
        Err(super::BindingError::ResetRefused(
            "partition_reset_unsupported",
        ))
    }
    async fn conversation_reset_state(
        &self,
        binding: &str,
    ) -> Result<super::ConversationResetState, SandboxStoreError> {
        let binding = self
            .binding(binding)
            .await?
            .ok_or(SandboxStoreError::NotFound)?;
        if binding.reset_format.is_some() {
            return Err(SandboxStoreError::Unavailable(
                "conversation reset reader unavailable".into(),
            ));
        }
        Ok(Default::default())
    }
    async fn reset_binding_conversation(
        &self,
        _request: super::ConversationResetRequest,
        _enabled: bool,
    ) -> Result<super::ConversationResetReceipt, super::BindingError> {
        Err(super::BindingError::ResetRefused(
            "partition_reset_unsupported",
        ))
    }
    /// Current and retained former binding keys cannot acquire ambient authority.
    async fn contains_key_hash(&self, hash: &str) -> Result<bool, SandboxStoreError>;
    /// Fresh classification for an external security decision. Implementations
    /// must observe prior acknowledged registrations/rotations across clients;
    /// ordinary majority lookups alone cannot authorize a negative result.
    /// Unsupported adapters fail closed instead of silently weakening the read.
    async fn classify_worker_key_hash(&self, _hash: &str) -> Result<bool, SandboxStoreError> {
        Err(SandboxStoreError::Unavailable(
            "fresh worker credential classification is unavailable".into(),
        ))
    }
    /// Unique hash lookup. Retired bindings never authenticate.
    async fn find_by_key_hash(
        &self,
        hash: &str,
    ) -> Result<Option<ProviderBinding>, SandboxStoreError>;
    async fn binding_conversation_for_agent(
        &self,
        binding: &str,
        generation: u64,
        agent: &crate::AgentId,
    ) -> Result<Option<BindingConversation>, SandboxStoreError>;

    async fn binding(&self, id: &str) -> Result<Option<ProviderBinding>, SandboxStoreError>;
    async fn save_binding(
        &self,
        expected: Option<u64>,
        next: ProviderBinding,
    ) -> Result<(), SandboxStoreError>;
    async fn pending_bindings(
        &self,
        after: Option<&str>,
        limit: u16,
    ) -> Result<Vec<ProviderBinding>, SandboxStoreError>;
    async fn binding_conversation(
        &self,
        binding: &str,
        generation: u64,
        id: &str,
    ) -> Result<Option<BindingConversation>, SandboxStoreError>;
    async fn save_binding_conversation(
        &self,
        binding_revision: u64,
        expected: Option<u64>,
        next: BindingConversation,
    ) -> Result<(), SandboxStoreError>;
    async fn binding_conversations(
        &self,
        binding: &str,
        generation: u64,
        after: Option<&str>,
        limit: u16,
    ) -> Result<Vec<BindingConversation>, SandboxStoreError>;
    async fn pending_binding_conversations(
        &self,
        after: Option<&str>,
        limit: u16,
    ) -> Result<Vec<BindingConversation>, SandboxStoreError>;
}

pub fn validate_binding_write(
    current: Option<&ProviderBinding>,
    expected: Option<u64>,
    next: &ProviderBinding,
) -> Result<(), SandboxStoreError> {
    next.validate().map_err(SandboxStoreError::Corrupt)?;
    match (current, expected) {
        (None, None)
            if next.revision == 1
                && next.generation == 1
                && next.generation_guard.is_none()
                && next.reset_format.is_none()
                && next.previous.is_none()
                && (matches!(
                    (&next.state, next.work),
                    (BindingState::Provisioning, BindingWork::AwaitingCredential)
                ) || matches!(
                    (&next.state, next.work),
                    (BindingState::Failed { .. }, BindingWork::Idle)
                ) || (next.existing_agent.is_some()
                    && next.state == BindingState::Ready
                    && next.work == BindingWork::Idle)) =>
        {
            Ok(())
        }
        (Some(current), Some(expected))
            if current.revision == expected && expected.checked_add(1) == Some(next.revision) =>
        {
            current.validate().map_err(SandboxStoreError::Corrupt)?;
            if current.binding_id != next.binding_id
                || current.owner != next.owner
                || current.manager != next.manager
                || current.tool_endpoint != next.tool_endpoint
                || current.existing_agent != next.existing_agent
                || current.created_at != next.created_at
                || current.generation_guard != next.generation_guard
                || current.reset_format != next.reset_format
                || crate::seconds_from_rfc3339(&next.updated_at)
                    < crate::seconds_from_rfc3339(&current.updated_at)
                || (current.state == BindingState::Retired && next.state != BindingState::Retired)
            {
                return Err(SandboxStoreError::Conflict);
            }
            if current.generation != next.generation {
                return Err(SandboxStoreError::Conflict);
            }
            if current.profile != next.profile {
                let proposed = current
                    .reconfigure(
                        next.profile.clone(),
                        next.key_sha256.clone(),
                        next.updated_at.clone(),
                    )
                    .map_err(|_| SandboxStoreError::Conflict)?;
                return if &proposed == next {
                    Ok(())
                } else {
                    Err(SandboxStoreError::Conflict)
                };
            }
            if current.generation == next.generation && current.key_sha256 != next.key_sha256 {
                let proposed = current
                    .rotate_credential(next.key_sha256.clone(), next.updated_at.clone())
                    .map_err(|_| SandboxStoreError::Conflict)?;
                return if &proposed == next {
                    Ok(())
                } else {
                    Err(SandboxStoreError::Conflict)
                };
            }
            if current.sandbox_id != next.sandbox_id
                || current.workspace_id != next.workspace_id
                || (current.previous != next.previous && next.previous.is_some())
            {
                return Err(SandboxStoreError::Conflict);
            }
            Ok(())
        }
        _ => Err(SandboxStoreError::Conflict),
    }
}

pub fn validate_conversation_write(
    binding: &ProviderBinding,
    binding_revision: u64,
    current: Option<&BindingConversation>,
    expected: Option<u64>,
    next: &BindingConversation,
) -> Result<(), SandboxStoreError> {
    binding.validate().map_err(SandboxStoreError::Corrupt)?;
    next.validate().map_err(SandboxStoreError::Corrupt)?;
    let parent_matches = if next.generation == binding.generation {
        next.sandbox_id == binding.sandbox_id && next.workspace_id == binding.workspace_id
    } else {
        binding.previous.as_ref().is_some_and(|old| {
            old.generation == next.generation
                && old.sandbox_id == next.sandbox_id
                && old.workspace_id == next.workspace_id
        })
    };
    if binding.revision != binding_revision
        || binding.binding_id != next.binding_id
        || binding.owner != next.owner
        || !parent_matches
        || next.borrowed != binding.existing_agent.is_some()
        || binding.existing_agent.as_ref().is_some_and(|agent| {
            next.agent_id != agent.agent_id
                || next.worktree_id != agent.worktree_id
                || next.conversation_id != super::EXISTING_AGENT_CONVERSATION
        })
        || (matches!(
            next.state,
            BindingConversationState::Provisioning | BindingConversationState::Ready
        ) && (binding.state != BindingState::Ready || next.generation != binding.generation))
    {
        return Err(SandboxStoreError::Conflict);
    }
    match (current, expected) {
        (None, None)
            if binding.state == BindingState::Ready
                && next.generation == binding.generation
                && next.revision == 1
                && next.profile_transfer.is_none()
                && (matches!(
                    next.state,
                    BindingConversationState::Provisioning | BindingConversationState::Retiring
                ) || (binding.existing_agent.is_some()
                    && next.state == BindingConversationState::Ready)) =>
        {
            Ok(())
        }
        (Some(current), Some(expected))
            if current.revision == expected && expected.checked_add(1) == Some(next.revision) =>
        {
            current.validate().map_err(SandboxStoreError::Corrupt)?;
            let mut immutable = next.clone();
            immutable.state = current.state;
            immutable.updated_at = current.updated_at.clone();
            immutable.revision = current.revision;
            if &immutable != current
                || !conversation_transition(current.state, next.state)
                || crate::seconds_from_rfc3339(&next.updated_at)
                    < crate::seconds_from_rfc3339(&current.updated_at)
            {
                return Err(SandboxStoreError::Conflict);
            }
            Ok(())
        }
        _ => Err(SandboxStoreError::Conflict),
    }
}

fn conversation_transition(from: BindingConversationState, to: BindingConversationState) -> bool {
    use BindingConversationState::*;
    from == to
        || matches!(
            (from, to),
            (Provisioning, Ready | Failed | Retiring)
                | (Ready, Failed | Retiring)
                | (Failed, Retiring)
                | (Retiring, Retired)
        )
}

pub fn binding_has_work(binding: &ProviderBinding) -> bool {
    !matches!(
        binding.work,
        BindingWork::Idle | BindingWork::AwaitingCredential
    )
}

/// Failed records retain their original intent without being rewritten merely
/// to rediscover them. Parent/custody recovery is proved only at admission.
pub fn binding_is_recovery_candidate(binding: &ProviderBinding) -> bool {
    matches!(binding.state, BindingState::Failed { .. })
        && binding.work == BindingWork::Idle
        && binding.previous.is_none()
        && binding.existing_agent.is_none()
        && binding.profile.environment.is_some()
}

pub fn conversation_storage_key(value: &BindingConversation) -> String {
    conversation_storage_key_for(&value.binding_id, value.generation, &value.conversation_id)
}

pub fn conversation_storage_key_for(binding: &str, generation: u64, conversation: &str) -> String {
    format!("{binding}:{generation:020}:{conversation}")
}

pub fn conversation_has_work(value: &BindingConversation) -> bool {
    matches!(
        value.state,
        BindingConversationState::Provisioning | BindingConversationState::Retiring
    )
}

pub fn validate_page_limit(limit: u16) -> Result<(), SandboxStoreError> {
    if (1..=200).contains(&limit) {
        Ok(())
    } else {
        Err(SandboxStoreError::InvalidTransaction(
            "binding page limit must be between 1 and 200",
        ))
    }
}
