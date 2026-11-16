use super::*;

impl AgentCommandService {
    pub(super) async fn authorize_owner_mutation(
        &self,
        context: &CallerContext,
        credential: &str,
        mutation: &OwnerCommandMutation,
    ) -> Result<(), CommandAdmissionError> {
        let command = match mutation {
            OwnerCommandMutation::Admit { submission, .. } => &submission.command,
            OwnerCommandMutation::TargetedControl(value)
            | OwnerCommandMutation::DirectTargetedControl(value) => &value.command,
            OwnerCommandMutation::QueuedInput(_)
            | OwnerCommandMutation::QueuedCancellation(_)
            | OwnerCommandMutation::InterruptingMessage(_) => &AgentCommand::Interrupt,
        };
        self.authorize_intent(context, credential, mutation.agent_id(), command)
            .await?;
        if let OwnerCommandMutation::InterruptingMessage(value) = mutation {
            // An atomic interruption contains two intents with independent
            // capabilities. OperateAgent never grants permission to send input.
            self.authorize_intent(
                context,
                credential,
                &value.agent_id,
                &AgentCommand::message(value.text.clone()),
            )
            .await?;
        }
        Ok(())
    }
}

impl OwnerCommandMutation {
    pub(super) fn validate_reply(
        &self,
        reply: &OwnerCommandReply,
    ) -> Result<(), CommandAdmissionError> {
        let bound = |view: &CommandStatusView| view.receipt.agent_id == *self.agent_id();
        let valid = match (self, reply) {
            (Self::Admit { submission, .. }, OwnerCommandReply::Admit(Ok(value))) => {
                bound(value)
                    && value.receipt.response_id.is_some()
                        == matches!(submission.command, AgentCommand::Message { .. })
            }
            (Self::TargetedControl(request), OwnerCommandReply::TargetedControl(Ok(value)))
            | (
                Self::DirectTargetedControl(request),
                OwnerCommandReply::DirectTargetedControl(Ok(value)),
            ) => {
                bound(&value.control)
                    && value.control.receipt.response_id.is_none()
                    && value.target == request.target
                    && value.expected_agent_revision == request.expected_agent_revision
            }
            (
                Self::InterruptingMessage(request),
                OwnerCommandReply::InterruptingMessage(Ok(value)),
            ) => {
                bound(&value.control)
                    && bound(&value.response)
                    && value.control.receipt.response_id.is_none()
                    && value.response.receipt.response_id.is_some()
                    && value.control.receipt.command_id != value.response.receipt.command_id
                    && value.control.receipt.agent_order.checked_add(1)
                        == Some(value.response.receipt.agent_order)
                    && value.target == request.target
                    && value.expected_agent_revision == request.expected_agent_revision
            }
            (Self::QueuedInput(request), OwnerCommandReply::QueuedInput(Ok(value))) => {
                bound(&value.edit)
                    && bound(&value.response)
                    && value.edit.receipt.response_id.is_none()
                    && value.response.receipt.command_id == request.target.source_command_id
                    && value.response.receipt.response_id.as_ref()
                        == Some(&request.target.response_id)
                    && value.binding.target == request.target
                    && value.binding.text == request.text
                    && value.binding.command_id == value.edit.receipt.command_id
                    && value.binding.request_fingerprint
                        == queued_input_fingerprint(
                            &request.agent_id,
                            &request.target,
                            &request.text,
                        )?
            }
            (
                Self::QueuedCancellation(request),
                OwnerCommandReply::QueuedCancellation(Ok(value)),
            ) => {
                bound(&value.cancellation)
                    && bound(&value.response)
                    && value.cancellation.receipt.response_id.is_none()
                    && value.response.receipt.command_id == request.target.source_command_id
                    && value.response.receipt.response_id.as_ref()
                        == Some(&request.target.response_id)
                    && value.target == request.target
            }
            (Self::Admit { .. }, OwnerCommandReply::Admit(Err(_)))
            | (Self::TargetedControl(_), OwnerCommandReply::TargetedControl(Err(_)))
            | (Self::DirectTargetedControl(_), OwnerCommandReply::DirectTargetedControl(Err(_)))
            | (Self::InterruptingMessage(_), OwnerCommandReply::InterruptingMessage(Err(_)))
            | (Self::QueuedInput(_), OwnerCommandReply::QueuedInput(Err(_)))
            | (Self::QueuedCancellation(_), OwnerCommandReply::QueuedCancellation(Err(_))) => true,
            _ => false,
        };
        if valid {
            Ok(())
        } else {
            Err(CommandLedgerError::Unavailable.into())
        }
    }
}
