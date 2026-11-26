use crate::{
    auth::{AuthError, NyxIdAuth, Principal},
    cloud::Executor,
    model::*,
    permissions as p,
    store::Store,
};
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use serde_json::Value;
use utoipa_axum::{router::OpenApiRouter, routes};
pub(crate) mod shapes;
use shapes::*;
use std::sync::Arc;

pub struct App {
    pub store: Store,
    pub auth: NyxIdAuth,
    pub executor: Executor,
    pub registry: ImageRegistry,
}
/// The handler annotations register both HTTP methods and OpenAPI operations.
pub(crate) fn api_router() -> OpenApiRouter<Arc<App>> {
    OpenApiRouter::new()
        .routes(routes!(permissions))
        .routes(routes!(list, create))
        .routes(routes!(read, edit, retire))
        .routes(routes!(visibility))
        .routes(routes!(share, unshare))
        .routes(routes!(builds, build))
        .routes(routes!(versions))
        .routes(routes!(version))
        .routes(routes!(resolve))
        .routes(routes!(build_read))
        .routes(routes!(logs))
}
pub fn router(state: Arc<App>) -> Router {
    api_router()
        .split_for_parts()
        .0
        .route("/healthz", get(|| async { "ok" }))
        .route("/readyz", get(ready))
        .layer(tower_http::limit::RequestBodyLimitLayer::new(32 * 1024))
        .with_state(state)
}
#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    #[error(transparent)]
    Auth(#[from] AuthError),
    #[error(transparent)]
    Domain(#[from] Error),
}
impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        let status = match &self {
            Self::Auth(AuthError::Unauthenticated) => StatusCode::UNAUTHORIZED,
            Self::Auth(AuthError::Forbidden) => StatusCode::FORBIDDEN,
            Self::Domain(Error::Invalid(_)) => StatusCode::BAD_REQUEST,
            Self::Domain(Error::NotFound) => StatusCode::NOT_FOUND,
            Self::Domain(Error::Conflict) => StatusCode::CONFLICT,
            _ => StatusCode::SERVICE_UNAVAILABLE,
        };
        (
            status,
            [("content-type", "application/problem+json")],
            Json(Problem {
                kind: "about:blank",
                title: status.canonical_reason(),
                status: status.as_u16(),
                detail: self.to_string(),
            }),
        )
            .into_response()
    }
}
type ResponseResult<T> = std::result::Result<T, HttpError>;
async fn actor(app: &App, headers: &HeaderMap, permission: &str) -> ResponseResult<Principal> {
    let principal = app.auth.authenticate(headers).await?;
    principal.require(permission)?;
    Ok(principal)
}
fn key(headers: &HeaderMap) -> ResponseResult<&str> {
    if headers.get_all("idempotency-key").iter().count() != 1 {
        return Err(Error::Invalid("one Idempotency-Key is required").into());
    }
    headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .filter(|s| bounded_text(s, 256))
        .ok_or(Error::Invalid("invalid Idempotency-Key").into())
}
fn revision(headers: &HeaderMap) -> ResponseResult<i64> {
    if headers.get_all("if-match").iter().count() != 1 {
        return Err(Error::Invalid("If-Match revision is required").into());
    }
    headers
        .get("if-match")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim_matches('"').parse().ok())
        .filter(|v| *v > 0)
        .ok_or(Error::Invalid("invalid If-Match revision").into())
}
#[derive(Default, Deserialize)]
struct Page {
    #[serde(default)]
    after: String,
    #[serde(default)]
    scope: ImageScopeParam,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ImageScopeParam {
    Private,
    Owned,
    Shared,
    Public,
    #[default]
    All,
}

impl From<ImageScopeParam> for crate::store::ImageListScope {
    fn from(value: ImageScopeParam) -> Self {
        match value {
            ImageScopeParam::Private => Self::Private,
            ImageScopeParam::Owned => Self::Owned,
            ImageScopeParam::Shared => Self::Shared,
            ImageScopeParam::Public => Self::Public,
            ImageScopeParam::All => Self::All,
        }
    }
}
fn paginated<T, V>(
    mut items: Vec<T>,
    id: impl Fn(&T) -> &str,
    view: impl Fn(T) -> V,
) -> PageView<V> {
    let more = items.len() > 100;
    items.truncate(100);
    let next_cursor = if more {
        items.last().map(|x| id(x).to_owned())
    } else {
        None
    };
    PageView {
        items: items.into_iter().map(view).collect(),
        next_cursor,
    }
}
async fn ready(State(a): State<Arc<App>>) -> ResponseResult<&'static str> {
    a.store.ping().await?;
    Ok("ok")
}
#[utoipa::path(get, path = "/v1/permissions", operation_id = "image_manager_permissions", tag = "permissions",
    responses((status = 200, body = PermissionsView)))]
