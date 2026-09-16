use axum::{
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use layer2_core::bridge::{allocation::AllocationError, partition_commands::PartitionCommandError};
pub(super) struct CommandProblem(pub PartitionCommandError);
impl IntoResponse for CommandProblem {
    fn into_response(self) -> Response {
        use PartitionCommandError::*;
        let (status, code, detail) = match self.0 {
            Bridge(error) => return super::super::bridges::problem::BridgeProblem(error).into_response(),
            Invalid => (StatusCode::BAD_REQUEST, "invalid_bridge_partition_command", "Provide an exact partition identity, current Bridge and partition revisions, and an idempotency key."),
            NotFound => (StatusCode::NOT_FOUND, "bridge_partition_not_found", "Partition not found in this Bridge."),
            Conflict | Allocation(AllocationError::Conflict | AllocationError::ResetRequired | AllocationError::LeaseUnavailable | AllocationError::InvalidTransition) =>
                (StatusCode::CONFLICT, "bridge_partition_command_conflict", "The partition or Bridge changed, an effect is unresolved, or this key identifies different input. Recover an uncertain command with its original request and key."),
            Allocation(AllocationError::Capacity) => (StatusCode::CONFLICT, "bridge_partition_capacity", "The next partition generation exceeds retained resource capacity. Existing history and resources still count."),
            SourceUnavailable => (StatusCode::CONFLICT, "bridge_partition_source_unavailable", "The original accepted input cannot currently be verified. The partition was not changed."),
            _ => (StatusCode::SERVICE_UNAVAILABLE, "bridge_partition_command_unavailable", "Partition state or live authority is unavailable. Retry the identical command and key to recover an uncertain commit."),
        };
        let mut response = super::super::problem_response(
            status,
            super::super::problem_document(status, code, detail, code),
        );
        if status == StatusCode::SERVICE_UNAVAILABLE {
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
        }
        response
    }
}
