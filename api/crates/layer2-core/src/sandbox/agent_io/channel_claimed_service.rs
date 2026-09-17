use crate::sandbox::{
    AgentMessageOutcome, AgentOperationError, AgentOperationService, AgentStatus,
};
use crate::{now_rfc3339, RuntimeCapability};
use layer3_adapter::{ContentBlock, ConversationInput};

use super::{AdmittedAgentCommand, AgentCommand, ChannelExecutionAuthorization};

const CHANNEL_DELIVERY_INSTRUCTIONS: &str = "[Trusted channel delivery instructions]\n\
This input is a message from a user in the channel that created this Agent. \
Call read_channel_context for this input, then use reply_to_channel with the returned \
reply_context to send your response to the user before ending the turn. You may send \
multiple progress updates and continue working between replies. Final assistant text \
is visible only in CMA and is not sent to the channel. Never reuse a reply_context \
from a previous input. Use a new request_key for each intentional message; retain \
the same request_key and text when retrying that message. A queued receipt does not \
mean delivery has completed.\n[/Trusted channel delivery instructions]";

fn add_channel_delivery_instructions(input: &mut ConversationInput) {
    if let ConversationInput::RecoverablePrompt { content, .. } = input {
        content.insert(0, ContentBlock::from(CHANNEL_DELIVERY_INSTRUCTIONS));
    }
}

impl AgentOperationService {
    /// Execute exactly the claimed Channel message through the common
    /// conversation transaction and ordered writer, without a human Caller.
    pub async fn execute_channel_claim(
        &self,
        authority: &ChannelExecutionAuthorization,
        command: &AdmittedAgentCommand,
    ) -> Result<AgentMessageOutcome, AgentOperationError> {
        command
            .validate()
            .map_err(|_| AgentOperationError::Corrupt("command ledger is invalid".into()))?;
        let fence = authority.fence();
        fence
            .require_dispatch(command)
            .map_err(|_| AgentOperationError::Forbidden)?;
        let claim = &command
            .work
            .lease
            .as_ref()
            .ok_or(AgentOperationError::Conflict)?
            .claim;
        command
            .require_claim(claim, &now_rfc3339())
            .map_err(|_| AgentOperationError::Conflict)?;
        if command.work.dispatch.is_some() {
            return Err(AgentOperationError::Conflict);
        }
        let AgentCommand::Message {
            input, metadata, ..
        } = &command.receipt.envelope.command
        else {
            return Err(AgentOperationError::Forbidden);
        };
        let text = input.clone().into_text()?;
        self.validate_message(&text)?;
        let current = fence
            .proof()
            .resources()
            .lineage
            .agent
            .clone()
            .ok_or(AgentOperationError::NotFound)?;
        if current.status != AgentStatus::Ready || current.requested_stop_reason.is_some() {
            return Err(AgentOperationError::StatusConflict {
                status: current.status,
            });
        }
        current
            .capabilities
            .require([RuntimeCapability::Conversation])
            .map_err(|capability| AgentOperationError::CapabilityUnsupported { capability })?;
        self.require_first_message_settled(&current)?;
        self.require_dispatch_available(&current)?;
        self.require_agent_connections(&current).await?;
        let (parent, queues) = self.parent_admission(&current).await?;
        if queues {
            return Err(AgentOperationError::Conflict);
        }
        let service = Self {
            command_claim: Some(claim.clone()),
            execution_authority: None,
            channel_control_authority: None,
            channel_authority: Some(authority.clone()),
            trigger_authority: None,
            connections: self.connections.clone(),
            store: self.store.clone(),
            runtimes: self.runtimes.clone(),
        };
        let mut prepared = service
            .fence_message(current, text, parent, metadata.clone())
            .await?;
        // The execution fence validates immutable channel-created provenance.
        // Add this on every channel dispatch, including queued inputs in an
        // existing conversation, while retaining the original ledger text.
        if fence.proof().grant().channel.is_some() {
            add_channel_delivery_instructions(&mut prepared.input);
        }
        service
            .dispatch_prepared(prepared)
            .await
            .map(AgentMessageOutcome::Dispatched)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use layer3_adapter::PromptIdentity;

    #[test]
    fn channel_delivery_instructions_reach_each_input_in_existing_conversation() {
        // Successive channel inputs must each carry the instructions even when
        // the conversation predates the channel-delivery fix.
        for (turn, text) in [(8, "Hey"), (12, "Shining")] {
            let prompt_id = PromptIdentity::new("existing-conversation", 3, turn);
            let mut input = ConversationInput::RecoverablePrompt {
                content: vec![ContentBlock::from(text)],
                prompt_id: prompt_id.clone(),
            };
            add_channel_delivery_instructions(&mut input);
            let ConversationInput::RecoverablePrompt {
                content,
                prompt_id: actual_id,
            } = input
            else {
                panic!("channel input must remain recoverable");
            };
            assert_eq!(actual_id, prompt_id, "retain the exact turn identity");
            let [ContentBlock::Text(instructions), ContentBlock::Text(original)] =
                content.as_slice()
            else {
                panic!("delivery instructions must precede the original user message");
            };
            assert_eq!(original.text, text);
            assert!(instructions.text.contains("Call read_channel_context"));
            assert!(instructions.text.contains("use reply_to_channel"));
            assert!(instructions.text.contains("before ending the turn"));
            assert!(instructions.text.contains("visible only in CMA"));
            assert!(instructions.text.contains("same request_key and text"));
            assert!(instructions
                .text
                .contains("does not mean delivery has completed"));
        }
    }
}
