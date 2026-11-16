use super::*;

impl AgentCommandService {
    /// Validate input and current credential custody before contacting an owner.
    /// The prepared snapshot is deliberately discarded: the owner prepares its
    /// own reservation and must not inherit source-AM execution authority.
    pub(super) async fn prepare_owner_mutation(
        &self,
        context: &CallerContext,
        credential: &str,
        source: CommandSubmissionSource,
        mutation: &OwnerCommandMutation,
    ) -> Result<(), CommandAdmissionError> {
        let plain = |agent_id: &AgentId, key: &str| CommandSubmission {
            agent_id: agent_id.clone(),
            command: AgentCommand::Interrupt,
            idempotency_key: key.to_owned(),
            expected_agent_revision: None,
            requested_model: None,
        };
        match mutation {
            OwnerCommandMutation::Admit { submission, .. } => {
                self.prepare(context, credential, source, submission.clone())
                    .await?;
            }
            OwnerCommandMutation::TargetedControl(value)
            | OwnerCommandMutation::DirectTargetedControl(value) => {
                self.prepare_targeted(context, credential, source, value.clone())
                    .await?;
            }
            OwnerCommandMutation::QueuedInput(value) => {
                queued_input_fingerprint(&value.agent_id, &value.target, &value.text)?;
                self.prepare(
                    context,
                    credential,
                    source,
                    plain(&value.agent_id, &value.idempotency_key),
                )
                .await?;
            }
            OwnerCommandMutation::QueuedCancellation(value) => {
                value.target.fingerprint(&value.agent_id)?;
                self.prepare(
                    context,
                    credential,
                    source,
                    plain(&value.agent_id, &value.idempotency_key),
                )
                .await?;
            }
            OwnerCommandMutation::InterruptingMessage(value) => {
                self.prepare_targeted(
                    context,
                    credential,
                    source,
                    TargetedControlSubmission {
                        agent_id: value.agent_id.clone(),
                        target: value.target.clone(),
                        expected_agent_revision: value.expected_agent_revision,
                        command: AgentCommand::Interrupt,
                        idempotency_key: value.idempotency_key.clone(),
                    },
                )
                .await?;
                self.prepare(
                    context,
                    credential,
                    source,
                    CommandSubmission {
                        agent_id: value.agent_id.clone(),
                        command: AgentCommand::Message {
                            input: AgentInput::text(value.text.clone()),
                            order: AgentInputOrder::InterruptAfterTurn {
                                turn_sequence: value.target.turn_sequence,
                            },
                            metadata: Default::default(),
                        },
                        idempotency_key: value.idempotency_key.clone(),
                        expected_agent_revision: Some(value.expected_agent_revision),
                        requested_model: None,
                    },
                )
                .await?;
            }
        }
        Ok(())
    }
}
