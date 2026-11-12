//! Durable admission and bounded observation shared by legacy stream adapters.
use std::time::Duration;

use axum::{
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use layer2_core::{ids::AgentCommandId, sandbox::agent_io::*, AgentId};

use crate::{
    rest::instance_problem::InstanceProblem, AgentConversationState, AuthenticatedResourceCaller,
};

mod errors;
pub(crate) use errors::{admission_failure, receipt_headers};

pub(crate) struct BoundCommand {
    pub receipt: CommandReceiptView,
    pub turn: u64,
    pub operation_sequence: u64,
    pub resource: AuthenticatedResourceCaller,
}

pub(crate) fn submission(
    agent_id: AgentId,
    command: AgentCommand,
    headers: &HeaderMap,
) -> Result<CommandSubmission, Box<Response>> {
    if headers.get_all("idempotency-key").iter().count() > 1 {
        return Err(Box::new(errors::invalid_key()));
    }
    let key = match headers.get("idempotency-key") {
        Some(value) => value
            .to_str()
            .map_err(|_| Box::new(errors::invalid_key()))?
            .to_owned(),
        None => AgentCommandId::generate().to_string(),
    };
    command_key_digest(&key).map_err(|_| Box::new(errors::invalid_key()))?;
    Ok(CommandSubmission {
        agent_id,
        command,
        idempotency_key: key,
        expected_agent_revision: None,
        requested_model: None,
    })
}

pub(crate) async fn admit_and_wait(
    state: &AgentConversationState,
    resource: Option<AuthenticatedResourceCaller>,
    submission: CommandSubmission,
) -> Result<BoundCommand, Box<Response>> {
    let resource =
        resource.ok_or_else(|| Box::new(crate::auth::NotAuthenticated.into_response()))?;
    let credential = resource.credential.as_ref().ok_or_else(|| {
        Box::new(admission_failure(CommandAdmissionError::Authority(
            CommandAuthorizationError::Revoked,
        )))
    })?;
    let service = state
        .commands
        .clone()
        .ok_or_else(|| Box::new(InstanceProblem::Unavailable.into_response()))?;
    let source = match &resource.context.actor {
        AgentActor::Human { .. } => CommandSubmissionSource::UserInterface,
        AgentActor::Api {
            delegation_id: None,
            ..
        } => CommandSubmissionSource::Api,
        _ => return Err(Box::new(InstanceProblem::Forbidden.into_response())),
    };
    let command = submission.command.clone();
    let task_service = service.clone();
    let task_context = resource.context.clone();
    let task_credential = credential.clone();
    // Reserve to completion even when the HTTP future is dropped. The worker
    // owns readiness and execution; no stream observer runs in this task.
    let admitted = tokio::spawn(
        layer2_core::diagnostic_context::DiagnosticContext::current().scope(async move {
            task_service
                .admit(&task_context, task_credential.value(), source, submission)
                .await
        }),
    )
    .await
    .map_err(|error| {
        tracing::error!(%error, "legacy protocol command admission task failed");
        Box::new(InstanceProblem::Unavailable.into_response())
    })?
    .map_err(|error| Box::new(admission_failure(error)))?;
    let receipt = admitted.receipt;
    let observe = async {
        loop {
            let view = service
                .command_projection(
                    &resource.context,
                    Some(credential.value()),
                    &receipt.command_id,
                )
                .await
                .map_err(admission_failure)?;
            if view.status.receipt != receipt {
                return Err(InstanceProblem::Unavailable.into_response());
            }
            if let (Some(turn), Some(operation)) =
                (view.status.turn_sequence, view.operation_sequence)
            {
                return Ok((turn, operation));
            }
            if view.status.status.is_terminal() {
                return Err(errors::unbound_failure(&command, &view.status));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    };
    let (turn, operation_sequence) =
        match tokio::time::timeout(Duration::from_secs(600), observe).await {
            Ok(Ok(binding)) => binding,
            Ok(Err(response)) => return Err(Box::new(receipt_headers(response, &receipt))),
            Err(_) => {
                return Err(Box::new(receipt_headers(
                    errors::observation_timeout(),
                    &receipt,
                )))
            }
        };
    Ok(BoundCommand {
        receipt,
        turn,
        operation_sequence,
        resource,
    })
}
