use std::{sync::Arc, time::Instant};

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use layer2_core::resource_access::{
    organization_diagnostics, AccessError, MembershipError, OrganizationRole, ResourceAccessError,
    ResourceAccessService,
};
use serde::Serialize;
use utoipa::ToSchema;

use super::resource_access_problem::ResourceProblem;
use crate::{AuthenticatedResourceCaller, DelegatedCredential};

pub(super) fn routes() -> Router<Arc<ResourceAccessService>> {
    Router::new().route("/api/v1/organizations", get(list))
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct OrganizationChoicesView {
    items: Vec<OrganizationChoiceView>,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct OrganizationChoiceView {
    id: String,
    #[schema(value_type = String)]
    role: OrganizationRole,
    #[serde(skip_serializing_if = "Option::is_none")]
    display_name: Option<String>,
}

#[utoipa::path(get, path = "/api/v1/organizations", tag = "resource-access",
    responses((status = 200, body = OrganizationChoicesView), (status = 401, body = super::ProblemDocument), (status = 403, body = super::ProblemDocument), (status = 503, body = super::ProblemDocument)))]
pub(crate) async fn list(
    State(service): State<Arc<ResourceAccessService>>,
    caller: AuthenticatedResourceCaller,
) -> Result<Json<OrganizationChoicesView>, Response> {
    let started = Instant::now();
    let (result, failure) = organization_diagnostics::capture(service.organization_choices(
        &caller.context,
        caller.credential.as_ref().map(DelegatedCredential::value),
    ))
    .await;
    let choices = result
        .inspect_err(|error| {
            let reason = match error {
                ResourceAccessError::Membership(MembershipError::Unavailable)
                    if caller.credential.is_none() =>
                {
                    "credential_missing"
                }
                ResourceAccessError::Membership(MembershipError::Unavailable) => "unavailable",
                ResourceAccessError::Membership(MembershipError::InvalidEvidence) => {
                    "invalid_evidence"
                }
                ResourceAccessError::Access(AccessError::RevalidationRequired) => "revalidation",
                _ => return,
            };
            let context = layer2_core::diagnostic_context::DiagnosticContext::current();
            // Keep the final request outcome joinable even when a provider call
            // never starts. No caller, credential, or org payload enters the event.
            let span = tracing::warn_span!(parent: None, "organization_choices");
            let _entered = span.enter();
            tracing::warn!(
                parent: &span,
                event = "organization_choices_failure",
                request_hash = context.request_hash.as_ref(),
                reason,
                dependency_stage = failure.map(|value| value.stage.label()),
                dependency_reason = failure.map(|value| value.reason.label()),
                upstream_status = failure.and_then(|value| value.status),
                elapsed_ms = started.elapsed().as_millis() as u64,
                "Organization choices unavailable"
            );
        })
        .map_err(|error| {
            if matches!(
                error,
                ResourceAccessError::Membership(
                    MembershipError::Unavailable | MembershipError::InvalidEvidence
                )
            ) {
                if let Some(failure) = failure {
                    let status = StatusCode::SERVICE_UNAVAILABLE;
                    let code = "organization_authority_unavailable";
                    return super::problem_response(
                        status,
                        super::problem_document(status, code, &failure.detail(), code),
                    );
                }
            }
            ResourceProblem(error).into_response()
        })?;
    Ok(Json(OrganizationChoicesView {
        items: choices
            .into_iter()
            .map(|choice| OrganizationChoiceView {
                id: choice.id.to_string(),
                role: choice.role,
                display_name: choice.display_name,
            })
            .collect(),
    }))
}
