use super::*;
use futures_util::{future::BoxFuture, FutureExt};

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
            .run(owner_mutation_future(
                &local, context, credential, source, mutation,
            ))
            .await
            .map_err(|_| CommandLedgerError::Unavailable.into())
    }
}

// Finish constructing the large branch future before polling it. Keeping this
// frame separate prevents unused mutation branches from consuming the caller's
// worker stack throughout the selected operation. Async bodies still run only
// when polled inside the authority scope above.
#[inline(never)]
fn owner_mutation_future<'a>(
    local: &'a AgentCommandService,
    context: &'a CallerContext,
    credential: &'a str,
    source: CommandSubmissionSource,
    mutation: OwnerCommandMutation,
) -> BoxFuture<'a, OwnerCommandReply> {
    match mutation {
        OwnerCommandMutation::Admit {
            submission,
            binding_policy,
        } => local
            .admit_inner(context, credential, source, submission, binding_policy)
            .map(OwnerCommandReply::Admit)
            .boxed(),
        OwnerCommandMutation::TargetedControl(value) => local
            .admit_response_control(context, credential, source, value)
            .map(OwnerCommandReply::TargetedControl)
            .boxed(),
        OwnerCommandMutation::DirectTargetedControl(value) => local
            .admit_targeted_control(context, credential, source, value)
            .map(OwnerCommandReply::DirectTargetedControl)
            .boxed(),
        OwnerCommandMutation::InterruptingMessage(value) => local
            .interrupt_with_message(context, credential, source, value)
            .map(OwnerCommandReply::InterruptingMessage)
            .boxed(),
        OwnerCommandMutation::QueuedInput(value) => local
            .edit_queued_input(context, credential, source, value)
            .map(OwnerCommandReply::QueuedInput)
            .boxed(),
        OwnerCommandMutation::QueuedCancellation(value) => local
            .cancel_queued_response(context, credential, source, value)
            .map(OwnerCommandReply::QueuedCancellation)
            .boxed(),
    }
}
