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
    ) -> Result<Option<u64>, Box<Response>> {
        if submission.command.external_event().is_none() {
            return Ok(None);
        }
        let unavailable = || Box::new(error::unavailable());
        let bindings = self.bindings.as_ref().ok_or_else(unavailable)?;
        let principal = bindings
            .authenticate(&auth.context.credential.credential_id)
            .await
            .map_err(|_| unavailable())?
            .ok_or_else(unavailable)?;
        let proof = bindings
            .resolve(&principal, submission.agent_id.as_str())
            .await
            .map_err(|_| unavailable())?;
        let credential = auth.credential.as_ref().ok_or_else(unavailable)?;
        let retained = self
            .commands
            .retained_binding_message(
                &auth.context,
                credential.value(),
                &submission.agent_id,
                &submission.idempotency_key,
            )
            .await
            .map_err(|e| Box::new(error::binding_admission(e)))?;
        let (stored, response_id) = retained.map_or((None, None), |(command, receipt)| {
            (Some(command), receipt.response_id)
        });
        let sender_error = |error| {
            let mut response = routing::error(error);
            if let Some(id) = &response_id {
                // This key already admitted work. A verification failure must
                // preserve recovery, never assert an unstarted provider request.
                response.headers_mut().insert(
                    "x-cma-response-id",
                    id.as_str().parse().expect("generated response ID"),
                );
            }
            Box::new(response)
        };
        // Compare the complete original command, never strip a prefix from user
        // text. This also preserves historical off-policy admissions verbatim.
        if stored
            .as_ref()
            .is_some_and(|stored| submission.command.canonical().as_ref() == Ok(stored))
        {
            return Ok(None);
        }
        let conflict = || {
            Box::new(error::binding_admission(
                CommandLedgerError::IdempotencyConflict.into(),
            ))
        };
        if let Some(stored) = &stored {
            if !has_prefix(stored) {
                return Err(conflict());
            }
        } else if !proof.binding.profile.verified_sender_context_enabled() {
            return Ok(Some(proof.binding.revision));
        }
        let router = self
            .event_router
            .as_ref()
            .ok_or_else(|| sender_error(BindingRouteError::SenderUnavailable))?;
        let event = submission.command.external_event().expect("event checked");
        let prefix = router
            .verified_sender_line(auth, event, generation)
            .await
            .map_err(sender_error)?;
        let AgentCommand::Message { input, .. } = &mut submission.command else {
            return Err(conflict());
        };
        input.content.insert(0, AgentContent::Text { text: prefix });
        submission.command = submission
            .command
            .canonical()
            .map_err(|_| sender_error(BindingRouteError::SenderUnsupported))?;
        if let Some(stored) = stored {
            if submission.command != stored {
                return Err(conflict());
            }
            return Ok(None);
        }
        Ok(Some(proof.binding.revision))
    }
}

fn has_prefix(command: &AgentCommand) -> bool {
    matches!(command, AgentCommand::Message { input, .. }
        if matches!(input.content.first(), Some(AgentContent::Text { text }) if text.starts_with("cma_verified_sender: ")))
}
