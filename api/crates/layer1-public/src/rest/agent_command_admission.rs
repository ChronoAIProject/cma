//! V1 compatibility around the common command ledger. The original operation
//! receipt wins; an ambiguous pre-migration intent never authorizes new work.
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use layer2_core::sandbox::agent_io::{
    AgentActor, AgentCommand, CommandAdmissionError, CommandAuthorizationError, CommandLedgerError,
    CommandSubmission, CommandSubmissionSource,
};
use layer2_core::AgentId;
use sha2::{Digest, Sha256};

use super::{
    agent_openapi_shapes::InputAcceptedView,
    agents::AgentRoutesState,
    idempotency::{Idempotency, IdempotencyKey, IdempotencyProblem, RequestBody},
    instance_problem::InstanceProblem,
    resource_access_problem::ResourceProblem,
    BodyProblem,
};
use crate::AuthenticatedResourceCaller;

#[cfg(test)]
#[path = "agent_command_admission_tests.rs"]
mod tests;

pub(super) async fn with_body<T>(
    state: &AgentRoutesState,
    resource: Option<AuthenticatedResourceCaller>,
    key: IdempotencyKey,
    operation: String,
    agent_id: AgentId,
    body: Result<RequestBody<T>, BodyProblem>,
    command: impl FnOnce(T) -> AgentCommand,
) -> Response {
    let (body, fingerprint) = match body {
        Ok(body) => body.into_parts(),
        Err(error) => return error.into_response(),
    };
    execute(
        state,
        resource,
        key,
        operation,
        agent_id,
        body.map(command),
        fingerprint,
    )
    .await
}

pub(super) async fn without_body(
    state: &AgentRoutesState,
    resource: Option<AuthenticatedResourceCaller>,
    key: IdempotencyKey,
    operation: String,
    agent_id: AgentId,
    command: AgentCommand,
) -> Response {
    execute(
        state,
        resource,
        key,
        operation,
        agent_id,
        Ok(command),
        Sha256::digest([]).into(),
    )
    .await
}

async fn execute(
    state: &AgentRoutesState,
    resource: Option<AuthenticatedResourceCaller>,
    key: IdempotencyKey,
    operation: String,
    agent_id: AgentId,
    command: Result<AgentCommand, BodyProblem>,
    fingerprint: [u8; 32],
) -> Response {
    let Some(resource) = resource else {
        return crate::auth::NotAuthenticated.into_response();
    };
    let Some(credential) = resource.credential.as_ref() else {
        return admission_problem(CommandAdmissionError::Authority(
            CommandAuthorizationError::Revoked,
        ));
    };
    let key = match key.required() {
        Ok(key) => key,
        Err(error) => return error.into_response(),
    };
    let Some(service) = &state.commands else {
        return InstanceProblem::Unavailable.into_response();
    };
    // The adapter owns this namespace and source; neither is copied from JSON.
    let submission = command.as_ref().ok().map(|command| CommandSubmission {
        agent_id,
        command: command.clone(),
        idempotency_key: legacy_key(&operation, &key),
        expected_agent_revision: None,
        requested_model: None,
    });
    let source = match resource.context.actor {
        AgentActor::Human { .. } => CommandSubmissionSource::UserInterface,
        AgentActor::Api {
            delegation_id: None,
            ..
        } => CommandSubmissionSource::Api,
        _ => return InstanceProblem::Forbidden.into_response(),
    };
    if let Some(submission) = &submission {
        if let Err(error) = service
            .authorize_intent(
                &resource.context,
                credential.value(),
                &submission.agent_id,
                &submission.command,
            )
            .await
        {
            return admission_problem(error);
        }
    }
    let guard = match Idempotency::begin_recoverable(
        key,
        state.operation_records.clone(),
        &resource.context.caller,
        operation,
        fingerprint,
    )
    .await
    {
        Ok(Idempotency::Replay(response)) => return response,
        Ok(guard) => guard,
        Err(response) => return response,
    };
    let command = match command {
        Ok(command) => command,
        Err(error) => {
            return match guard {
                Idempotency::Fenced(problem) => problem.into_response(),
                guard => guard.finish(Err::<Response, _>(error)).await,
            }
        }
    };
    let submission = submission.expect("valid command");
    let admitted = match service
        .lookup_admitted(
            &resource.context,
            credential.value(),
            source,
            submission.clone(),
        )
        .await
    {
        Ok(value) => value,
        Err(error) => {
            // A failed lookup cannot rule out an older accepted command. Keep
            // this outer intent unfinished so fenced retries can recover it.
            return admission_problem(error);
        }
    };
    if let Idempotency::Fenced(problem) = guard {
        return if admitted.is_some() {
            accepted(&command)
        } else {
            problem.into_response()
        };
    }
    if admitted.is_some() {
        return guard.finish(Ok::<_, Response>(accepted(&command))).await;
    }
    match service
        .admit(&resource.context, credential.value(), source, submission)
        .await
    {
        Ok(_) => guard.finish(Ok::<_, Response>(accepted(&command))).await,
        // An authority fence can fail after durable reservation. Keep the outer
        // intent recoverable; never seal that uncertainty as a failed receipt.
        Err(error) => admission_problem(error),
    }
}

