use super::{BindingConversation, BindingError, BindingService, EXISTING_AGENT_CONVERSATION};
use crate::sandbox::AgentStatus;
use crate::Caller;

pub struct BindingConversationInspection {
    pub conversation: BindingConversation,
    pub agent_status: Option<AgentStatus>,
}

impl BindingService {
    pub async fn inspect_conversation(
        &self,
        caller: &Caller,
        id: &str,
        conversation_id: &str,
    ) -> Result<BindingConversationInspection, BindingError> {
        if !super::model::valid_id(conversation_id, 8) {
            return Err(BindingError::InvalidRequest(
                "invalid conversation identifier".into(),
            ));
        }
        let binding = self.read_for_retirement(caller, id).await?;
        let conversation_id = if binding.existing_agent.is_some() {
            EXISTING_AGENT_CONVERSATION
        } else {
            conversation_id
        };
        let conversation = self
            .store
            .binding_conversation(id, binding.generation, conversation_id)
            .await?
            .ok_or(BindingError::NotFound)?;
        conversation.validate().map_err(BindingError::Corrupt)?;
        if conversation.binding_id != binding.binding_id
            || conversation.generation != binding.generation
            || conversation.owner != binding.owner
            || conversation.sandbox_id != binding.sandbox_id
            || conversation.workspace_id != binding.workspace_id
        {
            return Err(BindingError::Corrupt(
                "conversation ownership is inconsistent".into(),
            ));
        }
        let agent = self.store.get_agent(&conversation.agent_id).await?;
        if agent.as_ref().is_some_and(|agent| {
            agent.owner != conversation.owner
                || agent.sandbox_id != conversation.sandbox_id
                || agent.workspace_id != conversation.workspace_id
                || agent.worktree_id != conversation.worktree_id
        }) {
            return Err(BindingError::Corrupt(
                "conversation Agent ownership is inconsistent".into(),
            ));
        }
        if self.read_for_retirement(caller, id).await? != binding {
            return Err(BindingError::Conflict);
        }
        Ok(BindingConversationInspection {
            conversation,
            agent_status: agent.map(|agent| agent.status),
        })
    }
}
