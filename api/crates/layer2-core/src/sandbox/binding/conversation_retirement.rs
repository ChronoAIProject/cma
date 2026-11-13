//! Retirement writes tombstones without admitting provisionable conversations.

use super::{
    BindingConversation, BindingConversationState, BindingError, BindingService, BindingState,
    EXISTING_AGENT_CONVERSATION,
};
use crate::sandbox::store::SandboxStoreError;
use crate::{now_rfc3339, Caller};

impl BindingService {
    pub async fn retire_conversation(
        &self,
        caller: &Caller,
        id: &str,
        conversation_id: &str,
    ) -> Result<(), BindingError> {
        if !super::model::valid_id(conversation_id, 8) {
            return Err(BindingError::InvalidRequest(
                "invalid conversation identifier".into(),
            ));
        }
        for _ in 0..8 {
            let binding = self.read_for_retirement(caller, id).await?;
            let conversation_id = if binding.existing_agent.is_some() {
                EXISTING_AGENT_CONVERSATION
            } else {
                conversation_id
            };
            let current = self
                .store
                .binding_conversation(id, binding.generation, conversation_id)
                .await?;
            let (expected, mut next) = match current {
                Some(value) => {
                    if matches!(
                        value.state,
                        BindingConversationState::Retiring | BindingConversationState::Retired
                    ) {
                        return Ok(());
                    }
                    let mut next = value.clone();
                    next.revision = next.revision.checked_add(1).ok_or(BindingError::Conflict)?;
                    next.updated_at = now_rfc3339();
                    (Some(value.revision), next)
                }
                None => {
                    if binding.state != BindingState::Ready {
                        return Ok(());
                    }
                    // Preserve orphan retirement semantics without creating
                    // either a tombstone or a provisionable conversation.
                    match super::readiness::validate(self.store.as_ref(), &binding).await {
                        Err(BindingError::ResourcesUnavailable) => return Ok(()),
                        result => result?,
                    }
                    let next =
                        BindingConversation::new(&binding, conversation_id.into(), now_rfc3339())
                            .map_err(BindingError::InvalidRequest)?;
                    (None, next)
                }
            };
            next.state = BindingConversationState::Retiring;
            match self
                .store
                .save_binding_conversation(binding.revision, expected, next)
                .await
            {
                Ok(()) => return Ok(()),
                Err(SandboxStoreError::Conflict) => continue,
                Err(error) => return Err(error.into()),
            }
        }
        Err(BindingError::Conflict)
    }
}
