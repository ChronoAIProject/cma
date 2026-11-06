use super::super::conversation::NativeCompletionEvidence;
use super::{
    AgentConversationError, AgentMessageAssembly, AgentUpdateOutcome, AgentUpdateRecorder,
    DurableAgentUpdateRecorder,
};
use crate::ids::AgentId;
use layer3_adapter::{ConversationUpdate, PromptResponse};

impl DurableAgentUpdateRecorder {
    pub(crate) async fn reconcile_completion(
        &self,
        id: &AgentId,
        runtime_generation: u64,
        assembly: &mut AgentMessageAssembly,
        evidence: NativeCompletionEvidence,
        response: PromptResponse,
        appended_at: String,
    ) -> Result<AgentUpdateOutcome, AgentConversationError> {
        if runtime_generation != evidence.runtime_generation {
            return Err(AgentConversationError::StaleGeneration);
        }
        if evidence.native_conversation_id.trim().is_empty()
            || evidence.native_turn_id.trim().is_empty()
        {
            return Err(AgentConversationError::InvalidState(
                "native completion proof is incomplete",
            ));
        }
        let final_answer = evidence.final_answer.as_str();
        if evidence.final_answer_durable {
            assembly.content.clear();
            assembly.message_id = None;
        } else if assembly.content != final_answer {
            if !final_answer.starts_with(&assembly.content) {
                return Err(AgentConversationError::InvalidState(
                    "native completion answer does not extend the durable prefix",
                ));
            }
            assembly.content = evidence.final_answer.clone();
            assembly.prompt_evidence = true;
        }
        assembly.reconciliation = Some(evidence.clone());
        let result = self
            .record_update(
                id,
                runtime_generation,
                assembly,
                ConversationUpdate::TurnEndedForPrompt {
                    prompt_id: layer3_adapter::PromptIdentity::new(
                        &evidence.native_conversation_id,
                        evidence.runtime_generation,
                        evidence.turn_sequence,
                    ),
                    response,
                },
                appended_at,
            )
            .await;
        assembly.reconciliation = None;
        result
    }
}
