use super::{BindingConversation, BindingError, BindingService, EXISTING_AGENT_CONVERSATION};
use crate::sandbox::{AgentInstructionPolicy, AgentStatus};
use crate::Caller;

mod drift;
pub use drift::{BindingProfileDrift, DriftState};

pub struct BindingConversationInspection {
    pub conversation: BindingConversation,
    pub incarnation: u64,
    pub transport_conversation_id: String,
    pub agent_status: Option<AgentStatus>,
    pub profile_drift: Option<BindingProfileDrift>,
}

impl BindingService {
    pub async fn inspect_conversation(
        &self,
        caller: &Caller,
        id: &str,
        conversation_id: &str,
    ) -> Result<BindingConversationInspection, BindingError> {
        self.inspect_conversation_with_profile_drift(caller, id, conversation_id, None)
            .await
    }

    pub async fn inspect_conversation_with_profile_drift(
        &self,
        caller: &Caller,
        id: &str,
        conversation_id: &str,
        instruction_policy: Option<&dyn AgentInstructionPolicy>,
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
        let transport_conversation_id = conversation_id.to_owned();
        let resets = self.store.conversation_reset_state(id).await?;
        let head = resets.head(binding.generation, conversation_id);
        let incarnation = head.map_or(0, |h| h.incarnation);
        let conversation_id =
            head.map_or(conversation_id, |h| h.successor.conversation_id.as_str());
        let conversation = self
            .store
            .binding_conversation(id, binding.generation, conversation_id)
            .await?
            .or_else(|| head.map(|h| h.successor.clone()))
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
            incarnation,
            transport_conversation_id,
            conversation,
            profile_drift: instruction_policy
                .map(|policy| BindingProfileDrift::compare(&binding, agent.as_ref(), policy)),
            agent_status: agent.map(|agent| agent.status),
        })
    }
}
