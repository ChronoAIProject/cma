use super::*;

impl AgentCommandService {
    /// Called only after independent receiver authentication and exact local
    /// coordinator ownership verification. A scope token is not a substitute
    /// for the durable ownership fence performed by the application store.
    pub async fn execute_owner_mutation(
        &self,
        context: &CallerContext,
        credential: &str,
        source: CommandSubmissionSource,
        mutation: OwnerCommandMutation,
    ) -> Result<OwnerCommandReply, CommandAdmissionError> {
        let authority = crate::agent_ownership::execution_authority()
            .filter(|authority| {
                authority.is_current()
                    && authority.token.epoch > 0
                    && authority.token.protocol == crate::agent_ownership::PROTOCOL_VERSION
                    && authority.token.agent_id == *mutation.agent_id()
            })
            .ok_or(CommandLedgerError::Unavailable)?;
        let local = Self {
            store: self.store.clone(),
            access: self.access.clone(),
            authority: self.authority.clone(),
            task_experience: self.task_experience.clone(),
            owner_routing: None,
        };
        authority
            .run(async {
                match mutation {
                    OwnerCommandMutation::Admit {
                        submission,
                        binding_policy,
                    } => OwnerCommandReply::Admit(
                        local
                            .admit_inner(context, credential, source, submission, binding_policy)
                            .await,
                    ),
                    OwnerCommandMutation::TargetedControl(value) => {
                        OwnerCommandReply::TargetedControl(
                            local
                                .admit_response_control(context, credential, source, value)
                                .await,
                        )
                    }
                    OwnerCommandMutation::DirectTargetedControl(value) => {
                        OwnerCommandReply::DirectTargetedControl(
                            local
                                .admit_targeted_control(context, credential, source, value)
                                .await,
                        )
                    }
                    OwnerCommandMutation::InterruptingMessage(value) => {
                        OwnerCommandReply::InterruptingMessage(
                            local
                                .interrupt_with_message(context, credential, source, value)
                                .await,
                        )
                    }
                    OwnerCommandMutation::QueuedInput(value) => OwnerCommandReply::QueuedInput(
                        local
                            .edit_queued_input(context, credential, source, value)
                            .await,
                    ),
                    OwnerCommandMutation::QueuedCancellation(value) => {
                        OwnerCommandReply::QueuedCancellation(
                            local
                                .cancel_queued_response(context, credential, source, value)
                                .await,
                        )
                    }
                }
            })
            .await
            .map_err(|_| CommandLedgerError::Unavailable.into())
    }
}
