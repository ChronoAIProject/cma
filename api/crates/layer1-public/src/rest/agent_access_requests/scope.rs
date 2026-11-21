//! One explicit owner Save replaces the shared Sandbox's desired service scope.
use super::*;

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReplaceAgentServiceAccess {
    #[schema(min_items = 1, max_items = 32)]
    pub services: Vec<String>,
    #[schema(min_items = 1, max_items = 32)]
    pub expected_services: Vec<String>,
}

/// Replace the exact shared Sandbox service scope as its direct owner.
/// A verified owner access token is accepted without browser sign-in. Runtime,
/// relay, service-account and delegated credentials cannot grant service access.
/// Independent clients must already have the selected service and backing-node
/// authority. Refusals identify `agent_access_owner_authority_required` (403) or
/// `agent_access_service_connection_required` (409). Token custody is finite and
/// revalidated before deferred effects; use the unchanged body and key on retry.
#[utoipa::path(put, operation_id = "put_api_v1_agents_by_id_service_access", path = "/api/v1/agents/{id}/service-access", tag = "agents",
    params(("id" = String, Path), ("Idempotency-Key" = String, Header)),
    request_body = ReplaceAgentServiceAccess,
    responses((status = 202, body = AgentAccessRequestView), (status = 400, body = super::super::ProblemDocument),
    (status = 401, body = super::super::ProblemDocument), (status = 403, body = super::super::ProblemDocument),
    (status = 404, body = super::super::ProblemDocument), (status = 409, body = super::super::ProblemDocument),
    (status = 413, body = super::super::ProblemDocument), (status = 415, body = super::super::ProblemDocument),
    (status = 422, body = super::super::ProblemDocument), (status = 503, body = super::super::ProblemDocument)))]
pub(crate) async fn replace(
    State(service): State<Arc<dyn AgentAccessRequests>>,
    resource: AuthenticatedResourceCaller,
    Path(id): Path<String>,
    key: IdempotencyKey,
    body: Result<RequestBody<ReplaceAgentServiceAccess>, BodyProblem>,
) -> Response {
    let key = match key.into_required_value() {
        Ok(key) => key,
        Err(error) => return error.into_response(),
    };
    let body = match body.and_then(|body| body.into_parts().0) {
        Ok(body) => body,
        Err(error) => return error.into_response(),
    };
    for services in [&body.services, &body.expected_services] {
        if let Err(error) = validate_access_services(services) {
            return problem(error);
        }
        if services
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != services.len()
        {
            return problem(AgentAccessError::Invalid);
        }
    }
    let Some(credential) = resource
        .credential
        .filter(|value| !value.value().is_empty())
    else {
        return problem(AgentAccessError::Forbidden);
    };
    match service
        .replace_services(
            &resource.context,
            credential.value(),
            &AgentId(id),
            body.services,
            body.expected_services,
            &key,
        )
        .await
    {
        Ok(record) => (
            StatusCode::ACCEPTED,
            Json(AgentAccessRequestView::from(record)),
        )
            .into_response(),
        Err(error) => problem(error),
    }
}
