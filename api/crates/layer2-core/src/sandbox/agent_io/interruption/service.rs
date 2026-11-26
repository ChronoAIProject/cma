use crate::resource_access::{CallerContext, ResourceCapability, ResourceTarget};

use super::super::{
    AgentCommand, AgentCommandService, AgentInput, AgentInputOrder, CommandLedgerError,
    CommandSubmission, CommandSubmissionSource, TargetedControlError, TargetedControlInspection,
    TargetedControlSubmission,
};
use super::{
    InterruptingMessageReservation, InterruptingMessageStatusView, InterruptingMessageSubmission,
};

impl AgentCommandService {
    pub async fn interrupt_with_message(
        &self,
        context: &CallerContext,
        credential: &str,
        source: CommandSubmissionSource,
        submission: InterruptingMessageSubmission,
    ) -> Result<InterruptingMessageStatusView, TargetedControlError> {
        let (control_authority, mut control) = self
            .prepare_targeted(
                context,
                credential,
                source,
                TargetedControlSubmission {
                    agent_id: submission.agent_id.clone(),
                    target: submission.target.clone(),
                    expected_agent_revision: submission.expected_agent_revision,
                    command: AgentCommand::Interrupt,
                    idempotency_key: submission.idempotency_key.clone(),
                },
            )
            .await?;
        control.retry_deadline =
            Some(tokio::time::Instant::now() + std::time::Duration::from_secs(5));
        let (message_authority, message) = self
            .prepare(
                context,
                credential,
                source,
                CommandSubmission {
                    agent_id: submission.agent_id.clone(),
                    command: AgentCommand::Message {
                        input: AgentInput::text(submission.text),
                        order: AgentInputOrder::InterruptAfterTurn {
                            turn_sequence: submission.target.turn_sequence,
                        },
                        metadata: Default::default(),
                    },
                    idempotency_key: submission.idempotency_key,
                    expected_agent_revision: Some(submission.expected_agent_revision),
                    requested_model: None,
                },
            )
            .await?;
        // Independently authorize both capabilities; a concurrent resource
        // revision cannot mix snapshots into a paired reservation.
        let message_agent = message_authority
            .snapshot
            .lineage
            .agent
            .as_ref()
            .ok_or(CommandLedgerError::Corrupt)?;
        if message_agent.revision != control.observed_agent_revision {
            return Err(CommandLedgerError::Conflict.into());
        }
        let request = InterruptingMessageReservation { control, message };
        request.validate()?;
        let target = ResourceTarget::Agent {
            agent_id: submission.agent_id,
        };
        let result = self
            .with_authority_bound(
                &control_authority,
                ResourceCapability::OperateAgent,
                self.with_authority_bound(
                    &message_authority,
                    ResourceCapability::Interact,
                    async {
                        match self
                            .store
                            .inspect_targeted_control(request.control.clone())
                            .await?
                        {
                            TargetedControlInspection::NotAdmitted => {
                                return Err(TargetedControlError::NotAdmitted)
                            }
                            TargetedControlInspection::Existing(_) => {}
                            TargetedControlInspection::Missing => {
                                let agent = control_authority
                                    .snapshot
                                    .lineage
                                    .agent
                                    .as_ref()
                                    .ok_or(CommandLedgerError::Corrupt)?;
                                let response = self
                                    .store
                                    .get_response_command(&request.control.target.response_id)
                                    .await?
                                    .ok_or(CommandLedgerError::NotFound)?;
                                request.control.validate_target(agent, &response)?;
                                if !agent
                                    .capabilities
                                    .supports(crate::RuntimeCapability::Conversation)
                                    || !agent
                                        .capabilities
                                        .supports(crate::RuntimeCapability::Interrupt)
                                {
                                    return Err(TargetedControlError::CapabilityUnsupported);
                                }
                            }
                        }
                        Ok(self
                            .store
                            .reserve_interrupting_message(request.clone())
                            .await?)
                    },
                ),
            )
            .await;
        self.fence(
            &control_authority,
            &target,
            ResourceCapability::OperateAgent,
        )
        .await?;
        self.fence(&message_authority, &target, ResourceCapability::Interact)
            .await?;
        let result = result???;
        let result = request.replay(result.control, result.message)?;
        self.notify_new_cancel(&request.control.command.envelope, &result.control.record)
            .await;
        Ok(result.into())
    }
}
