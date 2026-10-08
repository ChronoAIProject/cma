//! Bounded management mutation labels, shared by REST and record callers.
use super::state::ManagementError;
use std::future::Future;

pub(super) enum Operation {
    Add,
    Update,
    Remove,
    RoutingPut,
    RoutingDelete,
    SourceAttach,
    SourceDetach,
}

pub(super) async fn observe<T>(
    operation: Operation,
    mutation: impl Future<Output = Result<T, ManagementError>>,
) -> Result<T, ManagementError> {
    let result = mutation.await;
    let metric = match operation {
        Operation::SourceAttach | Operation::SourceDetach => {
            crate::telemetry::names::SOURCE_MUTATIONS
        }
        _ => crate::telemetry::names::CONSUMER_MUTATIONS,
    };
    let op = match operation {
        Operation::Add => "add",
        Operation::Update => "update",
        Operation::Remove => "remove",
        Operation::RoutingPut => "routing_put",
        Operation::RoutingDelete => "routing_delete",
        Operation::SourceAttach => "attach",
        Operation::SourceDetach => "detach",
    };
    let outcome = match &result {
        Ok(_) => "ok",
        Err(ManagementError::Invalid(_) | ManagementError::InvalidReason(_)) => "invalid",
        Err(ManagementError::Conflict { .. } | ManagementError::ConflictReason { .. })
        | Err(ManagementError::Record(crate::records::RecordError::Conflict { .. })) => "conflict",
        Err(ManagementError::NotFound | ManagementError::SourceNotFound) => "not_found",
        Err(ManagementError::Provider(_)) => "provider_error",
        Err(ManagementError::Record(_)) => "record_error",
        Err(ManagementError::Unauthorized) => "unauthorized",
        Err(ManagementError::Internal(_)) => "internal",
    };
    metrics::counter!(metric,
        crate::telemetry::names::OP => op, crate::telemetry::names::OUTCOME => outcome)
    .increment(1);
    result
}
