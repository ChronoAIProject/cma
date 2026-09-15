use axum::{
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use layer2_core::bridge::cleanup::CleanupError;
pub(super) struct CleanupProblem(pub CleanupError);
impl IntoResponse for CleanupProblem {
    fn into_response(self) -> Response {
        use CleanupError::*;
        let (status, code, detail) = match self.0 {
            Bridge(error) => return super::super::bridges::problem::BridgeProblem(error).into_response(),
            Invalid => (StatusCode::BAD_REQUEST, "invalid_bridge_child_cleanup", "Provide a valid allocation, cursor, current Bridge and allocation revisions, and the explicit remove_child action."),
            NotFound => (StatusCode::NOT_FOUND, "bridge_child_cleanup_not_found", "Child cleanup was not found in this Bridge and parent."),
            Conflict => (StatusCode::CONFLICT, "bridge_child_cleanup_conflict", "Cleanup evidence or current revisions changed. Retain the original key and body to recover uncertain acceptance."),
            OriginalActorRequired => (StatusCode::CONFLICT, "bridge_child_cleanup_original_actor_required", "This cleanup requires the current authority of the actor who configured the reused child."),
            Unavailable => (StatusCode::SERVICE_UNAVAILABLE, "bridge_child_cleanup_unavailable", "Current cleanup state or authority is unavailable. Recover uncertain acceptance with the identical key and body."),
            Corrupt => (StatusCode::SERVICE_UNAVAILABLE, "bridge_child_cleanup_evidence_inconsistent", "Retained cleanup evidence is inconsistent. Cleanup cannot advance until the original evidence is recovered."),
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

pub(super) struct WorkspaceProblem(pub CleanupError);
impl IntoResponse for WorkspaceProblem {
    fn into_response(self) -> Response {
        use CleanupError::*;
        let (status, code, detail) = match self.0 {
            Bridge(error) => return super::super::bridges::problem::BridgeProblem(error).into_response(),
            Invalid => (StatusCode::BAD_REQUEST, "invalid_bridge_workspace_cleanup", "Provide the exact allocation, current Bridge, allocation and Workspace revisions, original child cleanup receipt, and explicit remove_owned_workspace action."),
            NotFound => (StatusCode::NOT_FOUND, "bridge_workspace_cleanup_not_found", "Workspace cleanup was not found in this Bridge and parent."),
            Conflict => (StatusCode::CONFLICT, "bridge_workspace_cleanup_conflict", "Cleanup evidence or current revisions changed. Retain the original key and body to recover uncertain acceptance."),
            OriginalActorRequired => (StatusCode::CONFLICT, "bridge_workspace_cleanup_original_actor_required", "This cleanup requires the current authority of the actor who configured the allocation-owned Workspace."),
            Unavailable => (StatusCode::SERVICE_UNAVAILABLE, "bridge_workspace_cleanup_unavailable", "Current cleanup state or authority is unavailable. Recover uncertain acceptance with the identical key and body."),
            Corrupt => (StatusCode::SERVICE_UNAVAILABLE, "bridge_workspace_cleanup_evidence_inconsistent", "Retained cleanup evidence is inconsistent. Cleanup cannot advance until the original evidence is recovered."),
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
