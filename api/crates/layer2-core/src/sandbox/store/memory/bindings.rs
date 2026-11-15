use super::*;
use crate::sandbox::binding::store::{
    binding_has_work, conversation_has_work, conversation_storage_key, validate_binding_write,
    validate_conversation_write, validate_page_limit,
};
use crate::sandbox::binding::{BindingConversation, BindingStore, ProviderBinding};

#[async_trait]
impl BindingStore for InMemorySandboxStore {
    async fn contains_key_hash(&self, hash: &str) -> Result<bool, SandboxStoreError> {
        Ok(self.lock().binding_key_history.contains(hash))
    }
    async fn classify_worker_key_hash(&self, hash: &str) -> Result<bool, SandboxStoreError> {
        self.contains_key_hash(hash).await
    }
    async fn find_by_key_hash(
        &self,
        hash: &str,
    ) -> Result<Option<ProviderBinding>, SandboxStoreError> {
        Ok(self
            .lock()
            .bindings
            .values()
            .find(|value| {
                value.key_sha256 == hash
                    && value.state != crate::sandbox::binding::BindingState::Retired
            })
            .cloned())
    }
    async fn binding_conversation_for_agent(
        &self,
        binding: &str,
        generation: u64,
        agent: &crate::AgentId,
    ) -> Result<Option<BindingConversation>, SandboxStoreError> {
        Ok(self
            .lock()
            .binding_conversations
            .values()
            .find(|value| {
                value.binding_id == binding
                    && value.generation == generation
                    && value.agent_id == *agent
            })
            .cloned())
    }

    async fn binding(&self, id: &str) -> Result<Option<ProviderBinding>, SandboxStoreError> {
        Ok(self.lock().bindings.get(id).cloned())
    }

    async fn save_binding(
        &self,
        expected: Option<u64>,
        next: ProviderBinding,
    ) -> Result<(), SandboxStoreError> {
        let mut state = self.lock();
        validate_binding_write(state.bindings.get(&next.binding_id), expected, &next)?;
        if next.existing_agent.is_none()
            && state
                .bindings
                .get(&next.binding_id)
                .is_none_or(|old| old.generation != next.generation)
        {
            super::admission::require_operator_open(
                &state,
                crate::admission::AdmissionCause::Fresh,
            )?;
        }
        if state
            .bindings
            .values()
            .any(|value| value.binding_id != next.binding_id && value.key_sha256 == next.key_sha256)
        {
            return Err(SandboxStoreError::Conflict);
        }
        state.binding_key_history.insert(next.key_sha256.clone());
        state.bindings.insert(next.binding_id.clone(), next);
        Ok(())
    }

    async fn pending_bindings(
        &self,
        after: Option<&str>,
        limit: u16,
    ) -> Result<Vec<ProviderBinding>, SandboxStoreError> {
        validate_page_limit(limit)?;
        Ok(self
            .lock()
            .bindings
            .values()
            .filter(|b| {
                (binding_has_work(b)
                    || crate::sandbox::binding::store::binding_is_recovery_candidate(b))
                    && after.is_none_or(|after| b.binding_id.as_str() > after)
            })
            .take(usize::from(limit))
            .cloned()
            .collect())
    }

    async fn binding_conversation(
        &self,
        binding: &str,
        generation: u64,
        id: &str,
    ) -> Result<Option<BindingConversation>, SandboxStoreError> {
        Ok(self
            .lock()
            .binding_conversations
            .get(&format!("{binding}:{generation:020}:{id}"))
            .cloned())
    }

    async fn save_binding_conversation(
        &self,
        binding_revision: u64,
        expected: Option<u64>,
        next: BindingConversation,
    ) -> Result<(), SandboxStoreError> {
        let mut state = self.lock();
        let binding = state
            .bindings
            .get(&next.binding_id)
            .ok_or(SandboxStoreError::NotFound)?;
        let key = conversation_storage_key(&next);
        validate_conversation_write(
            binding,
            binding_revision,
            state.binding_conversations.get(&key),
            expected,
            &next,
        )?;
        if expected.is_none()
            && !state.binding_conversations.contains_key(&key)
            && matches!(
                next.state,
                crate::sandbox::binding::BindingConversationState::Provisioning
                    | crate::sandbox::binding::BindingConversationState::Ready
            )
        {
            super::admission::require_operator_open(
                &state,
                crate::admission::AdmissionCause::Fresh,
            )?;
        }
        if state.binding_conversations.iter().any(|(id, value)| {
            id != &key
                && value.agent_id == next.agent_id
                && value.binding_id == next.binding_id
                && value.generation == next.generation
        }) {
            return Err(SandboxStoreError::Conflict);
        }
        state
            .binding_association_index
            .insert((next.owner.clone(), next.agent_id.clone(), key.clone()), ());
        state.binding_conversations.insert(key, next);
        Ok(())
    }

    async fn binding_conversations(
        &self,
        binding: &str,
        generation: u64,
        after: Option<&str>,
        limit: u16,
    ) -> Result<Vec<BindingConversation>, SandboxStoreError> {
        validate_page_limit(limit)?;
        Ok(self
            .lock()
            .binding_conversations
            .values()
            .filter(|c| {
                c.binding_id == binding
                    && c.generation == generation
                    && after.is_none_or(|after| c.conversation_id.as_str() > after)
            })
            .take(usize::from(limit))
            .cloned()
            .collect())
    }

    async fn pending_binding_conversations(
        &self,
        after: Option<&str>,
        limit: u16,
    ) -> Result<Vec<BindingConversation>, SandboxStoreError> {
        validate_page_limit(limit)?;
        Ok(self
            .lock()
            .binding_conversations
            .iter()
            .filter(|(key, c)| {
                conversation_has_work(c) && after.is_none_or(|after| key.as_str() > after)
            })
            .take(usize::from(limit))
            .map(|(_, c)| c.clone())
            .collect())
    }
}

pub(super) fn require_conversation_authority(
    state: &super::State,
    proof: Option<&crate::sandbox::binding::BindingConversationAuthority>,
) -> Result<(), SandboxStoreError> {
    let Some(proof) = proof else { return Ok(()) };
    let binding = state
        .bindings
        .get(&proof.binding.binding_id)
        .ok_or(SandboxStoreError::Conflict)?;
    let conversation = state
        .binding_conversations
        .get(&conversation_storage_key(&proof.conversation))
        .ok_or(SandboxStoreError::Conflict)?;
    proof.require_current(binding, conversation)
}

pub(super) fn continuation_retry_authority(
    state: &super::State,
    envelope: &crate::sandbox::agent_io::AgentCommandEnvelope,
) -> bool {
    let Some(id) = envelope.authorization.authority_id.strip_prefix("binding:") else {
        return false;
    };
    let Some(binding) = state.bindings.get(id) else {
        return false;
    };
    let Some(conversation) = state.binding_conversations.values().find(|conversation| {
        conversation.binding_id == id
            && conversation.generation == binding.generation
            && conversation.agent_id == envelope.agent_id
    }) else {
        return false;
    };
    crate::sandbox::binding::BindingConversationAuthority {
        binding: binding.clone(),
        conversation: conversation.clone(),
    }
    .require_command(envelope)
    .is_ok()
}
