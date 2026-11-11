//! Provider-only input decoration. Retained commands use existing text content.
use super::{error, routing, BindingRouteError, ResponsesV2State};
use crate::AuthenticatedResourceCaller;
use axum::response::Response;
use layer2_core::sandbox::agent_io::{
    AgentCommand, AgentContent, CommandLedgerError, CommandSubmission,
};

impl ResponsesV2State {
    pub(super) async fn apply_sender_context(
        &self,
        auth: &AuthenticatedResourceCaller,
        submission: &mut CommandSubmission,
        generation: u64,
    ) -> Result<(), Box<Response>> {
        let stored = self.retained_sender_command(auth, submission).await?;
        if let Some(stored) = stored {
            if submission.command.canonical().as_ref() == Ok(&stored) {
                return Ok(());
            }
            let candidate = self.locally_prefixed(submission);
            if candidate
                .as_ref()
                .is_some_and(|candidate| candidate.command == stored)
            {
                *submission = candidate.expect("matched candidate");
                return Ok(());
            }
            return Err(conflict());
        }
        let router = self
            .event_router
            .as_ref()
            .ok_or_else(|| Box::new(routing::error(BindingRouteError::SenderUnavailable)))?;
        let event = submission
            .command
            .external_event()
            .expect("event checked by caller");
        let line = router
            .verified_sender_line(auth, event, generation)
            .await
            .map_err(|error| Box::new(routing::error(error)))?;
        prefix(submission, line)
            .map_err(|_| Box::new(routing::error(BindingRouteError::SenderUnsupported)))
    }

    /// Disabled policies reach this only after the ordinary ledger reports a
    /// key conflict. Complete equality grants replay of existing work only.
    pub(super) async fn replay_sender_context(
        &self,
        auth: &AuthenticatedResourceCaller,
        submission: &CommandSubmission,
    ) -> Result<Option<CommandSubmission>, Box<Response>> {
        let Some(candidate) = self.locally_prefixed(submission) else {
            return Ok(None);
        };
        let stored = self.retained_sender_command(auth, submission).await?;
        Ok(stored
            .filter(|stored| *stored == candidate.command)
            .map(|_| candidate))
    }

    async fn retained_sender_command(
        &self,
        auth: &AuthenticatedResourceCaller,
        submission: &CommandSubmission,
    ) -> Result<Option<AgentCommand>, Box<Response>> {
        let credential = auth
            .credential
            .as_ref()
            .ok_or_else(|| Box::new(error::unauthenticated()))?;
        self.commands
            .retained_binding_message(
                &auth.context,
                credential.value(),
                &submission.agent_id,
                &submission.idempotency_key,
            )
            .await
            .map_err(|error| Box::new(super::error::binding_admission(error)))
    }

    fn locally_prefixed(&self, submission: &CommandSubmission) -> Option<CommandSubmission> {
        let line = self
            .event_router
            .as_ref()?
            .sender_line(submission.command.external_event()?)
            .ok()?;
        let mut candidate = submission.clone();
        prefix(&mut candidate, line).ok()?;
        Some(candidate)
    }
}

fn conflict() -> Box<Response> {
    Box::new(error::binding_admission(
        CommandLedgerError::IdempotencyConflict.into(),
    ))
}
fn prefix(submission: &mut CommandSubmission, line: String) -> Result<(), ()> {
    let AgentCommand::Message { input, .. } = &mut submission.command else {
        return Err(());
    };
    input.content.insert(0, AgentContent::Text { text: line });
    submission.command = submission.command.canonical().map_err(|_| ())?;
    Ok(())
}
