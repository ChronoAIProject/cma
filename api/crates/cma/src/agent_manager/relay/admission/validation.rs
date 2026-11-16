use super::*;

impl AdmissionRouter {
    pub(super) async fn context(
        &self,
        call: &Call,
    ) -> Result<CallerContext, CommandAdmissionError> {
        let context = self
            .auth
            .restore_owner_admission(&call.identity, &call.credential)
            .await?;
        if self.name != "access" && call.origin.is_none() {
            // Public services must retain the authenticated request, including
            // its browser logout lineage. The access callback has a separate
            // composition and cannot be selected by a public handler.
            return Err(CommandAuthorizationError::Revoked.into());
        }
        if let Some(origin) = &call.origin {
            let (method, uri, headers) = origin.parts().map_err(|_| unavailable())?;
            self.auth
                .verify_owner_admission_origin(&context, &call.credential, &method, &uri, &headers)
                .await?;
        }
        Ok(context)
    }
    pub(super) async fn authorize(
        &self,
        context: &CallerContext,
        credential: &str,
        mutation: &OwnerCommandMutation,
    ) -> Result<(), CommandAdmissionError> {
        let service = self.service()?;
        let id = mutation.agent_id();
        match mutation {
            OwnerCommandMutation::Admit { submission, .. } => {
                service
                    .authorize_intent(context, credential, id, &submission.command)
                    .await
            }
            OwnerCommandMutation::TargetedControl(value)
            | OwnerCommandMutation::DirectTargetedControl(value) => {
                service
                    .authorize_intent(context, credential, id, &value.command)
                    .await
            }
            OwnerCommandMutation::QueuedInput(_) | OwnerCommandMutation::QueuedCancellation(_) => {
                service
                    .authorize_intent(context, credential, id, &AgentCommand::Interrupt)
                    .await
            }
            OwnerCommandMutation::InterruptingMessage(value) => {
                service
                    .authorize_intent(context, credential, id, &AgentCommand::Interrupt)
                    .await?;
                service
                    .authorize_intent(
                        context,
                        credential,
                        id,
                        &AgentCommand::Message {
                            input: AgentInput::text(&value.text),
                            order: AgentInputOrder::InterruptAfterTurn {
                                turn_sequence: value.target.turn_sequence,
                            },
                            metadata: Default::default(),
                        },
                    )
                    .await
            }
        }
    }
}