fn legacy_key(operation: &str, key: &IdempotencyKey) -> String {
    format!(
        "cma-rest-agent-v1:{}",
        key.confirmation_hash(operation).expect("required key")
    )
}

fn accepted(command: &AgentCommand) -> Response {
    if matches!(command, AgentCommand::Message { .. }) {
        // This is the immutable admission outcome even if a worker finished
        // before the caller recovered its receipt.
        (
            StatusCode::ACCEPTED,
            Json(InputAcceptedView { queued: true }),
        )
            .into_response()
    } else {
        StatusCode::ACCEPTED.into_response()
    }
}

fn admission_problem(error: CommandAdmissionError) -> Response {
    match error {
        CommandAdmissionError::Ledger(CommandLedgerError::SessionCapacityUnavailable) => {
            InstanceProblem::SessionCapacityUnavailable.into_response()
        }
        CommandAdmissionError::Ledger(CommandLedgerError::AdmissionPaused) => {
            InstanceProblem::AdmissionPaused.into_response()
        }
        CommandAdmissionError::Ledger(CommandLedgerError::Budget(reason)) => {
            InstanceProblem::Budget(reason).into_response()
        }
        CommandAdmissionError::Access(error) => ResourceProblem(error).into_response(),
        CommandAdmissionError::Input(error) => InstanceProblem::from(error).into_response(),
        CommandAdmissionError::Authority(CommandAuthorizationError::Revoked) => {
            InstanceProblem::Forbidden.into_response()
        }
        CommandAdmissionError::Ledger(CommandLedgerError::IdempotencyConflict) => {
            IdempotencyProblem::Reused.into_response()
        }
        CommandAdmissionError::Ledger(CommandLedgerError::NotFound) => {
            InstanceProblem::AgentNotFound.into_response()
        }
        CommandAdmissionError::Ledger(CommandLedgerError::NotAdmissible) => {
            InstanceProblem::AgentNotReady.into_response()
        }
        CommandAdmissionError::Ledger(CommandLedgerError::QueueFull) => {
            InstanceProblem::MessageQueueFull { limit: 64 }.into_response()
        }
        CommandAdmissionError::Ledger(CommandLedgerError::Invalid)
        | CommandAdmissionError::ModelMismatch => InstanceProblem::InvalidRequest.into_response(),
        CommandAdmissionError::Ledger(
            CommandLedgerError::Conflict
            | CommandLedgerError::KnownAborted
            | CommandLedgerError::StaleClaim,
        ) => InstanceProblem::OperationInFlight.into_response(),
        CommandAdmissionError::Ledger(
            CommandLedgerError::Corrupt
            | CommandLedgerError::Unavailable
            | CommandLedgerError::ControlBusy,
        )
        | CommandAdmissionError::Authority(CommandAuthorizationError::Unavailable) => {
            InstanceProblem::Unavailable.into_response()
        }
    }
}

#[cfg(test)]
mod session_capacity_tests {
    use super::*;

    #[tokio::test]
    async fn session_capacity_refusals_keep_the_rest_problem_contract() {
        for response in [
            admission_problem(CommandAdmissionError::Ledger(
                CommandLedgerError::SessionCapacityUnavailable,
            )),
            admission_problem(CommandAdmissionError::Input(
                layer2_core::sandbox::AgentOperationError::SessionCapacityUnavailable,
            )),
        ] {
            assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(response.headers()["retry-after"], "60");
            let bytes = axum::body::to_bytes(response.into_body(), 4096)
                .await
                .unwrap();
            let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(body["code"], "session_capacity_unavailable");
            assert_eq!(
                body["detail"],
                layer2_core::admission::CapacityKind::ActiveSessions.guidance()
            );
            assert_eq!(body.as_object().unwrap().len(), 5);
        }
    }
}