async fn permissions(
    State(a): State<Arc<App>>,
    h: HeaderMap,
) -> ResponseResult<Json<PermissionsView>> {
    let actor = a.auth.authenticate(&h).await?;
    Ok(Json(PermissionsView {
        subject: actor.subject,
        permissions: actor.permissions,
        roles: p::ROLES,
    }))
}
#[utoipa::path(get, path = "/v1/images", operation_id = "image_manager_list", tag = "images",
    params(("after" = Option<String>, Query), ("scope" = Option<ImageScopeParam>, Query)),
    responses((status = 200, body = PageView<ImageView>)))]
async fn list(
    State(a): State<Arc<App>>,
    h: HeaderMap,
    Query(q): Query<Page>,
) -> ResponseResult<Json<PageView<ImageView>>> {
    let u = actor(&a, &h, p::READ).await?;
    let scope = q.scope.into();
    Ok(Json(paginated(
        a.store.list_images(&u.subject, &q.after, scope).await?,
        |i| &i.id,
        |image| ImageView::new(image, &u.subject),
    )))
}
#[utoipa::path(post, path = "/v1/images", operation_id = "image_manager_create", tag = "images",
    params(("Idempotency-Key" = String, Header)),
    request_body = CreateImage,
    responses((status = 201, body = ImageView)))]
async fn create(
    State(a): State<Arc<App>>,
    h: HeaderMap,
    Json(input): Json<CreateImage>,
) -> ResponseResult<(StatusCode, Json<ImageView>)> {
    let u = actor(&a, &h, p::CREATE).await?;
    Ok((
        StatusCode::CREATED,
        Json(ImageView::new(
            a.store.create_image(&u.subject, key(&h)?, input).await?,
            &u.subject,
        )),
    ))
}
#[utoipa::path(get, path = "/v1/images/{id}", operation_id = "image_manager_read", tag = "images",
    params(("id" = String, Path)),
    responses((status = 200, body = ImageView)))]
async fn read(
    State(a): State<Arc<App>>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> ResponseResult<Json<ImageView>> {
    let u = actor(&a, &h, p::READ).await?;
    Ok(Json(ImageView::new(
        a.store.image(&u.subject, &id, false).await?,
        &u.subject,
    )))
}
#[utoipa::path(patch, path = "/v1/images/{id}", operation_id = "image_manager_edit", tag = "images",
    params(("id" = String, Path), ("If-Match" = String, Header)),
    request_body = CreateImage,
    responses((status = 200, body = ImageView)))]
async fn edit(
    State(a): State<Arc<App>>,
    h: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<CreateImage>,
) -> ResponseResult<Json<ImageView>> {
    let u = actor(&a, &h, p::CONTROL).await?;
    Ok(Json(ImageView::new(
        a.store
            .update_image(&u.subject, &id, revision(&h)?, input)
            .await?,
        &u.subject,
    )))
}
#[utoipa::path(delete, path = "/v1/images/{id}", operation_id = "image_manager_retire", tag = "images",
    params(("id" = String, Path), ("If-Match" = String, Header)),
    responses((status = 200, body = ImageView)))]
async fn retire(
    State(a): State<Arc<App>>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> ResponseResult<Json<ImageView>> {
    let u = actor(&a, &h, p::DELETE).await?;
    Ok(Json(ImageView::new(
        a.store.retire(&u.subject, &id, revision(&h)?).await?,
        &u.subject,
    )))
}
#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
struct SetVisibility {
    visibility: ImageVisibility,
}
#[utoipa::path(put, path = "/v1/images/{id}/visibility", operation_id = "image_manager_visibility", tag = "images",
    params(("id" = String, Path), ("If-Match" = String, Header)),
    request_body = SetVisibility,
    responses((status = 200, body = ImageView)))]
async fn visibility(
    State(a): State<Arc<App>>,
    h: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<SetVisibility>,
) -> ResponseResult<Json<ImageView>> {
    let u = actor(&a, &h, p::SHARE).await?;
    Ok(Json(ImageView::new(
        a.store
            .set_visibility(&u.subject, &id, revision(&h)?, input.visibility)
            .await?,
        &u.subject,
    )))
}
#[utoipa::path(put, path = "/v1/images/{id}/shares/{subject}", operation_id = "image_manager_share", tag = "images",
    params(("id" = String, Path), ("subject" = String, Path), ("If-Match" = String, Header)),
    responses((status = 200, body = ImageView)))]
async fn share(
    State(a): State<Arc<App>>,
    h: HeaderMap,
    Path((id, subject)): Path<(String, String)>,
) -> ResponseResult<Json<ImageView>> {
    let u = actor(&a, &h, p::SHARE).await?;
    Ok(Json(ImageView::new(
        a.store
            .share(&u.subject, &id, revision(&h)?, &subject, true)
            .await?,
        &u.subject,
    )))
}
#[utoipa::path(delete, path = "/v1/images/{id}/shares/{subject}", operation_id = "image_manager_unshare", tag = "images",
    params(("id" = String, Path), ("subject" = String, Path), ("If-Match" = String, Header)),
    responses((status = 200, body = ImageView)))]
