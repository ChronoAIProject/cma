//! Public authoring lookups projected only through the Core authoring service.

use std::sync::Arc;

use axum::{
    extract::{rejection::JsonRejection, rejection::QueryRejection, Query, State},
    response::{IntoResponse, Response},
    routing::{get, post},
    Extension, Json, Router,
};
use layer2_core::profiles::authoring::{
    AuthoringCredential, ProfileAuthoringError, ProfileAuthoringService, RepositoryRequest,
    SkillCatalogQuery, DEFAULT_SKILL_PAGE_SIZE,
};

use crate::{AuthenticatedCaller, DelegatedCredential};

use super::BodyProblem;

mod problem;
pub(super) mod shapes;

use problem::AuthoringProblem;
use shapes::{
    AuthoringServiceView, AuthoringSkillPage, AuthoringSkillView, ProfileCapacityBoundsView,
    RepositoryResolutionView, ResolveRepositoryRequest, SkillCatalogParameters,
};

pub fn profile_authoring_router(
    service: Arc<ProfileAuthoringService>,
    nyxid_public_origin: Option<url::Url>,
) -> Router {
    Router::new()
        .route(
            "/api/v1/profile-authoring/services",
            get(services).layer(Extension(nyxid_public_origin)),
        )
        .route("/api/v1/profile-authoring/bounds", get(bounds))
        .route("/api/v1/profile-authoring/skills", get(skills))
        .route(
            "/api/v1/profile-authoring/repositories/resolve",
            post(resolve_repository),
        )
        .with_state(service)
}

#[utoipa::path(get, path = "/api/v1/profile-authoring/bounds", tag = "profile authoring",
    responses((status = 200, body = ProfileCapacityBoundsView),
    (status = 401, description = "not_authenticated", body = super::ProblemDocument, content_type = "application/problem+json"),
    (status = 403, description = "forbidden", body = super::ProblemDocument, content_type = "application/problem+json")))]
pub(super) async fn bounds(
    State(service): State<Arc<ProfileAuthoringService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
) -> Result<Json<ProfileCapacityBoundsView>, AuthoringProblem> {
    service
        .bounds(&caller)
        .map(Into::into)
        .map(Json)
        .map_err(Into::into)
}

#[utoipa::path(get, path = "/api/v1/profile-authoring/services", tag = "profile authoring",
    responses((status = 200, body = [AuthoringServiceView]),
    (status = 401, description = "not_authenticated", body = super::ProblemDocument, content_type = "application/problem+json"),
    (status = 403, description = "Requires UseOwnSandboxes or ManageOwnProfiles for the caller’s own catalog", body = super::ProblemDocument, content_type = "application/problem+json"),
    (status = 503, description = "profile_services_unavailable", body = super::ProblemDocument, content_type = "application/problem+json")))]
pub(super) async fn services(
    State(service): State<Arc<ProfileAuthoringService>>,
    Extension(nyxid_public_origin): Extension<Option<url::Url>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    credential: Option<Extension<DelegatedCredential>>,
) -> Result<Json<Vec<AuthoringServiceView>>, AuthoringProblem> {
    service.authorize_catalog(&caller)?;
    let credential =
        credential.map(|Extension(value)| AuthoringCredential::new(value.into_value()));
    Ok(Json(
        service
            .services(&caller, credential.as_ref())
            .await?
            .into_iter()
            .map(|entry| AuthoringServiceView::new(entry, nyxid_public_origin.as_ref()))
            .collect(),
    ))
}

#[utoipa::path(get, path = "/api/v1/profile-authoring/skills", tag = "profile authoring",
    params(SkillCatalogParameters),
    responses((status = 200, body = AuthoringSkillPage),
    (status = 400, description = "invalid_profile_authoring_query", body = super::ProblemDocument, content_type = "application/problem+json"),
    (status = 401, description = "not_authenticated", body = super::ProblemDocument, content_type = "application/problem+json"),
    (status = 403, description = "Requires UseOwnSandboxes or ManageOwnProfiles for the caller’s own catalog, plus service authorization (skill_catalog_authorization_required).", body = super::ProblemDocument, content_type = "application/problem+json"),
    (status = 503, description = "profile_skills_unavailable", body = super::ProblemDocument, content_type = "application/problem+json")))]
pub(super) async fn skills(
    State(service): State<Arc<ProfileAuthoringService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    credential: Option<Extension<DelegatedCredential>>,
    query: Result<Query<SkillCatalogParameters>, QueryRejection>,
) -> Result<Json<AuthoringSkillPage>, AuthoringProblem> {
    service.authorize_catalog(&caller)?;
    let credential =
        credential.map(|Extension(value)| AuthoringCredential::new(value.into_value()));
    let Query(query) = query.map_err(|_| ProfileAuthoringError::InvalidQuery)?;
    let page = service
        .skills(
            &caller,
            SkillCatalogQuery {
                query: query.q.unwrap_or_default(),
                limit: query.limit.unwrap_or(DEFAULT_SKILL_PAGE_SIZE),
                cursor: query.cursor,
            },
            credential.as_ref(),
        )
        .await?;
    Ok(Json(AuthoringSkillPage {
        items: page
            .items
            .into_iter()
            .map(AuthoringSkillView::from)
            .collect(),
        next: page.next,
    }))
}

#[utoipa::path(post, path = "/api/v1/profile-authoring/repositories/resolve", tag = "profile authoring",
    request_body = ResolveRepositoryRequest,
    responses((status = 200, body = RepositoryResolutionView),
    (status = 400, description = "invalid_json", body = super::ProblemDocument, content_type = "application/problem+json"),
    (status = 401, description = "not_authenticated", body = super::ProblemDocument, content_type = "application/problem+json"),
    (status = 403, description = "forbidden", body = super::ProblemDocument, content_type = "application/problem+json"),
    (status = 404, description = "repository_not_found or repository_reference_not_found", body = super::ProblemDocument, content_type = "application/problem+json"),
    (status = 413, description = "invalid_json", body = super::ProblemDocument, content_type = "application/problem+json"),
    (status = 415, description = "invalid_json", body = super::ProblemDocument, content_type = "application/problem+json"),
    (status = 422, description = "invalid_json, invalid_repository or invalid_repository_reference", body = super::ProblemDocument, content_type = "application/problem+json"),
    (status = 503, description = "repository_unavailable", body = super::ProblemDocument, content_type = "application/problem+json")))]
pub(super) async fn resolve_repository(
    State(service): State<Arc<ProfileAuthoringService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    body: Result<Json<ResolveRepositoryRequest>, JsonRejection>,
) -> Response {
    if let Err(error) = service.authorize(&caller) {
        return AuthoringProblem::from(error).into_response();
    }
    let Json(body) = match body {
        Ok(body) => body,
        Err(error) => return BodyProblem::from(error).into_response(),
    };
    match service
        .resolve_repository(
            &caller,
            RepositoryRequest {
                url: body.url,
                reference: body.reference,
            },
        )
        .await
    {
        Ok(resolution) => Json(RepositoryResolutionView::from(resolution)).into_response(),
        Err(error) => AuthoringProblem::from(error).into_response(),
    }
}
