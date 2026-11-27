use std::sync::Arc;

use axum::{
    extract::{Extension, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use layer2_core::Capability;

use super::{require_use, AgentRoutesState};
use crate::rest::agent_lifecycle::{
    AgentLifecycleCreateReceipt, AgentLifecycleCreateRequest, AgentLifecycleProvisioningError,
};
use crate::rest::idempotency::{IdempotencyKey, RequestBody};
use crate::rest::BodyProblem;
use crate::{AuthenticatedCaller, DelegatedCredential};

/// Create an Agent from one inline lifecycle request.  The provisioning port
/// owns the sandbox/workspace/agent orchestration; this boundary never accepts
/// profile, worktree, pod, or runtime identifiers. An optional owned workspace
/// selects an existing context; the service verifies its creation authority.
#[utoipa::path(
    post,
    path = "/api/v1/agents",
    tag = "agents",
    params(("Idempotency-Key" = String, Header)),
    request_body = AgentLifecycleCreateRequest,
    responses(
        (status = 202, body = AgentLifecycleCreateReceipt),
        (status = 422, description = "Invalid template inputs: task_template_field_required, task_template_field_placeholder, task_template_field_unknown, task_template_id_unknown, task_template_version_unsupported, task_template_message_mismatch or task_template_message_too_large; other invalid lifecycle requests use their existing problem codes.", body = crate::rest::ProblemDocument),
        (status = 429, description = "sandbox_limit_reached", body = crate::rest::ProblemDocument),
        (status = 503, body = crate::rest::ProblemDocument)
    )
)]
pub(crate) async fn create_lifecycle(
    State(state): State<AgentRoutesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    context: Option<Extension<layer2_core::resource_access::CallerContext>>,
    credential: Option<Extension<DelegatedCredential>>,
    creation: Option<Extension<Arc<dyn crate::rest::creation_outcomes::Backend>>>,
    key: IdempotencyKey,
    body: Result<RequestBody<AgentLifecycleCreateRequest>, BodyProblem>,
) -> Response {
    if let Err(problem) = require_use(&caller) {
        return problem.into_response();
    }
    let Some(provisioner) = state.lifecycle.clone() else {
        return AgentLifecycleProvisioningError::Unavailable.into_response();
    };
    // The lifecycle's permanent owner/key ledger owns admission and resumption.
    // An outer cached 202 would suppress requeue after connecting services.
    let key = match key.into_required_value() {
        Ok(key) => key,
        Err(error) => return error.into_response(),
    };
    let (request, wire_digest) = match body.and_then(|body| {
        let digest = body.creation_digest().map(str::to_owned);
        body.into_parts().0.map(|request| (request, digest))
    }) {
        Ok(value) => value,
        Err(error) => return error.into_response(),
    };
    if let Err(error) = request.validate_template() {
        return error.into_response();
    }
    if request.desktop && request.initial_message.is_none() {
        return AgentLifecycleProvisioningError::Invalid(
            "desktop creation requires initial_message".into(),
        )
        .into_response();
    }
    use crate::rest::creation_outcomes::{
        is_covered_key,
        routes::{begin_submission, problem},
        Operation, RefusalCode,
    };
    let backend = creation.as_ref().map(|Extension(backend)| backend.as_ref());
    let new_sandbox = request.sandbox_id.is_none() && request.workspace_id.is_none();
    let covered = is_covered_key(&key);
    if covered && !new_sandbox {
        return problem(StatusCode::BAD_REQUEST);
    }
    if covered && !caller.authorization.allows(Capability::CreateSandbox) {
        return problem(StatusCode::FORBIDDEN);
    }
    let submission = if new_sandbox {
        match begin_submission(
            backend,
            &caller,
            Operation::AgentBootstrap,
            Some(&key),
            wire_digest.as_deref(),
        )
        .await
        {
            Ok(submission) => submission,
            Err(response) => return response,
        }
    } else {
        None
    };
    let context = context.as_ref().map(|Extension(value)| value);
    let credential = credential.as_ref().map(|Extension(value)| value.value());
    let fence = submission
        .as_ref()
        .and_then(|submission| submission.fence.clone());
    let result = match submission {
        Some(submission) => {
            provisioner
                .create_with_submission(&caller, context, credential, request, &key, submission)
                .await
        }
        None => {
            provisioner
                .create(&caller, context, credential, request, &key)
                .await
        }
    };
    if let (Err(error), Some(backend), Some(fence)) = (&result, backend, fence) {
        let refusal = match error {
            AgentLifecycleProvisioningError::SandboxLimitReached { .. } => {
                Some(RefusalCode::SandboxLimitReached)
            }
            // These failures end admission. The proof-store CAS still requires
            // an unlinked submission, so an operation that won admission keeps authority.
            AgentLifecycleProvisioningError::Invalid(_)
            | AgentLifecycleProvisioningError::DesktopUnsupported(_)
            | AgentLifecycleProvisioningError::ImageAccessDenied
            | AgentLifecycleProvisioningError::ImageSelectionUnsupported
                if backend.terminal_settlement_enabled() =>
            {
                Some(RefusalCode::CreationFailed)
            }
            _ => None,
        };
        if let Some(code) = refusal {
            if !matches!(
                backend.refuse_direct(&caller.subject, &fence, code).await,
                Ok(true)
            ) {
                return problem(StatusCode::SERVICE_UNAVAILABLE);
            }
        }
    }
    result
        .map(|receipt| (StatusCode::ACCEPTED, Json(receipt)).into_response())
        .unwrap_or_else(IntoResponse::into_response)
}