async fn unshare(
    State(a): State<Arc<App>>,
    h: HeaderMap,
    Path((id, subject)): Path<(String, String)>,
) -> ResponseResult<Json<ImageView>> {
    let u = actor(&a, &h, p::SHARE).await?;
    Ok(Json(ImageView::new(
        a.store
            .share(&u.subject, &id, revision(&h)?, &subject, false)
            .await?,
        &u.subject,
    )))
}
#[utoipa::path(post, path = "/v1/images/{id}/builds", operation_id = "image_manager_build", tag = "builds",
    params(("id" = String, Path), ("Idempotency-Key" = String, Header)),
    request_body = BuildInput,
    responses((status = 202, body = BuildView)))]
async fn build(
    State(a): State<Arc<App>>,
    h: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<BuildInput>,
) -> ResponseResult<(StatusCode, Json<BuildView>)> {
    let u = actor(&a, &h, p::BUILD).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(BuildView::new(
            a.store
                .create_build(&u.subject, &id, key(&h)?, input)
                .await?,
            &a.registry,
        )),
    ))
}
#[utoipa::path(get, path = "/v1/images/{id}/builds", operation_id = "image_manager_builds", tag = "builds",
    params(("id" = String, Path), ("after" = Option<String>, Query)),
    responses((status = 200, body = PageView<BuildView>)))]
async fn builds(
    State(a): State<Arc<App>>,
    h: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<Page>,
) -> ResponseResult<Json<PageView<BuildView>>> {
    let u = actor(&a, &h, p::BUILD_READ).await?;
    Ok(Json(paginated(
        a.store
            .image_builds(&u.subject, &id, false, &q.after)
            .await?,
        |b| &b.id,
        |build| BuildView::new(build, &a.registry),
    )))
}
#[utoipa::path(get, path = "/v1/images/{id}/versions", operation_id = "image_manager_versions", tag = "versions",
    params(("id" = String, Path), ("after" = Option<String>, Query)),
    responses((status = 200, body = PageView<VersionView>)))]
async fn versions(
    State(a): State<Arc<App>>,
    h: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<Page>,
) -> ResponseResult<Json<PageView<VersionView>>> {
    let u = actor(&a, &h, p::READ).await?;
    Ok(Json(paginated(
        a.store
            .image_builds(&u.subject, &id, true, &q.after)
            .await?,
        |b| &b.id,
        |build| VersionView::new(build, &a.registry),
    )))
}
#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
struct Resolve {
    version: String,
}
#[utoipa::path(post, path = "/v1/images/{id}/resolve", operation_id = "image_manager_resolve", tag = "versions",
    params(("id" = String, Path)),
    request_body = Resolve,
    responses((status = 200, body = VersionView)))]
async fn resolve(
    State(a): State<Arc<App>>,
    h: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<Resolve>,
) -> ResponseResult<Json<VersionView>> {
    let u = actor(&a, &h, p::USE).await?;
    Ok(Json(VersionView::new(
        a.store.resolve(&u.subject, &id, &input.version).await?,
        &a.registry,
    )))
}
#[utoipa::path(get, path = "/v1/images/{id}/versions/{version}", operation_id = "image_manager_version", tag = "versions",
    params(("id" = String, Path), ("version" = String, Path)),
    responses((status = 200, body = VersionView)))]
async fn version(
    State(a): State<Arc<App>>,
    h: HeaderMap,
    Path((id, version)): Path<(String, String)>,
) -> ResponseResult<Json<VersionView>> {
    let u = actor(&a, &h, p::READ).await?;
    Ok(Json(VersionView::new(
        a.store.version(&u.subject, &id, &version).await?,
        &a.registry,
    )))
}
#[utoipa::path(get, path = "/v1/builds/{id}", operation_id = "image_manager_build_read", tag = "builds",
    params(("id" = String, Path)),
    responses((status = 200, body = BuildView)))]
async fn build_read(
    State(a): State<Arc<App>>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> ResponseResult<Json<BuildView>> {
    let u = actor(&a, &h, p::BUILD_READ).await?;
    Ok(Json(BuildView::new(
        a.store.build(&u.subject, &id).await?,
        &a.registry,
    )))
}
#[utoipa::path(get, path = "/v1/builds/{id}/logs", operation_id = "image_manager_logs", tag = "builds",
    params(("id" = String, Path)),
    responses((status = 200, body = Value)))]
async fn logs(
    State(a): State<Arc<App>>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> ResponseResult<Json<Value>> {
    let u = actor(&a, &h, p::BUILD_READ).await?;
    let build = a.store.build(&u.subject, &id).await?;
    Ok(Json(a.executor.logs(&build).await?))
}
