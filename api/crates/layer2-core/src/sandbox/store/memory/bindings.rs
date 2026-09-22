use super::*;
use crate::sandbox::binding::store::{
    binding_has_work, conversation_has_work, conversation_storage_key, validate_binding_write,
    validate_conversation_write, validate_page_limit,
};
use crate::sandbox::binding::{BindingConversation, BindingStore, ProviderBinding};

#[async_trait]
impl BindingStore for InMemorySandboxStore {
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
                binding_has_work(b) && after.is_none_or(|after| b.binding_id.as_str() > after)
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
