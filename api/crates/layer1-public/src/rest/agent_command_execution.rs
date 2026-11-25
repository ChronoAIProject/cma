//! Preserve the durable HTTP receipt around a newly acquired Agent command.

use super::{require_use, AgentRoutesState};
use crate::rest::{
    idempotency,
    idempotency::{IdempotencyKey, RequestBody},
    instance_problem::InstanceProblem,
    BodyProblem,
};
use axum::response::{IntoResponse, Response};
use layer2_core::Caller;
use std::{future::Future, sync::Arc};

pub(in crate::rest) async fn execute_command<T, F, Fut>(
    state: &AgentRoutesState,
    caller: &Caller,
    key: IdempotencyKey,
    operation: String,
    body: Result<RequestBody<T>, BodyProblem>,
    execute: F,
) -> Response
where
    T: serde::de::DeserializeOwned,
    F: FnOnce(T) -> Fut,
    Fut: Future<Output = Result<Response, InstanceProblem>>,
{
    execute_command_with_claim(state, caller, key, operation, body, |body, _| execute(body)).await
}

pub(in crate::rest) async fn execute_command_with_claim<T, F, Fut>(
    state: &AgentRoutesState,
    caller: &Caller,
    key: IdempotencyKey,
    operation: String,
    body: Result<RequestBody<T>, BodyProblem>,
    execute: F,
) -> Response
where
    T: serde::de::DeserializeOwned,
    F: FnOnce(T, &idempotency::Idempotency) -> Fut,
    Fut: Future<Output = Result<Response, InstanceProblem>>,
{
    if let Err(problem) = require_use(caller) {
        return problem.into_response();
    }
    let (guard, body, _) = match idempotency::required_command(
        key,
        Arc::clone(&state.operation_records),
        caller,
        operation,
        body,
    )
    .await
    {
        Ok(parts) => parts,
        Err(response) => return response,
    };
    let result: Result<Response, Response> = match body {
        Ok(body) => execute(body, &guard)
            .await
            .map_err(IntoResponse::into_response),
        Err(error) => Err(error.into_response()),
    };
    guard.finish(result).await
}

pub(in crate::rest) async fn execute_empty_command<F, Fut>(
    state: &AgentRoutesState,
    caller: &Caller,
    key: IdempotencyKey,
    operation: String,
    execute: F,
) -> Response
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<Response, InstanceProblem>>,
{
    execute_empty_command_with_claim(state, caller, key, operation, |_| execute()).await
}

pub(super) async fn execute_empty_command_with_claim<F, Fut>(
    state: &AgentRoutesState,
    caller: &Caller,
    key: IdempotencyKey,
    operation: String,
    execute: F,
) -> Response
where
    F: FnOnce(&idempotency::Idempotency) -> Fut,
    Fut: Future<Output = Result<Response, InstanceProblem>>,
{
    if let Err(problem) = require_use(caller) {
        return problem.into_response();
    }
    let guard = match idempotency::required_empty_command(
        key,
        Arc::clone(&state.operation_records),
        caller,
        operation,
    )
    .await
    {
        Ok(guard) => guard,
        Err(response) => return response,
    };
    let result = execute(&guard).await.map_err(IntoResponse::into_response);
    guard.finish(result).await
}
