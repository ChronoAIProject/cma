use std::sync::Arc;

use axum::{extract::State, routing::get, Json, Router};
use layer2_core::resource_access::{OrganizationRole, ResourceAccessService};
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
) -> Result<Json<OrganizationChoicesView>, ResourceProblem> {
    let choices = service
        .organization_choices(
            &caller.context,
            caller.credential.as_ref().map(DelegatedCredential::value),
        )
        .await?;
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
