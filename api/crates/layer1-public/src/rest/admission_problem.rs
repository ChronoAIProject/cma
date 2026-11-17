//! One safe public refusal for fresh work while the operator stop is active.

use axum::{
    http::{header, HeaderValue, StatusCode},
    response::Response,
};

pub(crate) fn paused() -> Response {
    let status = StatusCode::SERVICE_UNAVAILABLE;
    let mut response = super::problem_response(
        status,
        super::problem_document(
            status,
            "admission-paused",
            layer2_core::admission::ADMISSION_PAUSED_GUIDANCE,
            "admission_paused",
        ),
    );
    response
        .headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from_static("60"));
    response
}

pub(crate) fn session_capacity() -> Response {
    let status = StatusCode::SERVICE_UNAVAILABLE;
    let mut response = super::problem_response(
        status,
        super::problem_document(
            status,
            "session-capacity-unavailable",
            layer2_core::admission::CapacityKind::ActiveSessions.guidance(),
            "session_capacity_unavailable",
        ),
    );
    response
        .headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from_static("60"));
    response
}

pub(crate) fn home_capacity() -> Response {
    let status = StatusCode::SERVICE_UNAVAILABLE;
    let mut response = super::problem_response(
        status,
        super::problem_document(
            status,
            "sandbox-capacity-unavailable",
            layer2_core::admission::CapacityKind::RetainedHomes.guidance(),
            "sandbox_capacity_unavailable",
        ),
    );
    response
        .headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from_static("60"));
    response
}

pub(crate) fn writes_unavailable() -> Response {
    let status = StatusCode::SERVICE_UNAVAILABLE;
    super::problem_response(
        status,
        super::problem_document(
            status,
            "admission-control-writes-unavailable",
            "Saving admission control is not available yet.",
            "admission_control_writes_unavailable",
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rest::{
        instance_problem::InstanceProblem, instances_problem::HierarchyProblem,
        trigger_problem::TriggerProblem,
    };
    use axum::response::IntoResponse;
    use layer2_core::sandbox::{
        AgentOperationError, AgentServiceError, SandboxLifecycleCommandError, SandboxServiceError,
        WorkspaceServiceError,
    };

    #[tokio::test]
    async fn all_resource_projections_share_a_safe_retryable_pause_problem() {
        let responses = [
            InstanceProblem::from_create(AgentServiceError::AdmissionPaused).into_response(),
            InstanceProblem::from(AgentOperationError::AdmissionPaused).into_response(),
            HierarchyProblem::sandbox(SandboxServiceError::AdmissionPaused).into_response(),
            HierarchyProblem::workspace(WorkspaceServiceError::AdmissionPaused).into_response(),
            HierarchyProblem::sandbox_lifecycle(SandboxLifecycleCommandError::AdmissionPaused)
                .into_response(),
            TriggerProblem::from(layer2_core::trigger_domain::LaunchServiceError::AdmissionPaused)
                .into_response(),
            crate::rest::AgentLifecycleProvisioningError::AdmissionPaused.into_response(),
            crate::rest::Problem(layer2_core::CoreError::AdmissionPaused).into_response(),
        ];
        for response in responses {
            assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(response.headers()[header::RETRY_AFTER], "60");
            let bytes = axum::body::to_bytes(response.into_body(), 4096)
                .await
                .unwrap();
            let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(body["code"], "admission_paused");
            assert_eq!(
                body["detail"],
                layer2_core::admission::ADMISSION_PAUSED_GUIDANCE
            );
            assert_eq!(body.as_object().unwrap().len(), 5);
        }
    }
}
